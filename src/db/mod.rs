use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

use crate::error::AppError;

/// Initializes the embedded SQLite database connection pool and runs schema migrations.
pub async fn init_db(database_url: &str) -> Result<SqlitePool, AppError> {
    // If the database URL points to a local file, ensure parent directory exists
    if let Some(file_path) = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
    {
        let path_clean = file_path.split('?').next().unwrap_or(file_path);
        if path_clean != ":memory:" {
            let path = Path::new(path_clean);
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent).await?;
                }
            }
        }
    }

    let connection_options = SqliteConnectOptions::from_str(database_url)
        .map_err(|e| AppError::Config(format!("Invalid database connection string: {}", e)))?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(connection_options)
        .await?;

    // Execute schema initialization
    run_migrations(&pool).await?;

    info!(
        "SQLite database initialized successfully at: {}",
        database_url
    );
    Ok(pool)
}

/// Creates tables and indexes if they do not exist.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), AppError> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            initial_prompt TEXT NOT NULL,
            paypal_order_id TEXT,
            root_hash TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS decision_steps (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
            sequence_index INTEGER NOT NULL,
            action_type TEXT NOT NULL,
            payload TEXT NOT NULL,
            previous_hash TEXT NOT NULL,
            current_hash TEXT NOT NULL,
            created_at TEXT NOT NULL,
            CONSTRAINT uq_session_sequence UNIQUE (session_id, sequence_index)
        );

        CREATE INDEX IF NOT EXISTS idx_decision_steps_session_seq
            ON decision_steps(session_id, sequence_index ASC);

        CREATE INDEX IF NOT EXISTS idx_sessions_created_at
            ON sessions(created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_sessions_paypal_order_id
            ON sessions(paypal_order_id);

        CREATE INDEX IF NOT EXISTS idx_decision_steps_capture_id
            ON decision_steps(json_extract(payload, '$.capture_id'))
            WHERE action_type = 'PAYPAL_PAYMENT_CAPTURED';

        CREATE TABLE IF NOT EXISTS webhook_events (
            event_id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            resource_type TEXT,
            resource_id TEXT,
            transmission_id TEXT NOT NULL,
            processing_status TEXT NOT NULL CHECK(processing_status IN ('PENDING', 'PROCESSING', 'PROCESSED', 'RETRY', 'FAILED', 'IGNORED')),
            retry_count INTEGER NOT NULL DEFAULT 0 CHECK(retry_count >= 0),
            max_retries INTEGER NOT NULL DEFAULT 5 CHECK(max_retries >= 0),
            last_error TEXT,
            raw_payload BLOB NOT NULL,
            payload_hash TEXT NOT NULL,
            next_retry_at TEXT,
            created_at TEXT NOT NULL,
            processed_at TEXT,
            processing_started_at TEXT,
            lease_token TEXT,
            processing_worker_id TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_webhook_events_status_next_retry
            ON webhook_events(processing_status, next_retry_at);

        CREATE INDEX IF NOT EXISTS idx_webhook_events_resource_id
            ON webhook_events(resource_id);

        CREATE INDEX IF NOT EXISTS idx_webhook_events_created_at
            ON webhook_events(created_at DESC);

        CREATE TABLE IF NOT EXISTS disputes (
            dispute_id TEXT PRIMARY KEY,
            session_id TEXT REFERENCES sessions(session_id) ON DELETE RESTRICT,
            genesis_anchor_type TEXT NOT NULL CHECK(genesis_anchor_type IN ('PURCHASE_ROOT_HASH', 'GENESIS_HASH')),
            genesis_anchor_hash TEXT NOT NULL,
            purchase_root_hash TEXT,
            seller_transaction_id TEXT,
            paypal_order_id TEXT,
            status TEXT NOT NULL,
            dispute_lifecycle_stage TEXT,
            dispute_channel TEXT,
            reason TEXT,
            dispute_amount_value TEXT,
            dispute_amount_currency TEXT,
            correlation_status TEXT NOT NULL CHECK(correlation_status IN ('MATCHED', 'MANUAL_REVIEW_REQUIRED', 'NOT_FOUND')),
            matched_by TEXT,
            root_hash TEXT,
            dispute_create_time TEXT,
            dispute_update_time TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_disputes_session_id
            ON disputes(session_id);

        CREATE INDEX IF NOT EXISTS idx_disputes_correlation_status
            ON disputes(correlation_status);

        CREATE INDEX IF NOT EXISTS idx_disputes_status
            ON disputes(status);

        CREATE INDEX IF NOT EXISTS idx_disputes_created_at
            ON disputes(created_at DESC);

        CREATE TABLE IF NOT EXISTS dispute_audit_steps (
            id TEXT PRIMARY KEY,
            dispute_id TEXT NOT NULL REFERENCES disputes(dispute_id) ON DELETE RESTRICT,
            sequence_index INTEGER NOT NULL,
            action_type TEXT NOT NULL,
            payload TEXT NOT NULL,
            previous_hash TEXT NOT NULL,
            current_hash TEXT NOT NULL,
            created_at TEXT NOT NULL,
            CONSTRAINT uq_dispute_sequence UNIQUE (dispute_id, sequence_index)
        );

        CREATE INDEX IF NOT EXISTS idx_dispute_audit_steps_seq
            ON dispute_audit_steps(dispute_id, sequence_index ASC);

        CREATE TRIGGER IF NOT EXISTS trg_protect_dispute_genesis_anchor
        BEFORE UPDATE OF genesis_anchor_type, genesis_anchor_hash ON disputes
        BEGIN
            SELECT CASE
                WHEN OLD.genesis_anchor_type != NEW.genesis_anchor_type
                  OR OLD.genesis_anchor_hash != NEW.genesis_anchor_hash
                THEN RAISE(ABORT, 'Mutation of immutable dispute genesis anchor is strictly prohibited')
            END;
        END;
        "#,
    )
    .execute(pool)
    .await?;

    // Safe idempotent column additions for existing databases
    let existing_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('webhook_events');")
            .fetch_all(pool)
            .await?;

    if !existing_columns
        .iter()
        .any(|c| c == "processing_started_at")
    {
        sqlx::query("ALTER TABLE webhook_events ADD COLUMN processing_started_at TEXT;")
            .execute(pool)
            .await?;
    }
    if !existing_columns.iter().any(|c| c == "lease_token") {
        sqlx::query("ALTER TABLE webhook_events ADD COLUMN lease_token TEXT;")
            .execute(pool)
            .await?;
    }
    if !existing_columns.iter().any(|c| c == "processing_worker_id") {
        sqlx::query("ALTER TABLE webhook_events ADD COLUMN processing_worker_id TEXT;")
            .execute(pool)
            .await?;
    }

    // Webhook lease index creation (safe whether table was newly created or upgraded via ALTER TABLE)
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_webhook_events_lease
            ON webhook_events(processing_status, processing_started_at);
        "#,
    )
    .execute(pool)
    .await?;

    // Safe idempotent upgrade for disputes table if created under earlier schema
    upgrade_disputes_table_if_needed(pool).await?;

    Ok(())
}

/// Upgrades an existing disputes table if it was created under an earlier schema
/// (e.g. without the session_id foreign key constraint or with a NOT NULL dispute_update_time).
/// Uses a safe transactional table-rebuild pattern that preserves all rows, foreign keys,
/// triggers, and indexes.
async fn upgrade_disputes_table_if_needed(pool: &SqlitePool) -> Result<(), AppError> {
    let mut conn = pool.acquire().await?;

    let table_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'disputes';",
    )
    .fetch_one(&mut *conn)
    .await?;

    if table_exists == 0 {
        return Ok(());
    }

    let table_info: Vec<(i64, String, String, i64, Option<String>, i64)> =
        sqlx::query_as("PRAGMA table_info('disputes');")
            .fetch_all(&mut *conn)
            .await?;

    let update_time_notnull = table_info
        .iter()
        .find(|col| col.1 == "dispute_update_time")
        .map(|col| col.3 == 1)
        .unwrap_or(false);

    let fk_list: Vec<(
        i64,
        i64,
        String,
        String,
        Option<String>,
        String,
        String,
        Option<String>,
    )> = sqlx::query_as("PRAGMA foreign_key_list('disputes');")
        .fetch_all(&mut *conn)
        .await?;

    let has_session_restrict_fk = fk_list.iter().any(|fk| {
        fk.2 == "sessions"
            && fk.3 == "session_id"
            && fk.4.as_deref() == Some("session_id")
            && fk.6.to_uppercase() == "RESTRICT"
    });

    if !update_time_notnull && has_session_restrict_fk {
        // Table already matches the desired schema
        return Ok(());
    }

    info!("Disputes table requires schema migration; executing transactional table rebuild...");

    // PRAGMA foreign_keys = OFF must be executed outside a transaction
    sqlx::query("PRAGMA foreign_keys = OFF;")
        .execute(&mut *conn)
        .await?;

    let rebuild_result: Result<(), AppError> = async {
        sqlx::query("BEGIN IMMEDIATE;").execute(&mut *conn).await?;

        sqlx::query("DROP TABLE IF EXISTS disputes_new;")
            .execute(&mut *conn)
            .await?;

        sqlx::query(
            r#"
            CREATE TABLE disputes_new (
                dispute_id TEXT PRIMARY KEY,
                session_id TEXT REFERENCES sessions(session_id) ON DELETE RESTRICT,
                genesis_anchor_type TEXT NOT NULL CHECK(genesis_anchor_type IN ('PURCHASE_ROOT_HASH', 'GENESIS_HASH')),
                genesis_anchor_hash TEXT NOT NULL,
                purchase_root_hash TEXT,
                seller_transaction_id TEXT,
                paypal_order_id TEXT,
                status TEXT NOT NULL,
                dispute_lifecycle_stage TEXT,
                dispute_channel TEXT,
                reason TEXT,
                dispute_amount_value TEXT,
                dispute_amount_currency TEXT,
                correlation_status TEXT NOT NULL CHECK(correlation_status IN ('MATCHED', 'MANUAL_REVIEW_REQUIRED', 'NOT_FOUND')),
                matched_by TEXT,
                root_hash TEXT,
                dispute_create_time TEXT,
                dispute_update_time TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            "#,
        )
        .execute(&mut *conn)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO disputes_new (
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                purchase_root_hash, seller_transaction_id, paypal_order_id,
                status, dispute_lifecycle_stage, dispute_channel, reason,
                dispute_amount_value, dispute_amount_currency, correlation_status,
                matched_by, root_hash, dispute_create_time, dispute_update_time,
                created_at, updated_at
            )
            SELECT
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                purchase_root_hash, seller_transaction_id, paypal_order_id,
                status, dispute_lifecycle_stage, dispute_channel, reason,
                dispute_amount_value, dispute_amount_currency, correlation_status,
                matched_by, root_hash, dispute_create_time, dispute_update_time,
                created_at, updated_at
            FROM disputes;
            "#,
        )
        .execute(&mut *conn)
        .await?;

        // Verify foreign key integrity on the new table BEFORE dropping the original disputes table
        let pre_violations = sqlx::query("PRAGMA foreign_key_check('disputes_new');")
            .fetch_all(&mut *conn)
            .await?;

        if !pre_violations.is_empty() {
            return Err(AppError::Database(sqlx::Error::Configuration(
                format!(
                    "Foreign key violations detected in existing dispute records during rebuild: {} violations found",
                    pre_violations.len()
                )
                .into(),
            )));
        }

        sqlx::query("DROP TABLE disputes;").execute(&mut *conn).await?;

        sqlx::query("ALTER TABLE disputes_new RENAME TO disputes;")
            .execute(&mut *conn)
            .await?;

        // Recreate indexes
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_disputes_session_id
                ON disputes(session_id);

            CREATE INDEX IF NOT EXISTS idx_disputes_correlation_status
                ON disputes(correlation_status);

            CREATE INDEX IF NOT EXISTS idx_disputes_status
                ON disputes(status);

            CREATE INDEX IF NOT EXISTS idx_disputes_created_at
                ON disputes(created_at DESC);
            "#,
        )
        .execute(&mut *conn)
        .await?;

        // Recreate trigger
        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS trg_protect_dispute_genesis_anchor
            BEFORE UPDATE OF genesis_anchor_type, genesis_anchor_hash ON disputes
            BEGIN
                SELECT CASE
                    WHEN OLD.genesis_anchor_type != NEW.genesis_anchor_type
                      OR OLD.genesis_anchor_hash != NEW.genesis_anchor_hash
                    THEN RAISE(ABORT, 'Mutation of immutable dispute genesis anchor is strictly prohibited')
                END;
            END;
            "#,
        )
        .execute(&mut *conn)
        .await?;

        // Verify foreign key integrity on dispute_audit_steps before committing
        let audit_violations = sqlx::query("PRAGMA foreign_key_check('dispute_audit_steps');")
            .fetch_all(&mut *conn)
            .await?;

        if !audit_violations.is_empty() {
            return Err(AppError::Database(sqlx::Error::Configuration(
                format!(
                    "Foreign key violations detected in dispute audit steps during rebuild: {} violations found",
                    audit_violations.len()
                )
                .into(),
            )));
        }

        sqlx::query("COMMIT;").execute(&mut *conn).await?;
        Ok(())
    }
    .await;

    if let Err(e) = rebuild_result {
        let _ = sqlx::query("ROLLBACK;").execute(&mut *conn).await;
        let _ = sqlx::query("PRAGMA foreign_keys = ON;")
            .execute(&mut *conn)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS disputes_new;")
            .execute(&mut *conn)
            .await;
        return Err(e);
    }

    // Re-enable foreign keys
    sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&mut *conn)
        .await?;

    let fk_status: i64 = sqlx::query_scalar("PRAGMA foreign_keys;")
        .fetch_one(&mut *conn)
        .await?;

    if fk_status != 1 {
        return Err(AppError::Database(sqlx::Error::Configuration(
            "Foreign-key enforcement was not successfully restored on SQLite connection".into(),
        )));
    }

    // Verify foreign key integrity across the database
    let fk_violations = sqlx::query("PRAGMA foreign_key_check;")
        .fetch_all(&mut *conn)
        .await?;

    if !fk_violations.is_empty() {
        return Err(AppError::Database(sqlx::Error::Configuration(
            format!(
                "Foreign key violations detected after disputes table migration: {} violations found",
                fk_violations.len()
            )
            .into(),
        )));
    }

    info!("Disputes table successfully rebuilt and verified with foreign keys enabled");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migration_table_creation() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name ASC;",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert!(tables.contains(&"sessions".to_string()));
        assert!(tables.contains(&"decision_steps".to_string()));
        assert!(tables.contains(&"webhook_events".to_string()));
        assert!(tables.contains(&"disputes".to_string()));
        assert!(tables.contains(&"dispute_audit_steps".to_string()));
    }

    #[tokio::test]
    async fn test_migration_index_creation() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='index' ORDER BY name ASC;",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert!(indexes.contains(&"idx_sessions_paypal_order_id".to_string()));
        assert!(indexes.contains(&"idx_decision_steps_capture_id".to_string()));
        assert!(indexes.contains(&"idx_webhook_events_lease".to_string()));
        assert!(indexes.contains(&"idx_disputes_session_id".to_string()));
        assert!(indexes.contains(&"idx_disputes_correlation_status".to_string()));
        assert!(indexes.contains(&"idx_disputes_status".to_string()));
        assert!(indexes.contains(&"idx_disputes_created_at".to_string()));
        assert!(indexes.contains(&"idx_dispute_audit_steps_seq".to_string()));
    }

    #[tokio::test]
    async fn test_migration_foreign_key_restriction() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        // 0. Verify inserting dispute referencing a nonexistent session is REJECTED
        let invalid_session_res = sqlx::query(
            r#"
            INSERT INTO disputes (
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                status, correlation_status, created_at, updated_at
            ) VALUES (
                'disp-invalid-sess', 'nonexistent-session-id', 'GENESIS_HASH', '0000000000000000000000000000000000000000000000000000000000000000',
                'UNDER_REVIEW', 'NOT_FOUND', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z'
            );
            "#,
        )
        .execute(&pool)
        .await;
        assert!(
            invalid_session_res.is_err(),
            "Expected foreign key rejection when referencing nonexistent session"
        );

        // 1. Insert parent session
        sqlx::query(
            "INSERT INTO sessions (session_id, initial_prompt, created_at, updated_at) VALUES ('sess-1', 'prompt', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z');"
        )
        .execute(&pool)
        .await
        .unwrap();

        // 2. Insert dispute referencing session
        sqlx::query(
            r#"
            INSERT INTO disputes (
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                status, reason, dispute_amount_value, dispute_amount_currency,
                correlation_status, created_at, updated_at, dispute_create_time
            ) VALUES (
                'disp-1', 'sess-1', 'GENESIS_HASH', '0000000000000000000000000000000000000000000000000000000000000000',
                'UNDER_REVIEW', 'NOT_RECEIVED', '199.99', 'USD',
                'MATCHED', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z'
            );
            "#
        )
        .execute(&pool)
        .await
        .unwrap();

        // 3. Insert audit step referencing dispute
        sqlx::query(
            r#"
            INSERT INTO dispute_audit_steps (
                id, dispute_id, sequence_index, action_type, payload,
                previous_hash, current_hash, created_at
            ) VALUES (
                'step-1', 'disp-1', 0, 'DISPUTE_IMPORTED', '{}',
                '0000000000000000000000000000000000000000000000000000000000000000', 'hash1', '2026-11-08T00:00:00Z'
            );
            "#
        )
        .execute(&pool)
        .await
        .unwrap();

        // 4. Verify deleting dispute is RESTRICTED while audit steps exist
        let delete_disp_res = sqlx::query("DELETE FROM disputes WHERE dispute_id = 'disp-1';")
            .execute(&pool)
            .await;
        assert!(
            delete_disp_res.is_err(),
            "Expected FK restriction on deleting dispute with audit history"
        );

        // 5. Verify deleting session is RESTRICTED while dispute exists
        let delete_sess_res = sqlx::query("DELETE FROM sessions WHERE session_id = 'sess-1';")
            .execute(&pool)
            .await;
        assert!(
            delete_sess_res.is_err(),
            "Expected FK restriction on deleting session with dispute reference"
        );
    }

    #[tokio::test]
    async fn test_migration_nullable_dispute_update_time_accepted() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        // 1. Insert dispute with dispute_update_time explicitly NULL (and session_id NULL)
        sqlx::query(
            r#"
            INSERT INTO disputes (
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                status, correlation_status, created_at, updated_at,
                dispute_update_time, dispute_create_time, reason
            ) VALUES (
                'disp-null-time', NULL, 'GENESIS_HASH', '0000000000000000000000000000000000000000000000000000000000000000',
                'UNDER_REVIEW', 'NOT_FOUND', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z',
                NULL, NULL, NULL
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("Dispute with NULL dispute_update_time must be accepted");

        // 2. Fetch and assert the value is None
        let (update_time, create_time, reason, sess_id): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT dispute_update_time, dispute_create_time, reason, session_id FROM disputes WHERE dispute_id = 'disp-null-time';",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(update_time, None, "dispute_update_time must be NULL");
        assert_eq!(create_time, None, "dispute_create_time must be NULL");
        assert_eq!(reason, None, "reason must be NULL");
        assert_eq!(sess_id, None, "session_id must be NULL");
    }

    #[tokio::test]
    async fn test_migration_genesis_anchor_immutability() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        sqlx::query(
            r#"
            INSERT INTO disputes (
                dispute_id, genesis_anchor_type, genesis_anchor_hash,
                status, correlation_status, created_at, updated_at, dispute_create_time
            ) VALUES (
                'disp-anchor-test', 'GENESIS_HASH', '0000000000000000000000000000000000000000000000000000000000000000',
                'UNDER_REVIEW', 'NOT_FOUND', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z'
            );
            "#
        )
        .execute(&pool)
        .await
        .unwrap();

        // Allowed update: modifying status
        let status_res = sqlx::query(
            "UPDATE disputes SET status = 'RESOLVED' WHERE dispute_id = 'disp-anchor-test';",
        )
        .execute(&pool)
        .await;
        assert!(status_res.is_ok());

        // Allowed update: identical anchor values (no-op update)
        let noop_res = sqlx::query(
            "UPDATE disputes SET genesis_anchor_hash = '0000000000000000000000000000000000000000000000000000000000000000' WHERE dispute_id = 'disp-anchor-test';"
        )
        .execute(&pool)
        .await;
        assert!(noop_res.is_ok());

        // Prohibited update: altering genesis_anchor_hash
        let mutate_hash_res = sqlx::query(
            "UPDATE disputes SET genesis_anchor_hash = 'tampered_hash_value' WHERE dispute_id = 'disp-anchor-test';"
        )
        .execute(&pool)
        .await;
        assert!(
            mutate_hash_res.is_err(),
            "Trigger must abort mutation of genesis_anchor_hash"
        );

        // Prohibited update: altering genesis_anchor_type
        let mutate_type_res = sqlx::query(
            "UPDATE disputes SET genesis_anchor_type = 'PURCHASE_ROOT_HASH' WHERE dispute_id = 'disp-anchor-test';"
        )
        .execute(&pool)
        .await;
        assert!(
            mutate_type_res.is_err(),
            "Trigger must abort mutation of genesis_anchor_type"
        );
    }

    #[tokio::test]
    async fn test_migration_lease_column_availability() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('webhook_events');")
                .fetch_all(&pool)
                .await
                .unwrap();

        assert!(columns.contains(&"processing_started_at".to_string()));
        assert!(columns.contains(&"lease_token".to_string()));
        assert!(columns.contains(&"processing_worker_id".to_string()));

        // Insert and read back using lease columns
        sqlx::query(
            r#"
            INSERT INTO webhook_events (
                event_id, event_type, transmission_id, processing_status,
                raw_payload, payload_hash, created_at,
                processing_started_at, lease_token, processing_worker_id
            ) VALUES (
                'evt-lease-test', 'CUSTOMER.DISPUTE.CREATED', 'tx-1', 'PROCESSING',
                x'7b7d', 'hash', '2026-11-08T00:00:00Z',
                '2026-11-08T00:00:01Z', 'token-uuid-1', 'worker-1'
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let (started_at, token, worker_id): (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT processing_started_at, lease_token, processing_worker_id FROM webhook_events WHERE event_id = 'evt-lease-test';"
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(started_at.as_deref(), Some("2026-11-08T00:00:01Z"));
        assert_eq!(token.as_deref(), Some("token-uuid-1"));
        assert_eq!(worker_id.as_deref(), Some("worker-1"));
    }

    #[tokio::test]
    async fn test_migration_repeated_initialization_idempotency() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        // Run migrations repeatedly
        for _ in 0..3 {
            let res = run_migrations(&pool).await;
            assert!(
                res.is_ok(),
                "Repeated migration run must succeed idempotently"
            );
        }
    }

    #[tokio::test]
    async fn test_migration_existing_phase_1_to_3_data_preservation() {
        // Create an un-migrated database with older schema manually
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE sessions (
                session_id TEXT PRIMARY KEY,
                initial_prompt TEXT NOT NULL,
                paypal_order_id TEXT,
                root_hash TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE decision_steps (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                sequence_index INTEGER NOT NULL,
                action_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                previous_hash TEXT NOT NULL,
                current_hash TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE webhook_events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                resource_type TEXT,
                resource_id TEXT,
                transmission_id TEXT NOT NULL,
                processing_status TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                max_retries INTEGER NOT NULL DEFAULT 5,
                last_error TEXT,
                raw_payload BLOB NOT NULL,
                payload_hash TEXT NOT NULL,
                next_retry_at TEXT,
                created_at TEXT NOT NULL,
                processed_at TEXT
            );

            INSERT INTO sessions VALUES ('sess-legacy-1', 'Legacy prompt', 'ORD-LEGACY', 'root-legacy', '2026-11-07T00:00:00Z', '2026-11-07T00:00:00Z');
            INSERT INTO decision_steps VALUES ('step-legacy-1', 'sess-legacy-1', 0, 'PROMPT', '{}', '0000', 'hash-1', '2026-11-07T00:00:00Z');
            INSERT INTO webhook_events (event_id, event_type, transmission_id, processing_status, raw_payload, payload_hash, created_at)
            VALUES ('evt-legacy-1', 'CUSTOMER.DISPUTE.CREATED', 'tx-legacy', 'PENDING', x'7b7d', 'hash-legacy', '2026-11-07T00:00:00Z');
            "#
        )
        .execute(&pool)
        .await
        .unwrap();

        // Run full migrations on this existing database
        run_migrations(&pool).await.unwrap();

        // Verify existing data is completely intact
        let session_prompt: String = sqlx::query_scalar(
            "SELECT initial_prompt FROM sessions WHERE session_id = 'sess-legacy-1';",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(session_prompt, "Legacy prompt");

        let step_action: String = sqlx::query_scalar(
            "SELECT action_type FROM decision_steps WHERE id = 'step-legacy-1';",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            step_action,
            "PROCESSED".is_empty().then(|| "").unwrap_or("PROMPT")
        );

        let event_type: String = sqlx::query_scalar(
            "SELECT event_type FROM webhook_events WHERE event_id = 'evt-legacy-1';",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_type, "CUSTOMER.DISPUTE.CREATED");

        // Verify lease columns were added by migration to the existing table
        let (started_at, lease_token): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT processing_started_at, lease_token FROM webhook_events WHERE event_id = 'evt-legacy-1';"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(started_at, None);
        assert_eq!(lease_token, None);

        // Verify new tables are now available
        let dispute_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM disputes;")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(dispute_count, 0);
    }

    #[tokio::test]
    async fn test_migration_existing_earlier_disputes_schema_upgrade() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query("PRAGMA foreign_keys = ON;")
            .execute(&pool)
            .await
            .unwrap();

        // 1. Create older Step 1 schema:
        // - session_id in disputes lacks foreign key to sessions
        // - dispute_update_time in disputes is NOT NULL
        sqlx::query(
            r#"
            CREATE TABLE sessions (
                session_id TEXT PRIMARY KEY,
                initial_prompt TEXT NOT NULL,
                paypal_order_id TEXT,
                root_hash TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE decision_steps (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                sequence_index INTEGER NOT NULL,
                action_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                previous_hash TEXT NOT NULL,
                current_hash TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE webhook_events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                resource_type TEXT,
                resource_id TEXT,
                transmission_id TEXT NOT NULL,
                processing_status TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                max_retries INTEGER NOT NULL DEFAULT 5,
                last_error TEXT,
                raw_payload BLOB NOT NULL,
                payload_hash TEXT NOT NULL,
                next_retry_at TEXT,
                created_at TEXT NOT NULL,
                processed_at TEXT
            );

            CREATE TABLE disputes (
                dispute_id TEXT PRIMARY KEY,
                session_id TEXT,
                genesis_anchor_type TEXT NOT NULL,
                genesis_anchor_hash TEXT NOT NULL,
                purchase_root_hash TEXT,
                seller_transaction_id TEXT,
                paypal_order_id TEXT,
                status TEXT NOT NULL,
                dispute_lifecycle_stage TEXT,
                dispute_channel TEXT,
                reason TEXT,
                dispute_amount_value TEXT,
                dispute_amount_currency TEXT,
                correlation_status TEXT NOT NULL,
                matched_by TEXT,
                root_hash TEXT,
                dispute_create_time TEXT,
                dispute_update_time TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE dispute_audit_steps (
                id TEXT PRIMARY KEY,
                dispute_id TEXT NOT NULL REFERENCES disputes(dispute_id) ON DELETE RESTRICT,
                sequence_index INTEGER NOT NULL,
                action_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                previous_hash TEXT NOT NULL,
                current_hash TEXT NOT NULL,
                created_at TEXT NOT NULL,
                CONSTRAINT uq_dispute_sequence UNIQUE (dispute_id, sequence_index)
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // 2. Insert representative existing data
        sqlx::query(
            "INSERT INTO sessions VALUES ('sess-legacy-1', 'Legacy prompt', 'ORD-LEGACY', 'root-legacy', '2026-11-07T00:00:00Z', '2026-11-07T00:00:00Z');"
        )
        .execute(&pool)
        .await
        .unwrap();

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
                'disp-legacy-1', 'sess-legacy-1', 'PURCHASE_ROOT_HASH', 'root-legacy',
                'root-legacy', 'tx-123', 'ORD-LEGACY',
                'UNDER_REVIEW', 'INQUIRY', 'INTERNAL', 'MERCHANDISE_NOT_RECEIVED',
                '149.99', 'USD', 'MATCHED',
                'CAPTURE_ID', 'hash-disp-1', '2026-11-07T00:00:00Z', '2026-11-07T01:00:00Z',
                '2026-11-07T00:00:00Z', '2026-11-07T01:00:00Z'
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            INSERT INTO dispute_audit_steps VALUES (
                'step-disp-1', 'disp-legacy-1', 0, 'DISPUTE_IMPORTED',
                '{"reason":"test"}', '0000000000000000000000000000000000000000000000000000000000000000',
                'hash-disp-1', '2026-11-07T00:00:00Z'
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // 3. Run migration / initialization
        run_migrations(&pool).await.unwrap();

        // 4. Verify existing data is preserved
        let (dispute_id, sess_id, update_time): (String, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT dispute_id, session_id, dispute_update_time FROM disputes WHERE dispute_id = 'disp-legacy-1';",
            )
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(dispute_id, "disp-legacy-1");
        assert_eq!(sess_id.as_deref(), Some("sess-legacy-1"));
        assert_eq!(update_time.as_deref(), Some("2026-11-07T01:00:00Z"));

        let audit_step_id: String = sqlx::query_scalar(
            "SELECT id FROM dispute_audit_steps WHERE dispute_id = 'disp-legacy-1';",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit_step_id, "step-disp-1");

        // 5. Verify nonexistent session references are rejected
        let invalid_sess_res = sqlx::query(
            r#"
            INSERT INTO disputes (
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                status, correlation_status, created_at, updated_at
            ) VALUES (
                'disp-invalid-sess', 'nonexistent-session', 'GENESIS_HASH', '0000',
                'UNDER_REVIEW', 'NOT_FOUND', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z'
            );
            "#,
        )
        .execute(&pool)
        .await;
        assert!(
            invalid_sess_res.is_err(),
            "Must reject nonexistent session_id reference"
        );

        // 6. Verify linked sessions cannot be deleted
        let del_sess_res = sqlx::query("DELETE FROM sessions WHERE session_id = 'sess-legacy-1';")
            .execute(&pool)
            .await;
        assert!(
            del_sess_res.is_err(),
            "Must restrict deletion of linked session"
        );

        // 7. Verify dispute_update_time = NULL is accepted
        let null_update_res = sqlx::query(
            r#"
            INSERT INTO disputes (
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                status, correlation_status, created_at, updated_at,
                dispute_update_time
            ) VALUES (
                'disp-null-update', NULL, 'GENESIS_HASH', '0000',
                'UNDER_REVIEW', 'NOT_FOUND', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z',
                NULL
            );
            "#,
        )
        .execute(&pool)
        .await;
        assert!(
            null_update_res.is_ok(),
            "Must accept NULL dispute_update_time"
        );

        // 8. Verify genesis-anchor protection trigger still exists and protects data
        let tamper_res = sqlx::query(
            "UPDATE disputes SET genesis_anchor_hash = 'tampered' WHERE dispute_id = 'disp-legacy-1';",
        )
        .execute(&pool)
        .await;
        assert!(
            tamper_res.is_err(),
            "Must prevent tampering with genesis anchor"
        );

        // 9. Verify required indexes still exist
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='disputes';",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert!(indexes.contains(&"idx_disputes_session_id".to_string()));
        assert!(indexes.contains(&"idx_disputes_correlation_status".to_string()));
        assert!(indexes.contains(&"idx_disputes_status".to_string()));
        assert!(indexes.contains(&"idx_disputes_created_at".to_string()));

        // 10. Verify repeated migration runs are safe and idempotent
        for _ in 0..3 {
            run_migrations(&pool).await.unwrap();
        }

        let dispute_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM disputes;")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            dispute_count, 2,
            "Data must be fully preserved across repeated migration runs"
        );

        // 11. Verify foreign-key enforcement is enabled and active on pool connection
        let fk_status: i64 = sqlx::query_scalar("PRAGMA foreign_keys;")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            fk_status, 1,
            "Foreign keys must be enabled on connection after migration"
        );
    }

    #[tokio::test]
    async fn test_migration_failure_restores_foreign_keys_and_aborts_cleanly() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query("PRAGMA foreign_keys = ON;")
            .execute(&pool)
            .await
            .unwrap();

        // 1. Create older Step 1 schema
        sqlx::query(
            r#"
            CREATE TABLE sessions (
                session_id TEXT PRIMARY KEY,
                initial_prompt TEXT NOT NULL,
                paypal_order_id TEXT,
                root_hash TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE decision_steps (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                sequence_index INTEGER NOT NULL,
                action_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                previous_hash TEXT NOT NULL,
                current_hash TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE webhook_events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                resource_type TEXT,
                resource_id TEXT,
                transmission_id TEXT NOT NULL,
                processing_status TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                max_retries INTEGER NOT NULL DEFAULT 5,
                last_error TEXT,
                raw_payload BLOB NOT NULL,
                payload_hash TEXT NOT NULL,
                next_retry_at TEXT,
                created_at TEXT NOT NULL,
                processed_at TEXT
            );

            CREATE TABLE disputes (
                dispute_id TEXT PRIMARY KEY,
                session_id TEXT,
                genesis_anchor_type TEXT NOT NULL,
                genesis_anchor_hash TEXT NOT NULL,
                purchase_root_hash TEXT,
                seller_transaction_id TEXT,
                paypal_order_id TEXT,
                status TEXT NOT NULL,
                dispute_lifecycle_stage TEXT,
                dispute_channel TEXT,
                reason TEXT,
                dispute_amount_value TEXT,
                dispute_amount_currency TEXT,
                correlation_status TEXT NOT NULL,
                matched_by TEXT,
                root_hash TEXT,
                dispute_create_time TEXT,
                dispute_update_time TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // 2. Insert invalid data into the old unconstrained table:
        // A dispute referencing a session that DOES NOT exist in sessions table
        sqlx::query(
            r#"
            INSERT INTO disputes (
                dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
                status, correlation_status, dispute_update_time, created_at, updated_at
            ) VALUES (
                'disp-broken-1', 'nonexistent-session-id', 'GENESIS_HASH', '0000',
                'UNDER_REVIEW', 'NOT_FOUND', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z', '2026-11-08T00:00:00Z'
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // 3. Run migration: it MUST detect foreign-key violations and fail
        let mig_res = run_migrations(&pool).await;
        assert!(
            mig_res.is_err(),
            "Migration must fail due to foreign key violations in existing data"
        );

        // 4. Verify foreign-key enforcement is STILL restored on pool/connection
        let fk_status: i64 = sqlx::query_scalar("PRAGMA foreign_keys;")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            fk_status, 1,
            "Foreign-key enforcement must not be left disabled after migration failure"
        );

        // 5. Verify no partially rebuilt temp table disputes_new was left behind
        let temp_table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'disputes_new';",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            temp_table_count, 0,
            "No temp table disputes_new should remain after rollback"
        );

        // 6. Verify original disputes table is preserved untouched
        let dispute_id: String = sqlx::query_scalar(
            "SELECT dispute_id FROM disputes WHERE dispute_id = 'disp-broken-1';",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(dispute_id, "disp-broken-1");
    }
}
