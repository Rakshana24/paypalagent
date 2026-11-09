use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::error::AppError;
use crate::models::correlation::{
    extract_dispute_identifiers, is_valid_session_uuid, CorrelationResult,
    ExtractedDisputeIdentifiers, MatchedBy,
};
use crate::models::ledger::{DecisionStep, Session, VerificationStatus};
use crate::models::webhook::PayPalWebhookEvent;
use crate::services::auth::PayPalAuthService;
use crate::services::verification::VerificationService;

/// Internal outcome of evaluating the correlation hierarchy.
#[derive(Debug)]
enum HierarchyOutcome {
    Matched(CorrelationResult),
    ManualReview(CorrelationResult),
    NotFound(String),
    InsufficientIdentifiers,
}

/// Deterministic correlation engine matching PayPal disputes to AgentAudit purchase sessions.
#[derive(Clone)]
pub struct DisputeCorrelationService {
    config: Arc<Config>,
    http_client: reqwest::Client,
    auth_service: Arc<PayPalAuthService>,
    pool: SqlitePool,
    verification_service: Arc<VerificationService>,
}

impl DisputeCorrelationService {
    /// Creates a new `DisputeCorrelationService` instance.
    pub fn new(
        config: Arc<Config>,
        http_client: reqwest::Client,
        auth_service: Arc<PayPalAuthService>,
        pool: SqlitePool,
        verification_service: Arc<VerificationService>,
    ) -> Self {
        Self {
            config,
            http_client,
            auth_service,
            pool,
            verification_service,
        }
    }

    /// Correlates a PayPal webhook event notification to an AgentAudit session.
    pub async fn correlate_webhook_event(
        &self,
        event: &PayPalWebhookEvent,
    ) -> Result<CorrelationResult, AppError> {
        self.correlate_dispute(&event.resource).await
    }

    /// Correlates a dispute locally using only database identifiers (Tiers 1-3), without invoking external PayPal APIs.
    pub async fn correlate_locally(
        &self,
        dispute_resource: &serde_json::Value,
    ) -> Result<CorrelationResult, AppError> {
        let extracted = extract_dispute_identifiers(dispute_resource);
        let local_outcome = self.correlate_hierarchy(&extracted, false).await?;

        match local_outcome {
            HierarchyOutcome::Matched(result) => Ok(result),
            HierarchyOutcome::ManualReview(result) => Ok(result),
            HierarchyOutcome::NotFound(explanation) => {
                Ok(CorrelationResult::not_found(explanation))
            }
            HierarchyOutcome::InsufficientIdentifiers => Ok(CorrelationResult::not_found(
                "Insufficient transaction identifiers to correlate dispute to session",
            )),
        }
    }

    /// Deterministically correlates an incoming PayPal dispute resource payload to an AgentAudit purchase session.
    pub async fn correlate_dispute(
        &self,
        dispute_resource: &serde_json::Value,
    ) -> Result<CorrelationResult, AppError> {
        let extracted = extract_dispute_identifiers(dispute_resource);

        debug!(
            dispute_id = ?extracted.dispute_id,
            capture_ids = ?extracted.capture_ids,
            custom_ids = ?extracted.custom_ids,
            order_ids = ?extracted.order_ids,
            "Executing deterministic dispute correlation hierarchy"
        );

        // Run local deterministic correlation hierarchy
        let local_outcome = self.correlate_hierarchy(&extracted, false).await?;

        match local_outcome {
            HierarchyOutcome::Matched(result) => {
                info!(
                    session_id = ?result.session_id,
                    matched_by = ?result.matched_by,
                    "Dispute correlated successfully via local identifiers"
                );
                Ok(result)
            }
            HierarchyOutcome::ManualReview(result) => {
                warn!(
                    explanation = %result.explanation,
                    "Dispute correlation requires manual review"
                );
                Ok(result)
            }
            HierarchyOutcome::NotFound(local_explanation) => {
                // Tier 4: Fallback to PayPal Show Dispute Details API if dispute_id is available
                if let Some(ref dispute_id) = extracted.dispute_id {
                    info!(
                        dispute_id = %dispute_id,
                        "Local identifiers not found; attempting Tier 4 fallback to PayPal Show Dispute Details API"
                    );
                    match self.fetch_dispute_details(dispute_id).await {
                        Ok(details) => {
                            let details_extracted = extract_dispute_identifiers(&details);
                            let details_outcome =
                                self.correlate_hierarchy(&details_extracted, true).await?;
                            match details_outcome {
                                HierarchyOutcome::Matched(result) => {
                                    info!(
                                        session_id = ?result.session_id,
                                        matched_by = ?result.matched_by,
                                        "Dispute correlated successfully via Tier 4 Show Dispute Details API"
                                    );
                                    Ok(result)
                                }
                                HierarchyOutcome::ManualReview(result) => Ok(result),
                                HierarchyOutcome::NotFound(details_exp) => {
                                    Ok(CorrelationResult::not_found(details_exp))
                                }
                                HierarchyOutcome::InsufficientIdentifiers => {
                                    Ok(CorrelationResult::not_found(local_explanation))
                                }
                            }
                        }
                        Err(err) => {
                            warn!(
                                dispute_id = %dispute_id,
                                error = %err,
                                "Tier 4 PayPal Show Dispute Details API call failed"
                            );
                            Ok(CorrelationResult::manual_review(format!(
                                "PayPal Show Dispute Details API fallback failed: {}",
                                err
                            )))
                        }
                    }
                } else {
                    Ok(CorrelationResult::not_found(local_explanation))
                }
            }
            HierarchyOutcome::InsufficientIdentifiers => {
                // No transaction identifiers in local payload.
                // Check if dispute_id is available for Tier 4 fallback.
                if let Some(ref dispute_id) = extracted.dispute_id {
                    info!(
                        dispute_id = %dispute_id,
                        "No local transaction identifiers; attempting Tier 4 fallback to PayPal Show Dispute Details API"
                    );
                    match self.fetch_dispute_details(dispute_id).await {
                        Ok(details) => {
                            let details_extracted = extract_dispute_identifiers(&details);
                            let details_outcome =
                                self.correlate_hierarchy(&details_extracted, true).await?;
                            match details_outcome {
                                HierarchyOutcome::Matched(result) => {
                                    info!(
                                        session_id = ?result.session_id,
                                        matched_by = ?result.matched_by,
                                        "Dispute correlated successfully via Tier 4 Show Dispute Details API"
                                    );
                                    Ok(result)
                                }
                                HierarchyOutcome::ManualReview(result) => Ok(result),
                                HierarchyOutcome::NotFound(details_exp) => {
                                    Ok(CorrelationResult::not_found(details_exp))
                                }
                                HierarchyOutcome::InsufficientIdentifiers => {
                                    Ok(CorrelationResult::manual_review(
                                        "No transaction identifiers available in dispute or PayPal API response",
                                    ))
                                }
                            }
                        }
                        Err(err) => {
                            warn!(
                                dispute_id = %dispute_id,
                                error = %err,
                                "Tier 4 PayPal Show Dispute Details API call failed"
                            );
                            Ok(CorrelationResult::manual_review(format!(
                                "PayPal Show Dispute Details API fallback failed: {}",
                                err
                            )))
                        }
                    }
                } else {
                    // Tier 5: No identifiers and no dispute_id -> MANUAL_REVIEW_REQUIRED
                    Ok(CorrelationResult::manual_review(
                        "No transaction identifiers available in dispute payload",
                    ))
                }
            }
        }
    }

    /// Evaluates the deterministic correlation hierarchy:
    /// Tier 1: PayPal Capture ID
    /// Tier 2: Custom ID / Reference ID
    /// Tier 3: PayPal Order ID
    async fn correlate_hierarchy(
        &self,
        extracted: &ExtractedDisputeIdentifiers,
        is_from_details_api: bool,
    ) -> Result<HierarchyOutcome, AppError> {
        if extracted.capture_ids.is_empty()
            && extracted.custom_ids.is_empty()
            && extracted.order_ids.is_empty()
        {
            return Ok(HierarchyOutcome::InsufficientIdentifiers);
        }

        // Tier 1: PayPal Capture ID
        if !extracted.capture_ids.is_empty() {
            let mut candidate_sessions = Vec::new();

            for cap_id in &extracted.capture_ids {
                let session_ids: Vec<String> = sqlx::query_scalar(
                    r#"
                    SELECT DISTINCT session_id
                    FROM decision_steps
                    WHERE action_type = 'PAYPAL_PAYMENT_CAPTURED'
                      AND json_extract(payload, '$.capture_id') = ?;
                    "#,
                )
                .bind(cap_id)
                .fetch_all(&self.pool)
                .await?;

                for s_id in session_ids {
                    if !candidate_sessions.contains(&s_id) {
                        candidate_sessions.push(s_id);
                    }
                }
            }

            if candidate_sessions.len() > 1 {
                return Ok(HierarchyOutcome::ManualReview(
                    CorrelationResult::manual_review(format!(
                        "Multiple candidate sessions found for PayPal Capture IDs: {:?}",
                        candidate_sessions
                    )),
                ));
            }

            if candidate_sessions.len() == 1 {
                let session_id = candidate_sessions.into_iter().next().unwrap();
                let matched_by = if is_from_details_api {
                    MatchedBy::DisputeDetailsCaptureId
                } else {
                    MatchedBy::CaptureId
                };
                return self
                    .verify_and_finalize_match(&session_id, matched_by, extracted)
                    .await;
            }
        }

        // Tier 2: Custom ID / Reference ID
        if !extracted.custom_ids.is_empty() {
            let mut valid_uuids = Vec::new();
            let mut invalid_uuids = Vec::new();

            for cid in &extracted.custom_ids {
                if is_valid_session_uuid(cid) {
                    valid_uuids.push(cid.clone());
                } else {
                    invalid_uuids.push(cid.clone());
                }
            }

            // If invalid UUIDs present and no valid candidate exists
            if valid_uuids.is_empty() {
                if extracted.order_ids.is_empty() && extracted.capture_ids.is_empty() {
                    return Ok(HierarchyOutcome::NotFound(format!(
                        "Invalid session UUID format '{}'",
                        invalid_uuids.first().unwrap_or(&String::new())
                    )));
                }
            } else {
                let mut candidate_sessions = Vec::new();

                for uuid in &valid_uuids {
                    let session_exists: Option<String> = sqlx::query_scalar(
                        r#"
                        SELECT session_id
                        FROM sessions
                        WHERE session_id = ?;
                        "#,
                    )
                    .bind(uuid)
                    .fetch_optional(&self.pool)
                    .await?;

                    if let Some(s_id) = session_exists {
                        if !candidate_sessions.contains(&s_id) {
                            candidate_sessions.push(s_id);
                        }
                    }
                }

                if candidate_sessions.len() > 1 {
                    return Ok(HierarchyOutcome::ManualReview(
                        CorrelationResult::manual_review(format!(
                            "Multiple candidate sessions found for Custom IDs: {:?}",
                            candidate_sessions
                        )),
                    ));
                }

                if candidate_sessions.len() == 1 {
                    let session_id = candidate_sessions.into_iter().next().unwrap();
                    let matched_by = if is_from_details_api {
                        MatchedBy::DisputeDetailsCustomId
                    } else {
                        MatchedBy::CustomId
                    };
                    return self
                        .verify_and_finalize_match(&session_id, matched_by, extracted)
                        .await;
                }
            }
        }

        // Tier 3: PayPal Order ID
        if !extracted.order_ids.is_empty() {
            let mut candidate_sessions = Vec::new();

            for order_id in &extracted.order_ids {
                let session_ids: Vec<String> = sqlx::query_scalar(
                    r#"
                    SELECT session_id
                    FROM sessions
                    WHERE paypal_order_id = ?;
                    "#,
                )
                .bind(order_id)
                .fetch_all(&self.pool)
                .await?;

                for s_id in session_ids {
                    if !candidate_sessions.contains(&s_id) {
                        candidate_sessions.push(s_id);
                    }
                }
            }

            if candidate_sessions.len() > 1 {
                return Ok(HierarchyOutcome::ManualReview(
                    CorrelationResult::manual_review(format!(
                        "Multiple candidate sessions found for PayPal Order ID '{}'",
                        extracted.order_ids[0]
                    )),
                ));
            }

            if candidate_sessions.len() == 1 {
                let session_id = candidate_sessions.into_iter().next().unwrap();
                let matched_by = if is_from_details_api {
                    MatchedBy::DisputeDetailsOrderId
                } else {
                    MatchedBy::OrderId
                };
                return self
                    .verify_and_finalize_match(&session_id, matched_by, extracted)
                    .await;
            }
        }

        // None matched in local database
        if let Some(cap) = extracted.capture_ids.first() {
            Ok(HierarchyOutcome::NotFound(format!(
                "No session found matching PayPal capture ID '{}'",
                cap
            )))
        } else if let Some(ord) = extracted.order_ids.first() {
            Ok(HierarchyOutcome::NotFound(format!(
                "No session found matching PayPal order ID '{}'",
                ord
            )))
        } else if let Some(cust) = extracted.custom_ids.first() {
            Ok(HierarchyOutcome::NotFound(format!(
                "No session found matching custom ID '{}'",
                cust
            )))
        } else {
            Ok(HierarchyOutcome::InsufficientIdentifiers)
        }
    }

    /// Verifies candidate session ledger validity, finalization status, capture status,
    /// and cross-identifier consistency before producing a match.
    async fn verify_and_finalize_match(
        &self,
        session_id: &str,
        matched_by: MatchedBy,
        dispute_ids: &ExtractedDisputeIdentifiers,
    ) -> Result<HierarchyOutcome, AppError> {
        // 1. Fetch session record
        let session = match sqlx::query_as::<_, Session>(
            r#"
            SELECT session_id, initial_prompt, paypal_order_id, root_hash, created_at, updated_at
            FROM sessions
            WHERE session_id = ?;
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?
        {
            Some(s) => s,
            None => {
                return Ok(HierarchyOutcome::NotFound(format!(
                    "Session '{}' not found",
                    session_id
                )))
            }
        };

        // 2. Finalization verification: Never match unfinalized purchase sessions
        if session.root_hash.is_none() {
            return Ok(HierarchyOutcome::ManualReview(
                CorrelationResult::manual_review(format!(
                    "Session '{}' is not finalized; cannot correlate dispute",
                    session_id
                )),
            ));
        }

        // 3. Cryptographic hash-chain ledger verification
        let verification = self.verification_service.verify_session(session_id).await?;
        if verification.status != VerificationStatus::Valid {
            return Ok(HierarchyOutcome::ManualReview(
                CorrelationResult::manual_review(format!(
                    "Session '{}' failed cryptographic ledger verification: {}",
                    session_id, verification.explanation
                )),
            ));
        }

        // 4. Fetch confirmed PAYPAL_PAYMENT_CAPTURED event
        let capture_step = sqlx::query_as::<_, DecisionStep>(
            r#"
            SELECT id, session_id, sequence_index, action_type, payload, previous_hash, current_hash, created_at
            FROM decision_steps
            WHERE session_id = ? AND action_type = 'PAYPAL_PAYMENT_CAPTURED'
            ORDER BY sequence_index DESC
            LIMIT 1;
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        let capture_step = match capture_step {
            Some(step) => step,
            None => {
                return Ok(HierarchyOutcome::ManualReview(
                    CorrelationResult::manual_review(format!(
                        "Session '{}' has no confirmed PAYPAL_PAYMENT_CAPTURED event in ledger",
                        session_id
                    )),
                ));
            }
        };

        let payload: serde_json::Value = serde_json::from_str(&capture_step.payload)?;
        let session_capture_id = match payload.get("capture_id").and_then(|v| v.as_str()) {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => {
                return Ok(HierarchyOutcome::ManualReview(
                    CorrelationResult::manual_review(format!(
                        "Session '{}' PAYPAL_PAYMENT_CAPTURED event missing capture_id",
                        session_id
                    )),
                ));
            }
        };

        let capture_status = payload
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if capture_status != "COMPLETED" {
            return Ok(HierarchyOutcome::ManualReview(
                CorrelationResult::manual_review(format!(
                    "Session '{}' payment capture status is '{}' (expected COMPLETED)",
                    session_id, capture_status
                )),
            ));
        }

        let session_order_id = session.paypal_order_id.clone().unwrap_or_default();

        // 5. Cross-identifier consistency verification
        // Check capture IDs
        for disp_cap in &dispute_ids.capture_ids {
            if disp_cap != &session_capture_id {
                return Ok(HierarchyOutcome::ManualReview(
                    CorrelationResult::manual_review(format!(
                        "Conflicting identifiers: dispute capture ID '{}' does not match session capture ID '{}'",
                        disp_cap, session_capture_id
                    )),
                ));
            }
        }

        // Check order IDs
        for disp_ord in &dispute_ids.order_ids {
            if disp_ord != &session_order_id {
                return Ok(HierarchyOutcome::ManualReview(
                    CorrelationResult::manual_review(format!(
                        "Conflicting identifiers: dispute order ID '{}' does not match session order ID '{}'",
                        disp_ord, session_order_id
                    )),
                ));
            }
        }

        // Check custom IDs
        for disp_cust in &dispute_ids.custom_ids {
            if disp_cust != session_id {
                return Ok(HierarchyOutcome::ManualReview(
                    CorrelationResult::manual_review(format!(
                        "Conflicting identifiers: dispute custom ID '{}' does not match session ID '{}'",
                        disp_cust, session_id
                    )),
                ));
            }
        }

        // 6. Confirmed match
        let explanation = format!(
            "Deterministically correlated dispute to session '{}' via {}",
            session_id, matched_by
        );

        Ok(HierarchyOutcome::Matched(CorrelationResult::matched(
            session_id,
            session_order_id,
            session_capture_id,
            matched_by,
            explanation,
        )))
    }

    /// Fetches authoritative dispute details from PayPal's Customer Disputes API v1.
    pub async fn fetch_dispute_details(
        &self,
        dispute_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        let token = self.auth_service.get_valid_access_token().await?;
        let url = format!(
            "{}/v1/customer/disputes/{}",
            self.config.paypal_base_url, dispute_id
        );

        debug!(
            dispute_id = %dispute_id,
            url = %url,
            "Requesting dispute details from PayPal Show Dispute Details API"
        );

        let resp = self
            .http_client
            .get(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            error!(
                dispute_id = %dispute_id,
                status = %status,
                body = %err_body,
                "PayPal Show Dispute Details API returned error"
            );
            return Err(AppError::PayPalApi(format!(
                "PayPal Show Dispute Details API error ({}): {}",
                status, err_body
            )));
        }

        let body = resp.json::<serde_json::Value>().await?;
        Ok(body)
    }
}
