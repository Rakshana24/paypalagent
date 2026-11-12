use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, StatusCode},
    routing::post,
    Json, Router,
};
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::ServiceExt;

use agentic_commerce::config::Config;
use agentic_commerce::create_router;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::ai_investigation::AIDisputeInvestigationReport;
use agentic_commerce::models::correlation::{CorrelationStatus, MatchedBy};
use agentic_commerce::models::dispute::{
    GenesisAnchorType, UpsertDisputePayload, GENESIS_HASH_DEFAULT,
};
use agentic_commerce::models::ledger::{AppendEventRequest, CreateSessionRequest};
use agentic_commerce::services::{
    AIInvestigatorService, CatalogService, DisputeEvidenceService, DisputeService, LedgerService,
    PayPalAuthService, PayPalOrdersService, ShoppingAgentService, VerificationService,
    WebhookPersistenceService, WebhookVerifierService,
};
use agentic_commerce::AppState;

struct ApiTestContext {
    #[allow(dead_code)]
    pub pool: SqlitePool,
    pub router: Router,
    pub dispute_service: Arc<DisputeService>,
    pub ledger_service: Arc<LedgerService>,
    pub verification_service: Arc<VerificationService>,
}

async fn setup_api_test_context(
    gemini_base_url: Option<String>,
    gemini_api_key: Option<String>,
) -> ApiTestContext {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut config = Config::default();
    config.gemini_api_key = gemini_api_key;
    if let Some(url) = gemini_base_url {
        config.gemini_base_url = url;
    }
    let config = Arc::new(config);

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
    let ai_investigator = Arc::new(AIInvestigatorService::new(
        config.clone(),
        http_client.clone(),
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
        webhook_persistence,
        webhook_notify: Arc::new(tokio::sync::Notify::new()),
        dispute_evidence,
        ai_investigator,
    };

    let router = create_router(state);

    ApiTestContext {
        pool,
        router,
        dispute_service,
        ledger_service,
        verification_service,
    }
}

async fn create_finalized_test_session(
    ledger_service: &LedgerService,
    order_id: &str,
    capture_id: &str,
) -> (String, String) {
    let session = ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Find Sony wireless noise cancelling headphones under $350".to_string(),
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
                    "maximum_budget": 350.0,
                    "currency": "USD",
                    "required_features": ["wireless", "noise_cancelling"],
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
                action_type: "PRODUCT_SELECTED".to_string(),
                payload: json!({
                    "selected_product": {
                        "product_id": "prod-sony-1000xm4",
                        "name": "Sony WH-1000XM4",
                        "brand": "Sony",
                        "category": "Headphones",
                        "price": 329.99,
                        "currency": "USD"
                    },
                    "price": 329.99,
                    "currency": "USD",
                    "selection_rationale": "Matches all constraints"
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
                    "selected_product_id": "prod-sony-1000xm4",
                    "maximum_authorized_amount": 350.00,
                    "authorized_at": "2026-11-10T11:00:00Z",
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
                    "amount": "329.99",
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
        dispute_lifecycle_stage: Some("CHARGEBACK".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: reason.map(String::from),
        dispute_amount_value: Some(amount.to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status,
        matched_by,
        dispute_create_time: Some("2026-11-10T12:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-10T12:00:00Z".to_string()),
        raw_event_payload: Some(json!({ "id": dispute_id })),
    }
}

#[tokio::test]
async fn test_api_investigate_dispute_not_found_returns_404() {
    let ctx = setup_api_test_context(None, None).await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/disputes/NON-EXISTENT-DISPUTE/ai-investigation")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_investigate_dispute_fallback_empty_body_success() {
    let ctx = setup_api_test_context(None, None).await;
    let dispute_id = "PP-D-API-AI-001";
    let order_id = "ORD-API-AI-001";
    let capture_id = "CAP-API-AI-001";

    let (session_id, root_hash) =
        create_finalized_test_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(session_id.clone()),
        Some(root_hash),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED"),
        "329.99",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    // Call POST with completely empty body
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/disputes/{}/ai-investigation", dispute_id))
        .body(Body::empty())
        .unwrap();

    let resp = ctx.router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let report: AIDisputeInvestigationReport = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(report.dispute_id, dispute_id);
    assert_eq!(report.is_ai_generated, false);
    assert_eq!(report.provider, "deterministic_fallback");
    assert_eq!(report.ledger_hash_verified, true);
    assert!(!report.executive_summary.is_empty());
    assert!(!report.narrative_timeline.is_empty());
    assert!(!report.inconsistencies_or_red_flags.is_empty());
    assert!(report.discarded_unverified_references.is_empty());
}

#[tokio::test]
async fn test_api_investigate_dispute_force_fallback_explicit_flag() {
    let ctx = setup_api_test_context(None, Some("fake_gemini_key".to_string())).await;
    let dispute_id = "PP-D-API-AI-002";
    let order_id = "ORD-API-AI-002";
    let capture_id = "CAP-API-AI-002";

    let (session_id, root_hash) =
        create_finalized_test_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(session_id.clone()),
        Some(root_hash),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED"),
        "329.99",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    // Pass explicit force_fallback: true
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/disputes/{}/ai-investigation", dispute_id))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "force_fallback": true }).to_string()))
        .unwrap();

    let resp = ctx.router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let report: AIDisputeInvestigationReport = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(report.is_ai_generated, false);
    assert_eq!(report.provider, "deterministic_fallback");
    assert!(report
        .provider_note
        .contains("Deterministic analysis explicitly requested"));
}

#[tokio::test]
async fn test_api_investigate_dispute_invalid_evidence_claims_discarded_or_flagged() {
    let dispute_id = "PP-D-API-AI-003";
    let order_id = "ORD-API-AI-003";
    let capture_id = "CAP-API-AI-003";

    // Setup local mock Gemini server that attempts to return hallucinated evidence IDs
    let dispute_id_mock = dispute_id.to_string();
    let app = Router::new().route(
        "/v1beta/models/gemini-3.8-flash:generateContent",
        post(move || {
            let d_id = dispute_id_mock.clone();
            async move {
                let ai_output = json!({
                    "executive_summary": "Forensic AI analysis of dispute and authorization chain.",
                    "narrative_timeline": [
                        {
                            "phase": "Dispute Filing",
                            "description": "Customer claims unauthorized charge.",
                            "evidence_id": d_id,
                            "evidence_type": "RECORDED_PAYPAL_DATA"
                        },
                        {
                            "phase": "Fabricated Action",
                            "description": "A hallucinated event not in the ledger.",
                            "evidence_id": "HALLUCINATED_EVENT_HASH_12345",
                            "evidence_type": "VERIFIED_FACT"
                        }
                    ],
                    "inconsistencies_or_red_flags": [
                        {
                            "severity": "HIGH",
                            "finding": "Customer claim contradicts explicit authorization.",
                            "details": "Customer authorized item before checkout.",
                            "evidence_ref": "HALLUCINATED_AUTH_REF_99999"
                        }
                    ],
                    "evidence_supporting_dispute": [
                        {
                            "claim": "Customer filed unauthorized transaction dispute.",
                            "source": "PayPal Dispute",
                            "evidence_id": d_id
                        }
                    ],
                    "evidence_contradicting_dispute": [
                        {
                            "fact": "Purchase decision was sealed in ledger.",
                            "source": "Ledger Chain",
                            "evidence_id": "HALLUCINATED_CONTRADICTING_REF_77777",
                            "epistemic_category": "VERIFIED_FACT"
                        }
                    ],
                    "missing_evidence": [
                        {
                            "item": "Carrier Proof of Delivery",
                            "reason_unavailable": "Physical shipment tracking not in digital chain"
                        }
                    ],
                    "recommended_next_steps": [
                        {
                            "priority": 1,
                            "action": "Submit authorization proof to PayPal Resolution Center",
                            "rationale": "Conclusively proves explicit buyer authorization"
                        }
                    ]
                });

                let response_body = json!({
                    "candidates": [
                        {
                            "content": {
                                "parts": [
                                    {
                                        "text": serde_json::to_string(&ai_output).unwrap()
                                    }
                                ]
                            }
                        }
                    ]
                });

                (StatusCode::OK, Json(response_body))
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let ctx = setup_api_test_context(
        Some(format!("http://{}", addr)),
        Some("test_gemini_api_key".to_string()),
    )
    .await;

    let (session_id, root_hash) =
        create_finalized_test_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(session_id.clone()),
        Some(root_hash),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED"),
        "329.99",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/disputes/{}/ai-investigation", dispute_id))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "force_fallback": false }).to_string()))
        .unwrap();

    let resp = ctx.router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let report: AIDisputeInvestigationReport = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(report.is_ai_generated, true);
    assert_eq!(report.provider, "gemini-3.8-flash");

    // The backend must have authoritatively validated references:
    // Real dispute ID is retained in valid references
    assert!(report
        .valid_evidence_references
        .contains(&dispute_id.to_string()));

    // Hallucinated references must be flagged / moved to discarded_unverified_references
    assert!(report
        .discarded_unverified_references
        .contains(&"HALLUCINATED_EVENT_HASH_12345".to_string()));
    assert!(report
        .discarded_unverified_references
        .contains(&"HALLUCINATED_AUTH_REF_99999".to_string()));
    assert!(report
        .discarded_unverified_references
        .contains(&"HALLUCINATED_CONTRADICTING_REF_77777".to_string()));
}

#[tokio::test]
async fn test_api_investigate_dispute_endpoint_does_not_modify_ledger() {
    let ctx = setup_api_test_context(None, None).await;
    let dispute_id = "PP-D-API-AI-004";
    let order_id = "ORD-API-AI-004";
    let capture_id = "CAP-API-AI-004";

    let (session_id, root_hash_before) =
        create_finalized_test_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(session_id.clone()),
        Some(root_hash_before.clone()),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED"),
        "329.99",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    // 1. Inspect ledger state before invocation
    let events_before = ctx
        .ledger_service
        .get_session_events(&session_id)
        .await
        .unwrap();
    let session_before = ctx.ledger_service.get_session(&session_id).await.unwrap();
    let verification_before = ctx
        .verification_service
        .verify_session(&session_id)
        .await
        .unwrap();

    assert_eq!(events_before.len(), 6);
    assert_eq!(
        verification_before.status,
        agentic_commerce::models::ledger::VerificationStatus::Valid
    );
    assert_eq!(session_before.root_hash, Some(root_hash_before.clone()));

    // 2. Invoke POST /api/disputes/:id/ai-investigation repeatedly
    for _ in 0..3 {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/disputes/{}/ai-investigation", dispute_id))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap();

        let resp = ctx.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // 3. Inspect ledger state after invocations
    let events_after = ctx
        .ledger_service
        .get_session_events(&session_id)
        .await
        .unwrap();
    let session_after = ctx.ledger_service.get_session(&session_id).await.unwrap();
    let verification_after = ctx
        .verification_service
        .verify_session(&session_id)
        .await
        .unwrap();

    // The ledger must remain completely unchanged
    assert_eq!(
        events_after.len(),
        events_before.len(),
        "Ledger event count must not change"
    );
    assert_eq!(session_after.root_hash, session_before.root_hash);
    assert_eq!(
        session_after.paypal_order_id,
        session_before.paypal_order_id
    );
    assert_eq!(
        verification_after.status,
        agentic_commerce::models::ledger::VerificationStatus::Valid,
        "Session verification must remain valid"
    );

    for (before, after) in events_before.iter().zip(events_after.iter()) {
        assert_eq!(before.id, after.id);
        assert_eq!(before.sequence_index, after.sequence_index);
        assert_eq!(before.current_hash, after.current_hash);
        assert_eq!(before.previous_hash, after.previous_hash);
        assert_eq!(before.action_type, after.action_type);
        assert_eq!(before.payload, after.payload);
    }
}
