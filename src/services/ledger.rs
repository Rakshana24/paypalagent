use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::AppError;
use crate::ledger::hash::{compute_event_hash, to_canonical_json_bytes, GENESIS_HASH};
use crate::models::{
    AppendEventRequest, AppendEventResponse, CreateSessionRequest, CreateSessionResponse,
    DecisionStep, FinalizeSessionResponse, Session,
};

/// Maximum payload size in bytes (1 MB).
pub const MAX_PAYLOAD_SIZE_BYTES: usize = 1024 * 1024;

/// Core ledger service managing sessions and the cryptographic hash chain.
#[derive(Clone)]
pub struct LedgerService {
    pool: SqlitePool,
    write_lock: Arc<Mutex<()>>,
}

impl LedgerService {
    /// Creates a new `LedgerService` with the given SQLite connection pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Creates a new audit session and records the initial prompt as sequence index 0.
    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> Result<CreateSessionResponse, AppError> {
        let prompt_trimmed = request.initial_prompt.trim();
        if prompt_trimmed.is_empty() {
            return Err(AppError::InvalidPayload(
                "Initial prompt cannot be empty".to_string(),
            ));
        }

        let _guard = self.write_lock.lock().await;

        let session_id = Uuid::new_v4().to_string();
        let step_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let initial_payload = serde_json::json!({
            "initial_prompt": prompt_trimmed
        });
        let canonical_bytes = to_canonical_json_bytes(&initial_payload)?;
        let canonical_payload_str = String::from_utf8(canonical_bytes.clone()).map_err(|e| {
            AppError::Serialization(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e,
            )))
        })?;

        let initial_hash = compute_event_hash(0, "INITIAL_PROMPT", &canonical_bytes, GENESIS_HASH);

        // Execute atomically in a transaction
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO sessions (session_id, initial_prompt, root_hash, created_at, updated_at)
            VALUES (?, ?, NULL, ?, ?);
            "#,
        )
        .bind(&session_id)
        .bind(prompt_trimmed)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO decision_steps (id, session_id, sequence_index, action_type, payload, previous_hash, current_hash, created_at)
            VALUES (?, ?, 0, 'INITIAL_PROMPT', ?, ?, ?, ?);
            "#,
        )
        .bind(&step_id)
        .bind(&session_id)
        .bind(&canonical_payload_str)
        .bind(GENESIS_HASH)
        .bind(&initial_hash)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        info!(
            session_id = %session_id,
            initial_hash = %initial_hash,
            "Created new AgentAudit session"
        );

        Ok(CreateSessionResponse {
            success: true,
            session_id,
            initial_hash,
            created_at: now,
        })
    }

    /// Appends an observable decision step or shopping action to an active session.
    pub async fn append_event(
        &self,
        session_id: &str,
        request: AppendEventRequest,
    ) -> Result<AppendEventResponse, AppError> {
        let action_type = request.action_type.trim();
        if action_type.is_empty() {
            return Err(AppError::InvalidPayload(
                "action_type cannot be empty".to_string(),
            ));
        }

        let canonical_bytes = to_canonical_json_bytes(&request.payload)?;
        if canonical_bytes.len() > MAX_PAYLOAD_SIZE_BYTES {
            return Err(AppError::InvalidPayload(format!(
                "Payload size ({} bytes) exceeds maximum allowed limit of {} bytes",
                canonical_bytes.len(),
                MAX_PAYLOAD_SIZE_BYTES
            )));
        }

        let canonical_str = String::from_utf8(canonical_bytes.clone()).map_err(|e| {
            AppError::Serialization(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e,
            )))
        })?;

        let _guard = self.write_lock.lock().await;

        let step_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let mut tx = self.pool.begin().await?;

        // 1. Verify session exists and is not finalized
        let session_record: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT session_id, root_hash FROM sessions WHERE session_id = ?;")
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await?;

        let (_, root_hash) = match session_record {
            Some(s) => s,
            None => {
                return Err(AppError::SessionNotFound(format!(
                    "Session '{}' does not exist",
                    session_id
                )))
            }
        };

        if root_hash.is_some() {
            return Err(AppError::SessionFinalized(format!(
                "Session '{}' is finalized; no further events can be appended",
                session_id
            )));
        }

        // 2. Fetch the latest step to get sequence index and previous hash
        let last_step: Option<(i64, String)> = sqlx::query_as(
            r#"
            SELECT sequence_index, current_hash
            FROM decision_steps
            WHERE session_id = ?
            ORDER BY sequence_index DESC
            LIMIT 1;
            "#,
        )
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;

        let (new_sequence_index, previous_hash) = match last_step {
            Some((last_seq, last_hash)) => (last_seq + 1, last_hash),
            None => (0, GENESIS_HASH.to_string()),
        };

        let current_hash = compute_event_hash(
            new_sequence_index,
            action_type,
            &canonical_bytes,
            &previous_hash,
        );

        // 3. Insert the new decision step
        sqlx::query(
            r#"
            INSERT INTO decision_steps (id, session_id, sequence_index, action_type, payload, previous_hash, current_hash, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?);
            "#,
        )
        .bind(&step_id)
        .bind(session_id)
        .bind(new_sequence_index)
        .bind(action_type)
        .bind(&canonical_str)
        .bind(&previous_hash)
        .bind(&current_hash)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // 4. Update session updated_at timestamp
        sqlx::query(
            r#"
            UPDATE sessions
            SET updated_at = ?
            WHERE session_id = ?;
            "#,
        )
        .bind(&now)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        debug!(
            session_id = %session_id,
            sequence = new_sequence_index,
            current_hash = %current_hash,
            "Appended decision step to ledger"
        );

        Ok(AppendEventResponse {
            success: true,
            step_id,
            sequence_index: new_sequence_index,
            previous_hash,
            current_hash,
            created_at: now,
        })
    }

    /// Retrieves session metadata by session ID.
    pub async fn get_session(&self, session_id: &str) -> Result<Session, AppError> {
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

        Ok(session)
    }

    /// Retrieves all events for a session in chronological sequence order.
    pub async fn get_session_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<DecisionStep>, AppError> {
        // Ensure session exists
        let _ = self.get_session(session_id).await?;

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

        Ok(steps)
    }

    /// Finalizes a session, locking the final root hash and preventing further event appends.
    pub async fn finalize_session(
        &self,
        session_id: &str,
    ) -> Result<FinalizeSessionResponse, AppError> {
        let _guard = self.write_lock.lock().await;
        let mut tx = self.pool.begin().await?;

        let session = sqlx::query_as::<_, Session>(
            r#"
            SELECT session_id, initial_prompt, paypal_order_id, root_hash, created_at, updated_at
            FROM sessions
            WHERE session_id = ?;
            "#,
        )
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            AppError::SessionNotFound(format!("Session '{}' does not exist", session_id))
        })?;

        if session.root_hash.is_some() {
            return Err(AppError::SessionFinalized(format!(
                "Session '{}' is already finalized",
                session_id
            )));
        }

        // Fetch latest step to lock in the root hash
        let last_step: Option<(i64, String)> = sqlx::query_as(
            r#"
            SELECT sequence_index, current_hash
            FROM decision_steps
            WHERE session_id = ?
            ORDER BY sequence_index DESC
            LIMIT 1;
            "#,
        )
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;

        let (max_seq, final_root_hash) = last_step.ok_or_else(|| {
            AppError::LedgerIntegrity(format!(
                "Session '{}' has no events and cannot be finalized",
                session_id
            ))
        })?;

        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            UPDATE sessions
            SET root_hash = ?, updated_at = ?
            WHERE session_id = ?;
            "#,
        )
        .bind(&final_root_hash)
        .bind(&now)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        info!(
            session_id = %session_id,
            root_hash = %final_root_hash,
            total_steps = (max_seq + 1),
            "Finalized AgentAudit session"
        );

        Ok(FinalizeSessionResponse {
            success: true,
            session_id: session_id.to_string(),
            root_hash: final_root_hash,
            total_steps: (max_seq + 1) as usize,
            finalized_at: now,
        })
    }

    /// Associates a PayPal Order ID with the session.
    pub async fn set_paypal_order_id(
        &self,
        session_id: &str,
        paypal_order_id: &str,
    ) -> Result<(), AppError> {
        let _guard = self.write_lock.lock().await;
        let now = chrono::Utc::now().to_rfc3339();

        let rows_affected = sqlx::query(
            r#"
            UPDATE sessions
            SET paypal_order_id = ?, updated_at = ?
            WHERE session_id = ?;
            "#,
        )
        .bind(paypal_order_id)
        .bind(&now)
        .bind(session_id)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if rows_affected == 0 {
            return Err(AppError::SessionNotFound(format!(
                "Session '{}' does not exist",
                session_id
            )));
        }

        info!(
            session_id = %session_id,
            paypal_order_id = %paypal_order_id,
            "Associated PayPal Order ID with session"
        );

        Ok(())
    }
}
