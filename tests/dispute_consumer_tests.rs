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
use agentic_commerce::error::AppError;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::correlation::CorrelationStatus;
use agentic_commerce::models::dispute::{
    DisputeActionType, GenesisAnchorType, GENESIS_HASH_DEFAULT,
};
use agentic_commerce::models::ledger::{AppendEventRequest, CreateSessionRequest};
use agentic_commerce::models::webhook::{PayPalWebhookEvent, WebhookProcessingStatus};
use agentic_commerce::services::auth::PayPalAuthService;
use agentic_commerce::services::dispute_consumer::{
    ConsumerProcessingOutcome, DisputeConsumerService,
};
use agentic_commerce::services::dispute_correlation::DisputeCorrelationService;
use agentic_commerce::services::dispute_service::DisputeService;
use agentic_commerce::services::ledger::LedgerService;
use agentic_commerce::services::verification::VerificationService;
use agentic_commerce::services::webhook_persistence::WebhookPersistenceService;

#[derive(Clone)]
struct MockPayPalServerState {
    pub _token_request_count: Arc<AtomicUsize>,
    pub dispute_details_request_count: Arc<AtomicUsize>,
    pub dispute_details_status: Arc<RwLock<StatusCode>>,
    pub dispute_details_body: Arc<RwLock<serde_json::Value>>,
}

async fn setup_mock_paypal_server() -> (String, MockPayPalServerState) {
    let state = MockPayPalServerState {
        _token_request_count: Arc::new(AtomicUsize::new(0)),
        dispute_details_request_count: Arc::new(AtomicUsize::new(0)),
        dispute_details_status: Arc::new(RwLock::new(StatusCode::OK)),
        dispute_details_body: Arc::new(RwLock::new(json!({}))),
    };

    let token_handler = |_req: axum::extract::Request| async {
        let body = json!({
            "access_token": "mock-token",
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
                .body(body.to_string())
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

struct ConsumerTestContext {
    pub pool: SqlitePool,
    pub persistence_service: Arc<WebhookPersistenceService>,
    pub dispute_service: Arc<DisputeService>,
    pub _correlation_service: Arc<DisputeCorrelationService>,
    pub ledger_service: Arc<LedgerService>,
    pub consumer_service: DisputeConsumerService,
}

async fn setup_consumer_context() -> ConsumerTestContext {
    let (paypal_url, _mock_state) = setup_mock_paypal_server().await;
    let pool = init_db("sqlite::memory:").await.unwrap();

    let config = Arc::new(Config {
        port: 0,
        rust_log: "info".to_string(),
        paypal_base_url: paypal_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "mock_client".to_string(),
        paypal_client_secret: "mock_secret".to_string(),
        paypal_webhook_id: Some("mock_webhook_id".to_string()),
    });

    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), http_client.clone()));
    let verification_service = Arc::new(VerificationService::new(pool.clone()));
    let ledger_service = Arc::new(LedgerService::new(pool.clone()));
    let dispute_service = Arc::new(DisputeService::new(
        pool.clone(),
        verification_service.clone(),
    ));

    let correlation_service = Arc::new(DisputeCorrelationService::new(
        config,
        http_client,
        auth_service,
        pool.clone(),
        verification_service.clone(),
    ));

    let persistence_service = Arc::new(WebhookPersistenceService::new(pool.clone()));

    let consumer_service = DisputeConsumerService::new(
        pool.clone(),
        persistence_service.clone(),
        dispute_service.clone(),
        correlation_service.clone(),
    );

    ConsumerTestContext {
        pool,
        persistence_service,
        dispute_service,
        _correlation_service: correlation_service,
        ledger_service,
        consumer_service,
    }
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

#[tokio::test]
async fn test_01_consumer_processes_valid_dispute_webhook() {
    let ctx = setup_consumer_context().await;

    let payload = json!({
        "id": "WH-EVT-01",
        "create_time": "2026-11-08T10:00:00Z",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-01",
            "status": "UNDER_REVIEW",
            "dispute_life_cycle_stage": "INQUIRY",
            "dispute_channel": "INTERNAL",
            "reason": "MERCHANDISE_OR_SERVICE_NOT_RECEIVED",
            "dispute_amount": {
                "currency_code": "USD",
                "value": "299.00"
            },
            "create_time": "2026-11-08T10:00:00Z",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-01", &raw_bytes)
        .await
        .unwrap();

    let outcome = ctx
        .consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    assert!(outcome.is_some());
    match outcome.unwrap() {
        ConsumerProcessingOutcome::Processed {
            event_id,
            dispute_id,
            dispute,
        } => {
            assert_eq!(event_id, "WH-EVT-01");
            assert_eq!(dispute_id, "PP-D-CONSUMER-01");
            assert_eq!(dispute.status, "UNDER_REVIEW");
            assert_eq!(dispute.genesis_anchor_type, GenesisAnchorType::GenesisHash);
            assert_eq!(dispute.genesis_anchor_hash, GENESIS_HASH_DEFAULT);
        }
        other => panic!("Expected Processed outcome, got {:?}", other),
    }
}

#[tokio::test]
async fn test_02_dispute_record_and_audit_steps_created() {
    let ctx = setup_consumer_context().await;

    let payload = json!({
        "id": "WH-EVT-02",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-02",
            "status": "UNDER_REVIEW",
            "create_time": "2026-11-08T10:00:00Z",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-02", &raw_bytes)
        .await
        .unwrap();

    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // Verify dispute record
    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-CONSUMER-02")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dispute.dispute_id, "PP-D-CONSUMER-02");
    assert!(dispute.root_hash.is_some());

    // Verify audit step
    let steps = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-CONSUMER-02")
        .await
        .unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].sequence_index, 0);
    assert_eq!(steps[0].action_type, DisputeActionType::DISPUTE_IMPORTED);

    // Verify cryptographic audit chain
    let v_res = ctx
        .dispute_service
        .verify_dispute_chain("PP-D-CONSUMER-02")
        .await
        .unwrap();
    assert!(v_res.valid, "Verification failed: {:?}", v_res.errors);
}

#[tokio::test]
async fn test_03_webhook_becomes_processed_in_same_transaction() {
    let ctx = setup_consumer_context().await;

    let payload = json!({
        "id": "WH-EVT-03",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-03",
            "status": "UNDER_REVIEW",
            "create_time": "2026-11-08T10:00:00Z",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-03", &raw_bytes)
        .await
        .unwrap();

    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // Webhook event is PROCESSED with null lease token
    let webhook_rec = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-03")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        webhook_rec.processing_status,
        WebhookProcessingStatus::Processed
    );
    assert!(webhook_rec.processed_at.is_some());
    assert_eq!(webhook_rec.lease_token, None);
}

#[tokio::test]
async fn test_04_failure_before_dispute_insertion_schedules_retry() {
    let ctx = setup_consumer_context().await;

    // Payload has dispute_id, but status is empty -> validation fails before dispute insertion
    let payload = json!({
        "id": "WH-EVT-04",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-04",
            "status": "",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-04", &raw_bytes)
        .await
        .unwrap();

    let res = ctx
        .consumer_service
        .process_next_event("worker-1", 300)
        .await;
    assert!(res.is_err());

    // Verify webhook transitioned to RETRY
    let webhook_rec = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-04")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        webhook_rec.processing_status,
        WebhookProcessingStatus::Retry
    );
    assert_eq!(webhook_rec.retry_count, 1);
    assert!(webhook_rec.last_error.is_some());
    assert!(webhook_rec.next_retry_at.is_some());

    // Zero dispute records created
    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-CONSUMER-04")
        .await
        .unwrap();
    assert_eq!(dispute, None);
}

#[tokio::test]
async fn test_05_failure_during_dispute_processing_leaves_no_partial_writes() {
    let ctx = setup_consumer_context().await;

    // Insert an inconsistent dispute record directly into SQLite with NO audit steps
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO disputes (
            dispute_id, session_id, genesis_anchor_type, genesis_anchor_hash,
            status, correlation_status, created_at, updated_at
        ) VALUES ('PP-D-CORRUPTED', NULL, 'GENESIS_HASH', '0000000000000000000000000000000000000000000000000000000000000000',
                 'UNDER_REVIEW', 'NOT_FOUND', ?, ?);
        "#,
    )
    .bind(&now)
    .bind(&now)
    .execute(&ctx.pool)
    .await
    .unwrap();

    let payload = json!({
        "id": "WH-EVT-05",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.UPDATED",
        "resource": {
            "dispute_id": "PP-D-CORRUPTED",
            "status": "RESOLVED",
            "update_time": "2026-11-08T12:00:00Z"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-05", &raw_bytes)
        .await
        .unwrap();

    let res = ctx
        .consumer_service
        .process_next_event("worker-1", 300)
        .await;
    assert!(res.is_err());
    match res.unwrap_err() {
        AppError::DisputeAuditIntegrity(msg) => {
            assert!(msg.contains("has no audit steps"));
        }
        other => panic!("Expected DisputeAuditIntegrity, got {:?}", other),
    }

    // Verify rollback: zero audit steps persisted
    let steps = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-CORRUPTED")
        .await
        .unwrap();
    assert!(steps.is_empty());

    // Webhook scheduled for retry
    let webhook_rec = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-05")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        webhook_rec.processing_status,
        WebhookProcessingStatus::Retry
    );
}

#[tokio::test]
async fn test_06_fenced_completion_failure_rolls_back_dispute_changes() {
    let ctx = setup_consumer_context().await;

    let payload = json!({
        "id": "WH-EVT-06",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-06",
            "status": "UNDER_REVIEW",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-06", &raw_bytes)
        .await
        .unwrap();

    // 1. Worker 1 acquires lease
    let leased_event = ctx
        .persistence_service
        .acquire_lease("worker-1", 300)
        .await
        .unwrap()
        .unwrap();

    // 2. Simulate Worker 2 superseding the lease by modifying the database directly
    sqlx::query("UPDATE webhook_events SET lease_token = 'new-superseded-token' WHERE event_id = 'WH-EVT-06';")
        .execute(&ctx.pool)
        .await
        .unwrap();

    // 3. Worker 1 attempts to process the leased event
    let res = ctx
        .consumer_service
        .process_leased_event(leased_event)
        .await;
    assert!(res.is_err());
    match res.unwrap_err() {
        AppError::DisputeLeaseFencedOut(msg) => {
            assert!(msg.contains("no longer owned by lease token"));
        }
        other => panic!("Expected DisputeLeaseFencedOut, got {:?}", other),
    }

    // 4. Assert transaction rollback: zero dispute rows or audit steps persisted
    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-CONSUMER-06")
        .await
        .unwrap();
    assert_eq!(dispute, None);

    let steps = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-CONSUMER-06")
        .await
        .unwrap();
    assert!(steps.is_empty());
}

#[tokio::test]
async fn test_07_stale_worker_cannot_commit_after_lease_reclamation() {
    let ctx = setup_consumer_context().await;

    let payload = json!({
        "id": "WH-EVT-07",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-07",
            "status": "UNDER_REVIEW",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-07", &raw_bytes)
        .await
        .unwrap();

    // 1. Worker 1 claims lease
    let stale_claim = ctx
        .persistence_service
        .acquire_lease("worker-stale", 300)
        .await
        .unwrap()
        .unwrap();

    // 2. Simulate lease expiration: set processing_started_at to 400 seconds ago
    let old_time = (chrono::Utc::now() - chrono::Duration::seconds(400)).to_rfc3339();
    sqlx::query(
        "UPDATE webhook_events SET processing_started_at = ? WHERE event_id = 'WH-EVT-07';",
    )
    .bind(&old_time)
    .execute(&ctx.pool)
    .await
    .unwrap();

    // 3. Worker 2 reclaims the abandoned event
    let new_claim = ctx
        .persistence_service
        .acquire_lease("worker-active", 300)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(new_claim.event_id, "WH-EVT-07");
    assert_ne!(new_claim.lease_token, stale_claim.lease_token);

    // 3. Stale worker attempts to process using its old claim
    let res = ctx.consumer_service.process_leased_event(stale_claim).await;
    assert!(res.is_err());
    match res.unwrap_err() {
        AppError::DisputeLeaseFencedOut(_) => {}
        other => panic!("Expected DisputeLeaseFencedOut, got {:?}", other),
    }

    // 4. Confirm Worker 2 still owns the lease
    let current = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-07")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.lease_token, new_claim.lease_token);
    assert_eq!(
        current.processing_worker_id.as_deref(),
        Some("worker-active")
    );

    // 5. Worker 2 processes the event successfully
    let outcome = ctx.consumer_service.process_leased_event(new_claim).await;
    assert!(outcome.is_ok());

    let final_dispute = ctx
        .dispute_service
        .get_dispute("PP-D-CONSUMER-07")
        .await
        .unwrap();
    assert!(final_dispute.is_some());
}

#[tokio::test]
async fn test_08_malformed_payload_follows_failure_policy() {
    let ctx = setup_consumer_context().await;

    // Directly insert a record with invalid raw_payload bytes
    let raw_invalid = b"{ not a valid json at all }";
    let payload_hash = agentic_commerce::models::webhook::compute_payload_hash(raw_invalid);
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            event_id, event_type, resource_type, resource_id,
            transmission_id, processing_status, retry_count, max_retries,
            raw_payload, payload_hash, created_at
        ) VALUES ('WH-EVT-08', 'CUSTOMER.DISPUTE.CREATED', 'dispute', 'PP-D-08', 'TX-08', 'PENDING', 0, 5, ?, ?, ?);
        "#
    )
    .bind(raw_invalid.as_slice())
    .bind(&payload_hash)
    .bind(&now)
    .execute(&ctx.pool)
    .await
    .unwrap();

    let res = ctx
        .consumer_service
        .process_next_event("worker-1", 300)
        .await;
    assert!(res.is_err());
    match res.unwrap_err() {
        AppError::Serialization(_) => {}
        other => panic!("Expected AppError::Serialization, got {:?}", other),
    }

    let rec = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-08")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.processing_status, WebhookProcessingStatus::Retry);
    assert_eq!(rec.retry_count, 1);
}

#[tokio::test]
async fn test_09_unsupported_event_types_do_not_create_disputes() {
    let ctx = setup_consumer_context().await;

    let payload = json!({
        "id": "WH-EVT-09",
        "resource_type": "capture",
        "event_type": "PAYMENT.CAPTURE.COMPLETED",
        "resource": {
            "id": "CAP-09",
            "status": "COMPLETED"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-09", &raw_bytes)
        .await
        .unwrap();

    let outcome = ctx
        .consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    assert!(outcome.is_some());
    match outcome.unwrap() {
        ConsumerProcessingOutcome::Ignored { event_id, reason } => {
            assert_eq!(event_id, "WH-EVT-09");
            assert!(reason.contains("Unsupported webhook event type"));
        }
        other => panic!("Expected Ignored outcome, got {:?}", other),
    }

    let rec = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-09")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.processing_status, WebhookProcessingStatus::Ignored);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM disputes;")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "No dispute records should be created for ignored events"
    );
}

#[tokio::test]
async fn test_10_missing_required_dispute_fields_handled_safely() {
    let ctx = setup_consumer_context().await;

    // Resource lacks dispute_id
    let payload = json!({
        "id": "WH-EVT-10",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "status": "UNDER_REVIEW",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-10", &raw_bytes)
        .await
        .unwrap();

    let res = ctx
        .consumer_service
        .process_next_event("worker-1", 300)
        .await;
    assert!(res.is_err());
    match res.unwrap_err() {
        AppError::InvalidDisputeData(msg) => {
            assert!(msg.contains("missing required field: dispute_id"));
        }
        other => panic!("Expected InvalidDisputeData, got {:?}", other),
    }

    let rec = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.processing_status, WebhookProcessingStatus::Retry);
}

#[tokio::test]
async fn test_11_retry_uses_original_lease_token() {
    let ctx = setup_consumer_context().await;

    let payload = json!({
        "id": "WH-EVT-11",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "status": "UNDER_REVIEW"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-11", &raw_bytes)
        .await
        .unwrap();

    let claim = ctx
        .persistence_service
        .acquire_lease("worker-1", 300)
        .await
        .unwrap()
        .unwrap();
    let original_token = claim.lease_token.clone().unwrap();

    // Directly schedule retry with original token
    let retried = ctx
        .persistence_service
        .schedule_retry_fenced(&claim.event_id, &original_token, "Simulation failure")
        .await
        .unwrap();

    assert_eq!(retried.processing_status, WebhookProcessingStatus::Retry);
    assert_eq!(retried.retry_count, 1);
    assert_eq!(retried.lease_token, None);
}

#[tokio::test]
async fn test_12_superseded_lease_cannot_overwrite_another_workers_state() {
    let ctx = setup_consumer_context().await;

    let payload = json!({
        "id": "WH-EVT-12",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-12",
            "status": "UNDER_REVIEW"
        }
    });

    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    ctx.persistence_service
        .persist_event(&event, "TX-12", &raw_bytes)
        .await
        .unwrap();

    // 1. Worker 1 claims
    let claim1 = ctx
        .persistence_service
        .acquire_lease("worker-1", 300)
        .await
        .unwrap()
        .unwrap();
    let old_token = claim1.lease_token.unwrap();

    // 2. Simulate lease expiration: set processing_started_at to 400 seconds ago
    let old_time = (chrono::Utc::now() - chrono::Duration::seconds(400)).to_rfc3339();
    sqlx::query(
        "UPDATE webhook_events SET processing_started_at = ? WHERE event_id = 'WH-EVT-12';",
    )
    .bind(&old_time)
    .execute(&ctx.pool)
    .await
    .unwrap();

    // 3. Worker 2 claims
    let claim2 = ctx
        .persistence_service
        .acquire_lease("worker-2", 300)
        .await
        .unwrap()
        .unwrap();
    let new_token = claim2.lease_token.unwrap();

    // 3. Worker 1 tries schedule_retry_fenced with old token
    let res = ctx
        .persistence_service
        .schedule_retry_fenced("WH-EVT-12", &old_token, "Stale worker error")
        .await;
    assert!(res.is_err());
    match res.unwrap_err() {
        AppError::DisputeLeaseFencedOut(_) => {}
        other => panic!("Expected DisputeLeaseFencedOut, got {:?}", other),
    }

    // 4. Confirm Worker 2's lease is still intact
    let rec = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-12")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.lease_token.as_deref(), Some(new_token.as_str()));
    assert_eq!(rec.processing_worker_id.as_deref(), Some("worker-2"));
    assert_eq!(rec.processing_status, WebhookProcessingStatus::Processing);
}

#[tokio::test]
async fn test_13_duplicate_webhook_delivery_does_not_append_duplicate_audit_history() {
    let ctx = setup_consumer_context().await;

    let payload1 = json!({
        "id": "WH-EVT-13-A",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-13",
            "status": "UNDER_REVIEW",
            "create_time": "2026-11-08T10:00:00Z",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes1 = serde_json::to_vec(&payload1).unwrap();
    let event1: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes1).unwrap();
    ctx.persistence_service
        .persist_event(&event1, "TX-13-A", &raw_bytes1)
        .await
        .unwrap();

    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    let steps1 = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-CONSUMER-13")
        .await
        .unwrap();
    assert_eq!(steps1.len(), 1);

    // Second webhook event with identical timestamp and payload
    let payload2 = json!({
        "id": "WH-EVT-13-B",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.UPDATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-13",
            "status": "UNDER_REVIEW",
            "create_time": "2026-11-08T10:00:00Z",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw_bytes2 = serde_json::to_vec(&payload2).unwrap();
    let event2: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes2).unwrap();
    ctx.persistence_service
        .persist_event(&event2, "TX-13-B", &raw_bytes2)
        .await
        .unwrap();

    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // Audit step count remains 1 (idempotent no-op)
    let steps2 = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-CONSUMER-13")
        .await
        .unwrap();
    assert_eq!(steps2.len(), 1);
}

#[tokio::test]
async fn test_14_timestamp_ordering_behavior_preserved_through_consumer() {
    let ctx = setup_consumer_context().await;

    // 1. Initial event with update_time 12:00:00Z
    let payload1 = json!({
        "id": "WH-EVT-14-A",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-14",
            "status": "WAITING_FOR_SELLER_RESPONSE",
            "create_time": "2026-11-08T10:00:00Z",
            "update_time": "2026-11-08T12:00:00Z"
        }
    });

    let raw1 = serde_json::to_vec(&payload1).unwrap();
    let ev1: PayPalWebhookEvent = serde_json::from_slice(&raw1).unwrap();
    ctx.persistence_service
        .persist_event(&ev1, "TX-14-A", &raw1)
        .await
        .unwrap();
    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // 2. Older event with update_time 10:30:00Z arriving out of order
    let payload2 = json!({
        "id": "WH-EVT-14-B",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.UPDATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-14",
            "status": "UNDER_REVIEW",
            "create_time": "2026-11-08T10:00:00Z",
            "update_time": "2026-11-08T10:30:00Z"
        }
    });

    let raw2 = serde_json::to_vec(&payload2).unwrap();
    let ev2: PayPalWebhookEvent = serde_json::from_slice(&raw2).unwrap();
    ctx.persistence_service
        .persist_event(&ev2, "TX-14-B", &raw2)
        .await
        .unwrap();
    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // 3. Confirm dispute state preserved (still WAITING_FOR_SELLER_RESPONSE)
    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-CONSUMER-14")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dispute.status, "WAITING_FOR_SELLER_RESPONSE");
    assert_eq!(
        dispute.dispute_update_time.as_deref(),
        Some("2026-11-08T12:00:00Z")
    );

    // 4. Confirm audit step appended as DISPUTE_OUT_OF_ORDER_RECEIVED
    let steps = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-CONSUMER-14")
        .await
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1].action_type,
        DisputeActionType::DISPUTE_OUT_OF_ORDER_RECEIVED
    );

    let v_res = ctx
        .dispute_service
        .verify_dispute_chain("PP-D-CONSUMER-14")
        .await
        .unwrap();
    assert!(v_res.valid);
}

#[tokio::test]
async fn test_15_successful_late_correlation_uses_verified_correlation_path() {
    let ctx = setup_consumer_context().await;

    // 1. Ingest initial unmatched dispute
    let payload1 = json!({
        "id": "WH-EVT-15-A",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-15",
            "status": "UNDER_REVIEW",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw1 = serde_json::to_vec(&payload1).unwrap();
    let ev1: PayPalWebhookEvent = serde_json::from_slice(&raw1).unwrap();
    ctx.persistence_service
        .persist_event(&ev1, "TX-15-A", &raw1)
        .await
        .unwrap();
    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    let d1 = ctx
        .dispute_service
        .get_dispute("PP-D-CONSUMER-15")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d1.session_id, None);
    assert_eq!(d1.genesis_anchor_type, GenesisAnchorType::GenesisHash);

    // 2. Finalized purchase session is created in the database
    let (sess_id, expected_root) =
        create_finalized_session(&ctx.ledger_service, "ORD-CORR-15", "CAP-CORR-15").await;

    // 3. Second webhook arrives with the capture_id
    let payload2 = json!({
        "id": "WH-EVT-15-B",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.UPDATED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-15",
            "status": "UNDER_REVIEW",
            "seller_transaction_id": "CAP-CORR-15",
            "update_time": "2026-11-08T11:00:00Z"
        }
    });

    let raw2 = serde_json::to_vec(&payload2).unwrap();
    let ev2: PayPalWebhookEvent = serde_json::from_slice(&raw2).unwrap();
    ctx.persistence_service
        .persist_event(&ev2, "TX-15-B", &raw2)
        .await
        .unwrap();
    ctx.consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    // 4. Verify late correlation succeeded
    let d2 = ctx
        .dispute_service
        .get_dispute("PP-D-CONSUMER-15")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d2.session_id, Some(sess_id.clone()));
    assert_eq!(d2.purchase_root_hash, Some(expected_root));
    assert_eq!(d2.correlation_status, CorrelationStatus::Matched);
    // Genesis anchor is unchanged!
    assert_eq!(d2.genesis_anchor_type, GenesisAnchorType::GenesisHash);
    assert_eq!(d2.genesis_anchor_hash, GENESIS_HASH_DEFAULT);

    // 5. Dual-chain verification succeeds
    let dual_res = ctx
        .dispute_service
        .verify_dispute_and_purchase_ledger("PP-D-CONSUMER-15")
        .await
        .unwrap();
    assert!(
        dual_res.valid,
        "Dual-chain verification failed: {:?}",
        dual_res.errors
    );
}

#[tokio::test]
async fn test_16_unrelated_event_type_with_misleading_resource_fields_is_ignored() {
    let ctx = setup_consumer_context().await;

    // Payload has an unrelated event_type ("PAYMENT.CAPTURE.COMPLETED"), but intentionally
    // includes misleading resource_type "dispute" and full dispute fields in its resource.
    let payload = json!({
        "id": "WH-EVT-16-MISLEADING",
        "resource_type": "dispute",
        "event_type": "PAYMENT.CAPTURE.COMPLETED",
        "resource": {
            "dispute_id": "PP-D-CONSUMER-16-SPOOF",
            "status": "UNDER_REVIEW",
            "dispute_life_cycle_stage": "INQUIRY",
            "dispute_channel": "INTERNAL",
            "reason": "FRAUD",
            "dispute_amount": {
                "currency_code": "USD",
                "value": "150.00"
            },
            "create_time": "2026-11-08T10:00:00Z",
            "update_time": "2026-11-08T10:00:00Z"
        }
    });

    let raw = serde_json::to_vec(&payload).unwrap();
    let ev: PayPalWebhookEvent = serde_json::from_slice(&raw).unwrap();
    ctx.persistence_service
        .persist_event(&ev, "TX-16", &raw)
        .await
        .unwrap();

    let outcome = ctx
        .consumer_service
        .process_next_event("worker-1", 300)
        .await
        .unwrap();

    assert!(outcome.is_some());
    match outcome.unwrap() {
        ConsumerProcessingOutcome::Ignored { event_id, reason } => {
            assert_eq!(event_id, "WH-EVT-16-MISLEADING");
            assert!(
                reason.contains("Unsupported webhook event type 'PAYMENT.CAPTURE.COMPLETED'"),
                "Expected reason to identify unsupported event type, got: {}",
                reason
            );
        }
        ConsumerProcessingOutcome::Processed { .. } => {
            panic!(
                "Unrelated event type with misleading resource must NOT be processed as dispute!"
            );
        }
    }

    // Verify NO dispute row was created
    let dispute = ctx
        .dispute_service
        .get_dispute("PP-D-CONSUMER-16-SPOOF")
        .await
        .unwrap();
    assert!(
        dispute.is_none(),
        "No dispute record should exist for ignored event"
    );

    // Verify NO audit steps were recorded
    let steps = ctx
        .dispute_service
        .get_dispute_audit_steps("PP-D-CONSUMER-16-SPOOF")
        .await
        .unwrap();
    assert!(steps.is_empty(), "No audit steps should exist");

    // Verify webhook event record in DB is IGNORED with valid lease token cleared
    let webhook_record = ctx
        .persistence_service
        .get_event_by_id("WH-EVT-16-MISLEADING")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        webhook_record.processing_status,
        WebhookProcessingStatus::Ignored
    );
    assert!(webhook_record.lease_token.is_none());
}
