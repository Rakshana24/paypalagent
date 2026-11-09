use sqlx::{Row, SqlitePool};
use tracing::{debug, error, info, warn};

use crate::error::AppError;
use crate::models::webhook::{
    calculate_retry_delay_seconds, compute_payload_hash, PayPalWebhookEvent, WebhookEventRecord,
    WebhookPersistenceResult, WebhookProcessingStatus,
};

/// Default processing lease timeout in seconds (5 minutes).
pub const DEFAULT_LEASE_DURATION_SECS: u64 = 300;

/// Service responsible for durable persistence, hash verification, idempotent deduplication,
/// and worker-lease-fenced state management of PayPal webhook events.
#[derive(Clone)]
pub struct WebhookPersistenceService {
    pool: SqlitePool,
}

impl WebhookPersistenceService {
    /// Creates a new `WebhookPersistenceService` backed by the given SQLite pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Atomically persists an incoming PayPal webhook event with its raw byte payload.
    ///
    /// # Idempotency and Divergence Detection
    /// - If the event is newly received, it is inserted and returns `WebhookPersistenceResult::Inserted`.
    /// - If the event ID was previously recorded with identical payload SHA-256 hash,
    ///   it is recognized as an authentic duplicate delivery and returns `WebhookPersistenceResult::Duplicate`.
    /// - If the event ID was previously recorded with a **different** payload hash, it returns
    ///   `AppError::PayloadHashMismatch`, preventing payload spoofing or data corruption.
    pub async fn persist_event(
        &self,
        event: &PayPalWebhookEvent,
        transmission_id: &str,
        raw_payload: &[u8],
    ) -> Result<WebhookPersistenceResult, AppError> {
        let payload_hash = compute_payload_hash(raw_payload);
        let resource_id = event.extract_resource_id();
        let now = chrono::Utc::now().to_rfc3339();

        let query_result = sqlx::query(
            r#"
            INSERT INTO webhook_events (
                event_id, event_type, resource_type, resource_id,
                transmission_id, processing_status, retry_count, max_retries,
                last_error, raw_payload, payload_hash, next_retry_at,
                created_at, processed_at, processing_started_at, lease_token, processing_worker_id
            )
            VALUES (?, ?, ?, ?, ?, 'PENDING', 0, 5, NULL, ?, ?, NULL, ?, NULL, NULL, NULL, NULL)
            ON CONFLICT(event_id) DO NOTHING;
            "#,
        )
        .bind(&event.id)
        .bind(&event.event_type)
        .bind(&event.resource_type)
        .bind(&resource_id)
        .bind(transmission_id)
        .bind(raw_payload)
        .bind(&payload_hash)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        if query_result.rows_affected() == 0 {
            // Check for payload hash divergence on duplicate event ID
            let existing_hash: Option<String> =
                sqlx::query_scalar("SELECT payload_hash FROM webhook_events WHERE event_id = ?;")
                    .bind(&event.id)
                    .fetch_optional(&self.pool)
                    .await?;

            if let Some(stored_hash) = existing_hash {
                if stored_hash != payload_hash {
                    warn!(
                        event_id = %event.id,
                        stored_hash = %stored_hash,
                        incoming_hash = %payload_hash,
                        "Payload SHA-256 divergence detected for duplicate event ID"
                    );
                    return Err(AppError::PayloadHashMismatch(format!(
                        "Payload SHA-256 divergence detected for duplicate event ID '{}': stored hash {}, incoming hash {}",
                        event.id, stored_hash, payload_hash
                    )));
                }
            }

            debug!(
                event_id = %event.id,
                transmission_id = %transmission_id,
                "Duplicate PayPal webhook event ignored idempotently"
            );
            Ok(WebhookPersistenceResult::Duplicate {
                event_id: event.id.clone(),
            })
        } else {
            info!(
                event_id = %event.id,
                event_type = %event.event_type,
                payload_hash = %payload_hash,
                "Persisted new PayPal webhook event"
            );

            let record = WebhookEventRecord {
                event_id: event.id.clone(),
                event_type: event.event_type.clone(),
                resource_type: Some(event.resource_type.clone()),
                resource_id,
                transmission_id: transmission_id.to_string(),
                processing_status: WebhookProcessingStatus::Pending,
                retry_count: 0,
                max_retries: 5,
                last_error: None,
                raw_payload: raw_payload.to_vec(),
                payload_hash,
                next_retry_at: None,
                created_at: now,
                processed_at: None,
                processing_started_at: None,
                lease_token: None,
                processing_worker_id: None,
            };

            Ok(WebhookPersistenceResult::Inserted(record))
        }
    }

    /// Transaction-aware variant of `persist_event` sharing an active database transaction.
    pub async fn persist_event_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        event: &PayPalWebhookEvent,
        transmission_id: &str,
        raw_payload: &[u8],
    ) -> Result<WebhookPersistenceResult, AppError> {
        let payload_hash = compute_payload_hash(raw_payload);
        let resource_id = event.extract_resource_id();
        let now = chrono::Utc::now().to_rfc3339();

        let query_result = sqlx::query(
            r#"
            INSERT INTO webhook_events (
                event_id, event_type, resource_type, resource_id,
                transmission_id, processing_status, retry_count, max_retries,
                last_error, raw_payload, payload_hash, next_retry_at,
                created_at, processed_at, processing_started_at, lease_token, processing_worker_id
            )
            VALUES (?, ?, ?, ?, ?, 'PENDING', 0, 5, NULL, ?, ?, NULL, ?, NULL, NULL, NULL, NULL)
            ON CONFLICT(event_id) DO NOTHING;
            "#,
        )
        .bind(&event.id)
        .bind(&event.event_type)
        .bind(&event.resource_type)
        .bind(&resource_id)
        .bind(transmission_id)
        .bind(raw_payload)
        .bind(&payload_hash)
        .bind(&now)
        .execute(&mut **tx)
        .await?;

        if query_result.rows_affected() == 0 {
            let existing_hash: Option<String> =
                sqlx::query_scalar("SELECT payload_hash FROM webhook_events WHERE event_id = ?;")
                    .bind(&event.id)
                    .fetch_optional(&mut **tx)
                    .await?;

            if let Some(stored_hash) = existing_hash {
                if stored_hash != payload_hash {
                    return Err(AppError::PayloadHashMismatch(format!(
                        "Payload SHA-256 divergence detected for duplicate event ID '{}': stored hash {}, incoming hash {}",
                        event.id, stored_hash, payload_hash
                    )));
                }
            }

            Ok(WebhookPersistenceResult::Duplicate {
                event_id: event.id.clone(),
            })
        } else {
            let record = WebhookEventRecord {
                event_id: event.id.clone(),
                event_type: event.event_type.clone(),
                resource_type: Some(event.resource_type.clone()),
                resource_id,
                transmission_id: transmission_id.to_string(),
                processing_status: WebhookProcessingStatus::Pending,
                retry_count: 0,
                max_retries: 5,
                last_error: None,
                raw_payload: raw_payload.to_vec(),
                payload_hash,
                next_retry_at: None,
                created_at: now,
                processed_at: None,
                processing_started_at: None,
                lease_token: None,
                processing_worker_id: None,
            };

            Ok(WebhookPersistenceResult::Inserted(record))
        }
    }

    /// Atomically acquires an exclusive processing lease on the next available pending,
    /// retryable, or abandoned webhook event using SQLite-serialized BEGIN IMMEDIATE.
    ///
    /// # Concurrency & Fencing
    /// - Serializes lease acquisition using explicit `BEGIN IMMEDIATE;` (SQLite database write lock).
    /// - Generates a new cryptographically random UUID v4 `lease_token` for the claiming worker.
    /// - Reclaims abandoned events left in `PROCESSING` if `processing_started_at` is older
    ///   than `lease_duration_secs` (default 300 seconds).
    /// - Returns `Ok(Some(record))` with the acquired lease token, or `Ok(None)` if no events are ready.
    pub async fn acquire_lease(
        &self,
        worker_id: &str,
        lease_duration_secs: u64,
    ) -> Result<Option<WebhookEventRecord>, AppError> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE;").execute(&mut *conn).await?;

        let acquire_res: Result<Option<WebhookEventRecord>, AppError> = async {
            let now = chrono::Utc::now();
            let now_str = now.to_rfc3339();
            let abandoned_cutoff =
                (now - chrono::Duration::seconds(lease_duration_secs as i64)).to_rfc3339();

            let candidate_id: Option<String> = sqlx::query_scalar(
                r#"
                SELECT event_id
                FROM webhook_events
                WHERE (processing_status = 'PENDING')
                   OR (processing_status = 'RETRY' AND (next_retry_at IS NULL OR next_retry_at <= ?))
                   OR (processing_status = 'PROCESSING' AND processing_started_at IS NOT NULL AND processing_started_at <= ?)
                ORDER BY created_at ASC
                LIMIT 1;
                "#,
            )
            .bind(&now_str)
            .bind(&abandoned_cutoff)
            .fetch_optional(&mut *conn)
            .await?;

            let event_id = match candidate_id {
                Some(id) => id,
                None => {
                    sqlx::query("COMMIT;").execute(&mut *conn).await?;
                    return Ok(None);
                }
            };

            let new_lease_token = uuid::Uuid::new_v4().to_string();

            let update_result = sqlx::query(
                r#"
                UPDATE webhook_events
                SET processing_status = 'PROCESSING',
                    processing_started_at = ?,
                    lease_token = ?,
                    processing_worker_id = ?
                WHERE event_id = ?
                  AND (
                    processing_status IN ('PENDING', 'RETRY')
                    OR (processing_status = 'PROCESSING' AND processing_started_at <= ?)
                  );
                "#,
            )
            .bind(&now_str)
            .bind(&new_lease_token)
            .bind(worker_id)
            .bind(&event_id)
            .bind(&abandoned_cutoff)
            .execute(&mut *conn)
            .await?;

            if update_result.rows_affected() == 0 {
                sqlx::query("ROLLBACK;").execute(&mut *conn).await?;
                return Ok(None);
            }

            let row = sqlx::query(
                r#"
                SELECT event_id, event_type, resource_type, resource_id,
                       transmission_id, processing_status, retry_count, max_retries,
                       last_error, raw_payload, payload_hash, next_retry_at,
                       created_at, processed_at, processing_started_at, lease_token, processing_worker_id
                FROM webhook_events
                WHERE event_id = ?;
                "#,
            )
            .bind(&event_id)
            .fetch_one(&mut *conn)
            .await?;

            let record = self.map_row_and_verify_hash(row)?;
            sqlx::query("COMMIT;").execute(&mut *conn).await?;

            info!(
                event_id = %record.event_id,
                worker_id = %worker_id,
                lease_token = %new_lease_token,
                "Acquired processing lease on webhook event"
            );

            Ok(Some(record))
        }
        .await;

        match acquire_res {
            Ok(opt) => Ok(opt),
            Err(e) => {
                let _ = sqlx::query("ROLLBACK;").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Marks a webhook event as PROCESSED within an active transaction, guarded by lease fencing.
    ///
    /// # Fencing Guarantee
    /// Must match `event_id`, `lease_token`, and `processing_status = 'PROCESSING'`.
    /// If another worker reclaimed the lease, `rows_affected` is 0 and this returns
    /// `AppError::DisputeLeaseFencedOut`, causing the caller's transaction to roll back.
    pub async fn mark_processed_fenced_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        event_id: &str,
        lease_token: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let now = chrono::Utc::now().to_rfc3339();

        let update_result = sqlx::query(
            r#"
            UPDATE webhook_events
            SET processing_status = 'PROCESSED',
                processed_at = ?,
                lease_token = NULL
            WHERE event_id = ?
              AND lease_token = ?
              AND processing_status = 'PROCESSING';
            "#,
        )
        .bind(&now)
        .bind(event_id)
        .bind(lease_token)
        .execute(&mut **tx)
        .await?;

        if update_result.rows_affected() == 0 {
            warn!(
                event_id = %event_id,
                lease_token = %lease_token,
                "Worker lease fencing violation: event is no longer owned by this lease token"
            );
            return Err(AppError::DisputeLeaseFencedOut(format!(
                "Worker lease fencing violation: webhook event '{}' is no longer owned by lease token",
                event_id
            )));
        }

        let updated = self.get_event_by_id_tx(event_id, tx).await?;
        info!(
            event_id = %event_id,
            "Webhook event successfully marked PROCESSED under valid lease"
        );
        Ok(updated)
    }

    /// Non-transactional helper for fenced mark_processed.
    pub async fn mark_processed_fenced(
        &self,
        event_id: &str,
        lease_token: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;
        let record = self
            .mark_processed_fenced_tx(&mut tx, event_id, lease_token)
            .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Schedules a webhook event for retry with exponential backoff within a transaction,
    /// guarded by lease fencing. If max retries are exceeded, transitions to terminal `FAILED`.
    pub async fn schedule_retry_fenced_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        event_id: &str,
        lease_token: &str,
        error_message: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let current = self.get_event_by_id_tx(event_id, tx).await?;

        if current.lease_token.as_deref() != Some(lease_token)
            || current.processing_status != WebhookProcessingStatus::Processing
        {
            return Err(AppError::DisputeLeaseFencedOut(format!(
                "Worker lease fencing violation on retry: webhook event '{}' is no longer owned by lease token",
                event_id
            )));
        }

        let next_retry_count = current.retry_count + 1;
        let now = chrono::Utc::now().to_rfc3339();

        if next_retry_count >= current.max_retries {
            let final_err = format!(
                "Max retries ({}) exceeded: {}",
                current.max_retries, error_message
            );

            let update_res = sqlx::query(
                r#"
                UPDATE webhook_events
                SET processing_status = 'FAILED',
                    retry_count = ?,
                    last_error = ?,
                    processed_at = ?,
                    lease_token = NULL,
                    processing_started_at = NULL,
                    processing_worker_id = NULL
                WHERE event_id = ?
                  AND lease_token = ?
                  AND processing_status = 'PROCESSING';
                "#,
            )
            .bind(next_retry_count)
            .bind(&final_err)
            .bind(&now)
            .bind(event_id)
            .bind(lease_token)
            .execute(&mut **tx)
            .await?;

            if update_res.rows_affected() == 0 {
                return Err(AppError::DisputeLeaseFencedOut(format!(
                    "Worker lease fencing violation during fail transition: webhook event '{}'",
                    event_id
                )));
            }
        } else {
            let delay_secs = calculate_retry_delay_seconds(current.retry_count as u32);
            let next_retry_time = chrono::Utc::now() + chrono::Duration::seconds(delay_secs as i64);
            let next_retry_at = next_retry_time.to_rfc3339();

            let update_res = sqlx::query(
                r#"
                UPDATE webhook_events
                SET processing_status = 'RETRY',
                    retry_count = ?,
                    last_error = ?,
                    next_retry_at = ?,
                    lease_token = NULL,
                    processing_started_at = NULL,
                    processing_worker_id = NULL
                WHERE event_id = ?
                  AND lease_token = ?
                  AND processing_status = 'PROCESSING';
                "#,
            )
            .bind(next_retry_count)
            .bind(error_message)
            .bind(&next_retry_at)
            .bind(event_id)
            .bind(lease_token)
            .execute(&mut **tx)
            .await?;

            if update_res.rows_affected() == 0 {
                return Err(AppError::DisputeLeaseFencedOut(format!(
                    "Worker lease fencing violation during retry transition: webhook event '{}'",
                    event_id
                )));
            }
        }

        let updated = self.get_event_by_id_tx(event_id, tx).await?;
        Ok(updated)
    }

    /// Non-transactional helper for fenced schedule_retry.
    pub async fn schedule_retry_fenced(
        &self,
        event_id: &str,
        lease_token: &str,
        error_message: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;
        let record = self
            .schedule_retry_fenced_tx(&mut tx, event_id, lease_token, error_message)
            .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Immediately transitions an event to terminal `FAILED` status within a transaction,
    /// guarded by lease fencing.
    pub async fn mark_failed_fenced_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        event_id: &str,
        lease_token: &str,
        error_message: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let now = chrono::Utc::now().to_rfc3339();

        let update_result = sqlx::query(
            r#"
            UPDATE webhook_events
            SET processing_status = 'FAILED',
                last_error = ?,
                processed_at = ?,
                lease_token = NULL,
                processing_started_at = NULL,
                processing_worker_id = NULL
            WHERE event_id = ?
              AND lease_token = ?
              AND processing_status = 'PROCESSING';
            "#,
        )
        .bind(error_message)
        .bind(&now)
        .bind(event_id)
        .bind(lease_token)
        .execute(&mut **tx)
        .await?;

        if update_result.rows_affected() == 0 {
            return Err(AppError::DisputeLeaseFencedOut(format!(
                "Worker lease fencing violation during mark_failed: webhook event '{}'",
                event_id
            )));
        }

        let updated = self.get_event_by_id_tx(event_id, tx).await?;
        Ok(updated)
    }

    /// Non-transactional helper for fenced mark_failed.
    pub async fn mark_failed_fenced(
        &self,
        event_id: &str,
        lease_token: &str,
        error_message: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;
        let record = self
            .mark_failed_fenced_tx(&mut tx, event_id, lease_token, error_message)
            .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Transitions an event to terminal `IGNORED` status within a transaction, guarded by lease fencing.
    pub async fn mark_ignored_fenced_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        event_id: &str,
        lease_token: &str,
        reason: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let now = chrono::Utc::now().to_rfc3339();

        let update_result = sqlx::query(
            r#"
            UPDATE webhook_events
            SET processing_status = 'IGNORED',
                last_error = ?,
                processed_at = ?,
                lease_token = NULL,
                processing_started_at = NULL,
                processing_worker_id = NULL
            WHERE event_id = ?
              AND lease_token = ?
              AND processing_status = 'PROCESSING';
            "#,
        )
        .bind(reason)
        .bind(&now)
        .bind(event_id)
        .bind(lease_token)
        .execute(&mut **tx)
        .await?;

        if update_result.rows_affected() == 0 {
            return Err(AppError::DisputeLeaseFencedOut(format!(
                "Worker lease fencing violation during mark_ignored: webhook event '{}'",
                event_id
            )));
        }

        let updated = self.get_event_by_id_tx(event_id, tx).await?;
        Ok(updated)
    }

    /// Non-transactional helper for fenced mark_ignored.
    pub async fn mark_ignored_fenced(
        &self,
        event_id: &str,
        lease_token: &str,
        reason: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;
        let record = self
            .mark_ignored_fenced_tx(&mut tx, event_id, lease_token, reason)
            .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Retrieves a single webhook event by its event ID, verifying its SHA-256 payload integrity.
    pub async fn get_event_by_id(
        &self,
        event_id: &str,
    ) -> Result<Option<WebhookEventRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT event_id, event_type, resource_type, resource_id,
                   transmission_id, processing_status, retry_count, max_retries,
                   last_error, raw_payload, payload_hash, next_retry_at,
                   created_at, processed_at, processing_started_at, lease_token, processing_worker_id
            FROM webhook_events
            WHERE event_id = ?;
            "#,
        )
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let record = self.map_row_and_verify_hash(row)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Retrieves pending or retryable webhook events up to `limit`.
    pub async fn get_pending_events(
        &self,
        limit: u32,
    ) -> Result<Vec<WebhookEventRecord>, AppError> {
        let now = chrono::Utc::now().to_rfc3339();

        let rows = sqlx::query(
            r#"
            SELECT event_id, event_type, resource_type, resource_id,
                   transmission_id, processing_status, retry_count, max_retries,
                   last_error, raw_payload, payload_hash, next_retry_at,
                   created_at, processed_at, processing_started_at, lease_token, processing_worker_id
            FROM webhook_events
            WHERE (processing_status = 'PENDING')
               OR (processing_status = 'RETRY' AND (next_retry_at IS NULL OR next_retry_at <= ?))
            ORDER BY created_at ASC
            LIMIT ?;
            "#,
        )
        .bind(&now)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            records.push(self.map_row_and_verify_hash(row)?);
        }

        Ok(records)
    }

    /// Safely transitions an event to `PROCESSING` status (legacy un-fenced).
    pub async fn mark_processing(&self, event_id: &str) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;

        let current = self.get_event_by_id_tx(event_id, &mut tx).await?;
        if !current
            .processing_status
            .can_transition_to(WebhookProcessingStatus::Processing)
        {
            return Err(AppError::InvalidStateTransition(format!(
                "Cannot transition webhook event '{}' from {} to PROCESSING",
                event_id,
                current.processing_status.as_str()
            )));
        }

        sqlx::query(
            r#"
            UPDATE webhook_events
            SET processing_status = 'PROCESSING'
            WHERE event_id = ?;
            "#,
        )
        .bind(event_id)
        .execute(&mut *tx)
        .await?;

        let updated = self.get_event_by_id_tx(event_id, &mut tx).await?;
        tx.commit().await?;

        debug!(event_id = %event_id, "Webhook event marked as PROCESSING");
        Ok(updated)
    }

    /// Safely transitions an event to `PROCESSED` status and timestamps completion (legacy un-fenced).
    pub async fn mark_processed(&self, event_id: &str) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;

        let current = self.get_event_by_id_tx(event_id, &mut tx).await?;
        if !current
            .processing_status
            .can_transition_to(WebhookProcessingStatus::Processed)
        {
            return Err(AppError::InvalidStateTransition(format!(
                "Cannot transition webhook event '{}' from {} to PROCESSED",
                event_id,
                current.processing_status.as_str()
            )));
        }

        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE webhook_events
            SET processing_status = 'PROCESSED', processed_at = ?
            WHERE event_id = ?;
            "#,
        )
        .bind(&now)
        .bind(event_id)
        .execute(&mut *tx)
        .await?;

        let updated = self.get_event_by_id_tx(event_id, &mut tx).await?;
        tx.commit().await?;

        info!(event_id = %event_id, "Webhook event marked as PROCESSED");
        Ok(updated)
    }

    /// Transitions an event to terminal `FAILED` status, recording failure details (legacy un-fenced).
    pub async fn mark_failed(
        &self,
        event_id: &str,
        error_message: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;

        let current = self.get_event_by_id_tx(event_id, &mut tx).await?;
        if !current
            .processing_status
            .can_transition_to(WebhookProcessingStatus::Failed)
        {
            return Err(AppError::InvalidStateTransition(format!(
                "Cannot transition webhook event '{}' from {} to FAILED",
                event_id,
                current.processing_status.as_str()
            )));
        }

        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE webhook_events
            SET processing_status = 'FAILED', last_error = ?, processed_at = ?
            WHERE event_id = ?;
            "#,
        )
        .bind(error_message)
        .bind(&now)
        .bind(event_id)
        .execute(&mut *tx)
        .await?;

        let updated = self.get_event_by_id_tx(event_id, &mut tx).await?;
        tx.commit().await?;

        warn!(
            event_id = %event_id,
            error = %error_message,
            "Webhook event marked as FAILED"
        );
        Ok(updated)
    }

    /// Schedules an event for retry using exponential backoff, or marks it `FAILED` if max retries exceeded (legacy un-fenced).
    pub async fn schedule_retry(
        &self,
        event_id: &str,
        error_message: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;

        let current = self.get_event_by_id_tx(event_id, &mut tx).await?;
        let next_retry_count = current.retry_count + 1;

        if next_retry_count >= current.max_retries {
            let now = chrono::Utc::now().to_rfc3339();
            let final_err = format!(
                "Max retries ({}) exceeded: {}",
                current.max_retries, error_message
            );

            sqlx::query(
                r#"
                UPDATE webhook_events
                SET processing_status = 'FAILED',
                    retry_count = ?,
                    last_error = ?,
                    processed_at = ?
                WHERE event_id = ?;
                "#,
            )
            .bind(next_retry_count)
            .bind(&final_err)
            .bind(&now)
            .bind(event_id)
            .execute(&mut *tx)
            .await?;

            let updated = self.get_event_by_id_tx(event_id, &mut tx).await?;
            tx.commit().await?;

            warn!(
                event_id = %event_id,
                retry_count = next_retry_count,
                "Webhook event exceeded maximum retries; marked as FAILED"
            );
            Ok(updated)
        } else {
            let delay_secs = calculate_retry_delay_seconds(current.retry_count as u32);
            let next_retry_time = chrono::Utc::now() + chrono::Duration::seconds(delay_secs as i64);
            let next_retry_at = next_retry_time.to_rfc3339();

            sqlx::query(
                r#"
                UPDATE webhook_events
                SET processing_status = 'RETRY',
                    retry_count = ?,
                    last_error = ?,
                    next_retry_at = ?
                WHERE event_id = ?;
                "#,
            )
            .bind(next_retry_count)
            .bind(error_message)
            .bind(&next_retry_at)
            .bind(event_id)
            .execute(&mut *tx)
            .await?;

            let updated = self.get_event_by_id_tx(event_id, &mut tx).await?;
            tx.commit().await?;

            info!(
                event_id = %event_id,
                retry_count = next_retry_count,
                delay_secs = delay_secs,
                next_retry_at = %next_retry_at,
                "Webhook event scheduled for retry"
            );
            Ok(updated)
        }
    }

    /// Internal helper to retrieve an event within an active transaction.
    pub async fn get_event_by_id_tx(
        &self,
        event_id: &str,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<WebhookEventRecord, AppError> {
        let row = sqlx::query(
            r#"
            SELECT event_id, event_type, resource_type, resource_id,
                   transmission_id, processing_status, retry_count, max_retries,
                   last_error, raw_payload, payload_hash, next_retry_at,
                   created_at, processed_at, processing_started_at, lease_token, processing_worker_id
            FROM webhook_events
            WHERE event_id = ?;
            "#,
        )
        .bind(event_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            AppError::WebhookNotFound(format!("Webhook event '{}' does not exist", event_id))
        })?;

        self.map_row_and_verify_hash(row)
    }

    /// Internal helper to map an SQLite row into `WebhookEventRecord` while verifying SHA-256 payload integrity.
    fn map_row_and_verify_hash(
        &self,
        row: sqlx::sqlite::SqliteRow,
    ) -> Result<WebhookEventRecord, AppError> {
        let event_id: String = row.get("event_id");
        let event_type: String = row.get("event_type");
        let resource_type: Option<String> = row.get("resource_type");
        let resource_id: Option<String> = row.get("resource_id");
        let transmission_id: String = row.get("transmission_id");
        let status_str: String = row.get("processing_status");
        let retry_count: i64 = row.get("retry_count");
        let max_retries: i64 = row.get("max_retries");
        let last_error: Option<String> = row.get("last_error");
        let raw_payload: Vec<u8> = row.get("raw_payload");
        let payload_hash: String = row.get("payload_hash");
        let next_retry_at: Option<String> = row.get("next_retry_at");
        let created_at: String = row.get("created_at");
        let processed_at: Option<String> = row.get("processed_at");
        let processing_started_at: Option<String> = row.get("processing_started_at");
        let lease_token: Option<String> = row.get("lease_token");
        let processing_worker_id: Option<String> = row.get("processing_worker_id");

        // Verify cryptographic hash integrity over raw payload bytes
        let computed_hash = compute_payload_hash(&raw_payload);
        if computed_hash != payload_hash {
            error!(
                event_id = %event_id,
                expected_hash = %payload_hash,
                computed_hash = %computed_hash,
                "Integrity violation: stored raw payload hash mismatch"
            );
            return Err(AppError::PayloadHashMismatch(format!(
                "Payload hash mismatch for webhook event '{}': expected {}, computed {}",
                event_id, payload_hash, computed_hash
            )));
        }

        let processing_status = WebhookProcessingStatus::from_str(&status_str)?;

        Ok(WebhookEventRecord {
            event_id,
            event_type,
            resource_type,
            resource_id,
            transmission_id,
            processing_status,
            retry_count,
            max_retries,
            last_error,
            raw_payload,
            payload_hash,
            next_retry_at,
            created_at,
            processed_at,
            processing_started_at,
            lease_token,
            processing_worker_id,
        })
    }
}
