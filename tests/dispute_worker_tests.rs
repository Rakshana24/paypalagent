use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tower::ServiceExt;

use agentic_commerce::config::Config;
use agentic_commerce::create_router;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::dispute::DisputeRecord;
use agentic_commerce::models::webhook::{
    PayPalWebhookEvent, WebhookProcessingStatus, PAYPAL_AUTH_ALGO_HEADER, PAYPAL_CERT_URL_HEADER,
    PAYPAL_TRANSMISSION_ID_HEADER, PAYPAL_TRANSMISSION_SIG_HEADER, PAYPAL_TRANSMISSION_TIME_HEADER,
};
use agentic_commerce::services::auth::PayPalAuthService;
use agentic_commerce::services::dispute_consumer::DisputeConsumerService;
use agentic_commerce::services::dispute_correlation::DisputeCorrelationService;
use agentic_commerce::services::dispute_service::DisputeService;
use agentic_commerce::services::dispute_worker::{BackgroundDisputeWorker, CancellationToken};
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
}

async fn spawn_mock_paypal_server() -> (String, Arc<MockPayPalServerState>) {
    let mock_state = Arc::new(MockPayPalServerState {
        token_request_count: Arc::new(AtomicUsize::new(0)),
        verify_request_count: Arc::new(AtomicUsize::new(0)),
    });

    let token_state = mock_state.clone();
    let token_handler = move |State(state): State<Arc<MockPayPalServerState>>, _req: Request| async move {
        state.token_request_count.fetch_add(1, Ordering::SeqCst);
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "access_token": "mock-token-worker-test",
                    "token_type": "Bearer",
                    "expires_in": 3600
                })
                .to_string(),
            ))
            .unwrap()
    };

    let verify_state = mock_state.clone();
    let verify_handler = move |State(state): State<Arc<MockPayPalServerState>>, _req: Request| async move {
        state.verify_request_count.fetch_add(1, Ordering::SeqCst);
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "verification_status": "SUCCESS"
                })
                .to_string(),
            ))
            .unwrap()
    };

    let app = Router::new()
        .route(
            "/v1/oauth2/token",
            post(token_handler).with_state(token_state),
        )
        .route(
            "/v1/notifications/verify-webhook-signature",
            post(verify_handler).with_state(verify_state),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mock_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (mock_url, mock_state)
}

struct WorkerTestContext {
    pub pool: SqlitePool,
    pub router: Router,
    pub persistence_service: Arc<WebhookPersistenceService>,
    pub dispute_service: Arc<DisputeService>,
    pub _webhook_notify: Arc<tokio::sync::Notify>,
    pub worker: BackgroundDisputeWorker,
}

async fn setup_worker_test_context() -> WorkerTestContext {
    let (mock_url, _) = spawn_mock_paypal_server().await;
    let pool = init_db("sqlite::memory:").await.unwrap();

    let config = Arc::new(Config {
        port: 0,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-WORKER-TEST-12345".to_string()),
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
    let dispute_correlation = Arc::new(DisputeCorrelationService::new(
        config.clone(),
        http_client.clone(),
        auth_service.clone(),
        pool.clone(),
        verification_service.clone(),
    ));
    let dispute_consumer = Arc::new(DisputeConsumerService::new(
        pool.clone(),
        persistence_service.clone(),
        dispute_service.clone(),
        dispute_correlation.clone(),
    ));

    let webhook_notify = Arc::new(tokio::sync::Notify::new());

    let worker = BackgroundDisputeWorker::new(dispute_consumer, webhook_notify.clone())
        .with_poll_interval(Duration::from_millis(50))
        .with_lease_duration_secs(30);

    let dispute_evidence = Arc::new(agentic_commerce::services::DisputeEvidenceService::new(
        pool.clone(),
        dispute_service.clone(),
        ledger_service.clone(),
        verification_service.clone(),
    ));

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
        webhook_persistence: persistence_service.clone(),
        webhook_notify: webhook_notify.clone(),
        dispute_evidence,
    };

    let router = create_router(state);

    WorkerTestContext {
        pool,
        router,
        persistence_service,
        dispute_service,
        _webhook_notify: webhook_notify,
        worker,
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
// 1. Worker processes pre-existing pending events on startup
// =========================================================================
#[tokio::test]
async fn test_worker_processes_pending_events_on_startup() {
    let ctx = setup_worker_test_context().await;

    // Enqueue 2 pending events in database BEFORE worker runs
    for i in 1..=2 {
        let event_id = format!("WH-STARTUP-{}", i);
        let dispute_id = format!("PP-D-STARTUP-{}", i);
        let payload = json!({
            "id": event_id,
            "event_type": "CUSTOMER.DISPUTE.CREATED",
            "resource_type": "dispute",
            "create_time": "2026-11-09T00:00:00Z",
            "resource": {
                "dispute_id": dispute_id,
                "status": "UNDER_REVIEW",
                "reason": "MERCHANDISE_OR_SERVICE_NOT_RECEIVED"
            }
        });
        let raw_bytes = serde_json::to_vec_pretty(&payload).unwrap();
        let parsed: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();

        ctx.persistence_service
            .persist_event(&parsed, &format!("TX-STARTUP-{}", i), &raw_bytes)
            .await
            .unwrap();
    }

    // Spawn background worker
    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx.worker.clone();

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    // Allow worker startup drainage
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Both events must have transitioned from PENDING to PROCESSED
    for i in 1..=2 {
        let event = ctx
            .persistence_service
            .get_event_by_id(&format!("WH-STARTUP-{}", i))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            event.processing_status,
            WebhookProcessingStatus::Processed,
            "Event must be processed on startup without notify signal"
        );

        let dispute: Option<DisputeRecord> = ctx
            .dispute_service
            .get_dispute(&format!("PP-D-STARTUP-{}", i))
            .await
            .unwrap();
        assert!(
            dispute.is_some(),
            "Dispute record must be created in database"
        );
    }

    // Cancel and safely await worker shutdown
    cancel_token.cancel();
    worker_handle.await.unwrap();
}

// =========================================================================
// 2. End-to-end automatic dispute processing via HTTP ingress and notification
// =========================================================================
#[tokio::test]
async fn test_worker_e2e_webhook_ingress_automatic_processing() {
    let ctx = setup_worker_test_context().await;

    // Start background worker
    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx.worker.clone();

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    // Post webhook event via actual Axum HTTP router
    let raw_payload = b"{\n  \"id\": \"WH-EVT-E2E-AUTO-01\",\n  \"event_type\": \"CUSTOMER.DISPUTE.CREATED\",\n  \"resource_type\": \"dispute\",\n  \"create_time\": \"2026-11-09T10:00:00Z\",\n  \"resource\": {\n    \"dispute_id\": \"PP-D-E2E-AUTO-01\",\n    \"status\": \"UNDER_REVIEW\",\n    \"reason\": \"MERCHANDISE_OR_SERVICE_NOT_RECEIVED\",\n    \"dispute_amount\": {\"currency_code\": \"USD\", \"value\": \"99.99\"}\n  }\n}";

    let mut req_builder = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");

    for (k, v) in
        valid_transmission_headers("TX-E2E-AUTO-01", "sig-valid-auto", "2026-11-09T10:00:00Z")
    {
        req_builder = req_builder.header(k, v);
    }

    let req = req_builder.body(Body::from(raw_payload.to_vec())).unwrap();
    let resp = ctx.router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Wait for the worker to receive notify and complete processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify webhook_events row is PROCESSED
    let event = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-E2E-AUTO-01")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.processing_status,
        WebhookProcessingStatus::Processed,
        "Worker must automatically process event following HTTP ingress notify"
    );

    // Verify dispute table has record
    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-E2E-AUTO-01")
        .await
        .unwrap()
        .expect("Dispute must exist in database");
    assert_eq!(dispute.dispute_id, "PP-D-E2E-AUTO-01");
    assert_eq!(dispute.status, "UNDER_REVIEW");

    // Clean shutdown
    cancel_token.cancel();
    worker_handle.await.unwrap();
}

// =========================================================================
// 3. Worker recovers scheduled retries on periodic poll interval
// =========================================================================
#[tokio::test]
async fn test_worker_processes_scheduled_retry_when_due() {
    let ctx = setup_worker_test_context().await;

    // Insert an event into database with status RETRY and next_retry_at in the past
    let raw_payload = b"{\n  \"id\": \"WH-EVT-RETRY-DUE\",\n  \"event_type\": \"CUSTOMER.DISPUTE.CREATED\",\n  \"resource_type\": \"dispute\",\n  \"create_time\": \"2026-11-09T00:00:00Z\",\n  \"resource\": {\n    \"dispute_id\": \"PP-D-RETRY-DUE\",\n    \"status\": \"UNDER_REVIEW\"\n  }\n}";
    let parsed: PayPalWebhookEvent = serde_json::from_slice(raw_payload).unwrap();

    ctx.persistence_service
        .persist_event(&parsed, "TX-RETRY-01", raw_payload)
        .await
        .unwrap();

    // Acquire lease and schedule retry
    let _leased = ctx
        .persistence_service
        .acquire_lease("temp-worker", 30)
        .await
        .unwrap()
        .unwrap();

    // Schedule retry with past next_retry_at to make it immediately eligible
    let past_retry_time = (chrono::Utc::now() - chrono::Duration::seconds(10)).to_rfc3339();
    sqlx::query(
        "UPDATE webhook_events SET processing_status = 'RETRY', next_retry_at = ?, lease_token = NULL WHERE event_id = ?;"
    )
    .bind(&past_retry_time)
    .bind("WH-EVT-RETRY-DUE")
    .execute(&ctx.pool)
    .await
    .unwrap();

    // Start background worker (with 50ms poll interval)
    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx.worker.clone();

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    // Allow worker poll cycle to pick up the due retry
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify event has now been processed
    let event = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-RETRY-DUE")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.processing_status,
        WebhookProcessingStatus::Processed,
        "Worker must pick up and process due RETRY event on poll interval"
    );

    cancel_token.cancel();
    worker_handle.await.unwrap();
}

// =========================================================================
// 4. Graceful shutdown completes without database locks or unhandled panics
// =========================================================================
#[tokio::test]
async fn test_worker_graceful_shutdown_completes_cleanly() {
    let ctx = setup_worker_test_context().await;

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx.worker.clone();

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    // Let the worker run briefly
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Signal shutdown
    cancel_token.cancel();

    // Await join handle with 2s timeout
    let join_res = tokio::time::timeout(Duration::from_secs(2), worker_handle).await;
    assert!(join_res.is_ok(), "Worker must shut down within timeout");
    assert!(
        join_res.unwrap().is_ok(),
        "Worker must not panic on shutdown"
    );

    // Verify database pool remains healthy and operational
    let test_query: Result<i64, _> = sqlx::query_scalar("SELECT 1;").fetch_one(&ctx.pool).await;
    assert_eq!(test_query.unwrap(), 1);
}

// =========================================================================
// 5. Recovery of abandoned PROCESSING leases past timeout
// =========================================================================
#[tokio::test]
async fn test_worker_reclaims_and_processes_abandoned_lease() {
    let ctx = setup_worker_test_context().await;

    // Insert an event into database
    let raw_payload = b"{\n  \"id\": \"WH-EVT-ABANDONED-01\",\n  \"event_type\": \"CUSTOMER.DISPUTE.CREATED\",\n  \"resource_type\": \"dispute\",\n  \"create_time\": \"2026-11-09T00:00:00Z\",\n  \"resource\": {\n    \"dispute_id\": \"PP-D-ABANDONED-01\",\n    \"status\": \"UNDER_REVIEW\",\n    \"reason\": \"MERCHANDISE_OR_SERVICE_NOT_RECEIVED\"\n  }\n}";
    let parsed: PayPalWebhookEvent = serde_json::from_slice(raw_payload).unwrap();

    ctx.persistence_service
        .persist_event(&parsed, "TX-ABAN-01", raw_payload)
        .await
        .unwrap();

    // Simulate an abandoned event: status=PROCESSING, started 60s ago, stale worker ID and token
    let stale_start_time = (chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc3339();
    sqlx::query(
        r#"
        UPDATE webhook_events
        SET processing_status = 'PROCESSING',
            processing_started_at = ?,
            lease_token = 'stale-lease-token-crashed',
            processing_worker_id = 'crashed-worker-old'
        WHERE event_id = ?;
        "#,
    )
    .bind(&stale_start_time)
    .bind("WH-EVT-ABANDONED-01")
    .execute(&ctx.pool)
    .await
    .unwrap();

    // Start background worker configured with 30s lease duration (so 60s is abandoned)
    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx
        .worker
        .clone()
        .with_lease_duration_secs(30)
        .with_poll_interval(Duration::from_millis(50));

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    // Allow worker startup batch to reclaim the abandoned event
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify event has now been reclaimed and PROCESSED
    let event = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-ABANDONED-01")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.processing_status,
        WebhookProcessingStatus::Processed,
        "Worker must reclaim and process abandoned PROCESSING event"
    );

    // Verify dispute record exists
    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-ABANDONED-01")
        .await
        .unwrap();
    assert!(
        dispute.is_some(),
        "Dispute must be created following abandoned lease reclamation"
    );

    cancel_token.cancel();
    worker_handle.await.unwrap();
}

// =========================================================================
// 6. Duplicate webhook delivery does not cause duplicate processing
// =========================================================================
#[tokio::test]
async fn test_worker_handles_duplicate_deliveries_idempotently() {
    let ctx = setup_worker_test_context().await;

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx.worker.clone();

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    let raw_payload = b"{\n  \"id\": \"WH-EVT-DUP-WORKER-01\",\n  \"event_type\": \"CUSTOMER.DISPUTE.CREATED\",\n  \"resource_type\": \"dispute\",\n  \"create_time\": \"2026-11-09T00:00:00Z\",\n  \"resource\": {\n    \"dispute_id\": \"PP-D-DUP-01\",\n    \"status\": \"UNDER_REVIEW\",\n    \"reason\": \"MERCHANDISE_OR_SERVICE_NOT_RECEIVED\"\n  }\n}";

    // First delivery (new event)
    let mut req1 = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in valid_transmission_headers("TX-DUP-1", "sig-dup", "2026-11-09T00:00:00Z") {
        req1 = req1.header(k, v);
    }
    let resp1 = ctx
        .router
        .clone()
        .oneshot(req1.body(Body::from(raw_payload.to_vec())).unwrap())
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);

    // Wait for worker to complete first processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check 1 dispute audit step exists
    let step_count_1: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dispute_audit_steps WHERE dispute_id = 'PP-D-DUP-01';",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(step_count_1, 1);

    // Second delivery (identical duplicate)
    let mut req2 = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in valid_transmission_headers("TX-DUP-2", "sig-dup", "2026-11-09T00:00:00Z") {
        req2 = req2.header(k, v);
    }
    let resp2 = ctx
        .router
        .clone()
        .oneshot(req2.body(Body::from(raw_payload.to_vec())).unwrap())
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK); // Recognized as DUPLICATE

    // Allow worker time
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify still exactly 1 dispute record and 1 audit step (no duplicate mutations)
    let dispute_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM disputes WHERE dispute_id = 'PP-D-DUP-01';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(dispute_count, 1);

    let step_count_2: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dispute_audit_steps WHERE dispute_id = 'PP-D-DUP-01';",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(
        step_count_2, 1,
        "Duplicate webhook delivery must not create duplicate audit steps"
    );

    cancel_token.cancel();
    worker_handle.await.unwrap();
}

// =========================================================================
// 7. Multiple concurrent webhook deliveries under SQLite WAL mode
// =========================================================================
#[tokio::test]
async fn test_worker_concurrent_webhook_deliveries_processed_safely() {
    let ctx = setup_worker_test_context().await;

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx.worker.clone();

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    // Ingest 5 distinct dispute webhooks concurrently
    let mut ingress_handles = Vec::new();
    for i in 1..=5 {
        let router = ctx.router.clone();
        let handle = tokio::spawn(async move {
            let event_id = format!("WH-EVT-CONC-{}", i);
            let dispute_id = format!("PP-D-CONC-{}", i);
            let payload = json!({
                "id": event_id,
                "event_type": "CUSTOMER.DISPUTE.CREATED",
                "resource_type": "dispute",
                "create_time": "2026-11-09T00:00:00Z",
                "resource": {
                    "dispute_id": dispute_id,
                    "status": "UNDER_REVIEW",
                    "reason": "MERCHANDISE_OR_SERVICE_NOT_RECEIVED"
                }
            });
            let raw_bytes = serde_json::to_vec_pretty(&payload).unwrap();

            let mut req = Request::builder()
                .method("POST")
                .uri("/api/webhooks/paypal")
                .header(header::CONTENT_TYPE, "application/json");

            for (k, v) in valid_transmission_headers(
                &format!("TX-CONC-{}", i),
                &format!("sig-{}", i),
                "2026-11-09T00:00:00Z",
            ) {
                req = req.header(k, v);
            }

            let resp = router
                .oneshot(req.body(Body::from(raw_bytes)).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::ACCEPTED);
        });
        ingress_handles.push(handle);
    }

    // Await all concurrent ingress requests
    for h in ingress_handles {
        h.await.unwrap();
    }

    // Wait for the background worker to drain all 5 events
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify all 5 events transitioned to PROCESSED
    let processed_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_events WHERE event_id LIKE 'WH-EVT-CONC-%' AND processing_status = 'PROCESSED';",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(
        processed_events, 5,
        "All 5 concurrent events must be successfully processed"
    );

    // Verify all 5 disputes exist in disputes table
    let created_disputes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM disputes WHERE dispute_id LIKE 'PP-D-CONC-%';")
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(
        created_disputes, 5,
        "All 5 disputes must exist without data loss"
    );

    cancel_token.cancel();
    worker_handle.await.unwrap();
}

// =========================================================================
// 8. Unsupported and malformed events never create dispute records
// =========================================================================
#[tokio::test]
async fn test_worker_unsupported_and_malformed_events_create_no_dispute_records() {
    let ctx = setup_worker_test_context().await;

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx.worker.clone();

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    // 1. Ingest an unsupported event type (e.g. PAYMENT.CAPTURE.COMPLETED)
    let unsupported_payload = json!({
        "id": "WH-EVT-UNSUP-01",
        "event_type": "PAYMENT.CAPTURE.COMPLETED",
        "resource_type": "capture",
        "create_time": "2026-11-09T00:00:00Z",
        "resource": {
            "id": "CAP-UNSUP-01",
            "status": "COMPLETED"
        }
    });
    let raw_unsupported = serde_json::to_vec_pretty(&unsupported_payload).unwrap();

    let mut req1 = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in valid_transmission_headers("TX-UNSUP", "sig-unsup", "2026-11-09T00:00:00Z") {
        req1 = req1.header(k, v);
    }
    let resp1 = ctx
        .router
        .clone()
        .oneshot(req1.body(Body::from(raw_unsupported)).unwrap())
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);

    // 2. Ingest an incomplete dispute event (missing status)
    let incomplete_payload = json!({
        "id": "WH-EVT-INCOMPL-01",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource_type": "dispute",
        "create_time": "2026-11-09T00:00:00Z",
        "resource": {
            "dispute_id": "PP-D-INCOMPL-01"
            // missing status field!
        }
    });
    let raw_incomplete = serde_json::to_vec_pretty(&incomplete_payload).unwrap();

    let mut req2 = Request::builder()
        .method("POST")
        .uri("/api/webhooks/paypal")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in valid_transmission_headers("TX-INCOMPL", "sig-incompl", "2026-11-09T00:00:00Z") {
        req2 = req2.header(k, v);
    }
    let resp2 = ctx
        .router
        .clone()
        .oneshot(req2.body(Body::from(raw_incomplete)).unwrap())
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::ACCEPTED);

    // Allow worker time to process both
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify unsupported event transitioned to IGNORED
    let event1 = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-UNSUP-01")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event1.processing_status, WebhookProcessingStatus::Ignored);

    // Verify incomplete event transitioned to RETRY
    let event2 = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-INCOMPL-01")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event2.processing_status, WebhookProcessingStatus::Retry);

    // CRITICAL SECURITY ASSERTION: No records created in disputes or dispute_audit_steps
    let total_disputes: i64 = sqlx::query_scalar("SELECT count(*) FROM disputes;")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(
        total_disputes, 0,
        "Failed or unsupported events must NEVER create dispute records"
    );

    let total_steps: i64 = sqlx::query_scalar("SELECT count(*) FROM dispute_audit_steps;")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(
        total_steps, 0,
        "Failed or unsupported events must NEVER append dispute audit steps"
    );

    cancel_token.cancel();
    worker_handle.await.unwrap();
}

// =========================================================================
// 9. Shutdown timeout behavior
// =========================================================================
#[tokio::test]
async fn test_worker_shutdown_timeout_behavior() {
    let ctx = setup_worker_test_context().await;

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    let worker = ctx.worker.clone();

    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await;
    });

    // Worker is idle, signal shutdown immediately
    cancel_token.cancel();

    // Verify timeout of 10s safely completes within milliseconds
    let timeout_result = tokio::time::timeout(Duration::from_secs(10), worker_handle).await;
    assert!(
        timeout_result.is_ok(),
        "Worker must terminate well within the 10-second timeout window"
    );
    assert!(timeout_result.unwrap().is_ok());
}
