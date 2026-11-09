use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, StatusCode},
    Router,
};
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

use agentic_commerce::config::Config;
use agentic_commerce::create_router;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::correlation::{CorrelationStatus, MatchedBy};
use agentic_commerce::models::dispute::{
    GenesisAnchorType, UpsertDisputePayload, GENESIS_HASH_DEFAULT,
};
use agentic_commerce::models::evidence::{DisputeEvidenceBrief, EpistemicCategory};
use agentic_commerce::models::ledger::{AppendEventRequest, CreateSessionRequest};
use agentic_commerce::services::{
    CatalogService, DisputeEvidenceService, DisputeService, LedgerService, PayPalAuthService,
    PayPalOrdersService, ShoppingAgentService, VerificationService, WebhookPersistenceService,
    WebhookVerifierService,
};
use agentic_commerce::AppState;

struct ApiTestContext {
    pub pool: SqlitePool,
    pub router: Router,
    pub dispute_service: Arc<DisputeService>,
    pub ledger_service: Arc<LedgerService>,
}

async fn setup_api_test_context() -> ApiTestContext {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let config = Arc::new(Config {
        paypal_client_id: "test_client_id".to_string(),
        paypal_client_secret: "test_client_secret".to_string(),
        database_url: "sqlite::memory:".to_string(),
        paypal_webhook_id: Some("test_webhook_id".to_string()),
        ..Config::default()
    });

    let http_client =
        create_http_client(&HttpClientConfig::default()).expect("Failed to initialize HTTP client");
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
    let dispute_service = Arc::new(DisputeService::new(
        pool.clone(),
        verification_service.clone(),
    ));
    let dispute_evidence = Arc::new(DisputeEvidenceService::new(
        pool.clone(),
        dispute_service.clone(),
        ledger_service.clone(),
        verification_service.clone(),
    ));

    let state = AppState {
        config,
        http_client,
        auth_service,
        ledger_service: ledger_service.clone(),
        verification_service,
        catalog_service,
        shopping_agent,
        paypal_orders,
        webhook_verifier,
        webhook_persistence,
        webhook_notify: Arc::new(tokio::sync::Notify::new()),
        dispute_evidence,
    };

    let router = create_router(state);

    ApiTestContext {
        pool,
        router,
        dispute_service,
        ledger_service,
    }
}

fn make_upsert_payload(
    dispute_id: &str,
    session_id: Option<String>,
    root_hash: Option<String>,
    order_id: Option<&str>,
    capture_id: Option<&str>,
    reason: Option<&str>,
    amount: &str,
    correlation_status: CorrelationStatus,
    matched_by: Option<MatchedBy>,
) -> UpsertDisputePayload {
    let (anchor_type, anchor_hash) = if let Some(ref r) = root_hash {
        (GenesisAnchorType::PurchaseRootHash, r.clone())
    } else {
        (
            GenesisAnchorType::GenesisHash,
            GENESIS_HASH_DEFAULT.to_string(),
        )
    };

    UpsertDisputePayload {
        dispute_id: dispute_id.to_string(),
        session_id,
        genesis_anchor_type: anchor_type,
        genesis_anchor_hash: anchor_hash,
        purchase_root_hash: root_hash,
        seller_transaction_id: capture_id.map(String::from),
        paypal_order_id: order_id.map(String::from),
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("INQUIRY".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: reason.map(String::from),
        dispute_amount_value: Some(amount.to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status,
        matched_by,
        dispute_create_time: Some("2026-11-09T18:30:00Z".to_string()),
        dispute_update_time: Some("2026-11-09T18:30:00Z".to_string()),
        raw_event_payload: Some(json!({ "id": dispute_id })),
    }
}

async fn create_finalized_purchase_session(
    ledger_service: &LedgerService,
    order_id: &str,
    capture_id: &str,
) -> (String, String) {
    let session = ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Find premium wireless headphones under $300".to_string(),
        })
        .await
        .unwrap();

    let session_id = session.session_id;

    ledger_service
        .set_paypal_order_id(&session_id, order_id)
        .await
        .unwrap();

    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "REQUIREMENTS_EXTRACTED".to_string(),
                payload: json!({
                    "category": "Headphones",
                    "maximum_budget": 300.0,
                    "currency": "USD",
                    "required_features": ["bluetooth", "noise_cancelling"],
                    "minimum_battery_hours": 20,
                    "preferred_brand": "Sony"
                }),
            },
        )
        .await
        .unwrap();

    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "CATALOG_SEARCH".to_string(),
                payload: json!({
                    "category": "Headphones",
                    "candidates_retrieved_count": 3,
                    "candidates": []
                }),
            },
        )
        .await
        .unwrap();

    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PRODUCT_SELECTED".to_string(),
                payload: json!({
                    "selected_product": {
                        "product_id": "prod-sony-wh",
                        "name": "Sony WH-1000XM4",
                        "brand": "Sony",
                        "category": "Headphones",
                        "price": 278.00,
                        "currency": "USD"
                    },
                    "price": 278.00,
                    "currency": "USD",
                    "selection_rationale": "Meets all criteria under $300"
                }),
            },
        )
        .await
        .unwrap();

    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PURCHASE_AUTHORIZED".to_string(),
                payload: json!({
                    "session_id": session_id,
                    "selected_product_id": "prod-sony-wh",
                    "maximum_authorized_amount": 300.00,
                    "authorized_at": "2026-11-09T18:00:00Z",
                    "user_confirmed": true
                }),
            },
        )
        .await
        .unwrap();

    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PAYPAL_ORDER_CREATED".to_string(),
                payload: json!({
                    "order_id": order_id,
                    "status": "CREATED",
                    "amount": "278.00",
                    "currency": "USD"
                }),
            },
        )
        .await
        .unwrap();

    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PAYPAL_PAYMENT_CAPTURED".to_string(),
                payload: json!({
                    "order_id": order_id,
                    "capture_id": capture_id,
                    "status": "COMPLETED"
                }),
            },
        )
        .await
        .unwrap();

    let finalized = ledger_service.finalize_session(&session_id).await.unwrap();
    (session_id, finalized.root_hash)
}

#[tokio::test]
async fn test_api_get_evidence_json_success() {
    let ctx = setup_api_test_context().await;
    let dispute_id = "PP-D-API-JSON-001";
    let order_id = "ORD-API-001";
    let capture_id = "CAP-API-001";

    let (session_id, root_hash) =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(session_id.clone()),
        Some(root_hash),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED_TRANSACTION"),
        "278.00",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/disputes/{}/evidence", dispute_id))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp.headers().get(header::CONTENT_TYPE).unwrap();
    assert!(content_type.to_str().unwrap().contains("application/json"));

    let body_bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let brief: DisputeEvidenceBrief = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(brief.dispute_id, dispute_id);
    assert_eq!(brief.correlation_summary.status, CorrelationStatus::Matched);
    assert_eq!(
        brief.correlation_summary.category,
        EpistemicCategory::DeterministicInference
    );
    assert!(brief.cryptographic_proof.purchase_chain_valid);
    assert!(brief.cryptographic_proof.dispute_chain_valid);
    assert!(brief.cryptographic_proof.dual_chain_integrity_verified);
    assert!(brief.agent_audit_trail.is_some());
    assert!(brief.transaction_proof.is_some());
    assert_eq!(
        brief.rebuttal_summary.recommended_action,
        "CONTEST_WITH_EVIDENCE"
    );
    assert!(!brief.missing_evidence.missing_items.is_empty());
}

#[tokio::test]
async fn test_api_get_evidence_markdown_success() {
    let ctx = setup_api_test_context().await;
    let dispute_id = "PP-D-API-MD-001";
    let order_id = "ORD-API-MD";
    let capture_id = "CAP-API-MD";

    let (session_id, root_hash) =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(session_id.clone()),
        Some(root_hash),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED_TRANSACTION"),
        "278.00",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/disputes/{}/evidence/markdown", dispute_id))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp.headers().get(header::CONTENT_TYPE).unwrap();
    assert_eq!(
        content_type.to_str().unwrap(),
        "text/markdown; charset=utf-8"
    );

    let body_bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let md = String::from_utf8(body_bytes.to_vec()).unwrap();

    // Verify sections and structure
    assert!(md.contains(&format!("# Dispute Evidence Brief: {}", dispute_id)));
    assert!(md.contains("## Executive Summary & Recommended Action"));
    assert!(md.contains("## 1. PayPal Dispute Summary"));
    assert!(md.contains("## 2. Session Correlation Assessment"));
    assert!(md.contains("## 3. Cryptographic Proof & Ledger Integrity"));
    assert!(md.contains("## 4. Agent Decision Audit Trail"));
    assert!(md.contains("## 5. Transaction Proof & Settlement"));
    assert!(md.contains("## 6. Factual Rebuttal Arguments"));
    assert!(md.contains("## 7. Unavailable Evidence Disclaimers"));

    // Verify epistemic classifications
    assert!(md.contains("`VERIFIED_FACT`"));
    assert!(md.contains("`RECORDED_PAYPAL_DATA`"));
    assert!(md.contains("`DETERMINISTIC_INFERENCE`"));
    assert!(md.contains("`UNAVAILABLE_EVIDENCE`"));

    // Verify required disclaimers are present
    assert!(md.contains("does not prevent an attacker with full database access"));
    assert!(md.contains("does not independently establish the physical identity"));
    assert!(md.contains("CONTEST_WITH_EVIDENCE"));
}

#[tokio::test]
async fn test_api_get_evidence_not_found() {
    let ctx = setup_api_test_context().await;
    let missing_dispute_id = "PP-D-MISSING-404";

    // 1. JSON endpoint
    let req_json = Request::builder()
        .method("GET")
        .uri(format!("/api/disputes/{}/evidence", missing_dispute_id))
        .body(Body::empty())
        .unwrap();

    let resp_json = ctx.router.clone().oneshot(req_json).await.unwrap();
    assert_eq!(resp_json.status(), StatusCode::NOT_FOUND);

    let body_bytes = to_bytes(resp_json.into_body(), 1024 * 1024).await.unwrap();
    let err_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(err_json["success"], false);
    assert!(err_json["message"].as_str().unwrap().contains("not found"));

    // 2. Markdown endpoint
    let req_md = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/disputes/{}/evidence/markdown",
            missing_dispute_id
        ))
        .body(Body::empty())
        .unwrap();

    let resp_md = ctx.router.oneshot(req_md).await.unwrap();
    assert_eq!(resp_md.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_get_evidence_uncorrelated() {
    let ctx = setup_api_test_context().await;
    let dispute_id = "PP-D-API-UNCORR-001";

    let payload = make_upsert_payload(
        dispute_id,
        None,
        None,
        None,
        Some("UNKNOWN-CAP"),
        Some("UNAUTHORISED_TRANSACTION"),
        "150.00",
        CorrelationStatus::NotFound,
        None,
    );
    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    // 1. JSON endpoint
    let req_json = Request::builder()
        .method("GET")
        .uri(format!("/api/disputes/{}/evidence", dispute_id))
        .body(Body::empty())
        .unwrap();

    let resp_json = ctx.router.clone().oneshot(req_json).await.unwrap();
    assert_eq!(resp_json.status(), StatusCode::OK);

    let body_bytes = to_bytes(resp_json.into_body(), 1024 * 1024).await.unwrap();
    let brief: DisputeEvidenceBrief = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(
        brief.correlation_summary.status,
        CorrelationStatus::NotFound
    );
    assert!(brief.agent_audit_trail.is_none());
    assert!(brief.transaction_proof.is_none());
    assert!(!brief.cryptographic_proof.dual_chain_integrity_verified);
    assert_eq!(brief.rebuttal_summary.recommended_action, "MANUAL_REVIEW");

    // 2. Markdown endpoint
    let req_md = Request::builder()
        .method("GET")
        .uri(format!("/api/disputes/{}/evidence/markdown", dispute_id))
        .body(Body::empty())
        .unwrap();

    let resp_md = ctx.router.oneshot(req_md).await.unwrap();
    assert_eq!(resp_md.status(), StatusCode::OK);

    let body_bytes_md = to_bytes(resp_md.into_body(), 1024 * 1024).await.unwrap();
    let md = String::from_utf8(body_bytes_md.to_vec()).unwrap();
    assert!(md.contains("N/A (Uncorrelated)"));
    assert!(md.contains("MANUAL_REVIEW"));
    assert!(!md.contains("## 4. Agent Decision Audit Trail"));
}

#[tokio::test]
async fn test_api_get_evidence_tampered_ledger() {
    let ctx = setup_api_test_context().await;
    let dispute_id = "PP-D-API-TAMPER-001";
    let order_id = "ORD-TAMPER-API";
    let capture_id = "CAP-TAMPER-API";

    let (session_id, root_hash) =
        create_finalized_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(session_id.clone()),
        Some(root_hash),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED_TRANSACTION"),
        "278.00",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    // Tamper with decision step in database
    sqlx::query(
        "UPDATE decision_steps SET payload = '{\"tampered\": true}' WHERE session_id = ? AND sequence_index = 2;",
    )
    .bind(&session_id)
    .execute(&ctx.pool)
    .await
    .unwrap();

    // 1. JSON endpoint
    let req_json = Request::builder()
        .method("GET")
        .uri(format!("/api/disputes/{}/evidence", dispute_id))
        .body(Body::empty())
        .unwrap();

    let resp_json = ctx.router.clone().oneshot(req_json).await.unwrap();
    assert_eq!(resp_json.status(), StatusCode::OK);

    let body_bytes = to_bytes(resp_json.into_body(), 1024 * 1024).await.unwrap();
    let brief: DisputeEvidenceBrief = serde_json::from_slice(&body_bytes).unwrap();

    assert!(!brief.cryptographic_proof.purchase_chain_valid);
    assert!(!brief.cryptographic_proof.dual_chain_integrity_verified);
    assert!(
        brief.agent_audit_trail.is_none(),
        "Tampered ledger must not emit agent audit trail facts"
    );
    assert!(
        brief.transaction_proof.is_none(),
        "Tampered ledger must not emit transaction proof facts"
    );
    assert_eq!(brief.rebuttal_summary.recommended_action, "MANUAL_REVIEW");

    // 2. Markdown endpoint
    let req_md = Request::builder()
        .method("GET")
        .uri(format!("/api/disputes/{}/evidence/markdown", dispute_id))
        .body(Body::empty())
        .unwrap();

    let resp_md = ctx.router.oneshot(req_md).await.unwrap();
    assert_eq!(resp_md.status(), StatusCode::OK);

    let body_bytes_md = to_bytes(resp_md.into_body(), 1024 * 1024).await.unwrap();
    let md = String::from_utf8(body_bytes_md.to_vec()).unwrap();
    assert!(md.contains("FAILED / TAMPERED"));
    assert!(md.contains("MANUAL_REVIEW"));
    assert!(!md.contains("## 4. Agent Decision Audit Trail"));
}
