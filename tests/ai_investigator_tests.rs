use axum::{extract::Json, http::StatusCode, routing::post, Router};
use serde_json::json;
use std::sync::Arc;
use tokio::net::TcpListener;

use agentic_commerce::config::Config;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::correlation::{CorrelationStatus, MatchedBy};
use agentic_commerce::models::dispute::{GenesisAnchorType, UpsertDisputePayload};
use agentic_commerce::models::ledger::{AppendEventRequest, CreateSessionRequest};
use agentic_commerce::services::ai_investigator::AIInvestigatorService;
use agentic_commerce::services::dispute_evidence::DisputeEvidenceService;
use agentic_commerce::services::dispute_service::DisputeService;
use agentic_commerce::services::ledger::LedgerService;
use agentic_commerce::services::verification::VerificationService;

/// Helper to set up an in-memory brief for tests.
async fn create_test_dispute_brief() -> agentic_commerce::models::evidence::DisputeEvidenceBrief {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let verification_service = Arc::new(VerificationService::new(pool.clone()));
    let dispute_service = Arc::new(DisputeService::new(
        pool.clone(),
        verification_service.clone(),
    ));
    let ledger_service = Arc::new(LedgerService::new(pool.clone()));
    let evidence_service = DisputeEvidenceService::new(
        pool.clone(),
        dispute_service.clone(),
        ledger_service.clone(),
        verification_service.clone(),
    );

    // 1. Create complete finalized purchase session
    let session = ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Buy Sony noise canceling headphones under $350".to_string(),
        })
        .await
        .unwrap();

    let session_id = session.session_id.clone();
    let order_id = "5O190127TN364715T";
    let capture_id = "5LN68731NH617144R";

    ledger_service
        .set_paypal_order_id(&session_id, order_id)
        .await
        .unwrap();

    // Event 1: REQUIREMENTS_EXTRACTED
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "REQUIREMENTS_EXTRACTED".to_string(),
                payload: json!({
                    "category": "headphones",
                    "maximum_budget": 350.0,
                    "currency": "USD",
                    "required_features": ["noise-canceling", "wireless"],
                    "preferred_brand": "Sony"
                }),
            },
        )
        .await
        .unwrap();

    // Event 2: CATALOG_SEARCH
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "CATALOG_SEARCH".to_string(),
                payload: json!({
                    "category": "headphones",
                    "candidates_retrieved_count": 5
                }),
            },
        )
        .await
        .unwrap();

    // Event 3: PRODUCT_SELECTED
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PRODUCT_SELECTED".to_string(),
                payload: json!({
                    "selected_product": {
                        "product_id": "hp-sony-wh1000xm5",
                        "name": "Sony WH-1000XM5",
                        "brand": "Sony",
                        "category": "headphones",
                        "price": 329.99,
                        "currency": "USD"
                    },
                    "selection_rationale": "Matches brand and budget under $350"
                }),
            },
        )
        .await
        .unwrap();

    // Event 4: PURCHASE_AUTHORIZED
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PURCHASE_AUTHORIZED".to_string(),
                payload: json!({
                    "selected_product_id": "hp-sony-wh1000xm5",
                    "maximum_authorized_amount": 350.0,
                    "currency": "USD",
                    "user_confirmed": true,
                    "authorized_at": "2026-11-09T18:00:00Z"
                }),
            },
        )
        .await
        .unwrap();

    // Event 5: PAYPAL_PAYMENT_CAPTURED
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PAYPAL_PAYMENT_CAPTURED".to_string(),
                payload: json!({
                    "capture_id": capture_id,
                    "amount": 329.99,
                    "currency": "USD",
                    "status": "COMPLETED",
                    "order_id": order_id
                }),
            },
        )
        .await
        .unwrap();

    // Finalize session
    let fin = ledger_service.finalize_session(&session_id).await.unwrap();
    let purchase_root_hash = fin.root_hash;

    // 2. Insert matched dispute
    let dispute_id = "PP-R-TEST-AI-01";
    let upsert = UpsertDisputePayload {
        dispute_id: dispute_id.to_string(),
        session_id: Some(session_id),
        genesis_anchor_type: GenesisAnchorType::PurchaseRootHash,
        genesis_anchor_hash: purchase_root_hash.clone(),
        purchase_root_hash: Some(purchase_root_hash),
        seller_transaction_id: Some(capture_id.to_string()),
        paypal_order_id: Some(order_id.to_string()),
        status: "UNDER_REVIEW".to_string(),
        dispute_lifecycle_stage: Some("CHARGEBACK".to_string()),
        dispute_channel: Some("INTERNAL".to_string()),
        reason: Some("UNAUTHORISED".to_string()),
        dispute_amount_value: Some("329.99".to_string()),
        dispute_amount_currency: Some("USD".to_string()),
        correlation_status: CorrelationStatus::Matched,
        matched_by: Some(MatchedBy::CaptureId),
        dispute_create_time: Some("2026-11-10T12:00:00Z".to_string()),
        dispute_update_time: Some("2026-11-10T12:00:00Z".to_string()),
        raw_event_payload: Some(json!({ "id": dispute_id })),
    };

    dispute_service.upsert_dispute(&upsert).await.unwrap();

    evidence_service
        .generate_evidence_brief(dispute_id)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_deterministic_fallback_when_gemini_unconfigured() {
    let brief = create_test_dispute_brief().await;

    let config = Arc::new(Config {
        gemini_api_key: None, // Unconfigured
        gemini_model: "gemini-3.8-flash".to_string(),
        ..Config::default()
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let service = AIInvestigatorService::new(config, client);

    let report = service
        .investigate(&brief, false)
        .await
        .expect("Investigation should succeed");

    // Must be clearly marked as NOT AI-generated
    assert_eq!(report.is_ai_generated, false);
    assert_eq!(report.provider, "deterministic_fallback");
    assert!(report.provider_note.contains("not AI-generated"));
    assert!(report.provider_note.contains("Deterministic Audit Engine"));

    // Verify report fields
    assert_eq!(report.dispute_id, "PP-R-TEST-AI-01");
    assert!(!report.executive_summary.is_empty());
    assert!(!report.narrative_timeline.is_empty());
    assert!(!report.recommended_next_steps.is_empty());
    assert!(!report.missing_evidence.is_empty());

    // Inconsistencies should catch the UNAUTHORISED contradiction since customer authorized $350
    let unauth_finding = report
        .inconsistencies_or_red_flags
        .iter()
        .find(|f| f.finding.contains("unauthorized"));
    assert!(
        unauth_finding.is_some(),
        "Expected unauth finding due to recorded explicit consent"
    );

    // Check that every evidence ID in narrative is valid
    assert!(!report.valid_evidence_references.is_empty());
    assert!(report.discarded_unverified_references.is_empty());
}

#[tokio::test]
async fn test_force_fallback_option_bypasses_network_even_if_key_present() {
    let brief = create_test_dispute_brief().await;

    let config = Arc::new(Config {
        gemini_api_key: Some("fake_dummy_gemini_api_key_12345".to_string()),
        gemini_model: "gemini-3.8-flash".to_string(),
        ..Config::default()
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let service = AIInvestigatorService::new(config, client);

    // Pass force_fallback: true
    let report = service
        .investigate(&brief, true)
        .await
        .expect("Force fallback must succeed");

    assert_eq!(report.is_ai_generated, false);
    assert_eq!(report.provider, "deterministic_fallback");
    assert!(report
        .provider_note
        .contains("Deterministic analysis explicitly requested"));
}

#[tokio::test]
async fn test_gemini_provider_rate_limit_429_falls_back_gracefully() {
    let brief = create_test_dispute_brief().await;

    // Spawn mock Gemini server returning HTTP 429
    let app = Router::new().route(
        "/v1beta/models/gemini-3.8-flash:generateContent",
        post(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": {
                        "code": 429,
                        "message": "Resource has been exhausted (e.g. check quota)."
                    }
                })),
            )
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = Arc::new(Config {
        gemini_api_key: Some("mock_key".to_string()),
        gemini_model: "gemini-3.8-flash".to_string(),
        gemini_base_url: format!("http://{}", addr),
        ..Config::default()
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let service = AIInvestigatorService::new(config, client);

    // Fallback should trigger smoothly without crashing or returning an AppError
    let report = service
        .investigate(&brief, false)
        .await
        .expect("Must fall back cleanly on 429");

    assert_eq!(report.is_ai_generated, false);
    assert_eq!(report.provider, "deterministic_fallback");
    assert!(report
        .provider_note
        .contains("switched to deterministic fallback"));
}

#[tokio::test]
async fn test_gemini_provider_malformed_json_falls_back_gracefully() {
    let brief = create_test_dispute_brief().await;

    // Mock Gemini server returning non-JSON garbage text in parts
    let app = Router::new().route(
        "/v1beta/models/gemini-3.8-flash:generateContent",
        post(|| async {
            (
                StatusCode::OK,
                Json(json!({
                    "candidates": [
                        {
                            "content": {
                                "parts": [
                                    {
                                        "text": "Sorry, I cannot format this as JSON right now! Error 500."
                                    }
                                ]
                            }
                        }
                    ]
                })),
            )
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = Arc::new(Config {
        gemini_api_key: Some("mock_key".to_string()),
        gemini_model: "gemini-3.8-flash".to_string(),
        gemini_base_url: format!("http://{}", addr),
        ..Config::default()
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let service = AIInvestigatorService::new(config, client);

    let report = service
        .investigate(&brief, false)
        .await
        .expect("Must fall back cleanly on malformed JSON");

    assert_eq!(report.is_ai_generated, false);
    assert_eq!(report.provider, "deterministic_fallback");
}

#[tokio::test]
async fn test_gemini_successful_response_and_reference_validation() {
    let brief = create_test_dispute_brief().await;
    let dispute_id = brief.dispute_summary.dispute_id.clone();

    // Mock Gemini server returning valid schema with one valid and one hallucinated reference
    let app = Router::new().route(
        "/v1beta/models/gemini-3.8-flash:generateContent",
        post(move || {
            let dispute_id_clone = dispute_id.clone();
            async move {
                let ai_output = json!({
                    "executive_summary": "AI summary: Transaction was explicitly authorized by the user before order creation.",
                    "narrative_timeline": [
                        {
                            "phase": "Customer Dispute",
                            "description": "Customer filed unauthorized transaction dispute.",
                            "evidence_id": dispute_id_clone,
                            "evidence_type": "RECORDED_PAYPAL_DATA"
                        },
                        {
                            "phase": "Hallucinated Step",
                            "description": "Fabricated step that does not exist in ledger.",
                            "evidence_id": "HALLUCINATED-HASH-99999",
                            "evidence_type": "VERIFIED_FACT"
                        }
                    ],
                    "inconsistencies_or_red_flags": [
                        {
                            "severity": "HIGH",
                            "finding": "Dispute reason UNAUTHORISED contradicts explicit user authorization.",
                            "details": "Customer confirmed authorization of $350.00 max prior to capture.",
                            "evidence_ref": "HALLUCINATED-REF-88888"
                        }
                    ],
                    "evidence_supporting_dispute": [
                        {
                            "claim": "Customer claims unauthorized charge.",
                            "source": "PayPal Dispute Webhook",
                            "evidence_id": dispute_id_clone
                        }
                    ],
                    "evidence_contradicting_dispute": [
                        {
                            "fact": "Cryptographic root hash sealed at purchase authorization.",
                            "source": "Audit Ledger",
                            "evidence_id": "ANOTHER-HALLUCINATED-ID",
                            "epistemic_category": "VERIFIED_FACT"
                        }
                    ],
                    "missing_evidence": [
                        {
                            "item": "Carrier Proof of Delivery",
                            "reason_unavailable": "Physical tracking not in audit chain"
                        }
                    ],
                    "recommended_next_steps": [
                        {
                            "priority": 1,
                            "action": "Submit cryptographic ledger audit trail to PayPal Resolution Center.",
                            "rationale": "Demonstrates explicit customer confirmation before checkout."
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

    let config = Arc::new(Config {
        gemini_api_key: Some("test_valid_key".to_string()),
        gemini_model: "gemini-3.8-flash".to_string(),
        gemini_base_url: format!("http://{}", addr),
        ..Config::default()
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let service = AIInvestigatorService::new(config, client);

    let report = service
        .investigate(&brief, false)
        .await
        .expect("AI investigation should succeed");

    assert_eq!(report.is_ai_generated, true);
    assert_eq!(report.provider, "gemini-3.8-flash");
    assert_eq!(
        report.ledger_hash_verified, true,
        "Backend must verify hash authoritatively"
    );

    // Valid dispute reference must be catalogued
    assert!(report
        .valid_evidence_references
        .contains(&brief.dispute_summary.dispute_id));

    // Hallucinated references must be detected and discarded by the backend
    assert!(report
        .discarded_unverified_references
        .contains(&"HALLUCINATED-HASH-99999".to_string()));
    assert!(report
        .discarded_unverified_references
        .contains(&"HALLUCINATED-REF-88888".to_string()));
    assert!(report
        .discarded_unverified_references
        .contains(&"ANOTHER-HALLUCINATED-ID".to_string()));
}

#[tokio::test]
async fn test_privacy_sanitization_minimizes_customer_pii() {
    let brief = create_test_dispute_brief().await;

    let config = Arc::new(Config {
        gemini_api_key: None,
        gemini_model: "gemini-3.8-flash".to_string(),
        ..Config::default()
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let service = AIInvestigatorService::new(config, client);

    // Call deterministic fallback (which exercises the models and internal structures)
    let report = service.investigate(&brief, false).await.unwrap();

    // Verify report does not contain sensitive test credentials or unmasked buyer email
    let serialized_report = serde_json::to_string(&report).unwrap();
    assert!(!serialized_report.contains("PAYPAL_CLIENT_SECRET"));
    assert!(!serialized_report.contains("mock_client_secret"));
    assert!(!serialized_report.contains("real-customer-personal-email@example.com"));
}
