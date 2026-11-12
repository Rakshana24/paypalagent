use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
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
use models::webhook::{PayPalWebhookEvent, PayPalWebhookHeaders, WebhookPersistenceResult};
use models::{
    ai_investigation::{AIDisputeInvestigationReport, AIInvestigationRequest},
    AppendEventRequest, AppendEventResponse, AuthCheckResponse, AuthorizePurchaseRequest,
    AuthorizePurchaseResponse, CaptureOrderResponse, CreateOrderResponse, CreateSessionRequest,
    CreateSessionResponse, DecisionStep, FinalizeSessionResponse, HealthResponse,
    PayPalOrderStatusResponse, Product, Session, ShoppingRequest, ShoppingSessionDetail,
    ShoppingSessionResponse, VerificationResult,
};
use services::{
    ai_investigator::AIInvestigatorService, CatalogService, DisputeEvidenceService, LedgerService,
    PayPalAuthService, PayPalOrdersService, ShoppingAgentService, VerificationService,
    WebhookPersistenceService, WebhookVerifierService,
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
    pub webhook_verifier: Arc<WebhookVerifierService>,
    pub webhook_persistence: Arc<WebhookPersistenceService>,
    pub webhook_notify: Arc<tokio::sync::Notify>,
    pub dispute_evidence: Arc<DisputeEvidenceService>,
    pub ai_investigator: Arc<AIInvestigatorService>,
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

/// GET /api/shopping/sessions/:session_id/order-status handler to query authoritative live PayPal order status.
pub async fn get_shopping_order_status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<PayPalOrderStatusResponse>, AppError> {
    let res = state.paypal_orders.get_order_status(&session_id).await?;
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

/// Maximum allowable request body size for PayPal webhooks (256 KB).
pub const MAX_WEBHOOK_PAYLOAD_SIZE: usize = 256 * 1024;

/// POST /api/webhooks/paypal handler to ingest, verify, and durably persist PayPal webhooks.
pub async fn paypal_webhook_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body_bytes: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    // 1. Enforce payload size limit
    if body_bytes.len() > MAX_WEBHOOK_PAYLOAD_SIZE {
        return Err(AppError::InvalidPayload(format!(
            "Webhook request payload exceeds maximum allowed size of {} bytes",
            MAX_WEBHOOK_PAYLOAD_SIZE
        )));
    }

    if body_bytes.is_empty() {
        return Err(AppError::InvalidPayload("Empty request body".to_string()));
    }

    // 2. Extract PayPal transmission headers case-insensitively
    let webhook_headers = PayPalWebhookHeaders::from_header_map(&headers)?;

    // 3. Parse JSON from preserved raw bytes into PayPalWebhookEvent and serde_json::Value
    let event: PayPalWebhookEvent = serde_json::from_slice(&body_bytes)
        .map_err(|e| AppError::InvalidPayload(format!("Malformed webhook JSON payload: {}", e)))?;

    let event_json: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| AppError::InvalidPayload(format!("Malformed webhook JSON payload: {}", e)))?;

    // 4. Verify signature using existing WebhookVerifierService and configured PAYPAL_WEBHOOK_ID (preserving raw bytes for diagnostic verification)
    state
        .webhook_verifier
        .verify_signature_with_raw(&webhook_headers, &event_json, Some(&body_bytes))
        .await?;

    // 5. Persist the exact original raw bytes using WebhookPersistenceService
    let persistence_result = state
        .webhook_persistence
        .persist_event(&event, &webhook_headers.transmission_id, &body_bytes)
        .await?;

    // 6. Return appropriate success acknowledgement based on idempotency outcome
    match persistence_result {
        WebhookPersistenceResult::Inserted(record) => {
            // Wake up background dispute worker immediately upon new event persistence
            state.webhook_notify.notify_one();

            tracing::info!(
                event_id = %record.event_id,
                event_type = %record.event_type,
                transmission_id = %webhook_headers.transmission_id,
                "PayPal webhook event accepted and persisted"
            );
            Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "success": true,
                    "status": "ACCEPTED",
                    "event_id": record.event_id,
                    "message": "Webhook event accepted and persisted"
                })),
            ))
        }
        WebhookPersistenceResult::Duplicate { event_id } => {
            tracing::info!(
                event_id = %event_id,
                transmission_id = %webhook_headers.transmission_id,
                "Duplicate PayPal webhook event acknowledged idempotently"
            );
            Ok((
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "status": "DUPLICATE",
                    "event_id": event_id,
                    "message": "Duplicate webhook event acknowledged idempotently"
                })),
            ))
        }
    }
}

/// GET /api/disputes/:dispute_id/evidence handler to generate and retrieve structured evidence brief JSON.
pub async fn get_dispute_evidence(
    State(state): State<AppState>,
    Path(dispute_id): Path<String>,
) -> Result<Json<crate::models::evidence::DisputeEvidenceBrief>, AppError> {
    let brief = state
        .dispute_evidence
        .generate_evidence_brief(&dispute_id)
        .await?;
    Ok(Json(brief))
}

/// GET /api/disputes/:dispute_id/evidence/markdown handler to generate and retrieve human-readable Markdown evidence brief.
pub async fn get_dispute_evidence_markdown(
    State(state): State<AppState>,
    Path(dispute_id): Path<String>,
) -> Result<([(axum::http::HeaderName, &'static str); 1], String), AppError> {
    let brief = state
        .dispute_evidence
        .generate_evidence_brief(&dispute_id)
        .await?;
    let markdown = brief.to_markdown();
    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            "text/markdown; charset=utf-8",
        )],
        markdown,
    ))
}

/// POST /api/disputes/:dispute_id/ai-investigation handler to synthesize an AI forensic report.
///
/// Accepts an optional request body containing `force_fallback`. When Gemini is unconfigured
/// or offline, automatically and safely falls back to the deterministic audit engine.
/// This endpoint is strictly read-only and never modifies the underlying ledger.
pub async fn investigate_dispute(
    State(state): State<AppState>,
    Path(dispute_id): Path<String>,
    body_bytes: Bytes,
) -> Result<Json<AIDisputeInvestigationReport>, AppError> {
    let req: AIInvestigationRequest = if body_bytes.is_empty() {
        AIInvestigationRequest::default()
    } else {
        serde_json::from_slice(&body_bytes).map_err(|e| {
            AppError::InvalidPayload(format!("Invalid investigation request: {}", e))
        })?
    };

    // 1. Authoritatively retrieve the dispute evidence brief
    let brief = state
        .dispute_evidence
        .generate_evidence_brief(&dispute_id)
        .await?;

    // 2. Synthesize AI dispute investigation report
    let report = state
        .ai_investigator
        .investigate(&brief, req.force_fallback)
        .await?;

    Ok(Json(report))
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
            "/api/shopping/sessions/:session_id/order-status",
            get(get_shopping_order_status),
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
        // Phase 4 Webhook Route
        .route(
            "/api/webhooks/paypal",
            post(paypal_webhook_handler).layer(DefaultBodyLimit::max(MAX_WEBHOOK_PAYLOAD_SIZE)),
        )
        // Phase 5 Dispute Evidence Routes
        .route(
            "/api/disputes/:dispute_id/evidence",
            get(get_dispute_evidence),
        )
        .route(
            "/api/disputes/:dispute_id/evidence/markdown",
            get(get_dispute_evidence_markdown),
        )
        // Phase 6 AI Dispute Investigation Route
        .route(
            "/api/disputes/:dispute_id/ai-investigation",
            post(investigate_dispute),
        )
        .with_state(state)
}
