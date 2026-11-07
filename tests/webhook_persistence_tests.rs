use serde_json::json;
use std::sync::Arc;
use tokio::task::JoinSet;

use agentic_commerce::db::init_db;
use agentic_commerce::error::AppError;
use agentic_commerce::models::webhook::{
    calculate_retry_delay_seconds, compute_payload_hash, PayPalWebhookEvent,
    WebhookPersistenceResult, WebhookProcessingStatus,
};
use agentic_commerce::services::webhook_persistence::WebhookPersistenceService;

/// Helper to spin up a clean in-memory database with all Phase 1-4 migrations applied.
async fn setup_test_db() -> (sqlx::SqlitePool, WebhookPersistenceService) {
    let pool = init_db("sqlite::memory:")
        .await
        .expect("Failed to initialize test database schema");
    let service = WebhookPersistenceService::new(pool.clone());
    (pool, service)
}

fn create_sample_webhook_event(event_id: &str, dispute_id: &str) -> (PayPalWebhookEvent, Vec<u8>) {
    let payload = json!({
        "id": event_id,
        "create_time": "2026-11-07T14:44:47Z",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "summary": "Customer opened dispute PP-D-9988",
        "resource": {
            "dispute_id": dispute_id,
            "status": "WAITING_FOR_SELLER_RESPONSE",
            "dispute_amount": {
                "currency_code": "USD",
                "value": "129.99"
            }
        }
    });

    let raw_bytes = serde_json::to_vec_pretty(&payload).unwrap();
    let event: PayPalWebhookEvent = serde_json::from_slice(&raw_bytes).unwrap();
    (event, raw_bytes)
}

#[tokio::test]
async fn test_successful_webhook_event_persistence() {
    let (_pool, service) = setup_test_db().await;
    let (event, raw_payload) = create_sample_webhook_event("WH-EVT-100", "PP-D-100");

    let result = service
        .persist_event(&event, "trans-id-100", &raw_payload)
        .await
        .expect("Persistence must succeed");

    assert!(result.is_inserted());
    assert!(!result.is_duplicate());
    assert_eq!(result.event_id(), "WH-EVT-100");

    match result {
        WebhookPersistenceResult::Inserted(record) => {
            assert_eq!(record.event_id, "WH-EVT-100");
            assert_eq!(record.event_type, "CUSTOMER.DISPUTE.CREATED");
            assert_eq!(record.resource_type.as_deref(), Some("dispute"));
            assert_eq!(record.resource_id.as_deref(), Some("PP-D-100"));
            assert_eq!(record.transmission_id, "trans-id-100");
            assert_eq!(record.processing_status, WebhookProcessingStatus::Pending);
            assert_eq!(record.retry_count, 0);
            assert_eq!(record.max_retries, 5);
            assert_eq!(record.last_error, None);
            assert_eq!(record.raw_payload, raw_payload);
            assert_eq!(record.payload_hash, compute_payload_hash(&raw_payload));
            assert_eq!(record.next_retry_at, None);
            assert_eq!(record.processed_at, None);
        }
        WebhookPersistenceResult::Duplicate { .. } => panic!("Expected Inserted variant"),
    }

    // Verify record can be retrieved
    let retrieved = service
        .get_event_by_id("WH-EVT-100")
        .await
        .expect("Query must succeed")
        .expect("Record must exist");

    assert_eq!(retrieved.event_id, "WH-EVT-100");
    assert_eq!(retrieved.raw_payload, raw_payload);
}

#[tokio::test]
async fn test_duplicate_event_id_rejection() {
    let (_pool, service) = setup_test_db().await;
    let (event, raw_payload) = create_sample_webhook_event("WH-EVT-200", "PP-D-200");

    // First arrival
    let res1 = service
        .persist_event(&event, "trans-id-200-a", &raw_payload)
        .await
        .unwrap();
    assert!(res1.is_inserted());

    // Second arrival (duplicate delivery)
    let res2 = service
        .persist_event(&event, "trans-id-200-b", &raw_payload)
        .await
        .unwrap();
    assert!(res2.is_duplicate());
    assert!(!res2.is_inserted());
    assert_eq!(res2.event_id(), "WH-EVT-200");
}

#[tokio::test]
async fn test_duplicate_event_does_not_overwrite_original_payload() {
    let (_pool, service) = setup_test_db().await;
    let (event, original_payload) = create_sample_webhook_event("WH-EVT-300", "PP-D-300");

    // Persist original
    let res1 = service
        .persist_event(&event, "trans-id-300-original", &original_payload)
        .await
        .unwrap();
    assert!(res1.is_inserted());

    // Attempt to persist duplicate with completely different tampered payload bytes
    let tampered_payload = b"{\"attacker\":\"payload_tampered\"}".to_vec();
    let res2 = service
        .persist_event(&event, "trans-id-300-tampered", &tampered_payload)
        .await
        .unwrap();
    assert!(res2.is_duplicate());

    // Verify database record still contains the original untampered payload
    let record = service
        .get_event_by_id("WH-EVT-300")
        .await
        .unwrap()
        .expect("Record must exist");

    assert_eq!(record.raw_payload, original_payload);
    assert_eq!(record.transmission_id, "trans-id-300-original");
    assert_eq!(record.payload_hash, compute_payload_hash(&original_payload));
    assert_eq!(record.retry_count, 0);
}

#[tokio::test]
async fn test_raw_payload_byte_preservation() {
    let (_pool, service) = setup_test_db().await;

    // Payload with non-canonical formatting, arbitrary whitespace, comments-like strings, and UTF-8
    let unusual_bytes =
        b"{\n  \"message\": \"Unicode: \xE2\x9C\x93 \xF0\x9F\x94\x92\",\n  \"spaces\":   42\n}"
            .to_vec();

    let event = PayPalWebhookEvent {
        id: "WH-EVT-RAW-1".to_string(),
        create_time: None,
        resource_type: "capture".to_string(),
        event_version: None,
        event_type: "PAYMENT.CAPTURE.COMPLETED".to_string(),
        summary: None,
        resource_version: None,
        resource: json!({ "id": "CAP-8877" }),
        links: None,
    };

    let res = service
        .persist_event(&event, "trans-raw", &unusual_bytes)
        .await
        .unwrap();
    assert!(res.is_inserted());

    let record = service
        .get_event_by_id("WH-EVT-RAW-1")
        .await
        .unwrap()
        .unwrap();

    // Verify exact byte-for-byte preservation
    assert_eq!(record.raw_payload, unusual_bytes);
    assert_eq!(record.raw_payload.len(), unusual_bytes.len());
}

#[tokio::test]
async fn test_sha256_payload_hash_verification() {
    let (pool, service) = setup_test_db().await;
    let (event, raw_payload) = create_sample_webhook_event("WH-EVT-HASH-1", "PP-D-HASH");

    service
        .persist_event(&event, "trans-hash", &raw_payload)
        .await
        .unwrap();

    // Normal retrieval: payload matches hash -> Ok
    let valid_record = service.get_event_by_id("WH-EVT-HASH-1").await.unwrap();
    assert!(valid_record.is_some());

    // Tamper with the raw_payload directly in the database
    sqlx::query(
        "UPDATE webhook_events SET raw_payload = X'DEADBEEF' WHERE event_id = 'WH-EVT-HASH-1';",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Retrieval must detect hash mismatch and return PayloadHashMismatch error
    let tampered_result = service.get_event_by_id("WH-EVT-HASH-1").await;
    assert!(tampered_result.is_err());
    match tampered_result.err().unwrap() {
        AppError::PayloadHashMismatch(msg) => {
            assert!(msg.contains("WH-EVT-HASH-1"));
            assert!(msg.contains("Payload hash mismatch"));
        }
        other => panic!("Expected PayloadHashMismatch, got {:?}", other),
    }
}

#[tokio::test]
async fn test_pending_event_retrieval() {
    let (_pool, service) = setup_test_db().await;

    // Create 3 events
    let (event1, p1) = create_sample_webhook_event("WH-EVT-P1", "PP-1");
    let (event2, p2) = create_sample_webhook_event("WH-EVT-P2", "PP-2");
    let (event3, p3) = create_sample_webhook_event("WH-EVT-P3", "PP-3");

    service.persist_event(&event1, "t1", &p1).await.unwrap();
    service.persist_event(&event2, "t2", &p2).await.unwrap();
    service.persist_event(&event3, "t3", &p3).await.unwrap();

    // Mark event2 as PROCESSING, and event3 as PROCESSED
    service.mark_processing("WH-EVT-P2").await.unwrap();
    service.mark_processing("WH-EVT-P3").await.unwrap();
    service.mark_processed("WH-EVT-P3").await.unwrap();

    // Pending retrieval should only return event1
    let pending = service.get_pending_events(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].event_id, "WH-EVT-P1");

    // Pagination limit check
    let limited = service.get_pending_events(0).await.unwrap();
    assert_eq!(limited.len(), 0);
}

#[tokio::test]
async fn test_processing_status_transitions() {
    let (_pool, service) = setup_test_db().await;
    let (event, raw_payload) = create_sample_webhook_event("WH-EVT-TRANS", "PP-TRANS");

    service
        .persist_event(&event, "trans", &raw_payload)
        .await
        .unwrap();

    // 1. Initial state is PENDING
    let initial = service
        .get_event_by_id("WH-EVT-TRANS")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.processing_status, WebhookProcessingStatus::Pending);

    // 2. Transition PENDING -> PROCESSING
    let processing = service.mark_processing("WH-EVT-TRANS").await.unwrap();
    assert_eq!(
        processing.processing_status,
        WebhookProcessingStatus::Processing
    );

    // 3. Transition PROCESSING -> PROCESSED
    let processed = service.mark_processed("WH-EVT-TRANS").await.unwrap();
    assert_eq!(
        processed.processing_status,
        WebhookProcessingStatus::Processed
    );
    assert!(processed.processed_at.is_some());

    // 4. Invalid transition: PROCESSED cannot transition to PROCESSING
    let invalid = service.mark_processing("WH-EVT-TRANS").await;
    assert!(invalid.is_err());
    match invalid.err().unwrap() {
        AppError::InvalidStateTransition(msg) => {
            assert!(msg.contains(
                "Cannot transition webhook event 'WH-EVT-TRANS' from PROCESSED to PROCESSING"
            ));
        }
        other => panic!("Expected InvalidStateTransition, got {:?}", other),
    }

    // 5. Invalid transition: PROCESSED cannot transition to FAILED
    let invalid_fail = service.mark_failed("WH-EVT-TRANS", "Some error").await;
    assert!(invalid_fail.is_err());
    assert!(matches!(
        invalid_fail.err().unwrap(),
        AppError::InvalidStateTransition(_)
    ));
}

#[tokio::test]
async fn test_retry_count_increment() {
    let (_pool, service) = setup_test_db().await;
    let (event, raw_payload) = create_sample_webhook_event("WH-EVT-RETRY", "PP-RETRY");

    service
        .persist_event(&event, "trans", &raw_payload)
        .await
        .unwrap();
    service.mark_processing("WH-EVT-RETRY").await.unwrap();

    // Schedule retry #1
    let retry1 = service
        .schedule_retry("WH-EVT-RETRY", "Transient connection reset")
        .await
        .unwrap();
    assert_eq!(retry1.processing_status, WebhookProcessingStatus::Retry);
    assert_eq!(retry1.retry_count, 1);
    assert_eq!(
        retry1.last_error.as_deref(),
        Some("Transient connection reset")
    );
    assert!(retry1.next_retry_at.is_some());

    // Transition back to PROCESSING for attempt #2
    service.mark_processing("WH-EVT-RETRY").await.unwrap();

    // Schedule retry #2
    let retry2 = service
        .schedule_retry("WH-EVT-RETRY", "Service unavailable")
        .await
        .unwrap();
    assert_eq!(retry2.retry_count, 2);
    assert_eq!(retry2.last_error.as_deref(), Some("Service unavailable"));
}

#[tokio::test]
async fn test_exponential_backoff_calculation() {
    // Check pure formula
    assert_eq!(calculate_retry_delay_seconds(0), 5);
    assert_eq!(calculate_retry_delay_seconds(1), 10);
    assert_eq!(calculate_retry_delay_seconds(2), 20);
    assert_eq!(calculate_retry_delay_seconds(3), 40);
    assert_eq!(calculate_retry_delay_seconds(4), 80);
    assert_eq!(calculate_retry_delay_seconds(10), 3600); // capped at 1h

    // Check DB scheduled retry timestamp
    let (_pool, service) = setup_test_db().await;
    let (event, raw_payload) = create_sample_webhook_event("WH-EVT-BACKOFF", "PP-BO");

    service
        .persist_event(&event, "trans", &raw_payload)
        .await
        .unwrap();
    service.mark_processing("WH-EVT-BACKOFF").await.unwrap();

    let before = chrono::Utc::now();
    let retry_rec = service
        .schedule_retry("WH-EVT-BACKOFF", "Network blip")
        .await
        .unwrap();
    let after = chrono::Utc::now();

    let next_retry_str = retry_rec.next_retry_at.unwrap();
    let scheduled_time = chrono::DateTime::parse_from_rfc3339(&next_retry_str)
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Initial delay is 5 seconds
    let expected_min = before + chrono::Duration::seconds(5);
    let expected_max = after + chrono::Duration::seconds(5);

    assert!(scheduled_time >= expected_min);
    assert!(scheduled_time <= expected_max);
}

#[tokio::test]
async fn test_maximum_retry_handling() {
    let (_pool, service) = setup_test_db().await;
    let (event, raw_payload) = create_sample_webhook_event("WH-EVT-MAX-RETRY", "PP-MAX");

    service
        .persist_event(&event, "trans", &raw_payload)
        .await
        .unwrap();

    // Default max_retries is 5. Drive it through 4 retries:
    for i in 1..5 {
        service.mark_processing("WH-EVT-MAX-RETRY").await.unwrap();
        let rec = service
            .schedule_retry("WH-EVT-MAX-RETRY", &format!("Failure #{}", i))
            .await
            .unwrap();
        assert_eq!(rec.processing_status, WebhookProcessingStatus::Retry);
        assert_eq!(rec.retry_count, i);
    }

    // 5th failure: exceeds max_retries (5) -> must transition to FAILED
    service.mark_processing("WH-EVT-MAX-RETRY").await.unwrap();
    let final_rec = service
        .schedule_retry("WH-EVT-MAX-RETRY", "Persistent database deadlock")
        .await
        .unwrap();

    assert_eq!(final_rec.processing_status, WebhookProcessingStatus::Failed);
    assert_eq!(final_rec.retry_count, 5);
    assert!(final_rec.processed_at.is_some());
    assert!(final_rec
        .last_error
        .unwrap()
        .contains("Max retries (5) exceeded"));

    // Event is NOT deleted; failure details are preserved
    let persisted = service
        .get_event_by_id("WH-EVT-MAX-RETRY")
        .await
        .unwrap()
        .expect("Failed event must remain in database");
    assert_eq!(persisted.processing_status, WebhookProcessingStatus::Failed);
}

#[tokio::test]
async fn test_invalid_status_rejection() {
    let (pool, _service) = setup_test_db().await;

    // Direct SQL insertion of an invalid processing_status
    let res = sqlx::query(
        r#"
        INSERT INTO webhook_events (
            event_id, event_type, transmission_id, processing_status,
            retry_count, max_retries, raw_payload, payload_hash, created_at
        )
        VALUES ('WH-INVALID', 'TEST', 'trans-1', 'NOT_A_VALID_STATUS', 0, 5, X'0102', 'hash', '2026-11-07T12:00:00Z');
        "#,
    )
    .execute(&pool)
    .await;

    assert!(
        res.is_err(),
        "SQLite CHECK constraint must reject invalid processing status"
    );

    // Negative retry_count must also be rejected
    let res_neg = sqlx::query(
        r#"
        INSERT INTO webhook_events (
            event_id, event_type, transmission_id, processing_status,
            retry_count, max_retries, raw_payload, payload_hash, created_at
        )
        VALUES ('WH-NEG', 'TEST', 'trans-2', 'PENDING', -1, 5, X'0102', 'hash', '2026-11-07T12:00:00Z');
        "#,
    )
    .execute(&pool)
    .await;

    assert!(
        res_neg.is_err(),
        "SQLite CHECK constraint must reject negative retry_count"
    );
}

#[tokio::test]
async fn test_concurrent_duplicate_insertion() {
    let (_pool, service) = setup_test_db().await;
    let service_arc = Arc::new(service);
    let (event, raw_payload) = create_sample_webhook_event("WH-EVT-CONCURRENT", "PP-CONC");
    let event_arc = Arc::new(event);
    let payload_arc = Arc::new(raw_payload);

    let num_tasks = 20;
    let mut tasks = JoinSet::new();

    for i in 0..num_tasks {
        let s = service_arc.clone();
        let e = event_arc.clone();
        let p = payload_arc.clone();
        let trans_id = format!("trans-concurrent-{}", i);

        tasks.spawn(async move { s.persist_event(&e, &trans_id, &p).await });
    }

    let mut inserted_count = 0;
    let mut duplicate_count = 0;

    while let Some(res) = tasks.join_next().await {
        let persist_result = res.unwrap().expect("persist_event must not return error");
        match persist_result {
            WebhookPersistenceResult::Inserted(_) => inserted_count += 1,
            WebhookPersistenceResult::Duplicate { .. } => duplicate_count += 1,
        }
    }

    // Exactly 1 task must succeed as Inserted, and all other 19 must be Duplicate
    assert_eq!(
        inserted_count, 1,
        "Exactly one concurrent insertion must succeed"
    );
    assert_eq!(
        duplicate_count,
        num_tasks - 1,
        "All other concurrent insertions must be flagged as duplicates"
    );

    // Exactly one row exists in the database
    let record = service_arc
        .get_event_by_id("WH-EVT-CONCURRENT")
        .await
        .unwrap()
        .expect("Event must be in database");

    assert_eq!(record.event_id, "WH-EVT-CONCURRENT");
    assert_eq!(record.retry_count, 0);
}
