use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

/// Application-wide error types.
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("HTTP client error: {0}")]
    HttpClient(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("PayPal authentication error: {0}")]
    PayPalAuth(String),

    #[error("PayPal API error: {0}")]
    PayPalApi(String),

    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Session is finalized: {0}")]
    SessionFinalized(String),

    #[error("Invalid payload: {0}")]
    InvalidPayload(String),

    #[error("Product not found: {0}")]
    ProductNotFound(String),

    #[error("Unauthorized purchase: {0}")]
    UnauthorizedPurchase(String),

    #[error("Order capture failed: {0}")]
    OrderCaptureFailed(String),

    #[error("Ledger integrity error: {0}")]
    LedgerIntegrity(String),

    #[error("Missing webhook header: {0}")]
    MissingWebhookHeader(String),

    #[error("Webhook verification failed: {0}")]
    WebhookVerificationFailed(String),

    #[error("Webhook event not found: {0}")]
    WebhookNotFound(String),

    #[error("Invalid webhook state transition: {0}")]
    InvalidStateTransition(String),

    #[error("Payload hash mismatch: {0}")]
    PayloadHashMismatch(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, safe_message) = match &self {
            AppError::Config(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            AppError::PayPalAuth(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AppError::HttpClient(err) => {
                let msg = if err.is_timeout() {
                    "Request to PayPal timed out".to_string()
                } else if err.is_connect() {
                    "Failed to connect to PayPal server".to_string()
                } else {
                    "Error communicating with PayPal API".to_string()
                };
                (StatusCode::BAD_GATEWAY, msg)
            }
            AppError::Serialization(_) => (
                StatusCode::BAD_GATEWAY,
                "Malformed response received from API".to_string(),
            ),
            AppError::PayPalApi(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            AppError::SessionNotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::ProductNotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::SessionFinalized(msg) => (StatusCode::CONFLICT, msg.clone()),
            AppError::UnauthorizedPurchase(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            AppError::OrderCaptureFailed(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            AppError::InvalidPayload(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::LedgerIntegrity(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::MissingWebhookHeader(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::WebhookVerificationFailed(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AppError::WebhookNotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::InvalidStateTransition(msg) => (StatusCode::CONFLICT, msg.clone()),
            AppError::PayloadHashMismatch(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            AppError::Database(err) => {
                tracing::error!("Database operation failed: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database operation failed".to_string(),
                )
            }
            AppError::Io(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            ),
        };

        let body = Json(json!({
            "success": false,
            "message": safe_message,
        }));

        (status, body).into_response()
    }
}
