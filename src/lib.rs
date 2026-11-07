use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;

pub mod config;
pub mod db;
pub mod error;
pub mod http_client;
pub mod ledger;
pub mod models;
pub mod services;

use config::Config;
use error::AppError;
use models::{
    AppendEventRequest, AppendEventResponse, AuthCheckResponse, AuthorizePurchaseRequest,
    AuthorizePurchaseResponse, CaptureOrderResponse, CreateOrderResponse, CreateSessionRequest,
    CreateSessionResponse, DecisionStep, FinalizeSessionResponse, HealthResponse, Product, Session,
    ShoppingRequest, ShoppingSessionDetail, ShoppingSessionResponse, VerificationResult,
};
use services::{
    CatalogService, LedgerService, PayPalAuthService, PayPalOrdersService, ShoppingAgentService,
    VerificationService,
};

/// Shared application state across HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub http_client: reqwest::Client,
    pub auth_service: Arc<PayPalAuthService>,
    pub ledger_service: Arc<LedgerService>,
    pub verification_service: Arc<VerificationService>,
    pub catalog_service: CatalogService,
    pub shopping_agent: Arc<ShoppingAgentService>,
    pub paypal_orders: Arc<PayPalOrdersService>,
}

/// GET /health handler returning operational status.
pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        success: true,
        message: "AgenticCommerce backend is running".to_string(),
    })
}

/// GET /api/paypal/auth-check handler returning safe PayPal sandbox authentication status.
pub async fn paypal_auth_check(
    State(state): State<AppState>,
) -> Result<Json<AuthCheckResponse>, AppError> {
    let status = state.auth_service.get_auth_status().await?;
    Ok(Json(status))
}

/// POST /api/sessions handler to initialize an audit session.
pub async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), AppError> {
    let res = state.ledger_service.create_session(payload).await?;
    Ok((StatusCode::CREATED, Json(res)))
}

/// POST /api/sessions/:session_id/events handler to append an event to an audit session.
pub async fn append_event(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<AppendEventRequest>,
) -> Result<(StatusCode, Json<AppendEventResponse>), AppError> {
    let res = state
        .ledger_service
        .append_event(&session_id, payload)
        .await?;
    Ok((StatusCode::CREATED, Json(res)))
}

/// GET /api/sessions/:session_id handler to retrieve session metadata.
pub async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Session>, AppError> {
    let session = state.ledger_service.get_session(&session_id).await?;
    Ok(Json(session))
}

/// GET /api/sessions/:session_id/events handler to retrieve all steps in sequence.
pub async fn get_session_events(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<DecisionStep>>, AppError> {
    let events = state.ledger_service.get_session_events(&session_id).await?;
    Ok(Json(events))
}

/// GET /api/sessions/:session_id/verify handler to execute cryptographic integrity verification.
pub async fn verify_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<VerificationResult>, AppError> {
    let result = state
        .verification_service
        .verify_session(&session_id)
        .await?;
    Ok(Json(result))
}

/// POST /api/sessions/:session_id/finalize handler to finalize a session and freeze its root hash.
pub async fn finalize_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<FinalizeSessionResponse>, AppError> {
    let res = state.ledger_service.finalize_session(&session_id).await?;
    Ok(Json(res))
}

/// GET /api/catalog/products handler to list all available products.
pub async fn list_catalog_products(State(state): State<AppState>) -> Json<Vec<Product>> {
    Json(state.catalog_service.list_products())
}

/// POST /api/shopping/sessions handler to run the reference shopping agent.
pub async fn create_shopping_session(
    State(state): State<AppState>,
    Json(payload): Json<ShoppingRequest>,
) -> Result<(StatusCode, Json<ShoppingSessionResponse>), AppError> {
    let res = state
        .shopping_agent
        .process_shopping_request(&payload.prompt)
        .await?;
    Ok((StatusCode::CREATED, Json(res)))
}

/// POST /api/shopping/sessions/:session_id/authorize handler for customer purchase authorization.
pub async fn authorize_shopping_purchase(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<AuthorizePurchaseRequest>,
) -> Result<Json<AuthorizePurchaseResponse>, AppError> {
    let res = state
        .shopping_agent
        .authorize_purchase(&session_id, payload)
        .await?;
    Ok(Json(res))
}

/// POST /api/shopping/sessions/:session_id/orders handler to create a PayPal Sandbox order.
pub async fn create_shopping_order(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<(StatusCode, Json<CreateOrderResponse>), AppError> {
    let res = state.paypal_orders.create_order(&session_id).await?;
    Ok((StatusCode::CREATED, Json(res)))
}

/// POST /api/shopping/sessions/:session_id/capture handler to capture payment and finalize session.
pub async fn capture_shopping_order(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<CaptureOrderResponse>, AppError> {
    let res = state.paypal_orders.capture_order(&session_id).await?;
    Ok(Json(res))
}

/// GET /api/shopping/sessions/:session_id handler to get full shopping and payment lifecycle details.
pub async fn get_shopping_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<ShoppingSessionDetail>, AppError> {
    let res = state
        .shopping_agent
        .get_shopping_session_detail(&session_id)
        .await?;
    Ok(Json(res))
}

pub async fn checkout_success_handler() -> axum::response::Html<&'static str> {
    axum::response::Html("<html><body><h2>PayPal Sandbox Approval Successful</h2><p>Buyer approval recorded. You can return to AgentAudit to capture the payment.</p></body></html>")
}

pub async fn checkout_cancel_handler() -> axum::response::Html<&'static str> {
    axum::response::Html("<html><body><h2>PayPal Sandbox Approval Cancelled</h2><p>The transaction was cancelled by the buyer.</p></body></html>")
}

/// Builds the Axum application router with shared state.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/paypal/auth-check", get(paypal_auth_check))
        .route("/api/sessions", post(create_session))
        .route("/api/sessions/:session_id", get(get_session))
        .route(
            "/api/sessions/:session_id/events",
            post(append_event).get(get_session_events),
        )
        .route("/api/sessions/:session_id/verify", get(verify_session))
        .route("/api/sessions/:session_id/finalize", post(finalize_session))
        // Phase 3 Shopping & PayPal Orders routes
        .route("/api/catalog/products", get(list_catalog_products))
        .route("/api/shopping/sessions", post(create_shopping_session))
        .route(
            "/api/shopping/sessions/:session_id",
            get(get_shopping_session),
        )
        .route(
            "/api/shopping/sessions/:session_id/authorize",
            post(authorize_shopping_purchase),
        )
        .route(
            "/api/shopping/sessions/:session_id/orders",
            post(create_shopping_order),
        )
        .route(
            "/api/shopping/sessions/:session_id/capture",
            post(capture_shopping_order),
        )
        .route(
            "/api/shopping/checkout/success",
            get(checkout_success_handler),
        )
        .route(
            "/api/shopping/checkout/cancel",
            get(checkout_cancel_handler),
        )
        .with_state(state)
}
