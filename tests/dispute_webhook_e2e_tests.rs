use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, StatusCode},
    response::Response,
    routing::post,
    Router,
};
use serde_json::json;
use sqlx::{Row, SqlitePool};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower::ServiceExt;

use agentic_commerce::config::Config;
use agentic_commerce::create_router;
use agentic_commerce::db::init_db;
use agentic_commerce::error::AppError;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::correlation::CorrelationStatus;
use agentic_commerce::models::dispute::{
    DisputeActionType, GenesisAnchorType, GENESIS_HASH_DEFAULT,
};
use agentic_commerce::models::ledger::{AppendEventRequest, CreateSessionRequest};
use agentic_commerce::models::webhook::{
    PAYPAL_AUTH_ALGO_HEADER, PAYPAL_CERT_URL_HEADER, PAYPAL_TRANSMISSION_ID_HEADER,
    PAYPAL_TRANSMISSION_SIG_HEADER, PAYPAL_TRANSMISSION_TIME_HEADER,
};
use agentic_commerce::services::auth::PayPalAuthService;
use agentic_commerce::services::dispute_consumer::{
    ConsumerProcessingOutcome, DisputeConsumerService,
};
use agentic_commerce::services::dispute_correlation::DisputeCorrelationService;
use agentic_commerce::services::dispute_service::DisputeService;
use agentic_commerce::services::ledger::LedgerService;
use agentic_commerce::services::verification::VerificationService;
use agentic_commerce::services::webhook_persistence::WebhookPersistenceService;
use agentic_commerce::services::webhook_verifier::WebhookVerifierService;
use agentic_commerce::services::{CatalogService, PayPalOrdersService, ShoppingAgentService};
use agentic_commerce::AppState;

#[derive(Clone)]
struct MockPayPalServerState {
    pub token_request_count: Arc<AtomicUsize>,
    pub verify_request_count: Arc<AtomicUsize>,
    pub verify_response_status: Arc<RwLock<StatusCode>>,
    pub verify_response_body: Arc<RwLock<String>>,
}

async fn mock_token_handler(
    State(state): State<MockPayPalServerState>,
    _req: Request,
) -> Result<Response, StatusCode> {
    state.token_request_count.fetch_add(1, Ordering::SeqCst);
    let token_json = json!({
        "access_token": "mock-access-token-step7-e2e",
        "token_type": "Bearer",
        "expires_in": 3600
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(token_json.to_string()))
        .unwrap())
}

async fn mock_verify_handler(
    State(state): State<MockPayPalServerState>,
    _req: Request,
) -> Result<Response, StatusCode> {
    state.verify_request_count.fetch_add(1, Ordering::SeqCst);
    let status = *state.verify_response_status.read().await;
    let body = state.verify_response_body.read().await.clone();

    Ok(Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap())
}

async fn spawn_mock_paypal_server(
    initial_verify_status: StatusCode,
    initial_verify_body: &str,
) -> (String, MockPayPalServerState) {
    let state = MockPayPalServerState {
        token_request_count: Arc::new(AtomicUsize::new(0)),
        verify_request_count: Arc::new(AtomicUsize::new(0)),
        verify_response_status: Arc::new(RwLock::new(initial_verify_status)),
        verify_response_body: Arc::new(RwLock::new(initial_verify_body.to_string())),
    };

    let app = Router::new()
        .route("/v1/oauth2/token", post(mock_token_handler))
        .route(
            "/v1/notifications/verify-webhook-signature",
            post(mock_verify_handler),
        )
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mock_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (mock_url, state)
}

struct E2ETestContext {
    pub pool: SqlitePool,
    pub router: Router,
    pub _mock_state: MockPayPalServerState,
    pub persistence_service: Arc<WebhookPersistenceService>,
    pub dispute_service: Arc<DisputeService>,
    pub ledger_service: Arc<LedgerService>,
    pub verification_service: Arc<VerificationService>,
    pub consumer_service: Arc<DisputeConsumerService>,
}

async fn setup_e2e_test_context(
    initial_verify_status: StatusCode,
    initial_verify_body: &str,
) -> E2ETestContext {
    let (mock_url, mock_state) =
        spawn_mock_paypal_server(initial_verify_status, initial_verify_body).await;

    let pool = init_db("sqlite::memory:").await.unwrap();

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-E2E-TEST-ID".to_string()),
    });

    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();

    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), http_client.clone()));
    let ledger_service = Arc::new(LedgerService::new(pool.clone()));
    let verification_service = Arc::new(VerificationService::new(pool.clone()));
    let catalog_service = CatalogService::new();
    let shopping_agent = Arc::new(ShoppingAgentService::new(
        catalog_service.clone(),
        ledger_service.clone(),
        verification_service.clone(),
    ));
    let paypal_orders = Arc::new(PayPalOrdersService::new(
        config.clone(),
        http_client.clone(),
        auth_service.clone(),
        ledger_service.clone(),
    ));
    let webhook_verifier = Arc::new(WebhookVerifierService::new(
        config.clone(),
        http_client.clone(),
        auth_service.clone(),
    ));
    let persistence_service = Arc::new(WebhookPersistenceService::new(pool.clone()));

    let dispute_service = Arc::new(DisputeService::new(
        pool.clone(),
        verification_service.clone(),
    ));

    let correlation_service = Arc::new(DisputeCorrelationService::new(
        config.clone(),
        http_client.clone(),
        auth_service.clone(),
        pool.clone(),
        verification_service.clone(),
    ));

    let consumer_service = Arc::new(DisputeConsumerService::new(
        pool.clone(),
        persistence_service.clone(),
        dispute_service.clone(),
        correlation_service.clone(),
    ));

    let state = AppState {
        config,
        http_client,
        auth_service,
        ledger_service: ledger_service.clone(),
        verification_service: verification_service.clone(),
        catalog_service,
        shopping_agent,
        paypal_orders,
        webhook_verifier,
        webhook_persistence: persistence_service.clone(),
    };

    let router = create_router(state);

    E2ETestContext {
        pool,
        router,
        _mock_state: mock_state,
        persistence_service,
        dispute_service,
        ledger_service,
        verification_service,
        consumer_service,
    }
}

fn valid_e2e_headers(trans_id: &str, sig: &str, time: &str) -> Vec<(&'static str, String)> {
    vec![
        (PAYPAL_AUTH_ALGO_HEADER, "SHA256withRSA".to_string()),
        (
            PAYPAL_CERT_URL_HEADER,
            "https://api.sandbox.paypal.com/v1/notifications/certs/CERT-12345".to_string(),
        ),
        (PAYPAL_TRANSMISSION_ID_HEADER, trans_id.to_string()),
        (PAYPAL_TRANSMISSION_SIG_HEADER, sig.to_string()),
        (PAYPAL_TRANSMISSION_TIME_HEADER, time.to_string()),
    ]
}

async fn post_webhook(
    router: &Router,
    trans_id: &str,
    sig: &str,
    time: &str,
    raw_payload: &[u8],
) -> Response {
    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in valid_e2e_headers(trans_id, sig, time) {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(raw_payload.to_vec())).unwrap();
    router.clone().oneshot(req).await.unwrap()
}

async fn create_finalized_session(
    ledger_service: &LedgerService,
    order_id: &str,
    capture_id: &str,
) -> (String, String) {
    let session = ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Buy Sony WH-1000XM5".to_string(),
        })
        .await
        .unwrap();

    ledger_service
        .set_paypal_order_id(&session.session_id, order_id)
        .await
        .unwrap();

    ledger_service
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "PRODUCT_SELECTED".to_string(),
                payload: json!({
                    "product_id": "prod-1",
                    "price": 399.99
                }),
            },
        )
        .await
        .unwrap();

    ledger_service
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "PAYPAL_PAYMENT_CAPTURED".to_string(),
                payload: json!({
                    "order_id": order_id,
                    "capture_id": capture_id,
                    "amount": "399.99",
                    "status": "COMPLETED"
                }),
            },
        )
        .await
        .unwrap();

    let finalized = ledger_service
        .finalize_session(&session.session_id)
        .await
        .unwrap();

    (session.session_id, finalized.root_hash)
}

// =========================================================================
// 1. Successful matched dispute flow
// =========================================================================
#[tokio::test]
async fn test_01_e2e_successful_matched_dispute_flow() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    // 1. Create finalized purchase session with verified ledger
    let (session_id, expected_root) =
        create_finalized_session(&ctx.ledger_service, "ORD-E2E-01", "CAP-E2E-01").await;

    // 2. Submit dispute webhook through actual Axum HTTP route
    let raw_payload = b"{\n  \"id\": \"WH-EVT-E2E-01\",\n  \"event_type\": \"CUSTOMER.DISPUTE.CREATED\",\n  \"resource_type\": \"dispute\",\n  \"create_time\": \"2026-11-09T10:00:00Z\",\n  \"resource\": {\n    \"dispute_id\": \"PP-D-E2E-01\",\n    \"status\": \"UNDER_REVIEW\",\n    \"dispute_life_cycle_stage\": \"INQUIRY\",\n    \"dispute_channel\": \"INTERNAL\",\n    \"reason\": \"MERCHANDISE_OR_SERVICE_NOT_RECEIVED\",\n    \"dispute_amount\": {\n      \"currency_code\": \"USD\",\n      \"value\": \"399.99\"\n    },\n    \"seller_transaction_id\": \"CAP-E2E-01\",\n    \"create_time\": \"2026-11-09T10:00:00Z\",\n    \"update_time\": \"2026-11-09T10:00:00Z\"\n  }\n}";

    let resp = post_webhook(
        &ctx.router,
        "TX-E2E-01",
        "sig-01",
        "2026-11-09T10:00:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // 3. Verify SQLite webhook_events has exact raw bytes and status PENDING
    let event_row = sqlx::query(
        "SELECT event_id, processing_status, raw_payload, payload_hash FROM webhook_events WHERE event_id = ?;"
    )
    .bind("WH-EVT-E2E-01")
    .fetch_one(&ctx.pool)
    .await
    .unwrap();

    let stored_bytes: Vec<u8> = event_row.get("raw_payload");
    let stored_status: String = event_row.get("processing_status");
    assert_eq!(stored_bytes, raw_payload);
    assert_eq!(stored_status, "PENDING");

    // 4. Consume and process via DisputeConsumerService
    let outcome = ctx
        .consumer_service
        .process_next_event("worker-e2e-1", 300)
        .await
        .unwrap();

    assert!(outcome.is_some());
    match outcome.unwrap() {
        ConsumerProcessingOutcome::Processed {
            event_id,
            dispute_id,
            dispute,
        } => {
            assert_eq!(event_id, "WH-EVT-E2E-01");
            assert_eq!(dispute_id, "PP-D-E2E-01");
            assert_eq!(dispute.session_id, Some(session_id.clone()));
            assert_eq!(
                dispute.genesis_anchor_type,
                GenesisAnchorType::PurchaseRootHash
            );
            assert_eq!(dispute.genesis_anchor_hash, expected_root);
            assert_eq!(dispute.purchase_root_hash, Some(expected_root.clone()));
            assert_eq!(dispute.correlation_status, CorrelationStatus::Matched);
        }
        other => panic!("Expected Processed outcome, got {:?}", other),
    }

    // 5. Verify webhook_events reached PROCESSED
    let post_event_row = sqlx::query(
        "SELECT processing_status, lease_token, processed_at FROM webhook_events WHERE event_id = ?;"
    )
    .bind("WH-EVT-E2E-01")
    .fetch_one(&ctx.pool)
    .await
    .unwrap();

    let final_status: String = post_event_row.get("processing_status");
    let lease_token: Option<String> = post_event_row.get("lease_token");
    let processed_at: Option<String> = post_event_row.get("processed_at");
    assert_eq!(final_status, "PROCESSED");
    assert!(lease_token.is_none());
    assert!(processed_at.is_some());

    // 6. Verify dispute audit chain in SQLite
    let steps = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-E2E-01")
        .await
        .unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].sequence_index, 0);
    assert_eq!(steps[0].action_type, DisputeActionType::DISPUTE_IMPORTED);
    assert_eq!(steps[0].previous_hash, expected_root);

    let dispute_chain_verification = ctx
        .dispute_service
        .verify_dispute_chain("PP-D-E2E-01")
        .await
        .unwrap();
    assert!(dispute_chain_verification.valid);

    // 7. Verify original purchase ledger is valid and root hash unchanged
    let purchase_ledger_verification = ctx
        .verification_service
        .verify_session(&session_id)
        .await
        .unwrap();
    assert_eq!(
        purchase_ledger_verification.stored_root_hash.as_deref(),
        Some(expected_root.as_str())
    );

    // 8. Verify dual-chain verification succeeds
    let dual_res = ctx
        .dispute_service
        .verify_dispute_and_purchase_ledger("PP-D-E2E-01")
        .await
        .unwrap();
    assert!(dual_res.valid, "Dual-chain verification must pass");
}

// =========================================================================
// 2. Successful unmatched dispute flow requiring manual review
// =========================================================================
#[tokio::test]
async fn test_02_e2e_successful_unmatched_dispute_flow_requiring_manual_review() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    // Payload has no matching identifiers in database
    let raw_payload = b"{\"id\":\"WH-EVT-E2E-02\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-UNMATCHED\",\"status\":\"UNDER_REVIEW\",\"create_time\":\"2026-11-09T10:00:00Z\"}}";

    let resp = post_webhook(
        &ctx.router,
        "TX-E2E-02",
        "sig-02",
        "2026-11-09T10:00:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Consumer processes the unmatched event
    let outcome = ctx
        .consumer_service
        .process_next_event("worker-e2e-2", 300)
        .await
        .unwrap();

    assert!(outcome.is_some());
    match outcome.unwrap() {
        ConsumerProcessingOutcome::Processed {
            dispute_id,
            dispute,
            ..
        } => {
            assert_eq!(dispute_id, "PP-D-E2E-UNMATCHED");
            assert_eq!(dispute.session_id, None);
            assert_eq!(dispute.genesis_anchor_type, GenesisAnchorType::GenesisHash);
            assert_eq!(dispute.genesis_anchor_hash, GENESIS_HASH_DEFAULT);
            assert!(
                dispute.correlation_status == CorrelationStatus::NotFound
                    || dispute.correlation_status == CorrelationStatus::ManualReviewRequired
            );
        }
        other => panic!("Expected Processed outcome, got {:?}", other),
    }

    // Verify dispute audit chain is cryptographically valid
    let v_res = ctx
        .dispute_service
        .verify_dispute_chain("PP-D-E2E-UNMATCHED")
        .await
        .unwrap();
    assert!(v_res.valid);

    // Verify webhook reached PROCESSED
    let status: String = sqlx::query_scalar(
        "SELECT processing_status FROM webhook_events WHERE event_id = 'WH-EVT-E2E-02';",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(status, "PROCESSED");
}

// =========================================================================
// 3. Identical duplicate webhook delivery
// =========================================================================
#[tokio::test]
async fn test_03_e2e_identical_duplicate_webhook_delivery() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    let raw_payload = b"{\"id\":\"WH-EVT-E2E-03\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-03\",\"status\":\"UNDER_REVIEW\",\"create_time\":\"2026-11-09T10:00:00Z\"}}";

    // First delivery
    let resp1 = post_webhook(
        &ctx.router,
        "TX-E2E-03A",
        "sig-03A",
        "2026-11-09T10:00:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);

    // Consumer processes first delivery
    let outcome1 = ctx
        .consumer_service
        .process_next_event("worker-e2e-3", 300)
        .await
        .unwrap();
    assert!(outcome1.is_some());

    // Second delivery with identical bytes
    let resp2 = post_webhook(
        &ctx.router,
        "TX-E2E-03B",
        "sig-03B",
        "2026-11-09T10:05:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::OK);

    let resp_bytes = to_bytes(resp2.into_body(), 1024 * 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["status"], "DUPLICATE");

    // Verify exactly 1 webhook row and exactly 1 audit step exists
    let webhook_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-EVT-E2E-03';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(webhook_count, 1);

    let steps = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-E2E-03")
        .await
        .unwrap();
    assert_eq!(
        steps.len(),
        1,
        "Duplicate delivery must not append duplicate audit steps"
    );
}

// =========================================================================
// 4. Same event ID with different raw payload bytes
// =========================================================================
#[tokio::test]
async fn test_04_e2e_same_event_id_with_different_raw_payload_bytes() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    let original_payload = b"{\"id\":\"WH-EVT-E2E-04\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-04\",\"status\":\"UNDER_REVIEW\"}}";

    let resp1 = post_webhook(
        &ctx.router,
        "TX-E2E-04A",
        "sig-04A",
        "2026-11-09T10:00:00Z",
        original_payload,
    )
    .await;
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);

    // Second delivery with different bytes
    let divergent_payload = b"{\"id\":\"WH-EVT-E2E-04\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-04\",\"status\":\"RESOLVED\"}}";

    let resp2 = post_webhook(
        &ctx.router,
        "TX-E2E-04B",
        "sig-04B",
        "2026-11-09T10:00:10Z",
        divergent_payload,
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::CONFLICT);

    // Confirm original raw payload in SQLite is unchanged
    let stored_bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT raw_payload FROM webhook_events WHERE event_id = 'WH-EVT-E2E-04';",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(stored_bytes, original_payload);
}

// =========================================================================
// 5. Invalid signature: no webhook persistence
// =========================================================================
#[tokio::test]
async fn test_05_e2e_invalid_signature_no_webhook_persistence() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"FAILURE"}"#).await;

    let raw_payload = b"{\"id\":\"WH-EVT-E2E-05\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-05\",\"status\":\"UNDER_REVIEW\"}}";

    let resp = post_webhook(
        &ctx.router,
        "TX-E2E-05",
        "sig-invalid",
        "2026-11-09T10:00:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Assert zero rows in webhook_events and disputes
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-EVT-E2E-05';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);

    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-E2E-05")
        .await
        .unwrap();
    assert_eq!(dispute, None);
}

// =========================================================================
// 6. Unsupported webhook event: persisted, then marked IGNORED
// =========================================================================
#[tokio::test]
async fn test_06_e2e_unsupported_webhook_event_persisted_then_marked_ignored() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    let raw_payload = b"{\"id\":\"WH-EVT-E2E-06-UNSUPPORTED\",\"event_type\":\"PAYMENT.CAPTURE.COMPLETED\",\"resource_type\":\"capture\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"id\":\"CAP-E2E-06\",\"status\":\"COMPLETED\"}}";

    let resp = post_webhook(
        &ctx.router,
        "TX-E2E-06",
        "sig-06",
        "2026-11-09T10:00:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Consumer processes next event: must be marked Ignored
    let outcome = ctx
        .consumer_service
        .process_next_event("worker-e2e-6", 300)
        .await
        .unwrap();

    assert!(outcome.is_some());
    match outcome.unwrap() {
        ConsumerProcessingOutcome::Ignored { event_id, reason } => {
            assert_eq!(event_id, "WH-EVT-E2E-06-UNSUPPORTED");
            assert!(reason.contains("Unsupported webhook event type"));
        }
        other => panic!("Expected Ignored outcome, got {:?}", other),
    }

    // SQLite status must be IGNORED with lease_token cleared
    let row = sqlx::query(
        "SELECT processing_status, lease_token FROM webhook_events WHERE event_id = ?;",
    )
    .bind("WH-EVT-E2E-06-UNSUPPORTED")
    .fetch_one(&ctx.pool)
    .await
    .unwrap();

    let status: String = row.get("processing_status");
    let lease_token: Option<String> = row.get("lease_token");
    assert_eq!(status, "IGNORED");
    assert!(lease_token.is_none());

    // Zero dispute records or audit steps
    let dispute_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM disputes;")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(dispute_count, 0);
}

// =========================================================================
// 7. Malformed webhook payload: rejected
// =========================================================================
#[tokio::test]
async fn test_07_e2e_malformed_webhook_payload_rejected() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    let raw_payload = b"{\"id\":\"WH-EVT-E2E-07\", malformed json payload";

    let resp = post_webhook(
        &ctx.router,
        "TX-E2E-07",
        "sig-07",
        "2026-11-09T10:00:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-EVT-E2E-07';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

// =========================================================================
// 8. Consumer failure: no partial dispute/audit writes and retry scheduled
// =========================================================================
#[tokio::test]
async fn test_08_e2e_consumer_failure_no_partial_writes_and_retry_scheduled() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    // Webhook has valid envelope and is accepted by HTTP ingress
    let raw_payload = b"{\"id\":\"WH-EVT-E2E-08\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-08\",\"status\":\"UNDER_REVIEW\",\"create_time\":\"2026-11-09T10:00:00Z\"}}";

    let resp = post_webhook(
        &ctx.router,
        "TX-E2E-08",
        "sig-08",
        "2026-11-09T10:00:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Induce consumer failure during dispute processing by dropping dispute_audit_steps table
    sqlx::query("DROP TABLE dispute_audit_steps;")
        .execute(&ctx.pool)
        .await
        .unwrap();

    // Consumer processes next event: fails during dispute mutation
    let res = ctx
        .consumer_service
        .process_next_event("worker-e2e-8", 300)
        .await;
    assert!(res.is_err());

    // Verify atomic rollback: zero dispute rows persisted
    let dispute_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM disputes WHERE dispute_id = 'PP-D-E2E-08';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(
        dispute_count, 0,
        "Dispute writes must be completely rolled back on failure"
    );

    // Verify webhook transitioned to RETRY with retry_count incremented and next_retry_at populated
    let row = sqlx::query(
        "SELECT processing_status, retry_count, next_retry_at, last_error FROM webhook_events WHERE event_id = ?;"
    )
    .bind("WH-EVT-E2E-08")
    .fetch_one(&ctx.pool)
    .await
    .unwrap();

    let status: String = row.get("processing_status");
    let retry_count: i64 = row.get("retry_count");
    let next_retry_at: Option<String> = row.get("next_retry_at");
    let last_error: Option<String> = row.get("last_error");

    assert_eq!(status, "RETRY");
    assert_eq!(retry_count, 1);
    assert!(next_retry_at.is_some());
    assert!(last_error.is_some());
}

// =========================================================================
// 9. Stale lease: old worker cannot commit after lease reclamation
// =========================================================================
#[tokio::test]
async fn test_09_e2e_stale_lease_old_worker_cannot_commit_after_reclamation() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    let raw_payload = b"{\"id\":\"WH-EVT-E2E-09\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-09\",\"status\":\"UNDER_REVIEW\",\"create_time\":\"2026-11-09T10:00:00Z\"}}";

    let resp = post_webhook(
        &ctx.router,
        "TX-E2E-09",
        "sig-09",
        "2026-11-09T10:00:00Z",
        raw_payload,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Worker 1 claims lease
    let worker1_claim = ctx
        .persistence_service
        .acquire_lease("worker-1", 300)
        .await
        .unwrap()
        .unwrap();

    // Simulate Worker 1 stalling: fast forward 400 seconds
    let old_time = (chrono::Utc::now() - chrono::Duration::seconds(400)).to_rfc3339();
    sqlx::query(
        "UPDATE webhook_events SET processing_started_at = ? WHERE event_id = 'WH-EVT-E2E-09';",
    )
    .bind(&old_time)
    .execute(&ctx.pool)
    .await
    .unwrap();

    // Worker 2 reclaims the abandoned event with fresh lease token
    let worker2_claim = ctx
        .persistence_service
        .acquire_lease("worker-2", 300)
        .await
        .unwrap()
        .unwrap();

    assert_ne!(worker1_claim.lease_token, worker2_claim.lease_token);

    // Worker 1 wakes up and attempts to commit: rejected by lease fencing and rolled back
    let worker1_res = ctx
        .consumer_service
        .process_leased_event(worker1_claim)
        .await;
    assert!(worker1_res.is_err());
    match worker1_res.unwrap_err() {
        AppError::DisputeLeaseFencedOut(_) => {}
        other => panic!("Expected DisputeLeaseFencedOut, got {:?}", other),
    }

    // Zero dispute records committed by Worker 1
    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-E2E-09")
        .await
        .unwrap();
    assert_eq!(dispute, None);

    // Worker 2 processes the event successfully
    let worker2_res = ctx
        .consumer_service
        .process_leased_event(worker2_claim)
        .await;
    assert!(worker2_res.is_ok());

    // Webhook reaches PROCESSED and dispute row is committed
    let status: String = sqlx::query_scalar(
        "SELECT processing_status FROM webhook_events WHERE event_id = 'WH-EVT-E2E-09';",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(status, "PROCESSED");

    let committed_dispute = ctx
        .dispute_service
        .get_dispute("PP-D-E2E-09")
        .await
        .unwrap();
    assert!(committed_dispute.is_some());
}

// =========================================================================
// 10. Out-of-order dispute update: older authoritative update does not overwrite newer state
// =========================================================================
#[tokio::test]
async fn test_10_e2e_out_of_order_dispute_update_preserved_newer_state() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    // Webhook 1: update_time is 12:00:00Z, status is WAITING_FOR_SELLER_RESPONSE
    let raw_payload1 = b"{\"id\":\"WH-EVT-E2E-10-A\",\"event_type\":\"CUSTOMER.DISPUTE.UPDATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T12:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-10\",\"status\":\"WAITING_FOR_SELLER_RESPONSE\",\"create_time\":\"2026-11-09T10:00:00Z\",\"update_time\":\"2026-11-09T12:00:00Z\"}}";

    let resp1 = post_webhook(
        &ctx.router,
        "TX-E2E-10A",
        "sig-10A",
        "2026-11-09T12:00:00Z",
        raw_payload1,
    )
    .await;
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);
    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // Verify initial state
    let d1 = ctx
        .dispute_service
        .get_dispute("PP-D-E2E-10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d1.status, "WAITING_FOR_SELLER_RESPONSE");
    assert_eq!(
        d1.dispute_update_time.as_deref(),
        Some("2026-11-09T12:00:00Z")
    );

    // Webhook 2: older update_time is 10:30:00Z, status is UNDER_REVIEW
    let raw_payload2 = b"{\"id\":\"WH-EVT-E2E-10-B\",\"event_type\":\"CUSTOMER.DISPUTE.UPDATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:30:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-10\",\"status\":\"UNDER_REVIEW\",\"create_time\":\"2026-11-09T10:00:00Z\",\"update_time\":\"2026-11-09T10:30:00Z\"}}";

    let resp2 = post_webhook(
        &ctx.router,
        "TX-E2E-10B",
        "sig-10B",
        "2026-11-09T10:30:00Z",
        raw_payload2,
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::ACCEPTED);
    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // Verify dispute state in SQLite preserved newer status and update_time
    let d2 = ctx
        .dispute_service
        .get_dispute("PP-D-E2E-10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d2.status, "WAITING_FOR_SELLER_RESPONSE");
    assert_eq!(
        d2.dispute_update_time.as_deref(),
        Some("2026-11-09T12:00:00Z")
    );

    // Verify audit steps: step 0 is initial, step 1 is DISPUTE_OUT_OF_ORDER_RECEIVED
    let steps = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-E2E-10")
        .await
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1].action_type,
        DisputeActionType::DISPUTE_OUT_OF_ORDER_RECEIVED
    );

    // Dispute hash chain is cryptographically valid
    let v_res = ctx
        .dispute_service
        .verify_dispute_chain("PP-D-E2E-10")
        .await
        .unwrap();
    assert!(v_res.valid);
}

// =========================================================================
// 11. Late correlation: initially unmatched dispute becomes linked when session available
// =========================================================================
#[tokio::test]
async fn test_11_e2e_late_correlation_initially_unmatched_becomes_linked() {
    let ctx = setup_e2e_test_context(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    // Webhook 1: Initial unmatched dispute arrives
    let raw_payload1 = b"{\"id\":\"WH-EVT-E2E-11-A\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T10:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-11\",\"status\":\"UNDER_REVIEW\",\"create_time\":\"2026-11-09T10:00:00Z\"}}";

    let resp1 = post_webhook(
        &ctx.router,
        "TX-E2E-11A",
        "sig-11A",
        "2026-11-09T10:00:00Z",
        raw_payload1,
    )
    .await;
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);
    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    let d1 = ctx
        .dispute_service
        .get_dispute("PP-D-E2E-11")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d1.session_id, None);
    assert_eq!(d1.genesis_anchor_type, GenesisAnchorType::GenesisHash);
    assert_eq!(d1.genesis_anchor_hash, GENESIS_HASH_DEFAULT);

    // Purchase session is subsequently created and finalized in the database
    let (session_id, expected_root) =
        create_finalized_session(&ctx.ledger_service, "ORD-E2E-11", "CAP-E2E-11").await;

    // Webhook 2: Update arrives for the dispute with seller_transaction_id matching the capture_id
    let raw_payload2 = b"{\"id\":\"WH-EVT-E2E-11-B\",\"event_type\":\"CUSTOMER.DISPUTE.UPDATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T11:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-E2E-11\",\"status\":\"UNDER_REVIEW\",\"seller_transaction_id\":\"CAP-E2E-11\",\"update_time\":\"2026-11-09T11:00:00Z\"}}";

    let resp2 = post_webhook(
        &ctx.router,
        "TX-E2E-11B",
        "sig-11B",
        "2026-11-09T11:00:00Z",
        raw_payload2,
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::ACCEPTED);
    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // Verify dispute in SQLite is now correlated
    let d2 = ctx
        .dispute_service
        .get_dispute("PP-D-E2E-11")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d2.session_id, Some(session_id.clone()));
    assert_eq!(d2.purchase_root_hash, Some(expected_root.clone()));
    assert_eq!(d2.correlation_status, CorrelationStatus::Matched);
    // Genesis anchor remains unchanged!
    assert_eq!(d2.genesis_anchor_type, GenesisAnchorType::GenesisHash);
    assert_eq!(d2.genesis_anchor_hash, GENESIS_HASH_DEFAULT);

    // Verify dual-chain verification succeeds
    let dual_res = ctx
        .dispute_service
        .verify_dispute_and_purchase_ledger("PP-D-E2E-11")
        .await
        .unwrap();
    assert!(
        dual_res.valid,
        "Dual-chain verification must succeed after late correlation"
    );
}
