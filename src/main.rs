use std::net::SocketAddr;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use agentic_commerce::config::Config;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::services::{
    BackgroundDisputeWorker, CancellationToken, CatalogService, DisputeConsumerService,
    DisputeCorrelationService, DisputeEvidenceService, DisputeService, LedgerService,
    PayPalAuthService, PayPalOrdersService, ShoppingAgentService, VerificationService,
    WebhookPersistenceService, WebhookVerifierService,
};
use agentic_commerce::{create_router, AppState};

/// Listens for process termination signals (Ctrl+C / SIGINT, and SIGTERM on Unix).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C signal handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C (SIGINT) signal; initiating graceful shutdown...");
        }
        _ = terminate => {
            tracing::info!("Received SIGTERM signal; initiating graceful shutdown...");
        }
    }
}

#[tokio::main]
async fn main() {
    // Load environment variables from .env file if present
    dotenvy::dotenv().ok();

    // Centralized configuration loading
    let config = Config::from_env();

    // Initialize structured logging with tracing-subscriber
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| config.rust_log.clone().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Initializing AgenticCommerce backend...");
    // Safe logging - config Debug implementation redacts client secrets
    tracing::debug!("Loaded configuration: {:?}", config);

    // Initialize embedded SQLite database
    let db_pool = init_db(&config.database_url)
        .await
        .expect("Failed to initialize SQLite database");

    // Initialize reusable asynchronous HTTP client
    let http_client =
        create_http_client(&HttpClientConfig::default()).expect("Failed to initialize HTTP client");

    let config_arc = Arc::new(config.clone());

    let auth_service = Arc::new(PayPalAuthService::new(
        config_arc.clone(),
        http_client.clone(),
    ));

    let ledger_service = Arc::new(LedgerService::new(db_pool.clone()));
    let verification_service = Arc::new(VerificationService::new(db_pool.clone()));
    let catalog_service = CatalogService::new();
    let shopping_agent = Arc::new(ShoppingAgentService::new(
        catalog_service.clone(),
        ledger_service.clone(),
        verification_service.clone(),
    ));
    let paypal_orders = Arc::new(PayPalOrdersService::new(
        config_arc.clone(),
        http_client.clone(),
        auth_service.clone(),
        ledger_service.clone(),
    ));
    let webhook_verifier = Arc::new(WebhookVerifierService::new(
        config_arc.clone(),
        http_client.clone(),
        auth_service.clone(),
    ));
    let webhook_persistence = Arc::new(WebhookPersistenceService::new(db_pool.clone()));

    // Initialize dispute processing services
    let dispute_service = Arc::new(DisputeService::new(
        db_pool.clone(),
        verification_service.clone(),
    ));
    let dispute_correlation = Arc::new(DisputeCorrelationService::new(
        config_arc.clone(),
        http_client.clone(),
        auth_service.clone(),
        db_pool.clone(),
        verification_service.clone(),
    ));
    let dispute_consumer = Arc::new(DisputeConsumerService::new(
        db_pool.clone(),
        webhook_persistence.clone(),
        dispute_service.clone(),
        dispute_correlation.clone(),
    ));

    let dispute_evidence = Arc::new(DisputeEvidenceService::new(
        db_pool.clone(),
        dispute_service.clone(),
        ledger_service.clone(),
        verification_service.clone(),
    ));

    // Shared notification channel between HTTP ingress and background worker
    let webhook_notify = Arc::new(tokio::sync::Notify::new());

    // Initialize autonomous background dispute worker
    let worker = BackgroundDisputeWorker::new(dispute_consumer.clone(), webhook_notify.clone());

    // Cooperative shutdown token for the background worker
    let cancel_token = CancellationToken::new();
    let worker_cancel_token = cancel_token.clone();

    // Spawn background worker task (automatically drains pending events on startup)
    let worker_handle = tokio::spawn(async move {
        worker.run(worker_cancel_token).await;
    });

    let state = AppState {
        config: config_arc,
        http_client,
        auth_service,
        ledger_service,
        verification_service,
        catalog_service,
        shopping_agent,
        paypal_orders,
        webhook_verifier,
        webhook_persistence,
        webhook_notify: webhook_notify.clone(),
        dispute_evidence,
    };

    let app = create_router(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind TCP listener to {}: {}", addr, e));

    tracing::info!("Server running on http://{}", addr);

    // Run HTTP server with graceful shutdown on Ctrl+C / SIGTERM
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("Server encountered a fatal error while running");

    tracing::info!("HTTP server stopped; signaling background dispute worker to terminate...");
    cancel_token.cancel();

    // Await background worker termination to ensure in-flight events complete cleanly
    match tokio::time::timeout(std::time::Duration::from_secs(10), worker_handle).await {
        Ok(Ok(())) => {
            tracing::info!("Background dispute worker shut down cleanly");
        }
        Ok(Err(join_err)) => {
            tracing::error!("Background dispute worker join error: {}", join_err);
        }
        Err(_) => {
            tracing::warn!("Timed out waiting for background worker shutdown after 10s");
        }
    }
}

#[cfg(test)]
mod tests {
    use agentic_commerce::health_check;

    #[tokio::test]
    async fn test_health_check_handler() {
        let resp = health_check().await;
        assert!(resp.0.success);
        assert_eq!(resp.0.message, "AgenticCommerce backend is running");
    }
}
