use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;

use agentic_commerce::db::init_db;
use agentic_commerce::error::AppError;
use agentic_commerce::models::correlation::{CorrelationStatus, MatchedBy};
use agentic_commerce::models::dispute::{
    DisputeActionType, GenesisAnchorType, UpsertDisputePayload, GENESIS_HASH_DEFAULT,
};
use agentic_commerce::models::ledger::{AppendEventRequest, CreateSessionRequest};
use agentic_commerce::services::dispute_service::DisputeService;
use agentic_commerce::services::ledger::LedgerService;
use agentic_commerce::services::verification::VerificationService;

/// Helper initializing in-memory database and core services.
async fn setup_test_services() -> (
    SqlitePool,
    Arc<DisputeService>,
    Arc<VerificationService>,
    Arc<LedgerService>,
) {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let verification_service = Arc::new(VerificationService::new(pool.clone()));
    let dispute_service = Arc::new(DisputeService::new(
        pool.clone(),
        verification_service.clone(),
    ));
    let ledger_service = Arc::new(LedgerService::new(pool.clone()));

    (pool, dispute_service, verification_service, ledger_service)
}

/// Helper creating a valid finalized purchase session.
async fn create_finalized_purchase_session(
    ledger_service: &LedgerService,
    order_id: &str,
    capture_id: &str,
) -> (String, String) {
    let session = ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Purchase high-end noise cancelling headphones".to_string(),
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
                    "product_id": "prod-headphones-1",
                    "name": "Sony WH-1000XM5",
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
async fn test_01_initially_matched_dispute_uses_verified_purchase_root() {
    let (_pool, dispute_service, _v_svc, ledger_svc) = setup_test_services().await;

    let (session_id, expected_root) =
        create_finalized_purchase_session(&ledger_svc, "ORD-TEST-1", "CAP-TEST-1").await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-01".to_string(),
        session_id: Some(session_id.clone()),
        genesis_anchor_type: GenesisAnchorType::PurchaseRootHash,
        genesis_anchor_hash: expected_root.clone(),
        purchase_root_hash: Some(expected_root.clone()),
        seller_transaction_id: Some("CAP-TEST-1".to_string()),
        paypal_order_id: Some("ORD-TEST-1".to_string()),
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("MERCHANDISE_OR_SERVICE_NOT_RECEIVED".to_string()),
        dispute_amount_value: Some("399.99".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::Matched,
        matched_by: Some(MatchedBy::CaptureId),
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: Some(json!({"id": "PP-D-TEST-01"})),
    };

    let record = dispute_service.upsert_dispute(&payload).await.unwrap();

    assert_eq!(record.dispute_id, "PP-D-TEST-01");
    assert_eq!(
        record.genesis_anchor_type,
        GenesisAnchorType::PurchaseRootHash
    );
    assert_eq!(record.genesis_anchor_hash, expected_root);
    assert_eq!(record.purchase_root_hash, Some(expected_root));
    assert_eq!(record.session_id, Some(session_id));
    assert!(record.is_anchored_to_purchase());
    assert!(record.is_correlated());

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-01")
        .await
        .unwrap();
    assert!(v_res.valid, "Verification failed: {:?}", v_res.errors);
}

#[tokio::test]
async fn test_02_initially_unmatched_dispute_uses_zero_hash_genesis_anchor() {
    let (_pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-02".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("UNAUTHORISED".to_string()),
        dispute_amount_value: Some("150.00".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: Some(json!({"id": "PP-D-TEST-02"})),
    };

    let record = dispute_service.upsert_dispute(&payload).await.unwrap();

    assert_eq!(record.dispute_id, "PP-D-TEST-02");
    assert_eq!(record.genesis_anchor_type, GenesisAnchorType::GenesisHash);
    assert_eq!(record.genesis_anchor_hash, GENESIS_HASH_DEFAULT);
    assert_eq!(record.purchase_root_hash, None);
    assert_eq!(record.session_id, None);
    assert!(!record.is_anchored_to_purchase());
    assert!(!record.is_correlated());

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-02")
        .await
        .unwrap();
    assert!(v_res.valid, "Verification failed: {:?}", v_res.errors);
}

#[tokio::test]
async fn test_03_initial_audit_step_sequence_zero_and_previous_hash() {
    let (_pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-03".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "WAITING_FOR_SELLER_RESPONSE".to_string(),
        dispute_lifecycle_stage: None,
        dispute_channel: None,
        reason: None,
        dispute_amount_value: None,
        dispute_amount_currency: None,
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    let record = dispute_service.upsert_dispute(&payload).await.unwrap();
    let steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-03")
        .await
        .unwrap();

    assert_eq!(steps.len(), 1);
    let step0 = &steps[0];
    assert_eq!(step0.sequence_index, 0);
    assert_eq!(step0.action_type, DisputeActionType::DISPUTE_IMPORTED);
    assert_eq!(step0.previous_hash, GENESIS_HASH_DEFAULT);
    assert_eq!(step0.current_hash, record.root_hash.unwrap());
}

#[tokio::test]
async fn test_04_audit_steps_form_continuous_hash_chain() {
    let (_pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let mut payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-04".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("MERCHANDISE_OR_SERVICE_NOT_RECEIVED".to_string()),
        dispute_amount_value: Some("100.00".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: Some(json!({"event": 1})),
    };

    dispute_service.upsert_dispute(&payload).await.unwrap();

    // Event 2: Newer timestamp
    payload.status = "WAITING_FOR_SELLER_RESPONSE".to_string();
    payload.dispute_update_time = Some("2026-11-08T12:00:00Z".to_string());
    payload.raw_event_payload = Some(json!({"event": 2}));
    dispute_service.upsert_dispute(&payload).await.unwrap();

    // Event 3: Even newer timestamp
    payload.status = "RESOLVED".to_string();
    payload.dispute_update_time = Some("2026-11-08T15:00:00Z".to_string());
    payload.raw_event_payload = Some(json!({"event": 3}));
    let final_record = dispute_service.upsert_dispute(&payload).await.unwrap();

    let steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-04")
        .await
        .unwrap();
    assert_eq!(steps.len(), 3);

    assert_eq!(steps[0].sequence_index, 0);
    assert_eq!(steps[1].sequence_index, 1);
    assert_eq!(steps[2].sequence_index, 2);

    assert_eq!(steps[0].previous_hash, GENESIS_HASH_DEFAULT);
    assert_eq!(steps[1].previous_hash, steps[0].current_hash);
    assert_eq!(steps[2].previous_hash, steps[1].current_hash);
    assert_eq!(final_record.root_hash.unwrap(), steps[2].current_hash);

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-04")
        .await
        .unwrap();
    assert!(v_res.valid, "Chain verification failed: {:?}", v_res.errors);
    assert_eq!(v_res.total_steps, 3);
}

#[tokio::test]
async fn test_05_tampered_audit_payload_is_detected() {
    let (pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-05".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: None,
        dispute_channel: None,
        reason: None,
        dispute_amount_value: None,
        dispute_amount_currency: None,
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service.upsert_dispute(&payload).await.unwrap();

    // Tamper payload in SQLite directly
    sqlx::query(
        "UPDATE dispute_audit_steps SET payload = '{\"tampered\":true}' WHERE dispute_id = 'PP-D-TEST-05' AND sequence_index = 0;",
    )
    .execute(&pool)
    .await
    .unwrap();

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-05")
        .await
        .unwrap();
    assert!(!v_res.valid);
    assert!(!v_res.errors.is_empty());
    assert!(v_res.errors.iter().any(|e| e.contains("mismatch")));
}

#[tokio::test]
async fn test_06_dispute_terminal_root_hash_mismatch_is_detected() {
    let (pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-06".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: None,
        dispute_channel: None,
        reason: None,
        dispute_amount_value: None,
        dispute_amount_currency: None,
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service.upsert_dispute(&payload).await.unwrap();

    // Tamper terminal root_hash in disputes table
    sqlx::query(
        "UPDATE disputes SET root_hash = '111122223333444455556666777788889999aaaabbbbccccddddeeeeffff0000' WHERE dispute_id = 'PP-D-TEST-06';",
    )
    .execute(&pool)
    .await
    .unwrap();

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-06")
        .await
        .unwrap();
    assert!(!v_res.valid);
    assert!(v_res.errors.iter().any(|e| e.contains("root_hash")));
}

#[tokio::test]
async fn test_07_genesis_anchor_mismatch_is_detected() {
    let (pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-07".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: None,
        dispute_channel: None,
        reason: None,
        dispute_amount_value: None,
        dispute_amount_currency: None,
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service.upsert_dispute(&payload).await.unwrap();

    // Tamper initial step previous_hash
    sqlx::query(
        "UPDATE dispute_audit_steps SET previous_hash = 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef' WHERE dispute_id = 'PP-D-TEST-07' AND sequence_index = 0;",
    )
    .execute(&pool)
    .await
    .unwrap();

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-07")
        .await
        .unwrap();
    assert!(!v_res.valid);
    assert!(v_res
        .errors
        .iter()
        .any(|e| e.contains("genesis_anchor_hash") || e.contains("mismatch")));
}

#[tokio::test]
async fn test_08_late_correlation_appends_step_preserving_genesis_anchor() {
    let (_pool, dispute_service, _v_svc, ledger_svc) = setup_test_services().await;

    // 1. Initial unmatched dispute
    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-08".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: Some("CAP-LATE-1".to_string()),
        paypal_order_id: Some("ORD-LATE-1".to_string()),
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("UNAUTHORISED".to_string()),
        dispute_amount_value: Some("399.99".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    let initial_dispute = dispute_service.upsert_dispute(&payload).await.unwrap();
    let initial_steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-08")
        .await
        .unwrap();
    assert_eq!(initial_steps.len(), 1);
    let step0 = initial_steps[0].clone();

    // 2. Valid purchase session created later
    let (session_id, expected_root) =
        create_finalized_purchase_session(&ledger_svc, "ORD-LATE-1", "CAP-LATE-1").await;

    // 3. Late correlation
    let updated_dispute = dispute_service
        .correlate_dispute_late("PP-D-TEST-08", &session_id, MatchedBy::CaptureId)
        .await
        .unwrap();

    // Verify genesis anchor is UNTOUCHED
    assert_eq!(
        updated_dispute.genesis_anchor_type,
        GenesisAnchorType::GenesisHash
    );
    assert_eq!(
        updated_dispute.genesis_anchor_hash,
        initial_dispute.genesis_anchor_hash
    );
    assert_eq!(updated_dispute.session_id, Some(session_id.clone()));
    assert_eq!(
        updated_dispute.purchase_root_hash,
        Some(expected_root.clone())
    );
    assert_eq!(
        updated_dispute.correlation_status,
        CorrelationStatus::Matched
    );

    // Verify audit steps
    let steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-08")
        .await
        .unwrap();
    assert_eq!(steps.len(), 2);

    // Step 0 remains identical
    assert_eq!(steps[0].id, step0.id);
    assert_eq!(steps[0].previous_hash, step0.previous_hash);
    assert_eq!(steps[0].current_hash, step0.current_hash);

    // Step 1 is DISPUTE_CORRELATED
    assert_eq!(steps[1].sequence_index, 1);
    assert_eq!(steps[1].action_type, DisputeActionType::DISPUTE_CORRELATED);
    assert_eq!(steps[1].previous_hash, step0.current_hash);
    assert_eq!(steps[1].current_hash, updated_dispute.root_hash.unwrap());

    // Cryptographic verification passes
    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-08")
        .await
        .unwrap();
    assert!(v_res.valid, "Verification failed: {:?}", v_res.errors);
}

#[tokio::test]
async fn test_09_late_correlation_rejects_invalid_purchase_ledger() {
    let (pool, dispute_service, _v_svc, ledger_svc) = setup_test_services().await;

    let (session_id, _root) =
        create_finalized_purchase_session(&ledger_svc, "ORD-BAD-1", "CAP-BAD-1").await;

    // Tamper the session ledger
    sqlx::query(
        "UPDATE decision_steps SET payload = '{\"tampered\":true}' WHERE session_id = ? AND sequence_index = 0;",
    )
    .bind(&session_id)
    .execute(&pool)
    .await
    .unwrap();

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-09".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: Some("CAP-BAD-1".to_string()),
        paypal_order_id: Some("ORD-BAD-1".to_string()),
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: None,
        dispute_channel: None,
        reason: None,
        dispute_amount_value: None,
        dispute_amount_currency: None,
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service.upsert_dispute(&payload).await.unwrap();

    // Late correlation must be rejected
    let result = dispute_service
        .correlate_dispute_late("PP-D-TEST-09", &session_id, MatchedBy::CaptureId)
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        AppError::DisputeAuditIntegrity(msg) => {
            assert!(msg.contains("failed ledger verification"));
        }
        other => panic!("Expected DisputeAuditIntegrity, got: {:?}", other),
    }

    // Dispute remains unlinked
    let dispute = dispute_service
        .get_dispute("PP-D-TEST-09")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dispute.session_id, None);
    assert_eq!(dispute.purchase_root_hash, None);
}

#[tokio::test]
async fn test_10_newer_timestamp_advances_dispute() {
    let (_pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload1 = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-10".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("MERCHANDISE_OR_SERVICE_NOT_RECEIVED".to_string()),
        dispute_amount_value: Some("150.00".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service.upsert_dispute(&payload1).await.unwrap();

    let mut payload2 = payload1.clone();
    payload2.status = "RESOLVED".to_string();
    payload2.dispute_lifecycle_stage = Some("CHARGEBACK".to_string());
    payload2.dispute_update_time = Some("2026-11-08T12:00:00Z".to_string());

    let updated = dispute_service.upsert_dispute(&payload2).await.unwrap();

    assert_eq!(updated.status, "RESOLVED");
    assert_eq!(
        updated.dispute_lifecycle_stage,
        Some("CHARGEBACK".to_string())
    );
    assert_eq!(
        updated.dispute_update_time,
        Some("2026-11-08T12:00:00Z".to_string())
    );

    let steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-10")
        .await
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].action_type, DisputeActionType::DISPUTE_UPDATED);

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-10")
        .await
        .unwrap();
    assert!(v_res.valid);
}

#[tokio::test]
async fn test_11_older_timestamp_preserves_current_state_and_records_stale_event() {
    let (_pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload1 = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-11".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "RESOLVED".to_string(),
        dispute_lifecycle_stage: Some("CHARGEBACK".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("UNAUTHORISED".to_string()),
        dispute_amount_value: Some("200.00".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T15:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service.upsert_dispute(&payload1).await.unwrap();

    // Deliver an older event that arrived late / out of order
    let mut stale_payload = payload1.clone();
    stale_payload.status = "UNDER_REVIEW".to_string();
    stale_payload.dispute_lifecycle_stage = Some("INQUIRY".to_string());
    stale_payload.dispute_update_time = Some("2026-11-08T11:00:00Z".to_string());

    let record = dispute_service
        .upsert_dispute(&stale_payload)
        .await
        .unwrap();

    // Verify current state is preserved!
    assert_eq!(record.status, "RESOLVED");
    assert_eq!(
        record.dispute_lifecycle_stage,
        Some("CHARGEBACK".to_string())
    );
    assert_eq!(
        record.dispute_update_time,
        Some("2026-11-08T15:00:00Z".to_string())
    );

    // Verify out-of-order step was appended
    let steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-11")
        .await
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1].action_type,
        DisputeActionType::DISPUTE_OUT_OF_ORDER_RECEIVED
    );

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-11")
        .await
        .unwrap();
    assert!(v_res.valid);
}

#[tokio::test]
async fn test_12_missing_authoritative_timestamp_triggers_manual_review() {
    let (_pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    // 1. Initial creation with valid timestamp
    let payload1 = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-12".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("UNAUTHORISED".to_string()),
        dispute_amount_value: Some("50.00".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service.upsert_dispute(&payload1).await.unwrap();

    // 2. Incoming update event without authoritative dispute_update_time
    let mut ambiguous_payload = payload1.clone();
    ambiguous_payload.status = "RESOLVED".to_string();
    ambiguous_payload.dispute_update_time = None;

    let updated = dispute_service
        .upsert_dispute(&ambiguous_payload)
        .await
        .unwrap();

    // Current state preserved, manual review flagged
    assert_eq!(updated.status, "UNDER_REVIEW");
    assert_eq!(
        updated.correlation_status,
        CorrelationStatus::ManualReviewRequired
    );

    let steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-12")
        .await
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1].action_type,
        DisputeActionType::DISPUTE_TIMESTAMP_AMBIGUOUS_RECEIVED
    );

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-12")
        .await
        .unwrap();
    assert!(v_res.valid);
}

#[tokio::test]
async fn test_13_equal_timestamp_identical_payload_is_noop() {
    let (_pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-13".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("UNAUTHORISED".to_string()),
        dispute_amount_value: Some("120.00".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: Some(json!({"id": "PP-D-TEST-13", "dup": 1})),
    };

    let rec1 = dispute_service.upsert_dispute(&payload).await.unwrap();
    let steps1 = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-13")
        .await
        .unwrap();
    assert_eq!(steps1.len(), 1);

    // Re-send identical payload with equal timestamp
    let rec2 = dispute_service.upsert_dispute(&payload).await.unwrap();
    assert_eq!(rec1, rec2);

    let steps2 = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-13")
        .await
        .unwrap();
    assert_eq!(
        steps2.len(),
        1,
        "Expected no new audit step for identical no-op"
    );
}

#[tokio::test]
async fn test_14_equal_timestamp_conflicting_payload_triggers_manual_review() {
    let (_pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload1 = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-14".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("UNAUTHORISED".to_string()),
        dispute_amount_value: Some("80.00".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: Some(json!({"version": 1})),
    };

    dispute_service.upsert_dispute(&payload1).await.unwrap();

    // Send conflicting payload with the same timestamp
    let mut conflicting_payload = payload1.clone();
    conflicting_payload.status = "RESOLVED".to_string(); // Conflicting status!
    conflicting_payload.raw_event_payload = Some(json!({"version": 2}));

    let updated = dispute_service
        .upsert_dispute(&conflicting_payload)
        .await
        .unwrap();

    // Existing status preserved, manual review flagged
    assert_eq!(updated.status, "UNDER_REVIEW");
    assert_eq!(
        updated.correlation_status,
        CorrelationStatus::ManualReviewRequired
    );

    let steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-14")
        .await
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1].action_type,
        DisputeActionType::DISPUTE_CONCURRENT_CONFLICT_DETECTED
    );

    let v_res = dispute_service
        .verify_dispute_chain("PP-D-TEST-14")
        .await
        .unwrap();
    assert!(v_res.valid);
}

#[tokio::test]
async fn test_15_dual_chain_verification_detects_purchase_ledger_tampering() {
    let (pool, dispute_service, _v_svc, ledger_svc) = setup_test_services().await;

    let (session_id, expected_root) =
        create_finalized_purchase_session(&ledger_svc, "ORD-DUAL-1", "CAP-DUAL-1").await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-15".to_string(),
        session_id: Some(session_id.clone()),
        genesis_anchor_type: GenesisAnchorType::PurchaseRootHash,
        genesis_anchor_hash: expected_root.clone(),
        purchase_root_hash: Some(expected_root.clone()),
        seller_transaction_id: Some("CAP-DUAL-1".to_string()),
        paypal_order_id: Some("ORD-DUAL-1".to_string()),
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("MERCHANDISE_OR_SERVICE_NOT_RECEIVED".to_string()),
        dispute_amount_value: Some("399.99".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::Matched,
        matched_by: Some(MatchedBy::CaptureId),
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service.upsert_dispute(&payload).await.unwrap();

    // Dual-chain check before tampering passes
    let before_tamper = dispute_service
        .verify_dispute_and_purchase_ledger("PP-D-TEST-15")
        .await
        .unwrap();
    assert!(before_tamper.valid);

    // Tamper purchase session ledger
    sqlx::query(
        "UPDATE decision_steps SET payload = '{\"corrupted\":true}' WHERE session_id = ? AND sequence_index = 1;",
    )
    .bind(&session_id)
    .execute(&pool)
    .await
    .unwrap();

    // Dual-chain check detects the tampering
    let after_tamper = dispute_service
        .verify_dispute_and_purchase_ledger("PP-D-TEST-15")
        .await
        .unwrap();
    assert!(!after_tamper.valid);
    assert!(
        after_tamper
            .errors
            .iter()
            .any(|e| e.contains("Dual-chain check failed")
                || e.contains("ledger verification failed"))
    );
}

#[tokio::test]
async fn test_16_transaction_rollback_leaves_no_partial_dispute_mutations() {
    let (pool, dispute_service, _v_svc, _ledger_svc) = setup_test_services().await;

    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-16".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: None,
        dispute_channel: None,
        reason: None,
        dispute_amount_value: None,
        dispute_amount_currency: None,
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    // Open transaction, call upsert_dispute_tx, then rollback
    let mut tx = pool.begin().await.unwrap();
    let record = dispute_service
        .upsert_dispute_tx(&mut tx, &payload)
        .await
        .unwrap();
    assert_eq!(record.dispute_id, "PP-D-TEST-16");
    tx.rollback().await.unwrap();

    // Confirm that NO dispute record or audit step was persisted
    let query_dispute = dispute_service.get_dispute("PP-D-TEST-16").await.unwrap();
    assert_eq!(query_dispute, None);

    let query_steps = dispute_service
        .get_dispute_audit_steps("PP-D-TEST-16")
        .await
        .unwrap();
    assert!(query_steps.is_empty());
}

#[tokio::test]
async fn test_17_rejects_unfinalized_purchase_session() {
    let (_pool, dispute_service, _v_svc, ledger_svc) = setup_test_services().await;

    // Create session and append events WITHOUT calling finalize_session
    let session = ledger_svc
        .create_session(CreateSessionRequest {
            initial_prompt: "Buy Sony WH-1000XM5".to_string(),
        })
        .await
        .unwrap();

    ledger_svc
        .set_paypal_order_id(&session.session_id, "ORD-UNFINALIZED-1")
        .await
        .unwrap();

    ledger_svc
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "PRODUCT_SELECTED".to_string(),
                payload: json!({"name": "Sony Headphones"}),
            },
        )
        .await
        .unwrap();

    // 1. Initial ingestion with unfinalized session fails
    let payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-17".to_string(),
        session_id: Some(session.session_id.clone()),
        genesis_anchor_type: GenesisAnchorType::PurchaseRootHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: Some("ORD-UNFINALIZED-1".to_string()),
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: None,
        dispute_channel: None,
        reason: None,
        dispute_amount_value: None,
        dispute_amount_currency: None,
        correlation_status: CorrelationStatus::Matched,
        matched_by: Some(MatchedBy::OrderId),
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    let result1 = dispute_service.upsert_dispute(&payload).await;
    assert!(result1.is_err());
    match result1.unwrap_err() {
        AppError::DisputeAuditIntegrity(msg) => {
            assert!(msg.contains("not finalized"));
        }
        other => panic!("Expected DisputeAuditIntegrity, got {:?}", other),
    }

    // 2. Late correlation with unfinalized session fails
    let unmatched_payload = UpsertDisputePayload {
        dispute_id: "PP-D-TEST-17-UNMATCHED".to_string(),
        session_id: None,
        genesis_anchor_type: GenesisAnchorType::GenesisHash,
        genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
        purchase_root_hash: None,
        seller_transaction_id: None,
        paypal_order_id: None,
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: None,
        dispute_channel: None,
        reason: None,
        dispute_amount_value: None,
        dispute_amount_currency: None,
        correlation_status: CorrelationStatus::NotFound,
        matched_by: None,
        dispute_create_time: Some("2026-11-08T10:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-08T10:00:00Z".to_string()),
        raw_event_payload: None,
    };

    dispute_service
        .upsert_dispute(&unmatched_payload)
        .await
        .unwrap();

    let result2 = dispute_service
        .correlate_dispute_late(
            "PP-D-TEST-17-UNMATCHED",
            &session.session_id,
            MatchedBy::OrderId,
        )
        .await;

    assert!(result2.is_err());
    match result2.unwrap_err() {
        AppError::DisputeAuditIntegrity(msg) => {
            assert!(msg.contains("not finalized"));
        }
        other => panic!("Expected DisputeAuditIntegrity, got {:?}", other),
    }
}
