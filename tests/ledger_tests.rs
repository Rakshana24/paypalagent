use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

use agentic_commerce::config::Config;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::ledger::hash::{
    compute_event_hash, to_canonical_json_bytes, to_canonical_json_string, GENESIS_HASH,
};
use agentic_commerce::models::{
    AppendEventRequest, AppendEventResponse, CreateSessionRequest, CreateSessionResponse,
    DecisionStep, FinalizeSessionResponse, Session, VerificationResult, VerificationStatus,
};
use agentic_commerce::services::{
    CatalogService, LedgerService, PayPalAuthService, PayPalOrdersService, ShoppingAgentService,
    VerificationService,
};
use agentic_commerce::AppState;

/// Helper to create an in-memory test database and router
async fn setup_test_app() -> (axum::Router, sqlx::SqlitePool) {
    let pool = init_db("sqlite::memory:")
        .await
        .expect("Failed to init database schema");

    let config = Arc::new(Config {
        paypal_client_id: "test_client_id".to_string(),
        paypal_client_secret: "test_client_secret".to_string(),
        paypal_base_url: "https://api-m.sandbox.paypal.com".to_string(),
        port: 5000,
        rust_log: "info".to_string(),
        database_url: "sqlite::memory:".to_string(),
        paypal_webhook_id: None,
    });

    let http_client =
        create_http_client(&HttpClientConfig::default()).expect("Failed to build HTTP client");
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
    let webhook_verifier = Arc::new(agentic_commerce::services::WebhookVerifierService::new(
        config.clone(),
        http_client.clone(),
        auth_service.clone(),
    ));
    let webhook_persistence = Arc::new(agentic_commerce::services::WebhookPersistenceService::new(
        pool.clone(),
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
        webhook_persistence,
        webhook_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
    };

    let router = agentic_commerce::create_router(state);
    (router, pool)
}

// =========================================================================
// 1. HASH ENGINE & CANONICALIZATION TESTS
// =========================================================================

#[test]
fn test_genesis_hash_is_64_zeros() {
    assert_eq!(GENESIS_HASH.len(), 64);
    assert_eq!(
        GENESIS_HASH,
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
}

#[test]
fn test_canonical_json_key_order_independence() {
    let json_a: Value =
        serde_json::from_str(r#"{"zebra": 1, "apple": 2, "middle": {"b": 2, "a": 1}}"#).unwrap();
    let json_b: Value =
        serde_json::from_str(r#"{"apple": 2, "middle": {"a": 1, "b": 2}, "zebra": 1}"#).unwrap();

    let bytes_a = to_canonical_json_bytes(&json_a).unwrap();
    let bytes_b = to_canonical_json_bytes(&json_b).unwrap();

    assert_eq!(bytes_a, bytes_b);
    let str_a = to_canonical_json_string(&json_a).unwrap();
    assert_eq!(str_a, r#"{"apple":2,"middle":{"a":1,"b":2},"zebra":1}"#);
}

#[test]
fn test_deterministic_hash_computation() {
    let seq = 0;
    let action = "SEARCH_PRODUCTS";
    let payload = json!({"query": "mechanical keyboard", "max_price": 150});
    let canonical_payload = to_canonical_json_bytes(&payload).unwrap();
    let prev_hash = GENESIS_HASH;

    let hash1 = compute_event_hash(seq, action, &canonical_payload, prev_hash);
    let hash2 = compute_event_hash(seq, action, &canonical_payload, prev_hash);

    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 64);
}

#[test]
fn test_hash_sensitivity_to_each_field() {
    let seq = 1;
    let action = "SELECT_ITEM";
    let payload = json!({"item_id": "item_99"});
    let canonical_payload = to_canonical_json_bytes(&payload).unwrap();
    let prev_hash = GENESIS_HASH;

    let base_hash = compute_event_hash(seq, action, &canonical_payload, prev_hash);

    // Change sequence index
    let diff_seq = compute_event_hash(2, action, &canonical_payload, prev_hash);
    assert_ne!(base_hash, diff_seq);

    // Change action type
    let diff_action = compute_event_hash(seq, "ADD_TO_CART", &canonical_payload, prev_hash);
    assert_ne!(base_hash, diff_action);

    // Change payload
    let diff_payload_json = json!({"item_id": "item_100"});
    let diff_payload = to_canonical_json_bytes(&diff_payload_json).unwrap();
    let diff_hash_payload = compute_event_hash(seq, action, &diff_payload, prev_hash);
    assert_ne!(base_hash, diff_hash_payload);

    // Change previous hash
    let fake_prev = "1111111111111111111111111111111111111111111111111111111111111111";
    let diff_prev = compute_event_hash(seq, action, &canonical_payload, fake_prev);
    assert_ne!(base_hash, diff_prev);
}

// =========================================================================
// 2. DATABASE SCHEMA & CONSTRAINT TESTS
// =========================================================================

#[tokio::test]
async fn test_database_foreign_key_enforcement() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Attempt to insert a decision step with a non-existent session_id
    let res = sqlx::query(
        r#"
        INSERT INTO decision_steps (id, session_id, sequence_index, action_type, payload, previous_hash, current_hash, created_at)
        VALUES ('step1', 'nonexistent_session', 0, 'INITIAL_PROMPT', '{}', '0000', '1111', '2026-11-07T12:00:00Z');
        "#,
    )
    .execute(&pool)
    .await;

    assert!(
        res.is_err(),
        "Foreign key constraint must reject orphaned decision steps"
    );
}

#[tokio::test]
async fn test_database_unique_sequence_constraint() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let ledger = LedgerService::new(pool.clone());
    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Buy wireless mouse".to_string(),
        })
        .await
        .unwrap();

    // Sequence index 0 is already taken by the initial prompt
    // Attempting to manually insert another sequence 0 for the same session must fail
    let res = sqlx::query(
        r#"
        INSERT INTO decision_steps (id, session_id, sequence_index, action_type, payload, previous_hash, current_hash, created_at)
        VALUES ('step_conflict', ?, 0, 'DUPLICATE_STEP', '{}', '0000', '2222', '2026-11-07T12:00:00Z');
        "#,
    )
    .bind(&session.session_id)
    .execute(&pool)
    .await;

    assert!(
        res.is_err(),
        "Unique constraint (session_id, sequence_index) must reject duplicate indices"
    );
}

#[tokio::test]
async fn test_data_persistence_across_reconnection() {
    let temp_file = std::env::temp_dir().join(format!("test_persist_{}.db", uuid::Uuid::new_v4()));
    let db_url = format!(
        "sqlite:{}?mode=rwc",
        temp_file.to_str().unwrap().replace('\\', "/")
    );

    let session_id;
    let event_hash;

    // Session 1: open pool, create session, append event, close pool
    {
        let pool = init_db(&db_url).await.unwrap();
        let ledger = LedgerService::new(pool.clone());

        let session = ledger
            .create_session(CreateSessionRequest {
                initial_prompt: "Find USB-C cable".to_string(),
            })
            .await
            .unwrap();
        session_id = session.session_id;

        let event = ledger
            .append_event(
                &session_id,
                AppendEventRequest {
                    action_type: "SEARCH".to_string(),
                    payload: json!({"term": "usb-c 6ft"}),
                },
            )
            .await
            .unwrap();
        event_hash = event.current_hash;
        pool.close().await;
    }

    // Session 2: reconnect to same file and verify data persists
    {
        let pool = init_db(&db_url).await.unwrap();
        let ledger = LedgerService::new(pool.clone());
        let verifier = VerificationService::new(pool.clone());

        let retrieved = ledger.get_session(&session_id).await.unwrap();
        assert_eq!(retrieved.session_id, session_id);
        assert_eq!(retrieved.initial_prompt, "Find USB-C cable");

        let events = ledger.get_session_events(&session_id).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].current_hash, event_hash);

        let verification = verifier.verify_session(&session_id).await.unwrap();
        assert_eq!(verification.status, VerificationStatus::Valid);
        assert_eq!(verification.verified_steps, 2);
        pool.close().await;
    }

    let _ = std::fs::remove_file(temp_file);
}

// =========================================================================
// 3. INTEGRITY VERIFICATION SERVICE TESTS
// =========================================================================

#[tokio::test]
async fn test_verification_valid_chain() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Order green tea".to_string(),
        })
        .await
        .unwrap();

    for i in 1..=5 {
        ledger
            .append_event(
                &session.session_id,
                AppendEventRequest {
                    action_type: format!("STEP_{}", i),
                    payload: json!({"step": i}),
                },
            )
            .await
            .unwrap();
    }

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Valid);
    assert_eq!(result.total_steps, 6);
    assert_eq!(result.verified_steps, 6);
    assert!(result.first_invalid_sequence_index.is_none());
}

#[tokio::test]
async fn test_verification_detects_tampered_payload() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Buy running shoes".to_string(),
        })
        .await
        .unwrap();

    ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "ADD_TO_CART".to_string(),
                payload: json!({"price": 100}),
            },
        )
        .await
        .unwrap();

    // Tamper directly with the SQLite database row (change price to 10)
    sqlx::query(
        r#"
        UPDATE decision_steps
        SET payload = '{"price":10}'
        WHERE session_id = ? AND sequence_index = 1;
        "#,
    )
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    assert_eq!(result.first_invalid_sequence_index, Some(1));
    assert_eq!(result.error_code, Some("DIGEST_MISMATCH".to_string()));
}

#[tokio::test]
async fn test_verification_detects_reordered_events() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Buy standing desk".to_string(),
        })
        .await
        .unwrap();

    ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "STEP_1".to_string(),
                payload: json!({"step": 1}),
            },
        )
        .await
        .unwrap();

    ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "STEP_2".to_string(),
                payload: json!({"step": 2}),
            },
        )
        .await
        .unwrap();

    // Swap sequence indices 1 and 2 in SQLite
    sqlx::query(
        r#"
        UPDATE decision_steps SET sequence_index = 99 WHERE session_id = ? AND sequence_index = 1;
        "#,
    )
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        UPDATE decision_steps SET sequence_index = 1 WHERE session_id = ? AND sequence_index = 2;
        "#,
    )
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        UPDATE decision_steps SET sequence_index = 2 WHERE session_id = ? AND sequence_index = 99;
        "#,
    )
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    assert!(result.first_invalid_sequence_index.is_some());
}

#[tokio::test]
async fn test_verification_detects_root_hash_mismatch() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Buy coffee maker".to_string(),
        })
        .await
        .unwrap();

    ledger.finalize_session(&session.session_id).await.unwrap();

    // Tamper with the session root_hash in SQLite
    let fake_root = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    sqlx::query("UPDATE sessions SET root_hash = ? WHERE session_id = ?")
        .bind(fake_root)
        .bind(&session.session_id)
        .execute(&pool)
        .await
        .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    assert_eq!(result.error_code, Some("ROOT_HASH_MISMATCH".to_string()));
}

// =========================================================================
// 4. CONCURRENCY & SERIALIZATION TESTS
// =========================================================================

#[tokio::test]
async fn test_concurrent_appends_maintain_integrity() {
    let (_, pool) = setup_test_app().await;
    let ledger = Arc::new(LedgerService::new(pool.clone()));
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Stress test concurrency".to_string(),
        })
        .await
        .unwrap();

    let session_id = session.session_id;

    // Spawn 10 concurrent tasks appending events
    let mut handles = Vec::new();
    for i in 0..10 {
        let ledger_clone = ledger.clone();
        let sid = session_id.clone();
        handles.push(tokio::spawn(async move {
            ledger_clone
                .append_event(
                    &sid,
                    AppendEventRequest {
                        action_type: format!("CONCURRENT_STEP_{}", i),
                        payload: json!({"worker_id": i}),
                    },
                )
                .await
        }));
    }

    for handle in handles {
        let res = handle.await.unwrap();
        assert!(
            res.is_ok(),
            "Concurrent append must succeed: {:?}",
            res.err()
        );
    }

    // Verify all 11 events (0 = initial, 1..=10 = concurrent)
    let events = ledger.get_session_events(&session_id).await.unwrap();
    assert_eq!(events.len(), 11);

    // Verify monotonic continuity of sequence indices
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.sequence_index, i as i64);
    }

    // Cryptographic verification must pass cleanly
    let verify = verifier.verify_session(&session_id).await.unwrap();
    assert_eq!(verify.status, VerificationStatus::Valid);
    assert_eq!(verify.verified_steps, 11);
}

// =========================================================================
// 5. REST API END-TO-END TESTS
// =========================================================================

#[tokio::test]
async fn test_api_session_lifecycle() {
    let (app, _) = setup_test_app().await;

    // 1. POST /api/sessions -> 201 Created
    let create_payload = json!({
        "initial_prompt": "Find a noise-cancelling headset under $200"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/sessions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&create_payload).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let session_res: CreateSessionResponse = serde_json::from_slice(&body).unwrap();
    let session_id = session_res.session_id;

    // 2. POST /api/sessions/{session_id}/events -> 201 Created
    let event1_payload = json!({
        "action_type": "SEARCH_CATALOG",
        "payload": {
            "query": "noise cancelling headset",
            "category": "electronics"
        }
    });
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/sessions/{}/events", session_id))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&event1_payload).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let append_res: AppendEventResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(append_res.sequence_index, 1);
    assert_eq!(append_res.previous_hash, session_res.initial_hash);

    // 3. GET /api/sessions/{session_id} -> 200 OK
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/sessions/{}", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let get_session: Session = serde_json::from_slice(&body).unwrap();
    assert_eq!(get_session.session_id, session_id);
    assert_eq!(
        get_session.initial_prompt,
        "Find a noise-cancelling headset under $200"
    );
    assert!(get_session.root_hash.is_none());

    // 4. GET /api/sessions/{session_id}/events -> 200 OK
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/sessions/{}/events", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let events: Vec<DecisionStep> = serde_json::from_slice(&body).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].sequence_index, 0);
    assert_eq!(events[1].sequence_index, 1);

    // 5. GET /api/sessions/{session_id}/verify -> 200 OK with Valid status
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/sessions/{}/verify", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let verify: VerificationResult = serde_json::from_slice(&body).unwrap();
    assert_eq!(verify.status, VerificationStatus::Valid);
    assert_eq!(verify.total_steps, 2);

    // 6. POST /api/sessions/{session_id}/finalize -> 200 OK
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/sessions/{}/finalize", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let finalize: FinalizeSessionResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(finalize.total_steps, 2);
    assert_eq!(finalize.root_hash, append_res.current_hash);

    // 7. Attempting to append after finalization -> 409 Conflict
    let event2_payload = json!({
        "action_type": "CHECKOUT",
        "payload": {"cart_id": "cart_1"}
    });
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/sessions/{}/events", session_id))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&event2_payload).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // 8. Re-finalizing an already finalized session -> 409 Conflict
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/sessions/{}/finalize", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_api_session_not_found() {
    let (app, _) = setup_test_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/sessions/non-existent-uuid")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let req = Request::builder()
        .method("GET")
        .uri("/api/sessions/non-existent-uuid/verify")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_empty_prompt_rejected() {
    let (app, _) = setup_test_app().await;

    let payload = json!({ "initial_prompt": "   " });
    let req = Request::builder()
        .method("POST")
        .uri("/api/sessions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// =========================================================================
// 6. ISOLATED HASH-CHAIN TAMPER TESTS (AUDIT CHECK 1)
// =========================================================================

#[tokio::test]
async fn test_verification_detects_modified_previous_hash_genesis() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Genesis tamper test".to_string(),
        })
        .await
        .unwrap();

    // Tamper with genesis previous_hash (should be 64 zeros)
    let fake_genesis = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    sqlx::query(
        "UPDATE decision_steps SET previous_hash = ? WHERE session_id = ? AND sequence_index = 0;",
    )
    .bind(fake_genesis)
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    assert_eq!(result.first_invalid_sequence_index, Some(0));
    assert_eq!(result.error_code, Some("GENESIS_HASH_MISMATCH".to_string()));
    assert!(result.explanation.contains("does not match genesis hash"));
}

#[tokio::test]
async fn test_verification_detects_modified_previous_hash_intermediate() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Intermediate link test".to_string(),
        })
        .await
        .unwrap();

    ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "STEP_1".to_string(),
                payload: json!({"step": 1}),
            },
        )
        .await
        .unwrap();

    ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "STEP_2".to_string(),
                payload: json!({"step": 2}),
            },
        )
        .await
        .unwrap();

    // Tamper with step 2's previous_hash
    let corrupted_hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    sqlx::query(
        "UPDATE decision_steps SET previous_hash = ? WHERE session_id = ? AND sequence_index = 2;",
    )
    .bind(corrupted_hash)
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    assert_eq!(result.first_invalid_sequence_index, Some(2));
    assert_eq!(result.error_code, Some("CHAIN_LINK_BROKEN".to_string()));
    assert!(
        result
            .explanation
            .contains("Chain link broken between sequential steps")
            || result.explanation.contains("does not match step 1")
    );
}

#[tokio::test]
async fn test_verification_detects_deleted_intermediate_event() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Deletion test".to_string(),
        })
        .await
        .unwrap();

    for i in 1..=3 {
        ledger
            .append_event(
                &session.session_id,
                AppendEventRequest {
                    action_type: format!("STEP_{}", i),
                    payload: json!({"step": i}),
                },
            )
            .await
            .unwrap();
    }

    // Delete intermediate step 2 (sequence index 2)
    sqlx::query("DELETE FROM decision_steps WHERE session_id = ? AND sequence_index = 2;")
        .bind(&session.session_id)
        .execute(&pool)
        .await
        .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    // At position 2, step 3 is found (expected index 2, found 3)
    assert_eq!(result.first_invalid_sequence_index, Some(3));
    assert_eq!(
        result.error_code,
        Some("SEQUENCE_ORDER_INVALID".to_string())
    );
    assert!(result.explanation.contains("Sequence discontinuity"));
}

#[tokio::test]
async fn test_verification_detects_sequence_index_gap() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Gap test".to_string(),
        })
        .await
        .unwrap();

    ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "STEP_1".to_string(),
                payload: json!({"step": 1}),
            },
        )
        .await
        .unwrap();

    // Change step 1 sequence_index from 1 to 5, creating a gap
    sqlx::query(
        "UPDATE decision_steps SET sequence_index = 5 WHERE session_id = ? AND sequence_index = 1;",
    )
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    assert_eq!(result.first_invalid_sequence_index, Some(5));
    assert_eq!(
        result.error_code,
        Some("SEQUENCE_ORDER_INVALID".to_string())
    );
    assert!(result
        .explanation
        .contains("Sequence discontinuity at position 1"));
}

#[tokio::test]
async fn test_verification_detects_injected_event_into_finalized_session() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Injection test".to_string(),
        })
        .await
        .unwrap();

    let step1 = ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "STEP_1".to_string(),
                payload: json!({"step": 1}),
            },
        )
        .await
        .unwrap();

    ledger.finalize_session(&session.session_id).await.unwrap();

    // Attacker injects a rogue step 2 directly into SQLite
    let rogue_payload = r#"{"rogue":"action"}"#;
    let rogue_hash = compute_event_hash(
        2,
        "ROGUE_ACTION",
        rogue_payload.as_bytes(),
        &step1.current_hash,
    );
    sqlx::query(
        r#"
        INSERT INTO decision_steps (id, session_id, sequence_index, action_type, payload, previous_hash, current_hash, created_at)
        VALUES ('rogue_step', ?, 2, 'ROGUE_ACTION', ?, ?, ?, '2026-11-07T12:00:00Z');
        "#,
    )
    .bind(&session.session_id)
    .bind(rogue_payload)
    .bind(&step1.current_hash)
    .bind(&rogue_hash)
    .execute(&pool)
    .await
    .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    // Calculated root is now rogue_hash, which does not match session stored root_hash
    assert_eq!(result.error_code, Some("ROOT_HASH_MISMATCH".to_string()));
    assert!(result
        .explanation
        .contains("does not match finalized session root hash"));
}

#[tokio::test]
async fn test_verification_detects_modified_action_type() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Action type tamper test".to_string(),
        })
        .await
        .unwrap();

    ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "VIEW_CART".to_string(),
                payload: json!({"items": 3}),
            },
        )
        .await
        .unwrap();

    // Tamper with action_type: change VIEW_CART to UNAUTHORIZED_TRANSFER
    sqlx::query(
        "UPDATE decision_steps SET action_type = 'UNAUTHORIZED_TRANSFER' WHERE session_id = ? AND sequence_index = 1;",
    )
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    assert_eq!(result.first_invalid_sequence_index, Some(1));
    assert_eq!(result.error_code, Some("DIGEST_MISMATCH".to_string()));
    assert!(result
        .explanation
        .contains("Hash recalculation mismatch at step 1"));
}

#[tokio::test]
async fn test_verification_detects_non_canonical_payload_formatting() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());
    let verifier = VerificationService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Canonical check test".to_string(),
        })
        .await
        .unwrap();

    ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "SELECT_ITEM".to_string(),
                payload: json!({"item_id": 42}),
            },
        )
        .await
        .unwrap();

    // Tamper with payload formatting in SQLite (inject whitespace: {"item_id": 42} -> {"item_id":   42})
    sqlx::query(
        "UPDATE decision_steps SET payload = '{\"item_id\":   42}' WHERE session_id = ? AND sequence_index = 1;",
    )
    .bind(&session.session_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = verifier.verify_session(&session.session_id).await.unwrap();
    assert_eq!(result.status, VerificationStatus::Tampered);
    assert_eq!(result.first_invalid_sequence_index, Some(1));
    assert_eq!(result.error_code, Some("PAYLOAD_NON_CANONICAL".to_string()));
    assert!(result
        .explanation
        .contains("violates RFC 8785 canonical formatting"));
}

// =========================================================================
// 7. FINALIZATION & TRANSACTION CONSISTENCY (AUDIT CHECK 3)
// =========================================================================

#[tokio::test]
async fn test_finalized_root_hash_matches_final_event() {
    let (_, pool) = setup_test_app().await;
    let ledger = LedgerService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Root hash consistency".to_string(),
        })
        .await
        .unwrap();

    let mut last_hash = String::new();
    for i in 1..=4 {
        let ev = ledger
            .append_event(
                &session.session_id,
                AppendEventRequest {
                    action_type: format!("STEP_{}", i),
                    payload: json!({"val": i}),
                },
            )
            .await
            .unwrap();
        last_hash = ev.current_hash;
    }

    let finalization = ledger.finalize_session(&session.session_id).await.unwrap();
    assert_eq!(finalization.root_hash, last_hash);
    assert_eq!(finalization.total_steps, 5); // step 0 (initial) + steps 1..=4

    let retrieved = ledger.get_session(&session.session_id).await.unwrap();
    assert_eq!(retrieved.root_hash, Some(last_hash));
}

#[tokio::test]
async fn test_transaction_rollback_preserves_consistency_on_failure() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let ledger = LedgerService::new(pool.clone());

    let session = ledger
        .create_session(CreateSessionRequest {
            initial_prompt: "Rollback verification".to_string(),
        })
        .await
        .unwrap();

    // Verify initial state has exactly 1 step
    let steps_before = ledger
        .get_session_events(&session.session_id)
        .await
        .unwrap();
    assert_eq!(steps_before.len(), 1);

    // Attempting an append with a payload exceeding MAX_PAYLOAD_SIZE_BYTES
    let oversized_payload = json!({
        "large_data": "x".repeat(1024 * 1024 + 10)
    });

    let res = ledger
        .append_event(
            &session.session_id,
            AppendEventRequest {
                action_type: "OVERSIZED_STEP".to_string(),
                payload: oversized_payload,
            },
        )
        .await;

    assert!(res.is_err(), "Oversized append must be rejected");

    // Verify database state remains completely consistent (still exactly 1 step, no partial writes)
    let steps_after = ledger
        .get_session_events(&session.session_id)
        .await
        .unwrap();
    assert_eq!(steps_after.len(), 1);
    assert_eq!(steps_after[0].sequence_index, 0);
}
