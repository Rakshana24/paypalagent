use sqlx::SqlitePool;
use tracing::warn;

use crate::error::AppError;
use crate::ledger::hash::{compute_event_hash, to_canonical_json_bytes, GENESIS_HASH};
use crate::models::{DecisionStep, Session, VerificationResult, VerificationStatus};

/// Cryptographic verification service to detect tampering or data anomalies in the session ledger.
#[derive(Clone)]
pub struct VerificationService {
    pool: SqlitePool,
}

impl VerificationService {
    /// Creates a new `VerificationService` instance with the given database pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Validates the complete cryptographic hash chain for a given session.
    pub async fn verify_session(&self, session_id: &str) -> Result<VerificationResult, AppError> {
        // 1. Fetch session
        let session = sqlx::query_as::<_, Session>(
            r#"
            SELECT session_id, initial_prompt, paypal_order_id, root_hash, created_at, updated_at
            FROM sessions
            WHERE session_id = ?;
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' does not exist", session_id))
        })?;

        // 2. Fetch all decision steps in strictly ascending sequence index order
        let steps = sqlx::query_as::<_, DecisionStep>(
            r#"
            SELECT id, session_id, sequence_index, action_type, payload, previous_hash, current_hash, created_at
            FROM decision_steps
            WHERE session_id = ?
            ORDER BY sequence_index ASC;
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        if steps.is_empty() {
            warn!(session_id = %session_id, "Verification failed: empty decision chain");
            return Ok(VerificationResult {
                session_id: session_id.to_string(),
                status: VerificationStatus::Tampered,
                total_steps: 0,
                verified_steps: 0,
                calculated_root_hash: None,
                stored_root_hash: session.root_hash,
                first_invalid_sequence_index: Some(0),
                error_code: Some("EMPTY_CHAIN".to_string()),
                explanation: "No events recorded in session ledger".to_string(),
            });
        }

        let mut verified_count = 0;

        for (idx, step) in steps.iter().enumerate() {
            let expected_seq = idx as i64;

            // Check A: Sequence indices are continuous and start at 0
            if step.sequence_index != expected_seq {
                warn!(
                    session_id = %session_id,
                    expected = expected_seq,
                    actual = step.sequence_index,
                    "Sequence continuity broken"
                );
                return Ok(VerificationResult {
                    session_id: session_id.to_string(),
                    status: VerificationStatus::Tampered,
                    total_steps: steps.len(),
                    verified_steps: verified_count,
                    calculated_root_hash: None,
                    stored_root_hash: session.root_hash,
                    first_invalid_sequence_index: Some(step.sequence_index),
                    error_code: Some("SEQUENCE_ORDER_INVALID".to_string()),
                    explanation: format!(
                        "Sequence discontinuity at position {}: expected index {}, found {}",
                        idx, expected_seq, step.sequence_index
                    ),
                });
            }

            // Check B: Previous hash links
            if idx == 0 {
                if step.previous_hash != GENESIS_HASH {
                    warn!(
                        session_id = %session_id,
                        prev_hash = %step.previous_hash,
                        "Genesis previous hash mismatch"
                    );
                    return Ok(VerificationResult {
                        session_id: session_id.to_string(),
                        status: VerificationStatus::Tampered,
                        total_steps: steps.len(),
                        verified_steps: 0,
                        calculated_root_hash: None,
                        stored_root_hash: session.root_hash,
                        first_invalid_sequence_index: Some(0),
                        error_code: Some("GENESIS_HASH_MISMATCH".to_string()),
                        explanation: format!(
                            "Sequence 0 previous hash does not match genesis hash: '{}'",
                            step.previous_hash
                        ),
                    });
                }
            } else {
                let prev_expected_hash = &steps[idx - 1].current_hash;
                if &step.previous_hash != prev_expected_hash {
                    warn!(
                        session_id = %session_id,
                        sequence = step.sequence_index,
                        expected = %prev_expected_hash,
                        actual = %step.previous_hash,
                        "Chain link broken between sequential steps"
                    );
                    return Ok(VerificationResult {
                        session_id: session_id.to_string(),
                        status: VerificationStatus::Tampered,
                        total_steps: steps.len(),
                        verified_steps: verified_count,
                        calculated_root_hash: None,
                        stored_root_hash: session.root_hash,
                        first_invalid_sequence_index: Some(step.sequence_index),
                        error_code: Some("CHAIN_LINK_BROKEN".to_string()),
                        explanation: format!(
                            "Step {} previous hash '{}' does not match step {} current hash '{}'",
                            step.sequence_index,
                            step.previous_hash,
                            steps[idx - 1].sequence_index,
                            prev_expected_hash
                        ),
                    });
                }
            }

            // Check C: Cryptographic payload hash reproduction and RFC 8785 canonical validation
            let parsed_payload: Result<serde_json::Value, _> = serde_json::from_str(&step.payload);
            let canonical_payload_bytes = match parsed_payload {
                Ok(ref val) => match to_canonical_json_bytes(val) {
                    Ok(bytes) => {
                        if bytes != step.payload.as_bytes() {
                            warn!(
                                session_id = %session_id,
                                sequence = step.sequence_index,
                                "Payload in database violates RFC 8785 canonical formatting"
                            );
                            return Ok(VerificationResult {
                                session_id: session_id.to_string(),
                                status: VerificationStatus::Tampered,
                                total_steps: steps.len(),
                                verified_steps: verified_count,
                                calculated_root_hash: None,
                                stored_root_hash: session.root_hash,
                                first_invalid_sequence_index: Some(step.sequence_index),
                                error_code: Some("PAYLOAD_NON_CANONICAL".to_string()),
                                explanation: format!(
                                    "Payload at step {} violates RFC 8785 canonical formatting",
                                    step.sequence_index
                                ),
                            });
                        }
                        bytes
                    }
                    Err(e) => {
                        return Ok(VerificationResult {
                            session_id: session_id.to_string(),
                            status: VerificationStatus::Tampered,
                            total_steps: steps.len(),
                            verified_steps: verified_count,
                            calculated_root_hash: None,
                            stored_root_hash: session.root_hash,
                            first_invalid_sequence_index: Some(step.sequence_index),
                            error_code: Some("PAYLOAD_CANONICALIZATION_FAILED".to_string()),
                            explanation: format!(
                                "Failed to canonicalize payload at step {}: {}",
                                step.sequence_index, e
                            ),
                        });
                    }
                },
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        sequence = step.sequence_index,
                        err = %e,
                        "Malformed JSON payload in ledger step"
                    );
                    return Ok(VerificationResult {
                        session_id: session_id.to_string(),
                        status: VerificationStatus::Tampered,
                        total_steps: steps.len(),
                        verified_steps: verified_count,
                        calculated_root_hash: None,
                        stored_root_hash: session.root_hash,
                        first_invalid_sequence_index: Some(step.sequence_index),
                        error_code: Some("INVALID_PAYLOAD_JSON".to_string()),
                        explanation: format!(
                            "Payload at step {} is not valid JSON: {}",
                            step.sequence_index, e
                        ),
                    });
                }
            };

            let recomputed_hash = compute_event_hash(
                step.sequence_index,
                &step.action_type,
                &canonical_payload_bytes,
                &step.previous_hash,
            );

            if recomputed_hash != step.current_hash {
                warn!(
                    session_id = %session_id,
                    sequence = step.sequence_index,
                    recomputed = %recomputed_hash,
                    stored = %step.current_hash,
                    "Cryptographic digest mismatch for step"
                );
                return Ok(VerificationResult {
                    session_id: session_id.to_string(),
                    status: VerificationStatus::Tampered,
                    total_steps: steps.len(),
                    verified_steps: verified_count,
                    calculated_root_hash: None,
                    stored_root_hash: session.root_hash,
                    first_invalid_sequence_index: Some(step.sequence_index),
                    error_code: Some("DIGEST_MISMATCH".to_string()),
                    explanation: format!(
                        "Hash recalculation mismatch at step {}: expected '{}', found '{}'",
                        step.sequence_index, recomputed_hash, step.current_hash
                    ),
                });
            }

            verified_count += 1;
        }

        let calculated_root = steps.last().unwrap().current_hash.clone();

        // Check D: If finalized, stored root_hash must match calculated root
        if let Some(ref stored_root) = session.root_hash {
            if &calculated_root != stored_root {
                warn!(
                    session_id = %session_id,
                    calculated = %calculated_root,
                    stored = %stored_root,
                    "Session root hash mismatch"
                );
                return Ok(VerificationResult {
                    session_id: session_id.to_string(),
                    status: VerificationStatus::Tampered,
                    total_steps: steps.len(),
                    verified_steps: verified_count,
                    calculated_root_hash: Some(calculated_root.clone()),
                    stored_root_hash: Some(stored_root.clone()),
                    first_invalid_sequence_index: Some(steps.len() as i64 - 1),
                    error_code: Some("ROOT_HASH_MISMATCH".to_string()),
                    explanation: format!(
                        "Calculated root hash '{}' does not match finalized session root hash '{}'",
                        calculated_root, stored_root
                    ),
                });
            }
        }

        // All checks successfully passed
        Ok(VerificationResult {
            session_id: session_id.to_string(),
            status: VerificationStatus::Valid,
            total_steps: steps.len(),
            verified_steps: verified_count,
            calculated_root_hash: Some(calculated_root),
            stored_root_hash: session.root_hash,
            first_invalid_sequence_index: None,
            error_code: None,
            explanation: "Cryptographic hash chain is verified and continuous".to_string(),
        })
    }
}
