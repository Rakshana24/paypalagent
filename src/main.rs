use std::net::SocketAddr;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use agentic_commerce::config::Config;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::services::{
    CatalogService, LedgerService, PayPalAuthService, PayPalOrdersService, ShoppingAgentService,
    VerificationService,
};
use agentic_commerce::{create_router, AppState};

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

    let state = AppState {
        config: config_arc,
        http_client,
        auth_service,
        ledger_service,
        verification_service,
        catalog_service,
        shopping_agent,
        paypal_orders,
    };

    let app = create_router(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind TCP listener to {}: {}", addr, e));

    tracing::info!("Server running on http://{}", addr);

    axum::serve(listener, app)
        .await
        .expect("Server encountered a fatal error while running");
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
