use sqlx::{Row, SqlitePool};
use tracing::{debug, error, info, warn};

use crate::error::AppError;
use crate::models::webhook::{
    calculate_retry_delay_seconds, compute_payload_hash, PayPalWebhookEvent, WebhookEventRecord,
    WebhookPersistenceResult, WebhookProcessingStatus,
};

/// Service responsible for the durable persistence, hash verification, and idempotency
/// of received PayPal webhook events.
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
    /// # Idempotency Guarantee
    /// Uses SQLite's atomic `INSERT ... ON CONFLICT(event_id) DO NOTHING`.
    /// - If the event is newly received, it is inserted and returns `WebhookPersistenceResult::Inserted`.
    /// - If the event ID was previously recorded, no database modification occurs, the original
    ///   payload and metadata remain untouched, and it returns `WebhookPersistenceResult::Duplicate`.
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
                created_at, processed_at
            )
            VALUES (?, ?, ?, ?, ?, 'PENDING', 0, 5, NULL, ?, ?, NULL, ?, NULL)
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
            };

            Ok(WebhookPersistenceResult::Inserted(record))
        }
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
                   created_at, processed_at
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
                   created_at, processed_at
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

    /// Safely transitions an event to `PROCESSING` status.
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

    /// Safely transitions an event to `PROCESSED` status and timestamps completion.
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

    /// Transitions an event to terminal `FAILED` status, recording failure details.
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

    /// Schedules an event for retry using exponential backoff, or marks it `FAILED` if max retries exceeded.
    pub async fn schedule_retry(
        &self,
        event_id: &str,
        error_message: &str,
    ) -> Result<WebhookEventRecord, AppError> {
        let mut tx = self.pool.begin().await?;

        let current = self.get_event_by_id_tx(event_id, &mut tx).await?;
        let next_retry_count = current.retry_count + 1;

        if next_retry_count >= current.max_retries {
            // Maximum retry limit exceeded -> terminal FAILED
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
            // Safe transition to RETRY with exponential backoff delay
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

    /// Internal helper to retrieve an event within a transaction.
    async fn get_event_by_id_tx(
        &self,
        event_id: &str,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<WebhookEventRecord, AppError> {
        let row = sqlx::query(
            r#"
            SELECT event_id, event_type, resource_type, resource_id,
                   transmission_id, processing_status, retry_count, max_retries,
                   last_error, raw_payload, payload_hash, next_retry_at,
                   created_at, processed_at
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
        })
    }
}
