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
            processed_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_webhook_events_status_next_retry
            ON webhook_events(processing_status, next_retry_at);

        CREATE INDEX IF NOT EXISTS idx_webhook_events_resource_id
            ON webhook_events(resource_id);

        CREATE INDEX IF NOT EXISTS idx_webhook_events_created_at
            ON webhook_events(created_at DESC);
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}
