use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

use crate::error::AppError;
use crate::ledger::hash::{compute_event_hash, to_canonical_json_bytes, to_canonical_json_string};
use crate::models::correlation::{CorrelationStatus, MatchedBy};
use crate::models::dispute::{
    DisputeActionType, DisputeAuditStep, DisputeRecord, DisputeVerificationResult,
    GenesisAnchorType, UpsertDisputePayload, GENESIS_HASH_DEFAULT,
};
use crate::models::ledger::VerificationStatus;
use crate::services::verification::VerificationService;

/// Service managing dispute lifecycle, append-only hash chains, late correlation, and ledger verification.
#[derive(Clone)]
pub struct DisputeService {
    pool: SqlitePool,
    verification_service: Arc<VerificationService>,
}

impl DisputeService {
    /// Creates a new `DisputeService` instance.
    pub fn new(pool: SqlitePool, verification_service: Arc<VerificationService>) -> Self {
        Self {
            pool,
            verification_service,
        }
    }

    /// Ingests or updates a dispute within an existing caller-owned database transaction.
    ///
    /// - Initial ingestion creates an immutable genesis anchor:
    ///   - Matched: `PURCHASE_ROOT_HASH` anchored to verified purchase root hash.
    ///   - Unmatched: `GENESIS_HASH` anchored to 64-zero SHA-256 genesis hash.
    /// - Appends append-only dispute audit steps using RFC 8785 canonical JSON and deterministic event hashing.
    /// - Handles authoritative PayPal timestamps (newer advances, older records out-of-order, equal checks conflicts, missing flags manual review).
    pub async fn upsert_dispute_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        payload: &UpsertDisputePayload,
    ) -> Result<DisputeRecord, AppError> {
        let dispute_id = payload.dispute_id.trim();
        if dispute_id.is_empty() {
            return Err(AppError::InvalidDisputeData(
                "Dispute ID cannot be empty".to_string(),
            ));
        }

        // 1. Check if the dispute record already exists
        let existing = self.get_dispute_tx(&mut **tx, dispute_id).await?;

        match existing {
            None => {
                // Initial Dispute Ingestion
                self.insert_initial_dispute_tx(tx, payload).await
            }
            Some(existing_dispute) => {
                // Subsequent Dispute Update / Event Delivery
                self.update_existing_dispute_tx(tx, payload, existing_dispute)
                    .await
            }
        }
    }

    /// Standalone transaction wrapper for `upsert_dispute_tx`.
    pub async fn upsert_dispute(
        &self,
        payload: &UpsertDisputePayload,
    ) -> Result<DisputeRecord, AppError> {
        let mut tx = self.pool.begin().await?;
        let record = self.upsert_dispute_tx(&mut tx, payload).await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Correlates an unmatched dispute with a finalized purchase session within an existing transaction.
    ///
    /// - Cryptographically verifies the candidate purchase session ledger before linking.
    /// - Leaves the original genesis anchor and prior audit steps immutable.
    /// - Appends a `DISPUTE_CORRELATED` audit step.
    /// - Updates `session_id`, `purchase_root_hash`, `correlation_status`, and `matched_by`.
    pub async fn correlate_dispute_late_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        dispute_id: &str,
        session_id: &str,
        matched_by: MatchedBy,
    ) -> Result<DisputeRecord, AppError> {
        let dispute = self
            .get_dispute_tx(&mut **tx, dispute_id)
            .await?
            .ok_or_else(|| {
                AppError::DisputeNotFound(format!("Dispute '{}' does not exist", dispute_id))
            })?;

        // Check if dispute is already correlated
        if let Some(ref existing_session) = dispute.session_id {
            if existing_session == session_id {
                return Ok(dispute);
            } else {
                return Err(AppError::DisputeCorrelationConflict(format!(
                    "Dispute '{}' is already correlated with session '{}', cannot link to '{}'",
                    dispute_id, existing_session, session_id
                )));
            }
        }

        // 1. Verify candidate purchase session ledger
        let verification = self
            .verification_service
            .verify_session(session_id)
            .await
            .map_err(|e| match e {
                AppError::SessionNotFound(_) => AppError::SessionNotFound(format!(
                    "Purchase session '{}' not found",
                    session_id
                )),
                other => other,
            })?;

        if verification.status != VerificationStatus::Valid {
            return Err(AppError::DisputeAuditIntegrity(format!(
                "Candidate purchase session '{}' failed ledger verification: {}",
                session_id, verification.explanation
            )));
        }

        let verified_purchase_root = verification.stored_root_hash.clone().ok_or_else(|| {
            AppError::DisputeAuditIntegrity(format!(
                "Candidate purchase session '{}' is not finalized",
                session_id
            ))
        })?;

        // 2. Fetch latest audit step to extend hash chain
        let last_step = self
            .get_latest_audit_step_tx(&mut **tx, dispute_id)
            .await?
            .ok_or_else(|| {
                AppError::DisputeAuditIntegrity(format!(
                    "Dispute '{}' has no audit steps to extend",
                    dispute_id
                ))
            })?;

        let next_seq = last_step.sequence_index + 1;
        let prev_hash = last_step.current_hash;
        let now_iso = chrono::Utc::now().to_rfc3339();

        // 3. Create DISPUTE_CORRELATED audit payload
        let audit_payload = serde_json::json!({
            "dispute_id": dispute_id,
            "session_id": session_id,
            "purchase_root_hash": &verified_purchase_root,
            "matched_by": matched_by.to_string(),
            "correlated_at": &now_iso,
            "purchase_total_steps": verification.total_steps,
        });

        let canonical_bytes = to_canonical_json_bytes(&audit_payload)?;
        let canonical_str = to_canonical_json_string(&audit_payload)?;

        let new_step_hash = compute_event_hash(
            next_seq,
            DisputeActionType::DISPUTE_CORRELATED,
            &canonical_bytes,
            &prev_hash,
        );

        let step_id = Uuid::new_v4().to_string();

        // 4. Append audit step
        sqlx::query(
            r#"
            INSERT INTO dispute_audit_steps (
                id, dispute_id, sequence_index, action_type, payload,
                previous_hash, current_hash, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);
            "#,
        )
        .bind(&step_id)
        .bind(dispute_id)
        .bind(next_seq)
        .bind(DisputeActionType::DISPUTE_CORRELATED)
        .bind(&canonical_str)
        .bind(&prev_hash)
        .bind(&new_step_hash)
        .bind(&now_iso)
        .execute(&mut **tx)
        .await?;

        // 5. Update dispute record (genesis_anchor_type and genesis_anchor_hash remain untouched)
        let matched_by_str = matched_by.to_string();
        sqlx::query(
            r#"
            UPDATE disputes
            SET session_id = ?,
                purchase_root_hash = ?,
                correlation_status = 'MATCHED',
                matched_by = ?,
                root_hash = ?,
                updated_at = ?
            WHERE dispute_id = ?;
            "#,
        )
        .bind(session_id)
        .bind(&verified_purchase_root)
        .bind(&matched_by_str)
        .bind(&new_step_hash)
        .bind(&now_iso)
        .bind(dispute_id)
        .execute(&mut **tx)
        .await?;

        // 6. Return updated record
        self.get_dispute_tx(&mut **tx, dispute_id)
            .await?
            .ok_or_else(|| {
                AppError::DisputeNotFound(format!(
                    "Dispute '{}' not found after late correlation",
                    dispute_id
                ))
            })
    }

    /// Standalone transaction wrapper for `correlate_dispute_late_tx`.
    pub async fn correlate_dispute_late(
        &self,
        dispute_id: &str,
        session_id: &str,
        matched_by: MatchedBy,
    ) -> Result<DisputeRecord, AppError> {
        let mut tx = self.pool.begin().await?;
        let record = self
            .correlate_dispute_late_tx(&mut tx, dispute_id, session_id, matched_by)
            .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Fetches a dispute by ID from the database pool.
    pub async fn get_dispute(&self, dispute_id: &str) -> Result<Option<DisputeRecord>, AppError> {
        sqlx::query_as::<_, DisputeRecord>(
            r#"
            SELECT dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                   purchase_root_hash, seller_transaction_id, paypal_order_id,
                   status, dispute_lifecycle_stage, dispute_channel, reason,
                   dispute_amount_value, dispute_amount_currency, correlation_status,
                   matched_by, root_hash, dispute_create_time, dispute_update_time,
                   created_at, updated_at
            FROM disputes
            WHERE dispute_id = ?;
            "#,
        )
        .bind(dispute_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    /// Fetches all dispute audit steps in ascending sequence index order.
    pub async fn get_dispute_audit_steps(
        &self,
        dispute_id: &str,
    ) -> Result<Vec<DisputeAuditStep>, AppError> {
        sqlx::query_as::<_, DisputeAuditStep>(
            r#"
            SELECT id, dispute_id, sequence_index, action_type, payload,
                   previous_hash, current_hash, created_at
            FROM dispute_audit_steps
            WHERE dispute_id = ?
            ORDER BY sequence_index ASC;
            "#,
        )
        .bind(dispute_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    /// Cryptographically verifies the dispute's append-only audit hash chain.
    pub async fn verify_dispute_chain(
        &self,
        dispute_id: &str,
    ) -> Result<DisputeVerificationResult, AppError> {
        let dispute = self.get_dispute(dispute_id).await?.ok_or_else(|| {
            AppError::DisputeNotFound(format!("Dispute '{}' not found", dispute_id))
        })?;

        let steps = self.get_dispute_audit_steps(dispute_id).await?;

        if steps.is_empty() {
            return Ok(DisputeVerificationResult::invalid(
                dispute_id,
                0,
                dispute.genesis_anchor_type,
                &dispute.genesis_anchor_hash,
                dispute.purchase_root_hash,
                dispute.root_hash.unwrap_or_default(),
                "",
                vec!["Dispute audit chain is empty".to_string()],
            ));
        }

        let mut errors = Vec::new();
        let mut computed_root = String::new();

        for (idx, step) in steps.iter().enumerate() {
            let expected_seq = idx as i64;

            // 1. Sequence continuity check
            if step.sequence_index != expected_seq {
                errors.push(format!(
                    "Discontinuous sequence index at position {}: expected {}, found {}",
                    idx, expected_seq, step.sequence_index
                ));
            }

            // 2. Canonical JSON validation
            let parsed_payload: Result<serde_json::Value, _> = serde_json::from_str(&step.payload);
            let canonical_bytes = match parsed_payload {
                Ok(ref val) => match to_canonical_json_bytes(val) {
                    Ok(bytes) => {
                        if bytes != step.payload.as_bytes() {
                            errors.push(format!(
                                "Step {} payload violates RFC 8785 canonical formatting",
                                step.sequence_index
                            ));
                        }
                        bytes
                    }
                    Err(e) => {
                        errors.push(format!(
                            "Step {} payload canonical serialization failed: {}",
                            step.sequence_index, e
                        ));
                        Vec::new()
                    }
                },
                Err(e) => {
                    errors.push(format!(
                        "Step {} payload contains invalid JSON: {}",
                        step.sequence_index, e
                    ));
                    Vec::new()
                }
            };

            // 3. Cryptographic hash reproduction
            let computed_step_hash = compute_event_hash(
                step.sequence_index,
                &step.action_type,
                &canonical_bytes,
                &step.previous_hash,
            );

            if computed_step_hash != step.current_hash {
                errors.push(format!(
                    "Step {} current_hash mismatch: recorded '{}', recomputed '{}'",
                    step.sequence_index, step.current_hash, computed_step_hash
                ));
            }

            // 4. Chain link checks
            if idx == 0 {
                if step.previous_hash != dispute.genesis_anchor_hash {
                    errors.push(format!(
                        "Initial step previous_hash '{}' does not match genesis_anchor_hash '{}'",
                        step.previous_hash, dispute.genesis_anchor_hash
                    ));
                }
            } else {
                let prev_recorded_hash = &steps[idx - 1].current_hash;
                if &step.previous_hash != prev_recorded_hash {
                    errors.push(format!(
                        "Step {} previous_hash '{}' does not link to step {} current_hash '{}'",
                        step.sequence_index,
                        step.previous_hash,
                        steps[idx - 1].sequence_index,
                        prev_recorded_hash
                    ));
                }
            }

            computed_root = computed_step_hash;
        }

        // 5. Genesis anchor integrity checks
        match dispute.genesis_anchor_type {
            GenesisAnchorType::PurchaseRootHash => {
                if dispute.purchase_root_hash.as_deref() != Some(&dispute.genesis_anchor_hash) {
                    errors.push(format!(
                        "Genesis anchor hash '{}' does not equal purchase_root_hash '{:?}'",
                        dispute.genesis_anchor_hash, dispute.purchase_root_hash
                    ));
                }

                if let Some(ref session_id) = dispute.session_id {
                    match self.verification_service.verify_session(session_id).await {
                        Ok(v) => {
                            if v.status != VerificationStatus::Valid {
                                errors.push(format!(
                                    "Linked purchase session '{}' ledger verification failed: {}",
                                    session_id, v.explanation
                                ));
                            } else {
                                let sess_root = v.stored_root_hash.or(v.calculated_root_hash);
                                if sess_root.as_deref() != Some(&dispute.genesis_anchor_hash) {
                                    errors.push(format!(
                                        "Verified purchase session root '{:?}' does not match genesis anchor '{}'",
                                        sess_root, dispute.genesis_anchor_hash
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            errors.push(format!(
                                "Failed to verify linked purchase session '{}': {}",
                                session_id, e
                            ));
                        }
                    }
                } else {
                    errors.push(
                        "Dispute anchored to PURCHASE_ROOT_HASH has no session_id".to_string(),
                    );
                }
            }
            GenesisAnchorType::GenesisHash => {
                if dispute.genesis_anchor_hash != GENESIS_HASH_DEFAULT {
                    errors.push(format!(
                        "Genesis anchor hash '{}' does not match standard GENESIS_HASH_DEFAULT",
                        dispute.genesis_anchor_hash
                    ));
                }

                // If late correlated, check that DISPUTE_CORRELATED step exists and verify linked session
                if let Some(ref sess_root) = dispute.purchase_root_hash {
                    let has_correlated_step = steps
                        .iter()
                        .any(|s| s.action_type == DisputeActionType::DISPUTE_CORRELATED);
                    if !has_correlated_step {
                        errors.push(
                            "Dispute has purchase_root_hash but lacks DISPUTE_CORRELATED audit step"
                                .to_string(),
                        );
                    }

                    if let Some(ref session_id) = dispute.session_id {
                        match self.verification_service.verify_session(session_id).await {
                            Ok(v) => {
                                if v.status != VerificationStatus::Valid {
                                    errors.push(format!(
                                        "Late-correlated purchase session '{}' ledger verification failed: {}",
                                        session_id, v.explanation
                                    ));
                                } else {
                                    let actual_root = v.stored_root_hash.or(v.calculated_root_hash);
                                    if actual_root.as_deref() != Some(sess_root) {
                                        errors.push(format!(
                                            "Verified purchase session root '{:?}' does not match stored purchase_root_hash '{}'",
                                            actual_root, sess_root
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                errors.push(format!(
                                    "Failed to verify late-correlated purchase session '{}': {}",
                                    session_id, e
                                ));
                            }
                        }
                    } else {
                        errors.push(
                            "Dispute has purchase_root_hash but lacks session_id".to_string(),
                        );
                    }
                }
            }
        }

        // 6. Terminal root hash check
        let last_step_recorded_hash = &steps.last().unwrap().current_hash;
        if dispute.root_hash.as_deref() != Some(last_step_recorded_hash) {
            errors.push(format!(
                "Dispute terminal root_hash '{:?}' does not match last step current_hash '{}'",
                dispute.root_hash, last_step_recorded_hash
            ));
        }

        if dispute.root_hash.as_deref() != Some(&computed_root) {
            errors.push(format!(
                "Dispute terminal root_hash '{:?}' does not match computed chain root '{}'",
                dispute.root_hash, computed_root
            ));
        }

        if errors.is_empty() {
            Ok(DisputeVerificationResult::valid(
                dispute_id,
                steps.len(),
                dispute.genesis_anchor_type,
                dispute.genesis_anchor_hash,
                dispute.purchase_root_hash,
                computed_root,
            ))
        } else {
            Ok(DisputeVerificationResult::invalid(
                dispute_id,
                steps.len(),
                dispute.genesis_anchor_type,
                dispute.genesis_anchor_hash,
                dispute.purchase_root_hash,
                dispute.root_hash.unwrap_or_default(),
                computed_root,
                errors,
            ))
        }
    }

    /// Verifies both the dispute audit chain and the linked purchase ledger (dual-chain verification).
    pub async fn verify_dispute_and_purchase_ledger(
        &self,
        dispute_id: &str,
    ) -> Result<DisputeVerificationResult, AppError> {
        let dispute_res = self.verify_dispute_chain(dispute_id).await?;

        if !dispute_res.valid {
            return Ok(dispute_res);
        }

        let dispute = self.get_dispute(dispute_id).await?.ok_or_else(|| {
            AppError::DisputeNotFound(format!("Dispute '{}' not found", dispute_id))
        })?;

        if let Some(ref session_id) = dispute.session_id {
            let purchase_res = self
                .verification_service
                .verify_session(session_id)
                .await
                .map_err(|e| AppError::DisputeAuditIntegrity(e.to_string()))?;

            if purchase_res.status != VerificationStatus::Valid {
                let mut errors = dispute_res.errors;
                errors.push(format!(
                    "Dual-chain check failed: linked purchase session '{}' is invalid: {}",
                    session_id, purchase_res.explanation
                ));

                return Ok(DisputeVerificationResult::invalid(
                    dispute_id,
                    dispute_res.total_steps,
                    dispute.genesis_anchor_type,
                    dispute.genesis_anchor_hash,
                    dispute.purchase_root_hash,
                    dispute.root_hash.unwrap_or_default(),
                    dispute_res.computed_root_hash,
                    errors,
                ));
            }
        }

        Ok(dispute_res)
    }

    // --- Internal Helpers ---

    async fn get_dispute_tx(
        &self,
        conn: &mut sqlx::SqliteConnection,
        dispute_id: &str,
    ) -> Result<Option<DisputeRecord>, AppError> {
        sqlx::query_as::<_, DisputeRecord>(
            r#"
            SELECT dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                   purchase_root_hash, seller_transaction_id, paypal_order_id,
                   status, dispute_lifecycle_stage, dispute_channel, reason,
                   dispute_amount_value, dispute_amount_currency, correlation_status,
                   matched_by, root_hash, dispute_create_time, dispute_update_time,
                   created_at, updated_at
            FROM disputes
            WHERE dispute_id = ?;
            "#,
        )
        .bind(dispute_id)
        .fetch_optional(conn)
        .await
        .map_err(AppError::from)
    }

    async fn get_latest_audit_step_tx(
        &self,
        conn: &mut sqlx::SqliteConnection,
        dispute_id: &str,
    ) -> Result<Option<DisputeAuditStep>, AppError> {
        sqlx::query_as::<_, DisputeAuditStep>(
            r#"
            SELECT id, dispute_id, sequence_index, action_type, payload,
                   previous_hash, current_hash, created_at
            FROM dispute_audit_steps
            WHERE dispute_id = ?
            ORDER BY sequence_index DESC
            LIMIT 1;
            "#,
        )
        .bind(dispute_id)
        .fetch_optional(conn)
        .await
        .map_err(AppError::from)
    }

    async fn insert_initial_dispute_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        payload: &UpsertDisputePayload,
    ) -> Result<DisputeRecord, AppError> {
        let (genesis_anchor_type, genesis_anchor_hash, purchase_root_hash) =
            if let Some(ref session_id) = payload.session_id {
                let verification = self
                    .verification_service
                    .verify_session(session_id)
                    .await
                    .map_err(|e| match e {
                        AppError::SessionNotFound(_) => AppError::SessionNotFound(format!(
                            "Purchase session '{}' not found",
                            session_id
                        )),
                        other => other,
                    })?;

                if verification.status != VerificationStatus::Valid {
                    return Err(AppError::DisputeAuditIntegrity(format!(
                        "Linked purchase session '{}' failed ledger verification: {}",
                        session_id, verification.explanation
                    )));
                }

                let verified_root = verification.stored_root_hash.ok_or_else(|| {
                    AppError::DisputeAuditIntegrity(format!(
                        "Linked purchase session '{}' is not finalized",
                        session_id
                    ))
                })?;

                (
                    GenesisAnchorType::PurchaseRootHash,
                    verified_root.clone(),
                    Some(verified_root),
                )
            } else {
                (
                    GenesisAnchorType::GenesisHash,
                    GENESIS_HASH_DEFAULT.to_string(),
                    None,
                )
            };

        // Determine action type and correlation status based on timestamps
        let has_authoritative_timestamp =
            payload.dispute_update_time.is_some() || payload.dispute_create_time.is_some();

        let (action_type, correlation_status) = if has_authoritative_timestamp {
            (
                DisputeActionType::DISPUTE_IMPORTED,
                payload.correlation_status,
            )
        } else {
            (
                DisputeActionType::DISPUTE_TIMESTAMP_AMBIGUOUS_RECEIVED,
                CorrelationStatus::ManualReviewRequired,
            )
        };

        let now_iso = chrono::Utc::now().to_rfc3339();

        let audit_payload = serde_json::json!({
            "dispute_id": &payload.dispute_id,
            "session_id": &payload.session_id,
            "status": &payload.status,
            "dispute_lifecycle_stage": &payload.dispute_lifecycle_stage,
            "dispute_channel": &payload.dispute_channel,
            "reason": &payload.reason,
            "dispute_amount_value": &payload.dispute_amount_value,
            "dispute_amount_currency": &payload.dispute_amount_currency,
            "dispute_create_time": &payload.dispute_create_time,
            "dispute_update_time": &payload.dispute_update_time,
            "seller_transaction_id": &payload.seller_transaction_id,
            "paypal_order_id": &payload.paypal_order_id,
            "raw_event_payload": &payload.raw_event_payload,
        });

        let canonical_bytes = to_canonical_json_bytes(&audit_payload)?;
        let canonical_str = to_canonical_json_string(&audit_payload)?;

        let step_hash = compute_event_hash(0, action_type, &canonical_bytes, &genesis_anchor_hash);

        let genesis_anchor_type_str = genesis_anchor_type.as_str();
        let correlation_status_str = correlation_status.as_str();
        let matched_by_str = payload.matched_by.map(|m| m.to_string());

        // Insert dispute
        sqlx::query(
            r#"
            INSERT INTO disputes (
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                purchase_root_hash, seller_transaction_id, paypal_order_id,
                status, dispute_lifecycle_stage, dispute_channel, reason,
                dispute_amount_value, dispute_amount_currency, correlation_status,
                matched_by, root_hash, dispute_create_time, dispute_update_time,
                created_at, updated_at
            ) VALUES (
                ?, ?, ?, ?,
                ?, ?, ?,
                ?, ?, ?, ?,
                ?, ?, ?,
                ?, ?, ?, ?,
                ?, ?
            );
            "#,
        )
        .bind(&payload.dispute_id)
        .bind(&payload.session_id)
        .bind(genesis_anchor_type_str)
        .bind(&genesis_anchor_hash)
        .bind(&purchase_root_hash)
        .bind(&payload.seller_transaction_id)
        .bind(&payload.paypal_order_id)
        .bind(&payload.status)
        .bind(&payload.dispute_lifecycle_stage)
        .bind(&payload.dispute_channel)
        .bind(&payload.reason)
        .bind(&payload.dispute_amount_value)
        .bind(&payload.dispute_amount_currency)
        .bind(correlation_status_str)
        .bind(matched_by_str.as_deref())
        .bind(&step_hash)
        .bind(&payload.dispute_create_time)
        .bind(&payload.dispute_update_time)
        .bind(&now_iso)
        .bind(&now_iso)
        .execute(&mut **tx)
        .await?;

        // Insert step 0
        let step_id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO dispute_audit_steps (
                id, dispute_id, sequence_index, action_type, payload,
                previous_hash, current_hash, created_at
            ) VALUES (?, ?, 0, ?, ?, ?, ?, ?);
            "#,
        )
        .bind(&step_id)
        .bind(&payload.dispute_id)
        .bind(action_type)
        .bind(&canonical_str)
        .bind(&genesis_anchor_hash)
        .bind(&step_hash)
        .bind(&now_iso)
        .execute(&mut **tx)
        .await?;

        self.get_dispute_tx(&mut **tx, &payload.dispute_id)
            .await?
            .ok_or_else(|| {
                AppError::DisputeNotFound(format!(
                    "Dispute '{}' not found immediately after insertion",
                    payload.dispute_id
                ))
            })
    }

    async fn update_existing_dispute_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        payload: &UpsertDisputePayload,
        existing: DisputeRecord,
    ) -> Result<DisputeRecord, AppError> {
        let last_step = self
            .get_latest_audit_step_tx(&mut **tx, &existing.dispute_id)
            .await?
            .ok_or_else(|| {
                AppError::DisputeAuditIntegrity(format!(
                    "Dispute '{}' has no audit steps",
                    existing.dispute_id
                ))
            })?;

        let next_seq = last_step.sequence_index + 1;
        let prev_hash = last_step.current_hash.clone();
        let now_iso = chrono::Utc::now().to_rfc3339();

        // 1. Check for missing authoritative update timestamp
        let incoming_ts_opt = payload.dispute_update_time.as_deref();

        let incoming_ts = match incoming_ts_opt {
            Some(ts) => ts,
            None => {
                // Missing authoritative update timestamp -> flag MANUAL_REVIEW_REQUIRED and append DISPUTE_TIMESTAMP_AMBIGUOUS_RECEIVED
                let audit_payload = serde_json::json!({
                    "dispute_id": &existing.dispute_id,
                    "incoming_payload": {
                        "status": &payload.status,
                        "dispute_lifecycle_stage": &payload.dispute_lifecycle_stage,
                        "dispute_channel": &payload.dispute_channel,
                        "reason": &payload.reason,
                        "dispute_amount_value": &payload.dispute_amount_value,
                        "dispute_amount_currency": &payload.dispute_amount_currency,
                        "raw_event_payload": &payload.raw_event_payload,
                    },
                    "reason": "Missing authoritative dispute_update_time in event delivery",
                });

                let canonical_bytes = to_canonical_json_bytes(&audit_payload)?;
                let canonical_str = to_canonical_json_string(&audit_payload)?;

                let new_step_hash = compute_event_hash(
                    next_seq,
                    DisputeActionType::DISPUTE_TIMESTAMP_AMBIGUOUS_RECEIVED,
                    &canonical_bytes,
                    &prev_hash,
                );

                let step_id = Uuid::new_v4().to_string();
                sqlx::query(
                    r#"
                    INSERT INTO dispute_audit_steps (
                        id, dispute_id, sequence_index, action_type, payload,
                        previous_hash, current_hash, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);
                    "#,
                )
                .bind(&step_id)
                .bind(&existing.dispute_id)
                .bind(next_seq)
                .bind(DisputeActionType::DISPUTE_TIMESTAMP_AMBIGUOUS_RECEIVED)
                .bind(&canonical_str)
                .bind(&prev_hash)
                .bind(&new_step_hash)
                .bind(&now_iso)
                .execute(&mut **tx)
                .await?;

                sqlx::query(
                    r#"
                    UPDATE disputes
                    SET correlation_status = 'MANUAL_REVIEW_REQUIRED',
                        root_hash = ?,
                        updated_at = ?
                    WHERE dispute_id = ?;
                    "#,
                )
                .bind(&new_step_hash)
                .bind(&now_iso)
                .bind(&existing.dispute_id)
                .execute(&mut **tx)
                .await?;

                return self
                    .get_dispute_tx(&mut **tx, &existing.dispute_id)
                    .await?
                    .ok_or_else(|| {
                        AppError::DisputeNotFound(format!(
                            "Dispute '{}' not found after update",
                            existing.dispute_id
                        ))
                    });
            }
        };

        // 2. Compare incoming timestamp with existing authoritative timestamp
        let existing_authoritative_ts = existing
            .dispute_update_time
            .as_deref()
            .or(existing.dispute_create_time.as_deref());

        let comparison = match existing_authoritative_ts {
            None => TimestampOrdering::Newer,
            Some(existing_ts) => compare_timestamps(incoming_ts, existing_ts),
        };

        match comparison {
            TimestampOrdering::Newer => {
                // Advance dispute state
                let audit_payload = serde_json::json!({
                    "dispute_id": &existing.dispute_id,
                    "previous_status": &existing.status,
                    "new_status": &payload.status,
                    "dispute_lifecycle_stage": &payload.dispute_lifecycle_stage,
                    "dispute_channel": &payload.dispute_channel,
                    "reason": &payload.reason,
                    "dispute_amount_value": &payload.dispute_amount_value,
                    "dispute_amount_currency": &payload.dispute_amount_currency,
                    "dispute_update_time": incoming_ts,
                    "raw_event_payload": &payload.raw_event_payload,
                });

                let canonical_bytes = to_canonical_json_bytes(&audit_payload)?;
                let canonical_str = to_canonical_json_string(&audit_payload)?;

                let new_step_hash = compute_event_hash(
                    next_seq,
                    DisputeActionType::DISPUTE_UPDATED,
                    &canonical_bytes,
                    &prev_hash,
                );

                let step_id = Uuid::new_v4().to_string();
                sqlx::query(
                    r#"
                    INSERT INTO dispute_audit_steps (
                        id, dispute_id, sequence_index, action_type, payload,
                        previous_hash, current_hash, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);
                    "#,
                )
                .bind(&step_id)
                .bind(&existing.dispute_id)
                .bind(next_seq)
                .bind(DisputeActionType::DISPUTE_UPDATED)
                .bind(&canonical_str)
                .bind(&prev_hash)
                .bind(&new_step_hash)
                .bind(&now_iso)
                .execute(&mut **tx)
                .await?;

                let new_reason = payload.reason.clone().or(existing.reason);
                let new_stage = payload
                    .dispute_lifecycle_stage
                    .clone()
                    .or(existing.dispute_lifecycle_stage);
                let new_channel = payload.dispute_channel.clone().or(existing.dispute_channel);
                let new_amount_val = payload
                    .dispute_amount_value
                    .clone()
                    .or(existing.dispute_amount_value);
                let new_amount_cur = payload
                    .dispute_amount_currency
                    .clone()
                    .or(existing.dispute_amount_currency);

                sqlx::query(
                    r#"
                    UPDATE disputes
                    SET status = ?,
                        dispute_lifecycle_stage = ?,
                        dispute_channel = ?,
                        reason = ?,
                        dispute_amount_value = ?,
                        dispute_amount_currency = ?,
                        dispute_update_time = ?,
                        root_hash = ?,
                        updated_at = ?
                    WHERE dispute_id = ?;
                    "#,
                )
                .bind(&payload.status)
                .bind(&new_stage)
                .bind(&new_channel)
                .bind(&new_reason)
                .bind(&new_amount_val)
                .bind(&new_amount_cur)
                .bind(incoming_ts)
                .bind(&new_step_hash)
                .bind(&now_iso)
                .bind(&existing.dispute_id)
                .execute(&mut **tx)
                .await?;
            }
            TimestampOrdering::Older => {
                // Out-of-order received: preserve state, record event
                let audit_payload = serde_json::json!({
                    "dispute_id": &existing.dispute_id,
                    "incoming_payload": {
                        "status": &payload.status,
                        "dispute_lifecycle_stage": &payload.dispute_lifecycle_stage,
                        "dispute_channel": &payload.dispute_channel,
                        "reason": &payload.reason,
                        "dispute_amount_value": &payload.dispute_amount_value,
                        "dispute_amount_currency": &payload.dispute_amount_currency,
                        "dispute_update_time": incoming_ts,
                        "raw_event_payload": &payload.raw_event_payload,
                    },
                    "reason": "Stale event with older authoritative timestamp received out of order",
                });

                let canonical_bytes = to_canonical_json_bytes(&audit_payload)?;
                let canonical_str = to_canonical_json_string(&audit_payload)?;

                let new_step_hash = compute_event_hash(
                    next_seq,
                    DisputeActionType::DISPUTE_OUT_OF_ORDER_RECEIVED,
                    &canonical_bytes,
                    &prev_hash,
                );

                let step_id = Uuid::new_v4().to_string();
                sqlx::query(
                    r#"
                    INSERT INTO dispute_audit_steps (
                        id, dispute_id, sequence_index, action_type, payload,
                        previous_hash, current_hash, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);
                    "#,
                )
                .bind(&step_id)
                .bind(&existing.dispute_id)
                .bind(next_seq)
                .bind(DisputeActionType::DISPUTE_OUT_OF_ORDER_RECEIVED)
                .bind(&canonical_str)
                .bind(&prev_hash)
                .bind(&new_step_hash)
                .bind(&now_iso)
                .execute(&mut **tx)
                .await?;

                sqlx::query(
                    r#"
                    UPDATE disputes
                    SET root_hash = ?,
                        updated_at = ?
                    WHERE dispute_id = ?;
                    "#,
                )
                .bind(&new_step_hash)
                .bind(&now_iso)
                .bind(&existing.dispute_id)
                .execute(&mut **tx)
                .await?;
            }
            TimestampOrdering::Equal => {
                let is_identical = are_payloads_identical(payload, &existing, Some(&last_step));

                if is_identical {
                    // Idempotent no-op
                    return Ok(existing);
                } else {
                    // Conflicting payload with equal timestamp -> flag MANUAL_REVIEW_REQUIRED and append DISPUTE_CONCURRENT_CONFLICT_DETECTED
                    let audit_payload = serde_json::json!({
                        "dispute_id": &existing.dispute_id,
                        "incoming_payload": {
                            "status": &payload.status,
                            "dispute_lifecycle_stage": &payload.dispute_lifecycle_stage,
                            "dispute_channel": &payload.dispute_channel,
                            "reason": &payload.reason,
                            "dispute_amount_value": &payload.dispute_amount_value,
                            "dispute_amount_currency": &payload.dispute_amount_currency,
                            "dispute_update_time": incoming_ts,
                            "raw_event_payload": &payload.raw_event_payload,
                        },
                        "reason": "Concurrent conflicting payload with equal authoritative timestamp detected",
                    });

                    let canonical_bytes = to_canonical_json_bytes(&audit_payload)?;
                    let canonical_str = to_canonical_json_string(&audit_payload)?;

                    let new_step_hash = compute_event_hash(
                        next_seq,
                        DisputeActionType::DISPUTE_CONCURRENT_CONFLICT_DETECTED,
                        &canonical_bytes,
                        &prev_hash,
                    );

                    let step_id = Uuid::new_v4().to_string();
                    sqlx::query(
                        r#"
                        INSERT INTO dispute_audit_steps (
                            id, dispute_id, sequence_index, action_type, payload,
                            previous_hash, current_hash, created_at
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);
                        "#,
                    )
                    .bind(&step_id)
                    .bind(&existing.dispute_id)
                    .bind(next_seq)
                    .bind(DisputeActionType::DISPUTE_CONCURRENT_CONFLICT_DETECTED)
                    .bind(&canonical_str)
                    .bind(&prev_hash)
                    .bind(&new_step_hash)
                    .bind(&now_iso)
                    .execute(&mut **tx)
                    .await?;

                    sqlx::query(
                        r#"
                        UPDATE disputes
                        SET correlation_status = 'MANUAL_REVIEW_REQUIRED',
                            root_hash = ?,
                            updated_at = ?
                        WHERE dispute_id = ?;
                        "#,
                    )
                    .bind(&new_step_hash)
                    .bind(&now_iso)
                    .bind(&existing.dispute_id)
                    .execute(&mut **tx)
                    .await?;
                }
            }
        }

        self.get_dispute_tx(&mut **tx, &existing.dispute_id)
            .await?
            .ok_or_else(|| {
                AppError::DisputeNotFound(format!(
                    "Dispute '{}' not found after mutation",
                    existing.dispute_id
                ))
            })
    }
}

// --- Internal Helper Functions ---

#[derive(Debug, PartialEq, Eq)]
enum TimestampOrdering {
    Newer,
    Older,
    Equal,
}

fn parse_authoritative_timestamp(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%SZ")
                .map(|ndt| {
                    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(ndt, chrono::Utc)
                })
                .ok()
        })
}

fn compare_timestamps(incoming_str: &str, existing_str: &str) -> TimestampOrdering {
    match (
        parse_authoritative_timestamp(incoming_str),
        parse_authoritative_timestamp(existing_str),
    ) {
        (Some(inc), Some(ex)) => {
            if inc > ex {
                TimestampOrdering::Newer
            } else if inc < ex {
                TimestampOrdering::Older
            } else {
                TimestampOrdering::Equal
            }
        }
        _ => match incoming_str.cmp(existing_str) {
            std::cmp::Ordering::Greater => TimestampOrdering::Newer,
            std::cmp::Ordering::Less => TimestampOrdering::Older,
            std::cmp::Ordering::Equal => TimestampOrdering::Equal,
        },
    }
}

fn are_payloads_identical(
    incoming: &UpsertDisputePayload,
    existing: &DisputeRecord,
    last_audit_step: Option<&DisputeAuditStep>,
) -> bool {
    if incoming.status != existing.status {
        return false;
    }
    if incoming.dispute_lifecycle_stage != existing.dispute_lifecycle_stage {
        return false;
    }
    if incoming.dispute_channel != existing.dispute_channel {
        return false;
    }
    if incoming.reason != existing.reason {
        return false;
    }
    if incoming.dispute_amount_value != existing.dispute_amount_value {
        return false;
    }
    if incoming.dispute_amount_currency != existing.dispute_amount_currency {
        return false;
    }

    if let (Some(incoming_raw), Some(last_step)) = (&incoming.raw_event_payload, last_audit_step) {
        if let Ok(step_json) = serde_json::from_str::<serde_json::Value>(&last_step.payload) {
            let existing_raw = step_json.get("raw_event_payload").or_else(|| {
                step_json
                    .get("incoming_payload")
                    .and_then(|p| p.get("raw_event_payload"))
            });
            if let Some(existing_raw) = existing_raw {
                if !existing_raw.is_null() && incoming_raw != existing_raw {
                    return false;
                }
            }
        }
    }

    true
}
