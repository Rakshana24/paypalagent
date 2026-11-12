use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use agentic_commerce::config::Config;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::correlation::{CorrelationStatus, MatchedBy};
use agentic_commerce::models::ledger::{AppendEventRequest, CreateSessionRequest};
use agentic_commerce::models::webhook::PayPalWebhookEvent;
use agentic_commerce::services::auth::PayPalAuthService;
use agentic_commerce::services::dispute_correlation::DisputeCorrelationService;
use agentic_commerce::services::ledger::LedgerService;
use agentic_commerce::services::verification::VerificationService;

/// Shared state for mock PayPal HTTP server.
#[derive(Clone)]
struct MockPayPalServerState {
    pub _token_request_count: Arc<AtomicUsize>,
    pub dispute_details_request_count: Arc<AtomicUsize>,
    pub dispute_details_status: Arc<RwLock<StatusCode>>,
    pub dispute_details_body: Arc<RwLock<serde_json::Value>>,
}

/// Sets up a mock PayPal server providing OAuth and Show Dispute Details endpoints.
async fn setup_mock_paypal_server() -> (String, MockPayPalServerState) {
    let state = MockPayPalServerState {
        _token_request_count: Arc::new(AtomicUsize::new(0)),
        dispute_details_request_count: Arc::new(AtomicUsize::new(0)),
        dispute_details_status: Arc::new(RwLock::new(StatusCode::OK)),
        dispute_details_body: Arc::new(RwLock::new(json!({}))),
    };

    let token_handler = |_req: axum::extract::Request| async {
        let body = json!({
            "access_token": "mock-access-token-correlation-test",
            "token_type": "Bearer",
            "expires_in": 3600
        });
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            body.to_string(),
        )
    };

    let dispute_state = state.clone();
    let dispute_handler = move |Path(dispute_id): Path<String>| {
        let s = dispute_state.clone();
        async move {
            s.dispute_details_request_count
                .fetch_add(1, Ordering::SeqCst);

            let status = *s.dispute_details_status.read().await;
            let mut body = s.dispute_details_body.read().await.clone();

            if status.is_success() && body.get("dispute_id").is_none() {
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("dispute_id".to_string(), json!(dispute_id));
                }
            }

            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        }
    };

    let router = Router::new()
        .route("/v1/oauth2/token", post(token_handler))
        .route("/v1/customer/disputes/:dispute_id", get(dispute_handler));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    (url, state)
}

/// Helper to create a finalized purchase session with valid ledger events.
async fn create_finalized_purchase_session(
    ledger_service: &LedgerService,
    order_id: &str,
    capture_id: &str,
) -> String {
    let session = ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Buy Sony WH-1000XM5 headphones".to_string(),
        })
        .await
        .unwrap();

    ledger_service
        .set_paypal_order_id(&session.session_id, order_id)
        .await
        .unwrap();

    // 1. PRODUCT_SELECTED event
    ledger_service
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "PRODUCT_SELECTED".to_string(),
                payload: json!({
                    "product": {
                        "product_id": "prod-1",
                        "name": "Sony WH-1000XM5",
                        "price": 399.99,
                        "currency": "USD"
                    },
                    "reasoning": "Best match for requirements"
                }),
            },
        )
        .await
        .unwrap();

    // 2. PURCHASE_AUTHORIZED event
    ledger_service
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "PURCHASE_AUTHORIZED".to_string(),
                payload: json!({
                    "session_id": session.session_id,
                    "product_id": "prod-1",
                    "authorized_at": "2026-11-08T10:00:00Z"
                }),
            },
        )
        .await
        .unwrap();

    // 3. PAYPAL_ORDER_CREATED event
    ledger_service
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "PAYPAL_ORDER_CREATED".to_string(),
                payload: json!({
                    "order_id": order_id,
                    "status": "CREATED",
                    "amount": "399.99",
                    "currency": "USD"
                }),
            },
        )
        .await
        .unwrap();

    // 4. PAYPAL_PAYMENT_CAPTURED event
    ledger_service
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "PAYPAL_PAYMENT_CAPTURED".to_string(),
                payload: json!({
                    "order_id": order_id,
                    "capture_id": capture_id,
                    "status": "COMPLETED",
                    "verified": true
                }),
            },
        )
        .await
        .unwrap();

    // 5. Finalize session (locking root hash)
    ledger_service
        .finalize_session(&session.session_id)
        .await
        .unwrap();

    session.session_id
}

/// Helper context containing all initialized services for dispute correlation testing.
struct TestContext {
    pub pool: SqlitePool,
    pub ledger_service: Arc<LedgerService>,
    pub _verification_service: Arc<VerificationService>,
    pub correlation_service: DisputeCorrelationService,
    pub mock_server_state: MockPayPalServerState,
}

async fn setup_test_context() -> TestContext {
    let (paypal_url, mock_server_state) = setup_mock_paypal_server().await;

    let pool = init_db("sqlite::memory:").await.unwrap();

    let config = Arc::new(Config {
        port: 0,
        rust_log: "info".to_string(),
        paypal_base_url: paypal_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "mock_client_id".to_string(),
        paypal_client_secret: "mock_client_secret".to_string(),
        paypal_webhook_id: Some("mock_webhook_id".to_string()),
        ..Default::default()
    });

    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), http_client.clone()));
    let ledger_service = Arc::new(LedgerService::new(pool.clone()));
    let verification_service = Arc::new(VerificationService::new(pool.clone()));

    let correlation_service = DisputeCorrelationService::new(
        config,
        http_client,
        auth_service,
        pool.clone(),
        verification_service.clone(),
    );

    TestContext {
        pool,
        ledger_service,
        _verification_service: verification_service,
        correlation_service,
        mock_server_state,
    }
}

// ---------------------------------------------------------------------------
// 1. Successful Capture ID matching
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_01_successful_capture_id_matching() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-TEST-01";
    let capture_id = "CAP-TEST-01";

    let session_id =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    // Dispute notification with seller_transaction_id in disputed_transactions
    let dispute = json!({
        "dispute_id": "PP-D-0001",
        "disputed_transactions": [
            {
                "seller_transaction_id": capture_id,
                "gross_amount": { "currency_code": "USD", "value": "399.99" }
            }
        ]
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::Matched);
    assert!(result.is_matched());
    assert_eq!(result.matched_by, Some(MatchedBy::CaptureId));
    assert_eq!(result.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(result.paypal_order_id.as_deref(), Some(order_id));
    assert_eq!(result.paypal_capture_id.as_deref(), Some(capture_id));
    assert!(result.correlation_timestamp.is_some());
    assert!(result.explanation.contains("via CAPTURE_ID"));
}

// ---------------------------------------------------------------------------
// 2. Successful Custom ID matching
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_02_successful_custom_id_matching() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-TEST-02";
    let capture_id = "CAP-TEST-02";

    let session_id =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    // Dispute notification containing reference_id with "session-" prefix
    let dispute = json!({
        "dispute_id": "PP-D-0002",
        "disputed_transactions": [
            {
                "reference_id": format!("session-{}", session_id),
                "gross_amount": { "currency_code": "USD", "value": "399.99" }
            }
        ]
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::Matched);
    assert!(result.is_matched());
    assert_eq!(result.matched_by, Some(MatchedBy::CustomId));
    assert_eq!(result.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(result.paypal_order_id.as_deref(), Some(order_id));
    assert_eq!(result.paypal_capture_id.as_deref(), Some(capture_id));
    assert!(result.explanation.contains("via CUSTOM_ID"));
}

// ---------------------------------------------------------------------------
// 3. Successful Order ID matching
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_03_successful_order_id_matching() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-TEST-03";
    let capture_id = "CAP-TEST-03";

    let session_id =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    // Dispute notification providing order_id
    let dispute = json!({
        "dispute_id": "PP-D-0003",
        "disputed_transactions": [
            {
                "order_id": order_id,
                "gross_amount": { "currency_code": "USD", "value": "399.99" }
            }
        ]
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::Matched);
    assert!(result.is_matched());
    assert_eq!(result.matched_by, Some(MatchedBy::OrderId));
    assert_eq!(result.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(result.paypal_order_id.as_deref(), Some(order_id));
    assert_eq!(result.paypal_capture_id.as_deref(), Some(capture_id));
    assert!(result.explanation.contains("via ORDER_ID"));
}

// ---------------------------------------------------------------------------
// 4. Fallback to PayPal Show Dispute Details
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_04_fallback_to_paypal_show_dispute_details() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-TEST-04";
    let capture_id = "CAP-TEST-04";

    let session_id =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    // Configure mock PayPal Show Dispute Details response
    *ctx.mock_server_state.dispute_details_body.write().await = json!({
        "dispute_id": "PP-D-0004",
        "status": "UNDER_REVIEW",
        "disputed_transactions": [
            {
                "seller_transaction_id": capture_id,
                "buyer_transaction_id": "BUYER-TX-04",
                "gross_amount": { "currency_code": "USD", "value": "399.99" }
            }
        ]
    });

    // Incoming webhook dispute has only dispute_id (local identifiers are insufficient)
    let dispute = json!({
        "dispute_id": "PP-D-0004"
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::Matched);
    assert_eq!(result.matched_by, Some(MatchedBy::DisputeDetailsCaptureId));
    assert_eq!(result.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(result.paypal_order_id.as_deref(), Some(order_id));
    assert_eq!(result.paypal_capture_id.as_deref(), Some(capture_id));
    assert!(result
        .explanation
        .contains("via DISPUTE_DETAILS_CAPTURE_ID"));

    // Verify Show Dispute Details API was invoked exactly once
    assert_eq!(
        ctx.mock_server_state
            .dispute_details_request_count
            .load(Ordering::SeqCst),
        1
    );
}

// ---------------------------------------------------------------------------
// 5. Unknown capture ID
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_05_unknown_capture_id() {
    let ctx = setup_test_context().await;

    let dispute = json!({
        "seller_transaction_id": "CAP-UNKNOWN-99999",
        "disputed_transactions": [
            { "seller_transaction_id": "CAP-UNKNOWN-99999" }
        ]
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::NotFound);
    assert!(result.is_not_found());
    assert!(result.session_id.is_none());
    assert!(result.explanation.contains("CAP-UNKNOWN-99999"));
}

// ---------------------------------------------------------------------------
// 6. Unknown order ID
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_06_unknown_order_id() {
    let ctx = setup_test_context().await;

    let dispute = json!({
        "order_id": "ORD-UNKNOWN-99999"
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::NotFound);
    assert!(result.is_not_found());
    assert!(result.session_id.is_none());
    assert!(result.explanation.contains("ORD-UNKNOWN-99999"));
}

// ---------------------------------------------------------------------------
// 7. Invalid session UUID
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_07_invalid_session_uuid() {
    let ctx = setup_test_context().await;

    let dispute = json!({
        "custom": "not-a-valid-uuid-format-xyz"
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_ne!(result.status, CorrelationStatus::Matched);
    assert!(result.is_not_found() || result.is_manual_review_required());
    assert!(result.session_id.is_none());
    assert!(result.explanation.contains("Invalid session UUID format"));
}

// ---------------------------------------------------------------------------
// 8. Conflicting identifiers
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_08_conflicting_identifiers() {
    let ctx = setup_test_context().await;

    // Create Session A
    let _session_a =
        create_finalized_purchase_session(&ctx.ledger_service, "ORD-ALPHA", "CAP-ALPHA").await;

    // Create Session B
    let _session_b =
        create_finalized_purchase_session(&ctx.ledger_service, "ORD-BETA", "CAP-BETA").await;

    // Dispute presents Capture ID of Session A but Order ID of Session B
    let dispute = json!({
        "dispute_id": "PP-D-CONFLICT",
        "seller_transaction_id": "CAP-ALPHA",
        "order_id": "ORD-BETA"
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::ManualReviewRequired);
    assert!(result.is_manual_review_required());
    assert!(result.session_id.is_none());
    assert!(result
        .explanation
        .to_lowercase()
        .contains("conflicting identifiers"));
}

// ---------------------------------------------------------------------------
// 9. Multiple candidate sessions
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_09_multiple_candidate_sessions() {
    let ctx = setup_test_context().await;

    // Directly set up two sessions with identical paypal_order_id in SQLite
    let s1 = ctx
        .ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "First duplicate order".to_string(),
        })
        .await
        .unwrap();
    let s2 = ctx
        .ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Second duplicate order".to_string(),
        })
        .await
        .unwrap();

    let shared_order_id = "ORD-DUPLICATE-SHARED";
    ctx.ledger_service
        .set_paypal_order_id(&s1.session_id, shared_order_id)
        .await
        .unwrap();
    ctx.ledger_service
        .set_paypal_order_id(&s2.session_id, shared_order_id)
        .await
        .unwrap();

    let dispute = json!({
        "order_id": shared_order_id
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::ManualReviewRequired);
    assert!(result.is_manual_review_required());
    assert!(result.session_id.is_none());
    assert!(result
        .explanation
        .to_lowercase()
        .contains("multiple candidate sessions"));
}

// ---------------------------------------------------------------------------
// 10. Session with invalid ledger verification
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_10_session_with_invalid_ledger_verification() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-TEST-10";
    let capture_id = "CAP-TEST-10";

    let session_id =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    // Tamper with the ledger payload in database directly
    sqlx::query(
        r#"
        UPDATE decision_steps
        SET payload = '{"tampered":true}'
        WHERE session_id = ? AND sequence_index = 0;
        "#,
    )
    .bind(&session_id)
    .execute(&ctx.pool)
    .await
    .unwrap();

    let dispute = json!({
        "seller_transaction_id": capture_id
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::ManualReviewRequired);
    assert!(result.is_manual_review_required());
    assert!(result.session_id.is_none());
    assert!(result
        .explanation
        .to_lowercase()
        .contains("failed cryptographic ledger verification"));
}

// ---------------------------------------------------------------------------
// 11. Unfinalized session
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_11_unfinalized_session() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-TEST-11";
    let capture_id = "CAP-TEST-11";

    let session = ctx
        .ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Unfinalized test session".to_string(),
        })
        .await
        .unwrap();

    ctx.ledger_service
        .set_paypal_order_id(&session.session_id, order_id)
        .await
        .unwrap();

    ctx.ledger_service
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "PAYPAL_PAYMENT_CAPTURED".to_string(),
                payload: json!({
                    "order_id": order_id,
                    "capture_id": capture_id,
                    "status": "COMPLETED",
                    "verified": true
                }),
            },
        )
        .await
        .unwrap();

    // Session root_hash remains NULL (not finalized)
    let dispute = json!({
        "seller_transaction_id": capture_id
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::ManualReviewRequired);
    assert!(result.is_manual_review_required());
    assert!(result.session_id.is_none());
    assert!(result.explanation.to_lowercase().contains("not finalized"));
}

// ---------------------------------------------------------------------------
// 12. No available transaction identifiers
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_12_no_available_transaction_identifiers() {
    let ctx = setup_test_context().await;

    // Empty dispute object without transaction IDs or dispute_id
    let dispute = json!({});

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::ManualReviewRequired);
    assert!(result.is_manual_review_required());
    assert!(result.session_id.is_none());
    assert!(result
        .explanation
        .to_lowercase()
        .contains("no transaction identifiers"));
}

// ---------------------------------------------------------------------------
// 13. PayPal API failure during fallback
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_13_paypal_api_failure_during_fallback() {
    let ctx = setup_test_context().await;

    // Configure mock PayPal API to return 500 Internal Server Error
    *ctx.mock_server_state.dispute_details_status.write().await = StatusCode::INTERNAL_SERVER_ERROR;
    *ctx.mock_server_state.dispute_details_body.write().await = json!({
        "name": "INTERNAL_SERVER_ERROR",
        "message": "PayPal dispute subsystem temporarily unavailable"
    });

    let dispute = json!({
        "dispute_id": "PP-D-FAIL-API"
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::ManualReviewRequired);
    assert!(result.is_manual_review_required());
    assert!(result.session_id.is_none());
    assert!(result
        .explanation
        .to_lowercase()
        .contains("api fallback failed"));
}

// ---------------------------------------------------------------------------
// 14. Manual review result without creating an incorrect association
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_14_manual_review_result_without_creating_an_incorrect_association() {
    let ctx = setup_test_context().await;

    // Existing session in the database
    let _session_id =
        create_finalized_purchase_session(&ctx.ledger_service, "ORD-REAL-14", "CAP-REAL-14").await;

    // Incoming dispute with matching buyer metadata, amount, timestamps, but NO matching transaction identifiers
    let dispute = json!({
        "dispute_amount": {
            "currency_code": "USD",
            "value": "399.99"
        },
        "buyer": {
            "name": "John Doe",
            "email": "buyer@example.com"
        },
        "create_time": "2026-11-08T10:00:00Z"
    });

    let result = ctx
        .correlation_service
        .correlate_dispute(&dispute)
        .await
        .unwrap();

    // Must NEVER match based only on amount, buyer name, email, or timestamps
    assert_ne!(result.status, CorrelationStatus::Matched);
    assert_eq!(result.status, CorrelationStatus::ManualReviewRequired);
    assert!(result.session_id.is_none());
    assert!(result.paypal_order_id.is_none());
    assert!(result.paypal_capture_id.is_none());
    assert!(result.matched_by.is_none());
}

// ---------------------------------------------------------------------------
// Extra test: Correlating via PayPalWebhookEvent envelope directly
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_correlate_webhook_event_envelope() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-WEBHOOK-EVENT";
    let capture_id = "CAP-WEBHOOK-EVENT";

    let session_id =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    let webhook_event = PayPalWebhookEvent {
        id: "WH-EVT-CORR-1".to_string(),
        create_time: Some("2026-11-08T10:00:00Z".to_string()),
        resource_type: "dispute".to_string(),
        event_version: Some("1.0".to_string()),
        event_type: "CUSTOMER.DISPUTE.CREATED".to_string(),
        summary: Some("Customer opened a dispute".to_string()),
        resource_version: Some("1.0".to_string()),
        resource: json!({
            "dispute_id": "PP-D-WEBHOOK-01",
            "seller_transaction_id": capture_id,
            "disputed_transactions": [
                {
                    "seller_transaction_id": capture_id
                }
            ]
        }),
        links: None,
    };

    let result = ctx
        .correlation_service
        .correlate_webhook_event(&webhook_event)
        .await
        .unwrap();

    assert_eq!(result.status, CorrelationStatus::Matched);
    assert_eq!(result.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(result.paypal_order_id.as_deref(), Some(order_id));
    assert_eq!(result.paypal_capture_id.as_deref(), Some(capture_id));
}
