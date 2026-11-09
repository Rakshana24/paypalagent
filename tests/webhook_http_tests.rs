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
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::webhook::{
    PAYPAL_AUTH_ALGO_HEADER, PAYPAL_CERT_URL_HEADER, PAYPAL_TRANSMISSION_ID_HEADER,
    PAYPAL_TRANSMISSION_SIG_HEADER, PAYPAL_TRANSMISSION_TIME_HEADER,
};
use agentic_commerce::services::{
    CatalogService, LedgerService, PayPalAuthService, PayPalOrdersService, ShoppingAgentService,
    VerificationService, WebhookPersistenceService, WebhookVerifierService,
};
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
        "access_token": "mock-access-token-step6-test",
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

struct TestAppContext {
    pub pool: SqlitePool,
    pub router: Router,
    pub mock_state: MockPayPalServerState,
}

async fn setup_test_app(
    initial_verify_status: StatusCode,
    initial_verify_body: &str,
    webhook_id: Option<String>,
) -> TestAppContext {
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
        paypal_webhook_id: webhook_id.or_else(|| Some("WH-TEST-ID-12345".to_string())),
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
    let webhook_persistence = Arc::new(WebhookPersistenceService::new(pool.clone()));

    let state = AppState {
        config,
        http_client,
        auth_service,
        ledger_service,
        verification_service,
        catalog_service,
        shopping_agent,
        paypal_orders,
        webhook_verifier,
        webhook_persistence,
    };

    let router = create_router(state);

    TestAppContext {
        pool,
        router,
        mock_state,
    }
}

fn valid_transmission_headers(
    trans_id: &str,
    sig: &str,
    time: &str,
) -> Vec<(&'static str, String)> {
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

// =========================================================================
// 1. Valid signature and new event: exact raw bytes persisted and 202
// =========================================================================
#[tokio::test]
async fn test_01_valid_signature_and_new_event_persisted_and_acknowledged_202() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#, None).await;

    // Use specific whitespace and indentation in raw bytes to verify byte-preservation
    let raw_payload = b"{\n  \"id\": \"WH-STEP6-01\",\n  \"event_type\": \"CUSTOMER.DISPUTE.CREATED\",\n  \"resource_type\": \"dispute\",\n  \"create_time\": \"2026-11-09T00:00:00Z\",\n  \"resource\": {\n    \"dispute_id\": \"PP-D-STEP6-01\",\n    \"status\": \"UNDER_REVIEW\",\n    \"create_time\": \"2026-11-09T00:00:00Z\"\n  }\n}";

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in valid_transmission_headers("TX-STEP6-01", "sig-valid-01", "2026-11-09T00:00:00Z")
    {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(raw_payload.to_vec())).unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let resp_bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["success"], true);
    assert_eq!(resp_json["status"], "ACCEPTED");
    assert_eq!(resp_json["event_id"], "WH-STEP6-01");

    // Directly query database to confirm exact raw payload byte preservation
    let row = sqlx::query(
        "SELECT event_id, event_type, resource_type, transmission_id, processing_status, raw_payload, payload_hash FROM webhook_events WHERE event_id = ?;"
    )
    .bind("WH-STEP6-01")
    .fetch_one(&ctx.pool)
    .await
    .unwrap();

    let stored_event_id: String = row.get("event_id");
    let stored_event_type: String = row.get("event_type");
    let stored_trans_id: String = row.get("transmission_id");
    let stored_status: String = row.get("processing_status");
    let stored_bytes: Vec<u8> = row.get("raw_payload");

    assert_eq!(stored_event_id, "WH-STEP6-01");
    assert_eq!(stored_event_type, "CUSTOMER.DISPUTE.CREATED");
    assert_eq!(stored_trans_id, "TX-STEP6-01");
    assert_eq!(stored_status, "PENDING");
    assert_eq!(
        stored_bytes, raw_payload,
        "Persisted bytes must match exact raw incoming request body byte-for-byte"
    );
}

// =========================================================================
// 2. Identical duplicate delivery: no duplicate row and acknowledged 200
// =========================================================================
#[tokio::test]
async fn test_02_identical_duplicate_delivery_acknowledged_idempotently() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#, None).await;

    let raw_payload = b"{\"id\":\"WH-STEP6-02\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-STEP6-02\",\"status\":\"UNDER_REVIEW\"}}";

    // First delivery
    let mut req1 = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in
        valid_transmission_headers("TX-STEP6-02A", "sig-valid-02A", "2026-11-09T00:00:00Z")
    {
        req1 = req1.header(k, v);
    }
    let resp1 = ctx
        .router
        .clone()
        .oneshot(req1.body(Body::from(raw_payload.to_vec())).unwrap())
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);

    // Second identical delivery (e.g. PayPal redelivery retry)
    let mut req2 = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in
        valid_transmission_headers("TX-STEP6-02B", "sig-valid-02B", "2026-11-09T00:00:05Z")
    {
        req2 = req2.header(k, v);
    }
    let resp2 = ctx
        .router
        .clone()
        .oneshot(req2.body(Body::from(raw_payload.to_vec())).unwrap())
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);

    let resp_bytes = to_bytes(resp2.into_body(), 1024 * 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["success"], true);
    assert_eq!(resp_json["status"], "DUPLICATE");
    assert_eq!(resp_json["event_id"], "WH-STEP6-02");

    // Verify exactly one row in SQLite
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-STEP6-02';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "Duplicate delivery must not create extra records");
}

// =========================================================================
// 3. Same event ID with different raw bytes: conflict 409, original unchanged
// =========================================================================
#[tokio::test]
async fn test_03_divergent_payload_returns_conflict_409() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#, None).await;

    let original_payload = b"{\"id\":\"WH-STEP6-03\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-ORIGINAL\",\"status\":\"UNDER_REVIEW\"}}";

    let mut req1 = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in
        valid_transmission_headers("TX-STEP6-03A", "sig-valid-03A", "2026-11-09T00:00:00Z")
    {
        req1 = req1.header(k, v);
    }
    let resp1 = ctx
        .router
        .clone()
        .oneshot(req1.body(Body::from(original_payload.to_vec())).unwrap())
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);

    // Second delivery with SAME event_id but DIFFERENT payload content
    let tampered_payload = b"{\"id\":\"WH-STEP6-03\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-TAMPERED\",\"status\":\"RESOLVED\"}}";

    let mut req2 = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in
        valid_transmission_headers("TX-STEP6-03B", "sig-valid-03B", "2026-11-09T00:00:10Z")
    {
        req2 = req2.header(k, v);
    }
    let resp2 = ctx
        .router
        .clone()
        .oneshot(req2.body(Body::from(tampered_payload.to_vec())).unwrap())
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::CONFLICT);

    let resp_bytes = to_bytes(resp2.into_body(), 1024 * 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["success"], false);
    assert!(resp_json["message"]
        .as_str()
        .unwrap()
        .contains("divergence"));

    // Verify original stored bytes are unchanged
    let stored_bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT raw_payload FROM webhook_events WHERE event_id = 'WH-STEP6-03';",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(
        stored_bytes, original_payload,
        "Original payload bytes must not be overwritten"
    );
}

// =========================================================================
// 4. Missing required transmission headers: HTTP 400
// =========================================================================
#[tokio::test]
async fn test_04_missing_required_transmission_headers_each_returns_400() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#, None).await;

    let payload = b"{\"id\":\"WH-STEP6-04\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-04\",\"status\":\"UNDER_REVIEW\"}}";

    let required_headers = [
        PAYPAL_AUTH_ALGO_HEADER,
        PAYPAL_CERT_URL_HEADER,
        PAYPAL_TRANSMISSION_ID_HEADER,
        PAYPAL_TRANSMISSION_SIG_HEADER,
        PAYPAL_TRANSMISSION_TIME_HEADER,
    ];

    for omitted_header in required_headers {
        let mut req_builder = Request::builder()
            .method("POST")
            .uri("/api/webhooks/paypal")
            .header(header::CONTENT_TYPE, "application/json");

        for (k, v) in valid_transmission_headers("TX-STEP6-04", "sig-04", "2026-11-09T00:00:00Z") {
            if k != omitted_header {
                req_builder = req_builder.header(k, v);
            }
        }

        let req = req_builder.body(Body::from(payload.to_vec())).unwrap();
        let resp = ctx.router.clone().oneshot(req).await.unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "Omitting header '{}' must return 400 Bad Request",
            omitted_header
        );

        let resp_bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
        assert_eq!(resp_json["success"], false);
        assert!(
            resp_json["message"]
                .as_str()
                .unwrap()
                .contains(omitted_header),
            "Error message must specify missing header '{}'",
            omitted_header
        );
    }

    // Verify no row was inserted
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-STEP6-04';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "No records should be created when headers are missing"
    );
}

// =========================================================================
// 5. Invalid signature: HTTP 401 and no persistence
// =========================================================================
#[tokio::test]
async fn test_05_invalid_signature_returns_401_and_no_persistence() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"FAILURE"}"#, None).await;

    let payload = b"{\"id\":\"WH-STEP6-05\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-05\",\"status\":\"UNDER_REVIEW\"}}";

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in
        valid_transmission_headers("TX-STEP6-05", "sig-invalid-05", "2026-11-09T00:00:00Z")
    {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(payload.to_vec())).unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp_bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["success"], false);

    // Verify no persistence occurred
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-STEP6-05';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "Unverified webhook must not be persisted");
}

// =========================================================================
// 6. PayPal verification service failure: appropriate server error and no persistence
// =========================================================================
#[tokio::test]
async fn test_06_paypal_service_failure_returns_server_error() {
    let ctx = setup_test_app(
        StatusCode::INTERNAL_SERVER_ERROR,
        r#"{"name":"INTERNAL_SERVER_ERROR","message":"An internal server error occurred."}"#,
        None,
    )
    .await;

    let payload = b"{\"id\":\"WH-STEP6-06\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-06\",\"status\":\"UNDER_REVIEW\"}}";

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in valid_transmission_headers("TX-STEP6-06", "sig-06", "2026-11-09T00:00:00Z") {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(payload.to_vec())).unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

    let resp_bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["success"], false);

    // Verify no persistence occurred
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-STEP6-06';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "Webhook must not be persisted on PayPal API failure"
    );
}

#[tokio::test]
async fn test_06b_unconfigured_webhook_id_returns_500() {
    // Webhook ID is empty / None
    let ctx = setup_test_app(
        StatusCode::OK,
        r#"{"verification_status":"SUCCESS"}"#,
        Some("".to_string()),
    )
    .await;

    let payload = b"{\"id\":\"WH-STEP6-06B\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-06B\",\"status\":\"UNDER_REVIEW\"}}";

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in valid_transmission_headers("TX-STEP6-06B", "sig-06B", "2026-11-09T00:00:00Z") {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(payload.to_vec())).unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-STEP6-06B';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

// =========================================================================
// 7. Malformed JSON: rejected and not persisted (400)
// =========================================================================
#[tokio::test]
async fn test_07_malformed_json_rejected_and_not_persisted() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#, None).await;

    let malformed_payload = b"{\"id\":\"WH-STEP6-07\", broken json here...";

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in valid_transmission_headers("TX-STEP6-07", "sig-07", "2026-11-09T00:00:00Z") {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder
        .body(Body::from(malformed_payload.to_vec()))
        .unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp_bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["success"], false);
    assert!(resp_json["message"].as_str().unwrap().contains("Malformed"));

    // Verify verification was NOT called (failed before signature check)
    assert_eq!(
        ctx.mock_state.verify_request_count.load(Ordering::SeqCst),
        0
    );
}

// =========================================================================
// 8. Database failure: no success acknowledgement (500)
// =========================================================================
#[tokio::test]
async fn test_08_database_failure_does_not_acknowledge_success() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#, None).await;

    // Simulate database failure by dropping the webhook_events table
    sqlx::query("DROP TABLE webhook_events;")
        .execute(&ctx.pool)
        .await
        .unwrap();

    let payload = b"{\"id\":\"WH-STEP6-08\",\"event_type\":\"CUSTOMER.DISPUTE.CREATED\",\"resource_type\":\"dispute\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"dispute_id\":\"PP-D-08\",\"status\":\"UNDER_REVIEW\"}}";

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in valid_transmission_headers("TX-STEP6-08", "sig-08", "2026-11-09T00:00:00Z") {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(payload.to_vec())).unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let resp_bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["success"], false);
    assert_eq!(resp_json["message"], "Database operation failed");
}

// =========================================================================
// 9. Oversized request body: rejected
// =========================================================================
#[tokio::test]
async fn test_09_oversized_request_body_rejected() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#, None).await;

    // Generate an oversized payload (> 256 KB)
    let large_comment = "x".repeat(300 * 1024);
    let payload_json = json!({
        "id": "WH-STEP6-09",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource_type": "dispute",
        "create_time": "2026-11-09T00:00:00Z",
        "resource": {
            "dispute_id": "PP-D-09",
            "status": "UNDER_REVIEW",
            "oversized_blob": large_comment
        }
    });

    let raw_bytes = serde_json::to_vec(&payload_json).unwrap();
    assert!(
        raw_bytes.len() > 256 * 1024,
        "Payload must exceed 256 KB limit"
    );

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in valid_transmission_headers("TX-STEP6-09", "sig-09", "2026-11-09T00:00:00Z") {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(raw_bytes)).unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();

    // Axum's DefaultBodyLimit returns 413 Payload Too Large
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // Confirm no row was persisted
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE event_id = 'WH-STEP6-09';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "Oversized request must not be persisted");
}

// =========================================================================
// 10. Validly signed unrelated event: persisted successfully for consumer to ignore later
// =========================================================================
#[tokio::test]
async fn test_10_validly_signed_unrelated_event_persisted_successfully() {
    let ctx = setup_test_app(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#, None).await;

    // Non-dispute event (payment capture completed)
    let raw_payload = b"{\"id\":\"WH-STEP6-10-UNRELATED\",\"event_type\":\"PAYMENT.CAPTURE.COMPLETED\",\"resource_type\":\"capture\",\"create_time\":\"2026-11-09T00:00:00Z\",\"resource\":{\"id\":\"CAP-9988\",\"status\":\"COMPLETED\",\"amount\":{\"currency_code\":\"USD\",\"value\":\"49.99\"}}}";

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in valid_transmission_headers("TX-STEP6-10", "sig-valid-10", "2026-11-09T00:00:00Z")
    {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(raw_payload.to_vec())).unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Verify row was persisted with exact raw bytes and status PENDING
    let row = sqlx::query(
        "SELECT event_id, event_type, resource_type, resource_id, processing_status, raw_payload FROM webhook_events WHERE event_id = ?;"
    )
    .bind("WH-STEP6-10-UNRELATED")
    .fetch_one(&ctx.pool)
    .await
    .unwrap();

    let stored_event_type: String = row.get("event_type");
    let stored_resource_type: Option<String> = row.get("resource_type");
    let stored_resource_id: Option<String> = row.get("resource_id");
    let stored_status: String = row.get("processing_status");
    let stored_bytes: Vec<u8> = row.get("raw_payload");

    assert_eq!(stored_event_type, "PAYMENT.CAPTURE.COMPLETED");
    assert_eq!(stored_resource_type.as_deref(), Some("capture"));
    assert_eq!(stored_resource_id.as_deref(), Some("CAP-9988"));
    assert_eq!(
        stored_status, "PENDING",
        "Must be persisted as PENDING for consumer to evaluate"
    );
    assert_eq!(
        stored_bytes, raw_payload,
        "Must preserve exact incoming bytes"
    );
}
