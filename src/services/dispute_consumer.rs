use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::error::AppError;
use crate::models::correlation::CorrelationStatus;
use crate::models::dispute::{DisputeRecord, GenesisAnchorType, UpsertDisputePayload};
use crate::models::webhook::{PayPalWebhookEvent, WebhookEventRecord};
use crate::services::dispute_correlation::DisputeCorrelationService;
use crate::services::dispute_service::DisputeService;
use crate::services::webhook_persistence::WebhookPersistenceService;

/// Outcome of processing a single leased PayPal webhook event by the consumer.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsumerProcessingOutcome {
    /// Webhook event was successfully processed into a dispute mutation and marked PROCESSED.
    Processed {
        event_id: String,
        dispute_id: String,
        dispute: DisputeRecord,
    },
    /// Webhook event was not a dispute event and was marked IGNORED without creating dispute records.
    Ignored { event_id: String, reason: String },
}

/// Service that consumes durably persisted PayPal dispute webhook events,
/// processes them through `DisputeService`, and coordinates atomic commit
/// with lease fencing.
#[derive(Clone)]
pub struct DisputeConsumerService {
    pool: SqlitePool,
    persistence_service: Arc<WebhookPersistenceService>,
    dispute_service: Arc<DisputeService>,
    correlation_service: Arc<DisputeCorrelationService>,
}

impl DisputeConsumerService {
    /// Creates a new `DisputeConsumerService` instance with required domain services.
    pub fn new(
        pool: SqlitePool,
        persistence_service: Arc<WebhookPersistenceService>,
        dispute_service: Arc<DisputeService>,
        correlation_service: Arc<DisputeCorrelationService>,
    ) -> Self {
        Self {
            pool,
            persistence_service,
            dispute_service,
            correlation_service,
        }
    }

    /// Acquires an exclusive processing lease on the next available webhook event
    /// and processes it to completion or scheduled retry.
    ///
    /// Returns `Ok(Some(outcome))` if an event was processed, or `Ok(None)` if no events are ready.
    pub async fn process_next_event(
        &self,
        worker_id: &str,
        lease_duration_secs: u64,
    ) -> Result<Option<ConsumerProcessingOutcome>, AppError> {
        let leased_event = match self
            .persistence_service
            .acquire_lease(worker_id, lease_duration_secs)
            .await?
        {
            Some(record) => record,
            None => return Ok(None),
        };

        let outcome = self.process_leased_event(leased_event).await?;
        Ok(Some(outcome))
    }

    /// Processes an already-leased webhook event through dispute correlation, dispute ingestion,
    /// and fenced webhook status transition within an atomic database transaction.
    pub async fn process_leased_event(
        &self,
        record: WebhookEventRecord,
    ) -> Result<ConsumerProcessingOutcome, AppError> {
        let event_id = record.event_id.clone();
        let lease_token = record.lease_token.clone().ok_or_else(|| {
            AppError::DisputeLeaseFencedOut(format!(
                "Event '{}' is missing a valid lease token",
                event_id
            ))
        })?;

        // 1. Parse raw payload bytes into PayPalWebhookEvent
        let webhook_event: PayPalWebhookEvent = match serde_json::from_slice(&record.raw_payload) {
            Ok(ev) => ev,
            Err(err) => {
                let err_msg = format!("Malformed webhook JSON payload: {}", err);
                warn!(event_id = %event_id, error = %err_msg, "Failed to parse webhook JSON; scheduling retry");
                let _ = self
                    .persistence_service
                    .schedule_retry_fenced(&event_id, &lease_token, &err_msg)
                    .await;
                return Err(AppError::Serialization(err));
            }
        };

        // 2. Validate event type: only process dispute events
        // Unrelated event types cannot be interpreted as disputes merely because of misleading resource fields.
        let is_dispute_event = webhook_event.event_type.starts_with("CUSTOMER.DISPUTE.");

        if !is_dispute_event {
            let reason = format!(
                "Unsupported webhook event type '{}' (resource type: '{}')",
                webhook_event.event_type, webhook_event.resource_type
            );
            info!(event_id = %event_id, reason = %reason, "Ignoring non-dispute webhook event");
            self.persistence_service
                .mark_ignored_fenced(&event_id, &lease_token, &reason)
                .await?;
            return Ok(ConsumerProcessingOutcome::Ignored { event_id, reason });
        }

        // 3. Extract and validate required dispute fields
        let extracted_id = webhook_event.extract_resource_id();
        let extracted_status = webhook_event
            .resource
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let (dispute_id, status) = match (extracted_id, extracted_status) {
            (Some(did), Some(st)) if !did.is_empty() && !st.is_empty() => (did, st),
            (did, st) => {
                let err_msg = if did.as_deref().unwrap_or("").is_empty() {
                    "Dispute webhook resource missing required field: dispute_id".to_string()
                } else if st.as_deref().unwrap_or("").is_empty() {
                    "Dispute webhook resource missing required field: status".to_string()
                } else {
                    "Dispute webhook resource missing required dispute fields".to_string()
                };

                warn!(event_id = %event_id, error = %err_msg, "Incomplete dispute payload; scheduling retry");
                let _ = self
                    .persistence_service
                    .schedule_retry_fenced(&event_id, &lease_token, &err_msg)
                    .await;
                return Err(AppError::InvalidDisputeData(err_msg));
            }
        };

        // 4. Extract optional dispute fields
        let dispute_lifecycle_stage = webhook_event
            .resource
            .get("dispute_life_cycle_stage")
            .or_else(|| webhook_event.resource.get("dispute_lifecycle_stage"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let dispute_channel = webhook_event
            .resource
            .get("dispute_channel")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let reason = webhook_event
            .resource
            .get("reason")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let dispute_amount_value = webhook_event
            .resource
            .get("dispute_amount")
            .and_then(|v| v.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let dispute_amount_currency = webhook_event
            .resource
            .get("dispute_amount")
            .and_then(|v| v.get("currency_code"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let dispute_create_time = webhook_event
            .resource
            .get("create_time")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let dispute_update_time = webhook_event
            .resource
            .get("update_time")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let seller_transaction_id = webhook_event
            .resource
            .get("seller_transaction_id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .or_else(|| {
                webhook_event
                    .resource
                    .get("disputed_transactions")
                    .and_then(|arr| arr.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|item| item.get("seller_transaction_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
            });

        let paypal_order_id = webhook_event
            .resource
            .get("paypal_order_id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .or_else(|| {
                webhook_event
                    .resource
                    .get("disputed_transactions")
                    .and_then(|arr| arr.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|item| item.get("paypal_order_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
            });

        // 5. Deterministic local correlation (no external PayPal API calls)
        let correlation_result = self
            .correlation_service
            .correlate_locally(&webhook_event.resource)
            .await?;

        let (session_id, correlation_status, matched_by) = match correlation_result.status {
            CorrelationStatus::Matched => (
                correlation_result.session_id,
                CorrelationStatus::Matched,
                correlation_result.matched_by,
            ),
            CorrelationStatus::ManualReviewRequired => {
                (None, CorrelationStatus::ManualReviewRequired, None)
            }
            CorrelationStatus::NotFound => (None, CorrelationStatus::NotFound, None),
        };

        // 6. Build UpsertDisputePayload
        let upsert_payload = UpsertDisputePayload {
            dispute_id: dispute_id.clone(),
            session_id: session_id.clone(),
            genesis_anchor_type: if session_id.is_some() {
                GenesisAnchorType::PurchaseRootHash
            } else {
                GenesisAnchorType::GenesisHash
            },
            genesis_anchor_hash: String::new(), // Populated by DisputeService
            purchase_root_hash: None,           // Populated by DisputeService
            seller_transaction_id,
            paypal_order_id,
            status,
            dispute_lifecycle_stage,
            dispute_channel,
            reason,
            dispute_amount_value,
            dispute_amount_currency,
            correlation_status,
            matched_by,
            dispute_create_time,
            dispute_update_time,
            raw_event_payload: Some(webhook_event.resource.clone()),
        };

        // 7. Atomic transaction coordinating dispute mutation and fenced webhook completion
        // Explicitly acquire BEGIN IMMEDIATE to serialize write access and prevent SQLite lock-upgrade deadlocks
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        // 8. Process dispute upsert in transaction
        let mut dispute_record = match self
            .dispute_service
            .upsert_dispute_tx(&mut tx, &upsert_payload)
            .await
        {
            Ok(record) => record,
            Err(err) => {
                warn!(event_id = %event_id, error = %err, "Dispute upsert failed; rolling back transaction");
                let _ = tx.rollback().await;
                let _ = self
                    .persistence_service
                    .schedule_retry_fenced(&event_id, &lease_token, &err.to_string())
                    .await;
                return Err(err);
            }
        };

        // 9. If existing dispute was unlinked and now correlated, perform late correlation in same transaction
        if dispute_record.session_id.is_none() {
            if let (Some(ref sess_id), Some(mb)) = (&session_id, matched_by) {
                match self
                    .dispute_service
                    .correlate_dispute_late_tx(&mut tx, &dispute_id, sess_id, mb)
                    .await
                {
                    Ok(updated_record) => {
                        dispute_record = updated_record;
                    }
                    Err(err) => {
                        warn!(event_id = %event_id, error = %err, "Late correlation failed; rolling back transaction");
                        let _ = tx.rollback().await;
                        let _ = self
                            .persistence_service
                            .schedule_retry_fenced(&event_id, &lease_token, &err.to_string())
                            .await;
                        return Err(err);
                    }
                }
            }
        }

        // 10. Mark webhook event PROCESSED guarded by lease fencing in the same transaction
        match self
            .persistence_service
            .mark_processed_fenced_tx(&mut tx, &event_id, &lease_token)
            .await
        {
            Ok(_) => {}
            Err(err) => {
                error!(
                    event_id = %event_id,
                    lease_token = %lease_token,
                    error = %err,
                    "Fenced webhook completion failed (stale worker or lease reclaimed); rolling back dispute mutations"
                );
                let _ = tx.rollback().await;
                return Err(err);
            }
        };

        // 11. Commit transaction only after both dispute mutations and fenced webhook status succeed
        tx.commit().await?;

        info!(
            event_id = %event_id,
            dispute_id = %dispute_id,
            "Successfully processed dispute webhook event under fenced transaction"
        );

        Ok(ConsumerProcessingOutcome::Processed {
            event_id,
            dispute_id,
            dispute: dispute_record,
        })
    }
}
