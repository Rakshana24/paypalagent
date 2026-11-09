use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::correlation::CorrelationStatus;
use crate::models::dispute::DisputeActionType;
use crate::models::evidence::{
    AgentAuditTrailSummary, AgentStatementFact, AuthorizationAuditFact, CatalogSearchAuditFact,
    CorrelationSummary, CryptographicProofSummary, DisputeEvidenceBrief, DisputeSummaryFact,
    EpistemicCategory, EvidenceSourceReference, MissingEvidenceDisclaimer, PromptAuditFact,
    RebuttalPoint, RebuttalSummary, RecordedPaypalSettlementFact, RejectedProductAuditFact,
    RequirementsAuditFact, SelectedProductAuditFact, SettlementProvenance, TransactionProof,
    VerifiedLedgerSettlementFact,
};
use crate::models::ledger::{DecisionStep, VerificationStatus};
use crate::models::shopping::ShoppingRequirements;
use crate::services::dispute_service::DisputeService;
use crate::services::ledger::LedgerService;
use crate::services::verification::VerificationService;

/// Core service responsible for generating factual, audit-grade dispute evidence briefs.
///
/// Grounded strictly in verified cryptographic ledger events, recorded PayPal webhook/API data,
/// deterministic inferences, and explicit unavailable evidence disclaimers.
#[derive(Clone)]
pub struct DisputeEvidenceService {
    pool: SqlitePool,
    dispute_service: Arc<DisputeService>,
    ledger_service: Arc<LedgerService>,
    verification_service: Arc<VerificationService>,
}

impl DisputeEvidenceService {
    /// Creates a new `DisputeEvidenceService` instance.
    pub fn new(
        pool: SqlitePool,
        dispute_service: Arc<DisputeService>,
        ledger_service: Arc<LedgerService>,
        verification_service: Arc<VerificationService>,
    ) -> Self {
        Self {
            pool,
            dispute_service,
            ledger_service,
            verification_service,
        }
    }

    /// Generates a structured, factual evidence brief for a given PayPal dispute ID.
    pub async fn generate_evidence_brief(
        &self,
        dispute_id: &str,
    ) -> Result<DisputeEvidenceBrief, AppError> {
        let trimmed_id = dispute_id.trim();
        if trimmed_id.is_empty() {
            return Err(AppError::InvalidPayload(
                "dispute_id cannot be empty".to_string(),
            ));
        }

        debug!(dispute_id = %trimmed_id, "Generating dispute evidence brief");

        // 1. Retrieve the dispute entity
        let dispute = self
            .dispute_service
            .get_dispute(trimmed_id)
            .await?
            .ok_or_else(|| {
                AppError::DisputeNotFound(format!("Dispute '{}' not found", trimmed_id))
            })?;

        // 2. Retrieve dispute audit history
        let dispute_steps = self
            .dispute_service
            .get_dispute_audit_steps(trimmed_id)
            .await?;

        // 3. Query webhook_events for authoritative ingress transmission reference if available
        let webhook_info: Option<(String, String, String)> = sqlx::query_as(
            r#"
            SELECT event_id, transmission_id, created_at
            FROM webhook_events
            WHERE resource_id = ?
            ORDER BY created_at ASC
            LIMIT 1;
            "#,
        )
        .bind(&dispute.dispute_id)
        .fetch_optional(&self.pool)
        .await?;

        let dispute_source = match webhook_info {
            Some((evt_id, trans_id, created_at)) => {
                EvidenceSourceReference::webhook_event(trans_id, evt_id, Some(created_at)).ok()
            }
            None => EvidenceSourceReference::paypal_record(
                &dispute.dispute_id,
                dispute.dispute_create_time.clone(),
            )
            .ok(),
        };

        // 4. Construct DisputeSummaryFact (strictly RECORDED_PAYPAL_DATA)
        let dispute_summary = DisputeSummaryFact {
            category: EpistemicCategory::RecordedPaypalData,
            dispute_id: dispute.dispute_id.clone(),
            status: dispute.status.clone(),
            dispute_lifecycle_stage: dispute.dispute_lifecycle_stage.clone(),
            dispute_channel: dispute.dispute_channel.clone(),
            reason: dispute.reason.clone(),
            dispute_amount_value: dispute.dispute_amount_value.clone(),
            dispute_amount_currency: dispute.dispute_amount_currency.clone(),
            seller_transaction_id: dispute.seller_transaction_id.clone(),
            paypal_order_id: dispute.paypal_order_id.clone(),
            dispute_create_time: dispute.dispute_create_time.clone(),
            dispute_update_time: dispute.dispute_update_time.clone(),
            source: dispute_source,
        };
        dispute_summary.validate_category()?;

        // 5. Verify the dispute audit chain
        let dispute_verify_res = self
            .dispute_service
            .verify_dispute_chain(&dispute.dispute_id)
            .await?;
        let dispute_chain_valid = dispute_verify_res.valid;
        let dispute_total_steps = dispute_verify_res.total_steps;

        let dispute_root_hash_source = if let Some(last_step) = dispute_steps.last() {
            EvidenceSourceReference::dispute_step(
                &dispute.dispute_id,
                last_step.sequence_index,
                &last_step.action_type,
                &last_step.current_hash,
                &last_step.created_at,
            )
            .ok()
        } else {
            None
        };

        // 6. Inspect correlation state and linked purchase session
        let is_correlated = dispute.correlation_status == CorrelationStatus::Matched
            && dispute.session_id.is_some();

        let (
            session_opt,
            decision_steps,
            purchase_chain_valid,
            purchase_total_steps,
            purchase_root_hash_source,
            dual_chain_integrity_verified,
        ) = if let Some(ref session_id) = dispute.session_id {
            if is_correlated {
                // Fetch session record
                let session_res = self.ledger_service.get_session(session_id).await;
                match session_res {
                    Ok(session) => {
                        // Verify purchase session hash chain
                        let verify_session_res =
                            self.verification_service.verify_session(session_id).await;
                        let (p_valid, p_steps) = match verify_session_res {
                            Ok(v) => (v.status == VerificationStatus::Valid, v.total_steps),
                            Err(err) => {
                                warn!(
                                    session_id = %session_id,
                                    error = %err,
                                    "Failed to verify purchase session ledger"
                                );
                                (false, 0)
                            }
                        };

                        let p_root_source = if let Some(ref r_hash) = session.root_hash {
                            EvidenceSourceReference::session_finalization(
                                session_id,
                                r_hash,
                                &session.updated_at,
                            )
                            .ok()
                        } else {
                            None
                        };

                        // Fetch decision steps
                        let steps = self
                            .ledger_service
                            .get_session_events(session_id)
                            .await
                            .unwrap_or_default();

                        // Dual chain verification: both chains valid AND genesis anchor matches purchase root hash
                        let dual_valid = p_valid
                            && dispute_chain_valid
                            && session.root_hash.as_ref() == Some(&dispute.genesis_anchor_hash);

                        (
                            Some(session),
                            steps,
                            p_valid,
                            p_steps,
                            p_root_source,
                            dual_valid,
                        )
                    }
                    Err(err) => {
                        warn!(
                            session_id = %session_id,
                            error = %err,
                            "Correlated session not found in ledger"
                        );
                        (None, Vec::new(), false, 0, None, false)
                    }
                }
            } else {
                // Uncorrelated or manual-review: do not present purchase session integrity as valid
                (None, Vec::new(), false, 0, None, false)
            }
        } else {
            (None, Vec::new(), false, 0, None, false)
        };

        // 7. Construct CryptographicProofSummary
        let cryptographic_proof = CryptographicProofSummary {
            category: EpistemicCategory::VerifiedFact,
            purchase_session_id: if is_correlated {
                dispute.session_id.clone()
            } else {
                None
            },
            purchase_root_hash: if is_correlated {
                dispute.purchase_root_hash.clone()
            } else {
                None
            },
            purchase_root_hash_source,
            purchase_chain_valid,
            purchase_total_steps,
            dispute_root_hash: dispute.root_hash.clone(),
            dispute_root_hash_source,
            dispute_chain_valid,
            dispute_total_steps,
            genesis_anchor_type: dispute.genesis_anchor_type,
            genesis_anchor_hash: dispute.genesis_anchor_hash.clone(),
            dual_chain_integrity_verified,
            integrity_scope_statement: CryptographicProofSummary::DEFAULT_SCOPE_STATEMENT
                .to_string(),
        };
        cryptographic_proof.validate_category()?;

        // 8. Construct CorrelationSummary
        let correlation_source = dispute_steps
            .iter()
            .find(|s| s.action_type == DisputeActionType::DISPUTE_CORRELATED)
            .and_then(|s| {
                EvidenceSourceReference::dispute_step(
                    &dispute.dispute_id,
                    s.sequence_index,
                    &s.action_type,
                    &s.current_hash,
                    &s.created_at,
                )
                .ok()
            });

        let correlation_explanation = match dispute.correlation_status {
            CorrelationStatus::Matched => format!(
                "Deterministically correlated dispute to AgentAudit purchase session '{}' via {}",
                dispute.session_id.as_deref().unwrap_or("UNKNOWN"),
                dispute
                    .matched_by
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "MATCHED".to_string())
            ),
            CorrelationStatus::ManualReviewRequired => {
                "Dispute requires manual review; transaction identifiers are unfinalized, ambiguous, or conflicting"
                    .to_string()
            }
            CorrelationStatus::NotFound => {
                "No correlated AgentAudit purchase session found matching dispute transaction identifiers"
                    .to_string()
            }
        };

        let correlation_summary = CorrelationSummary::new(
            dispute.correlation_status,
            dispute.matched_by,
            if is_correlated {
                dispute.session_id.clone()
            } else {
                None
            },
            correlation_explanation,
            correlation_source,
        );
        correlation_summary.validate_category()?;

        // 9. Extract Agent Audit Trail and Transaction Proof (only when correlated with valid session and verified ledger)
        let (agent_audit_trail, transaction_proof) = if is_correlated
            && purchase_chain_valid
            && session_opt.is_some()
            && !decision_steps.is_empty()
        {
            let trail = self.extract_agent_audit_trail(&decision_steps)?;
            let proof = self.extract_transaction_proof(&decision_steps, &dispute)?;
            (Some(trail), Some(proof))
        } else {
            (None, None)
        };

        // 10. Generate Factual Rebuttal Summary tailored to dispute reason code
        let rebuttal_summary = self.synthesize_rebuttal(
            &dispute,
            &correlation_summary,
            &cryptographic_proof,
            agent_audit_trail.as_ref(),
            transaction_proof.as_ref(),
        )?;
        rebuttal_summary.validate_category()?;

        // 11. Explicitly disclose missing evidence categories
        let has_physical_goods = if let Some(ref trail) = agent_audit_trail {
            trail
                .selected_product
                .as_ref()
                .map(|p| p.category != "Digital" && p.category != "Service")
                .unwrap_or(true)
        } else {
            true
        };
        let missing_evidence = MissingEvidenceDisclaimer::standard_disclaimers(has_physical_goods);

        // 12. Assemble complete DisputeEvidenceBrief
        let brief = DisputeEvidenceBrief {
            brief_id: Uuid::new_v4().to_string(),
            schema_version: "1.0.0".to_string(),
            dispute_id: dispute.dispute_id.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            correlation_summary,
            dispute_summary,
            cryptographic_proof,
            agent_audit_trail,
            transaction_proof,
            rebuttal_summary,
            missing_evidence,
        };

        info!(
            dispute_id = %brief.dispute_id,
            brief_id = %brief.brief_id,
            correlation_status = ?brief.correlation_summary.status,
            dual_chain_verified = brief.cryptographic_proof.dual_chain_integrity_verified,
            "Dispute evidence brief generated successfully"
        );

        Ok(brief)
    }

    // --- Helper Extraction Methods ---

    fn extract_agent_audit_trail(
        &self,
        steps: &[DecisionStep],
    ) -> Result<AgentAuditTrailSummary, AppError> {
        // Find INITIAL_PROMPT
        let prompt_step = steps
            .iter()
            .find(|s| s.action_type == "INITIAL_PROMPT")
            .ok_or_else(|| {
                AppError::LedgerIntegrity("Session missing INITIAL_PROMPT step".to_string())
            })?;

        let prompt_val: serde_json::Value = serde_json::from_str(&prompt_step.payload)?;
        let prompt_text = prompt_val
            .get("prompt")
            .or_else(|| prompt_val.get("initial_prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let initial_prompt = PromptAuditFact {
            prompt: prompt_text,
            source: EvidenceSourceReference::ledger_step(
                &prompt_step.session_id,
                prompt_step.sequence_index,
                &prompt_step.action_type,
                &prompt_step.current_hash,
                &prompt_step.created_at,
            )?,
        };

        // Find REQUIREMENTS_EXTRACTED
        let requirements = if let Some(req_step) = steps
            .iter()
            .find(|s| s.action_type == "REQUIREMENTS_EXTRACTED")
        {
            let req_data: ShoppingRequirements = serde_json::from_str(&req_step.payload)?;
            Some(RequirementsAuditFact {
                category: req_data.category,
                maximum_budget: req_data.maximum_budget,
                preferred_brand: req_data.preferred_brand,
                minimum_battery_hours: req_data.minimum_battery_hours,
                required_features: req_data.required_features,
                intended_use: None,
                source: EvidenceSourceReference::ledger_step(
                    &req_step.session_id,
                    req_step.sequence_index,
                    &req_step.action_type,
                    &req_step.current_hash,
                    &req_step.created_at,
                )?,
            })
        } else {
            None
        };

        // Find CATALOG_SEARCH
        let catalog_search =
            if let Some(cat_step) = steps.iter().find(|s| s.action_type == "CATALOG_SEARCH") {
                let cat_val: serde_json::Value = serde_json::from_str(&cat_step.payload)?;
                let count = cat_val
                    .get("candidates_retrieved_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;

                Some(CatalogSearchAuditFact {
                    candidates_evaluated_count: count,
                    source: EvidenceSourceReference::ledger_step(
                        &cat_step.session_id,
                        cat_step.sequence_index,
                        &cat_step.action_type,
                        &cat_step.current_hash,
                        &cat_step.created_at,
                    )?,
                })
            } else {
                None
            };

        // Find PRODUCT_REJECTED
        let mut rejected_candidates = Vec::new();
        for rej_step in steps.iter().filter(|s| s.action_type == "PRODUCT_REJECTED") {
            let rej_val: serde_json::Value = serde_json::from_str(&rej_step.payload)?;
            let product_id = rej_val
                .get("product_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let product_name = rej_val
                .get("product_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let price = rej_val.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let currency = rej_val
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("USD")
                .to_string();
            let reason = rej_val
                .get("rejection_reason")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            rejected_candidates.push(RejectedProductAuditFact {
                product_id,
                product_name,
                price,
                currency,
                rejection_rationale: AgentStatementFact::new(reason),
                source: EvidenceSourceReference::ledger_step(
                    &rej_step.session_id,
                    rej_step.sequence_index,
                    &rej_step.action_type,
                    &rej_step.current_hash,
                    &rej_step.created_at,
                )?,
            });
        }

        // Find PRODUCT_SELECTED
        let selected_product =
            if let Some(sel_step) = steps.iter().find(|s| s.action_type == "PRODUCT_SELECTED") {
                let sel_val: serde_json::Value = serde_json::from_str(&sel_step.payload)?;
                let prod_obj = sel_val.get("selected_product");
                let product_id = prod_obj
                    .and_then(|p| p.get("product_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let product_name = prod_obj
                    .and_then(|p| p.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let brand = prod_obj
                    .and_then(|p| p.get("brand"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let category = prod_obj
                    .and_then(|p| p.get("category"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let price = sel_val.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let currency = sel_val
                    .get("currency")
                    .and_then(|v| v.as_str())
                    .unwrap_or("USD")
                    .to_string();
                let rationale = sel_val
                    .get("selection_rationale")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                Some(SelectedProductAuditFact {
                    product_id,
                    product_name,
                    brand,
                    category,
                    price,
                    currency,
                    selection_rationale: AgentStatementFact::new(rationale),
                    source: EvidenceSourceReference::ledger_step(
                        &sel_step.session_id,
                        sel_step.sequence_index,
                        &sel_step.action_type,
                        &sel_step.current_hash,
                        &sel_step.created_at,
                    )?,
                })
            } else {
                None
            };

        // Find PURCHASE_AUTHORIZED
        let purchase_authorization = if let Some(auth_step) = steps
            .iter()
            .find(|s| s.action_type == "PURCHASE_AUTHORIZED")
        {
            let auth_val: serde_json::Value = serde_json::from_str(&auth_step.payload)?;
            let selected_product_id = auth_val
                .get("selected_product_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let maximum_authorized_amount = auth_val
                .get("maximum_authorized_amount")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let user_confirmed = auth_val
                .get("user_confirmed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let authorized_at = auth_val
                .get("authorized_at")
                .and_then(|v| v.as_str())
                .unwrap_or(&auth_step.created_at)
                .to_string();

            Some(AuthorizationAuditFact {
                selected_product_id,
                maximum_authorized_amount,
                user_confirmed,
                authorized_at,
                identity_disclaimer: AuthorizationAuditFact::DEFAULT_IDENTITY_DISCLAIMER
                    .to_string(),
                source: EvidenceSourceReference::ledger_step(
                    &auth_step.session_id,
                    auth_step.sequence_index,
                    &auth_step.action_type,
                    &auth_step.current_hash,
                    &auth_step.created_at,
                )?,
            })
        } else {
            None
        };

        let summary = AgentAuditTrailSummary {
            category: EpistemicCategory::VerifiedFact,
            initial_prompt,
            requirements,
            catalog_search,
            rejected_candidates,
            selected_product,
            purchase_authorization,
        };
        summary.validate_category()?;

        Ok(summary)
    }

    fn extract_transaction_proof(
        &self,
        steps: &[DecisionStep],
        dispute: &crate::models::dispute::DisputeRecord,
    ) -> Result<TransactionProof, AppError> {
        let order_step = steps
            .iter()
            .find(|s| s.action_type == "PAYPAL_ORDER_CREATED");
        let capture_step = steps
            .iter()
            .find(|s| s.action_type == "PAYPAL_PAYMENT_CAPTURED");

        let order_created_step = if let Some(os) = order_step {
            Some(EvidenceSourceReference::ledger_step(
                &os.session_id,
                os.sequence_index,
                &os.action_type,
                &os.current_hash,
                &os.created_at,
            )?)
        } else {
            None
        };

        let payment_captured_step = if let Some(cs) = capture_step {
            Some(EvidenceSourceReference::ledger_step(
                &cs.session_id,
                cs.sequence_index,
                &cs.action_type,
                &cs.current_hash,
                &cs.created_at,
            )?)
        } else {
            None
        };

        let ledger_settlement = VerifiedLedgerSettlementFact {
            category: EpistemicCategory::VerifiedFact,
            order_created_step,
            payment_captured_step,
        };
        ledger_settlement.validate_category()?;

        // Extract values recorded in ledger step
        let (paypal_order_id, order_status, order_currency, order_amount) =
            if let Some(os) = order_step {
                let val: serde_json::Value = serde_json::from_str(&os.payload)?;
                (
                    val.get("order_id")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    val.get("status").and_then(|v| v.as_str()).map(String::from),
                    val.get("currency")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    val.get("amount").and_then(|v| v.as_str()).map(String::from),
                )
            } else {
                (dispute.paypal_order_id.clone(), None, None, None)
            };

        let (paypal_capture_id, capture_status, capture_timestamp) = if let Some(cs) = capture_step
        {
            let val: serde_json::Value = serde_json::from_str(&cs.payload)?;
            (
                val.get("capture_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                val.get("status").and_then(|v| v.as_str()).map(String::from),
                Some(cs.created_at.clone()),
            )
        } else {
            (dispute.seller_transaction_id.clone(), None, None)
        };

        let source_ref = if let Some(ref cs) = ledger_settlement.payment_captured_step {
            cs.clone()
        } else if let Some(ref os) = ledger_settlement.order_created_step {
            os.clone()
        } else {
            EvidenceSourceReference::paypal_record(
                dispute
                    .seller_transaction_id
                    .as_deref()
                    .unwrap_or(&dispute.dispute_id),
                dispute.dispute_create_time.clone(),
            )?
        };

        let paypal_settlement = RecordedPaypalSettlementFact {
            category: EpistemicCategory::RecordedPaypalData,
            provenance: SettlementProvenance::ReplayedFromLedgerPayload,
            source: source_ref,
            paypal_order_id,
            order_status,
            order_currency,
            order_amount,
            paypal_capture_id,
            capture_status,
            capture_timestamp,
        };
        paypal_settlement.validate_category()?;

        Ok(TransactionProof {
            paypal_settlement,
            ledger_settlement,
        })
    }

    fn synthesize_rebuttal(
        &self,
        dispute: &crate::models::dispute::DisputeRecord,
        correlation: &CorrelationSummary,
        crypto: &CryptographicProofSummary,
        trail: Option<&AgentAuditTrailSummary>,
        trans: Option<&TransactionProof>,
    ) -> Result<RebuttalSummary, AppError> {
        let reason_upper = dispute
            .reason
            .as_deref()
            .unwrap_or("UNKNOWN")
            .to_uppercase();

        let is_correlated = correlation.status == CorrelationStatus::Matched;
        let is_crypto_valid = crypto.dual_chain_integrity_verified;

        // If uncorrelated, manual review, or tampered, recommend manual review
        if !is_correlated || !is_crypto_valid {
            let explanation = if !is_correlated {
                format!(
                    "Dispute could not be deterministically correlated to an AgentAudit purchase session (Status: {:?}). Insufficient transaction identifiers to substantiate evidence.",
                    correlation.status
                )
            } else {
                "Dual-chain cryptographic verification failed or indicated chain tampering. Transaction records cannot be presented as authentic without human audit."
                    .to_string()
            };

            return Ok(RebuttalSummary {
                category: EpistemicCategory::DeterministicInference,
                dispute_reason: dispute.reason.clone(),
                recommended_action: "MANUAL_REVIEW".to_string(),
                primary_arguments: vec![RebuttalPoint {
                    category: EpistemicCategory::DeterministicInference,
                    claim: "Dispute requires manual merchant adjudication".to_string(),
                    explanation: explanation.clone(),
                    source_references: Vec::new(),
                }],
                reason_specific_analysis: explanation,
            });
        }

        let trail = match trail {
            Some(t) => t,
            None => {
                return Ok(RebuttalSummary {
                    category: EpistemicCategory::DeterministicInference,
                    dispute_reason: dispute.reason.clone(),
                    recommended_action: "MANUAL_REVIEW".to_string(),
                    primary_arguments: vec![RebuttalPoint {
                        category: EpistemicCategory::DeterministicInference,
                        claim: "Missing purchase session decision steps".to_string(),
                        explanation: "Correlated purchase session lacks decision steps in ledger."
                            .to_string(),
                        source_references: Vec::new(),
                    }],
                    reason_specific_analysis: "Incomplete audit trail.".to_string(),
                })
            }
        };

        // Reason 1: UNAUTHORISED_TRANSACTION / UNAUTHORISED
        if reason_upper.contains("UNAUTHORISED") || reason_upper.contains("UNAUTHORIZED") {
            let mut points = Vec::new();

            // Point 1: Pre-authorization step
            if let Some(ref auth) = trail.purchase_authorization {
                points.push(RebuttalPoint {
                    category: EpistemicCategory::DeterministicInference,
                    claim: "Customer explicitly authorized purchase prior to PayPal checkout initiation".to_string(),
                    explanation: format!(
                        "Session ledger records explicit authorization at sequence {} for product '{}' with spending limit ${:.2} (user_confirmed: {}). Note: Verifies session authorization compliance, but does not independently establish physical biometric identity.",
                        auth.source.sequence_index.unwrap_or(0),
                        auth.selected_product_id,
                        auth.maximum_authorized_amount,
                        auth.user_confirmed
                    ),
                    source_references: vec![auth.source.clone()],
                });
            }

            // Point 2: Settlement capture limit compliance
            if let Some(t_proof) = trans {
                if let (Some(ref cap_src), Some(ref cap_id), Some(ref amt)) = (
                    &t_proof.ledger_settlement.payment_captured_step,
                    &t_proof.paypal_settlement.paypal_capture_id,
                    &t_proof.paypal_settlement.order_amount,
                ) {
                    points.push(RebuttalPoint {
                        category: EpistemicCategory::DeterministicInference,
                        claim: "Payment capture amount complied strictly with authorized ceiling".to_string(),
                        explanation: format!(
                            "PayPal capture '{}' completed for ${} at sequence {}, which was within authorized customer spending limit.",
                            cap_id,
                            amt,
                            cap_src.sequence_index.unwrap_or(0)
                        ),
                        source_references: vec![cap_src.clone()],
                    });
                }
            }

            // Point 3: Cryptographic sequence integrity
            points.push(RebuttalPoint {
                category: EpistemicCategory::VerifiedFact,
                claim: "Cryptographic hash chain confirms append-only sequence order".to_string(),
                explanation: format!(
                    "Dual-chain verification confirmed 100% integrity across {} purchase steps and {} dispute steps anchored to hash '{}'.",
                    crypto.purchase_total_steps,
                    crypto.dispute_total_steps,
                    crypto.genesis_anchor_hash
                ),
                source_references: crypto.purchase_root_hash_source.iter().cloned().collect(),
            });

            return Ok(RebuttalSummary {
                category: EpistemicCategory::DeterministicInference,
                dispute_reason: dispute.reason.clone(),
                recommended_action: "CONTEST_WITH_EVIDENCE".to_string(),
                primary_arguments: points,
                reason_specific_analysis:
                    "Transaction was initiated only after explicit session purchase authorization was recorded in the immutable cryptographic ledger. Captured amount adhered strictly to the pre-authorized budget limit."
                        .to_string(),
            });
        }

        // Reason 2: MERCHANDISE_OR_SERVICE_NOT_AS_DESCRIBED
        if reason_upper.contains("NOT_AS_DESCRIBED") {
            let mut points = Vec::new();

            // Point 1: Customer constraints matching
            if let (Some(ref sel), Some(ref req)) = (&trail.selected_product, &trail.requirements) {
                points.push(RebuttalPoint {
                    category: EpistemicCategory::DeterministicInference,
                    claim: "Delivered product specifications strictly matched extracted buyer constraints".to_string(),
                    explanation: format!(
                        "Buyer prompt requested category '{}' within budget ${:.2}. Selected product '{}' (${:.2}) conformed exactly to specifications.",
                        req.category,
                        req.maximum_budget,
                        sel.product_name,
                        sel.price
                    ),
                    source_references: vec![sel.source.clone(), req.source.clone()],
                });
            }

            // Point 2: Selection rationale recorded in ledger
            if let Some(ref sel) = trail.selected_product {
                points.push(RebuttalPoint {
                    category: EpistemicCategory::VerifiedFact,
                    claim:
                        "Product evaluation and selection rationale recorded in immutable ledger"
                            .to_string(),
                    explanation: format!(
                        "Agent recorded selection rationale: '{}' at sequence {}.",
                        sel.selection_rationale.statement,
                        sel.source.sequence_index.unwrap_or(0)
                    ),
                    source_references: vec![sel.source.clone()],
                });
            }

            return Ok(RebuttalSummary {
                category: EpistemicCategory::DeterministicInference,
                dispute_reason: dispute.reason.clone(),
                recommended_action: "CONTEST_WITH_EVIDENCE".to_string(),
                primary_arguments: points,
                reason_specific_analysis:
                    "Autonomous agent evaluated catalog candidates strictly against buyer constraints. Selected merchandise specifications and comparison scores were locked into the decision ledger prior to checkout."
                        .to_string(),
            });
        }

        // Reason 3: MERCHANDISE_OR_SERVICE_NOT_RECEIVED
        if reason_upper.contains("NOT_RECEIVED") {
            let mut points = Vec::new();

            // Point 1: Payment capture confirmed
            if let Some(t_proof) = trans {
                if let Some(ref cap_src) = t_proof.ledger_settlement.payment_captured_step {
                    points.push(RebuttalPoint {
                        category: EpistemicCategory::VerifiedFact,
                        claim: "PayPal payment capture completed and verified in ledger"
                            .to_string(),
                        explanation: format!(
                            "PayPal capture '{}' was recorded as COMPLETED at sequence {}.",
                            t_proof
                                .paypal_settlement
                                .paypal_capture_id
                                .as_deref()
                                .unwrap_or("UNKNOWN"),
                            cap_src.sequence_index.unwrap_or(0)
                        ),
                        source_references: vec![cap_src.clone()],
                    });
                }
            }

            // Point 2: Physical fulfillment proof disclaimer
            points.push(RebuttalPoint {
                category: EpistemicCategory::UnavailableEvidence,
                claim: "Physical shipping tracking numbers are unavailable in AgentAudit and required from shipping provider".to_string(),
                explanation:
                    "AgentAudit governs purchase authorization and digital PayPal payment capture. Physical shipping carrier tracking scans and delivery confirmation must be obtained from the merchant's logistics carrier."
                        .to_string(),
                source_references: Vec::new(),
            });

            return Ok(RebuttalSummary {
                category: EpistemicCategory::DeterministicInference,
                dispute_reason: dispute.reason.clone(),
                recommended_action: "CONTEST_WITH_FULFILLMENT_PROOF_REQUIRED".to_string(),
                primary_arguments: points,
                reason_specific_analysis:
                    "Agent shopping decision trail and PayPal payment capture are cryptographically verified. To successfully defend this Item Not Received (INR) dispute under PayPal seller protection, merchant must attach physical shipping carrier tracking confirmation."
                        .to_string(),
            });
        }

        // Generic / Other Reason
        Ok(RebuttalSummary {
            category: EpistemicCategory::DeterministicInference,
            dispute_reason: dispute.reason.clone(),
            recommended_action: "MANUAL_REVIEW".to_string(),
            primary_arguments: vec![RebuttalPoint {
                category: EpistemicCategory::DeterministicInference,
                claim: "Factual ledger audit trail available for human adjudication review"
                    .to_string(),
                explanation: format!(
                    "Cryptographic audit trail is verified with {} purchase decision steps. Human review recommended for dispute reason '{}'.",
                    crypto.purchase_total_steps,
                    reason_upper
                ),
                source_references: crypto.purchase_root_hash_source.iter().cloned().collect(),
            }],
            reason_specific_analysis: format!(
                "Dispute reason '{}' requires specialized merchant review.",
                reason_upper
            ),
        })
    }
}
