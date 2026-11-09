use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;

use agentic_commerce::db::init_db;
use agentic_commerce::error::AppError;
use agentic_commerce::models::correlation::{CorrelationStatus, MatchedBy};
use agentic_commerce::models::dispute::{
    GenesisAnchorType, UpsertDisputePayload, GENESIS_HASH_DEFAULT,
};
use agentic_commerce::models::evidence::{
    EpistemicCategory, SettlementProvenance, UnavailableEvidenceType,
};
use agentic_commerce::models::ledger::{AppendEventRequest, CreateSessionRequest};
use agentic_commerce::services::dispute_evidence::DisputeEvidenceService;
use agentic_commerce::services::dispute_service::DisputeService;
use agentic_commerce::services::ledger::LedgerService;
use agentic_commerce::services::verification::VerificationService;

/// Test context holding database pool and instantiated core services.
struct TestContext {
    pool: SqlitePool,
    dispute_service: Arc<DisputeService>,
    ledger_service: Arc<LedgerService>,
    evidence_service: DisputeEvidenceService,
}

async fn setup_test_context() -> TestContext {
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

    TestContext {
        pool,
        dispute_service,
        ledger_service,
        evidence_service,
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

/// Helper creating a complete 8-step finalized purchase session.
async fn create_complete_purchase_session(
    ledger_service: &LedgerService,
    order_id: &str,
    capture_id: &str,
) -> (String, String) {
    let session = ledger_service
        .create_session(CreateSessionRequest {
            initial_prompt: "Find wireless noise cancelling headphones under $300".to_string(),
        })
        .await
        .unwrap();

    let session_id = session.session_id;

    ledger_service
        .set_paypal_order_id(&session_id, order_id)
        .await
        .unwrap();

    // Step 1: REQUIREMENTS_EXTRACTED
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "REQUIREMENTS_EXTRACTED".to_string(),
                payload: json!({
                    "category": "Headphones",
                    "maximum_budget": 300.0,
                    "currency": "USD",
                    "required_features": ["anc", "bluetooth"],
                    "minimum_battery_hours": 24,
                    "preferred_brand": "Sony"
                }),
            },
        )
        .await
        .unwrap();

    // Step 2: CATALOG_SEARCH
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "CATALOG_SEARCH".to_string(),
                payload: json!({
                    "category": "Headphones",
                    "candidates_retrieved_count": 4,
                    "candidates": []
                }),
            },
        )
        .await
        .unwrap();

    // Step 3: PRODUCT_REJECTED
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PRODUCT_REJECTED".to_string(),
                payload: json!({
                    "product_id": "prod-overbudget",
                    "product_name": "Premium ANC 900",
                    "price": 349.99,
                    "currency": "USD",
                    "rejection_reason": "Price $349.99 exceeds customer budget limit of $300.00"
                }),
            },
        )
        .await
        .unwrap();

    // Step 4: PRODUCT_SELECTED
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
                        "currency": "USD",
                        "rating": 4.8,
                        "review_count": 1250,
                        "in_stock": true,
                        "specifications": {
                            "battery_life_hours": 30,
                            "noise_cancellation": true,
                            "connectivity": "Bluetooth 5.0",
                            "warranty_months": 12
                        }
                    },
                    "price": 278.00,
                    "currency": "USD",
                    "selection_rationale": "Meets all customer requirements with 30h battery and ANC under $300 budget."
                }),
            },
        )
        .await
        .unwrap();

    // Step 5: PURCHASE_AUTHORIZED
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

    // Step 6: PAYPAL_ORDER_CREATED
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PAYPAL_ORDER_CREATED".to_string(),
                payload: json!({
                    "order_id": order_id,
                    "status": "CREATED",
                    "intent": "CAPTURE",
                    "product_id": "prod-sony-wh",
                    "amount": "278.00",
                    "currency": "USD"
                }),
            },
        )
        .await
        .unwrap();

    // Step 7: PAYPAL_PAYMENT_CAPTURED
    ledger_service
        .append_event(
            &session_id,
            AppendEventRequest {
                action_type: "PAYPAL_PAYMENT_CAPTURED".to_string(),
                payload: json!({
                    "order_id": order_id,
                    "capture_id": capture_id,
                    "status": "COMPLETED",
                    "verified": true
                }),
            },
        )
        .await
        .unwrap();

    // Finalize session
    let finalized = ledger_service.finalize_session(&session_id).await.unwrap();

    (session_id, finalized.root_hash)
}

#[tokio::test]
async fn test_evidence_brief_matched_dispute_full_pipeline() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-TEST-001";
    let capture_id = "CAP-TEST-001";
    let dispute_id = "PP-D-TEST-001";

    let (session_id, root_hash) =
        create_complete_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(session_id.clone()),
        Some(root_hash.clone()),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED_TRANSACTION"),
        "278.00",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );

    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    // Generate evidence brief
    let brief = ctx
        .evidence_service
        .generate_evidence_brief(dispute_id)
        .await
        .unwrap();

    // Assert top-level fields
    assert_eq!(brief.dispute_id, dispute_id);
    assert_eq!(brief.correlation_summary.status, CorrelationStatus::Matched);
    assert_eq!(
        brief.correlation_summary.category,
        EpistemicCategory::DeterministicInference
    );
    assert_eq!(
        brief.correlation_summary.evaluator,
        "AGENTAUDIT_LOCAL_CORRELATION_ENGINE"
    );
    assert_eq!(
        brief.correlation_summary.session_id.as_deref(),
        Some(session_id.as_str())
    );

    // Assert dispute summary
    assert_eq!(
        brief.dispute_summary.category,
        EpistemicCategory::RecordedPaypalData
    );
    assert_eq!(brief.dispute_summary.status, "UNDER_REVIEW");
    assert_eq!(
        brief.dispute_summary.reason.as_deref(),
        Some("UNAUTHORISED_TRANSACTION")
    );

    // Assert cryptographic proof
    assert_eq!(
        brief.cryptographic_proof.category,
        EpistemicCategory::VerifiedFact
    );
    assert!(brief.cryptographic_proof.purchase_chain_valid);
    assert!(brief.cryptographic_proof.dispute_chain_valid);
    assert!(brief.cryptographic_proof.dual_chain_integrity_verified);
    assert_eq!(
        brief.cryptographic_proof.genesis_anchor_type,
        GenesisAnchorType::PurchaseRootHash
    );
    assert_eq!(brief.cryptographic_proof.genesis_anchor_hash, root_hash);
    assert_eq!(brief.cryptographic_proof.purchase_total_steps, 8);
    assert!(brief
        .cryptographic_proof
        .purchase_root_hash_source
        .is_some());
    assert!(brief.cryptographic_proof.dispute_root_hash_source.is_some());

    // Assert agent audit trail
    let trail = brief.agent_audit_trail.expect("Trail must be present");
    assert_eq!(trail.category, EpistemicCategory::VerifiedFact);
    assert!(trail.initial_prompt.prompt.contains("noise cancelling"));
    assert_eq!(
        trail
            .catalog_search
            .as_ref()
            .unwrap()
            .candidates_evaluated_count,
        4
    );
    assert_eq!(trail.rejected_candidates.len(), 1);
    assert!(trail.rejected_candidates[0]
        .rejection_rationale
        .statement
        .contains("budget"));
    assert!(trail.rejected_candidates[0]
        .rejection_rationale
        .disclaimer
        .contains("agent-generated"));
    assert_eq!(
        trail.selected_product.as_ref().unwrap().product_name,
        "Sony WH-1000XM4"
    );
    assert_eq!(
        trail
            .purchase_authorization
            .as_ref()
            .unwrap()
            .maximum_authorized_amount,
        300.00
    );

    // Assert transaction proof
    let trans = brief
        .transaction_proof
        .expect("Transaction proof must be present");
    assert_eq!(
        trans.paypal_settlement.category,
        EpistemicCategory::RecordedPaypalData
    );
    assert_eq!(
        trans.paypal_settlement.provenance,
        SettlementProvenance::ReplayedFromLedgerPayload
    );
    assert_eq!(
        trans.paypal_settlement.paypal_order_id.as_deref(),
        Some(order_id)
    );
    assert_eq!(
        trans.paypal_settlement.paypal_capture_id.as_deref(),
        Some(capture_id)
    );
    assert_eq!(
        trans.ledger_settlement.category,
        EpistemicCategory::VerifiedFact
    );
    assert!(trans.ledger_settlement.order_created_step.is_some());
    assert!(trans.ledger_settlement.payment_captured_step.is_some());

    // Assert rebuttal summary for UNAUTHORISED_TRANSACTION
    assert_eq!(
        brief.rebuttal_summary.category,
        EpistemicCategory::DeterministicInference
    );
    assert_eq!(
        brief.rebuttal_summary.recommended_action,
        "CONTEST_WITH_EVIDENCE"
    );
    assert_eq!(brief.rebuttal_summary.primary_arguments.len(), 3);

    // 1. Recorded authorization is referenced
    let auth_arg = brief
        .rebuttal_summary
        .primary_arguments
        .iter()
        .find(|a| a.claim.contains("explicitly authorized purchase"))
        .expect("Authorization argument must be present");
    assert_eq!(auth_arg.category, EpistemicCategory::DeterministicInference);
    assert_eq!(auth_arg.source_references.len(), 1);
    assert_eq!(
        auth_arg.source_references[0].action_type.as_deref(),
        Some("PURCHASE_AUTHORIZED")
    );
    assert_eq!(auth_arg.source_references[0].sequence_index, Some(5));

    // 2. Buyer identity is not claimed as independently proven
    assert!(auth_arg
        .explanation
        .contains("does not independently establish physical biometric identity"));

    // 3. Authorized spending limit is compared with captured amount when both are available
    let ceiling_arg = brief
        .rebuttal_summary
        .primary_arguments
        .iter()
        .find(|a| a.claim.contains("authorized ceiling"))
        .expect("Spending ceiling argument must be present");
    assert_eq!(
        ceiling_arg.category,
        EpistemicCategory::DeterministicInference
    );
    assert_eq!(ceiling_arg.source_references.len(), 1);
    assert_eq!(
        ceiling_arg.source_references[0].action_type.as_deref(),
        Some("PAYPAL_PAYMENT_CAPTURED")
    );
    assert!(ceiling_arg.explanation.contains(capture_id));
    assert!(ceiling_arg.explanation.contains("278.00"));
    assert!(ceiling_arg
        .explanation
        .contains("within authorized customer spending limit"));

    // 4. Chain verification status is correctly reflected
    let chain_arg = brief
        .rebuttal_summary
        .primary_arguments
        .iter()
        .find(|a| a.claim.contains("Cryptographic hash chain"))
        .expect("Chain integrity argument must be present");
    assert_eq!(chain_arg.category, EpistemicCategory::VerifiedFact);
    assert!(chain_arg.explanation.contains("8 purchase steps"));
    assert!(chain_arg.explanation.contains(&root_hash));
    assert_eq!(chain_arg.source_references.len(), 1);
    assert_eq!(
        chain_arg.source_references[0].step_hash.as_deref(),
        Some(root_hash.as_str())
    );
}

#[tokio::test]
async fn test_evidence_brief_uncorrelated_dispute_no_session_evidence() {
    let ctx = setup_test_context().await;
    let dispute_id = "PP-D-UNCORRELATED-999";

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

    let brief = ctx
        .evidence_service
        .generate_evidence_brief(dispute_id)
        .await
        .unwrap();

    // Must have no session evidence
    assert_eq!(
        brief.correlation_summary.status,
        CorrelationStatus::NotFound
    );
    assert!(brief.correlation_summary.session_id.is_none());
    assert!(brief.agent_audit_trail.is_none());
    assert!(brief.transaction_proof.is_none());
    assert!(!brief.cryptographic_proof.dual_chain_integrity_verified);
    assert!(!brief.cryptographic_proof.purchase_chain_valid);
    assert_eq!(
        brief.cryptographic_proof.genesis_anchor_type,
        GenesisAnchorType::GenesisHash
    );
    assert_eq!(
        brief.cryptographic_proof.genesis_anchor_hash,
        GENESIS_HASH_DEFAULT
    );

    // Rebuttal must recommend MANUAL_REVIEW
    assert_eq!(brief.rebuttal_summary.recommended_action, "MANUAL_REVIEW");
    assert!(brief
        .rebuttal_summary
        .reason_specific_analysis
        .contains("could not be deterministically correlated"));
}

#[tokio::test]
async fn test_evidence_brief_manual_review_dispute() {
    let ctx = setup_test_context().await;
    let dispute_id = "PP-D-MANUAL-REVIEW-888";

    let payload = make_upsert_payload(
        dispute_id,
        None,
        None,
        None,
        None,
        Some("MERCHANDISE_OR_SERVICE_NOT_AS_DESCRIBED"),
        "200.00",
        CorrelationStatus::ManualReviewRequired,
        None,
    );

    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    let brief = ctx
        .evidence_service
        .generate_evidence_brief(dispute_id)
        .await
        .unwrap();

    assert_eq!(
        brief.correlation_summary.status,
        CorrelationStatus::ManualReviewRequired
    );
    assert!(brief.agent_audit_trail.is_none());
    assert!(brief.transaction_proof.is_none());
    assert_eq!(brief.rebuttal_summary.recommended_action, "MANUAL_REVIEW");
}

#[tokio::test]
async fn test_evidence_brief_detects_tampered_purchase_chain() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-TAMPER-001";
    let capture_id = "CAP-TAMPER-001";
    let dispute_id = "PP-D-TAMPER-001";

    let (session_id, root_hash) =
        create_complete_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

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

    // Tamper with sequence 3 payload directly in database
    sqlx::query(
        "UPDATE decision_steps SET payload = '{\"tampered\": true}' WHERE session_id = ? AND sequence_index = 3;",
    )
    .bind(&session_id)
    .execute(&ctx.pool)
    .await
    .unwrap();

    let brief = ctx
        .evidence_service
        .generate_evidence_brief(dispute_id)
        .await
        .unwrap();

    // Verification must detect tampering
    assert!(
        !brief.cryptographic_proof.purchase_chain_valid,
        "Purchase chain must be detected as tampered"
    );
    assert!(
        !brief.cryptographic_proof.dual_chain_integrity_verified,
        "Dual chain verification must fail"
    );
    assert!(
        brief.agent_audit_trail.is_none(),
        "Agent audit trail must be None when purchase chain is tampered"
    );
    assert!(
        brief.transaction_proof.is_none(),
        "Transaction proof must be None when purchase chain is tampered"
    );
    assert_eq!(brief.rebuttal_summary.recommended_action, "MANUAL_REVIEW");
    assert!(brief
        .rebuttal_summary
        .reason_specific_analysis
        .contains("tampering"));
}

#[tokio::test]
async fn test_evidence_brief_detects_tampered_dispute_chain() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-DISP-TAMPER";
    let capture_id = "CAP-DISP-TAMPER";
    let dispute_id = "PP-D-DISP-TAMPER";

    let (session_id, root_hash) =
        create_complete_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

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

    // Tamper with dispute step 0 in database
    sqlx::query(
        "UPDATE dispute_audit_steps SET payload = '{\"tampered\": true}' WHERE dispute_id = ? AND sequence_index = 0;",
    )
    .bind(dispute_id)
    .execute(&ctx.pool)
    .await
    .unwrap();

    let brief = ctx
        .evidence_service
        .generate_evidence_brief(dispute_id)
        .await
        .unwrap();

    assert!(
        !brief.cryptographic_proof.dispute_chain_valid,
        "Dispute audit chain must be detected as invalid"
    );
    assert!(!brief.cryptographic_proof.dual_chain_integrity_verified);
    assert_eq!(brief.rebuttal_summary.recommended_action, "MANUAL_REVIEW");
}

#[tokio::test]
async fn test_evidence_brief_rebuttal_reasons() {
    let ctx = setup_test_context().await;

    // 1. NOT_AS_DESCRIBED
    let (s1, r1) =
        create_complete_purchase_session(&ctx.ledger_service, "ORD-REAS-1", "CAP-REAS-1").await;
    let d1 = "PP-D-REAS-1";
    let p1 = make_upsert_payload(
        d1,
        Some(s1),
        Some(r1),
        Some("ORD-REAS-1"),
        Some("CAP-REAS-1"),
        Some("MERCHANDISE_OR_SERVICE_NOT_AS_DESCRIBED"),
        "278.00",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&p1).await.unwrap();

    let brief1 = ctx
        .evidence_service
        .generate_evidence_brief(d1)
        .await
        .unwrap();
    assert_eq!(
        brief1.rebuttal_summary.recommended_action,
        "CONTEST_WITH_EVIDENCE"
    );
    assert!(brief1
        .rebuttal_summary
        .primary_arguments
        .iter()
        .any(|a| a.claim.contains("specifications strictly matched")));

    // 2. NOT_RECEIVED
    let (s2, r2) =
        create_complete_purchase_session(&ctx.ledger_service, "ORD-REAS-2", "CAP-REAS-2").await;
    let d2 = "PP-D-REAS-2";
    let p2 = make_upsert_payload(
        d2,
        Some(s2),
        Some(r2),
        Some("ORD-REAS-2"),
        Some("CAP-REAS-2"),
        Some("MERCHANDISE_OR_SERVICE_NOT_RECEIVED"),
        "278.00",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );
    ctx.dispute_service.upsert_dispute(&p2).await.unwrap();

    let brief2 = ctx
        .evidence_service
        .generate_evidence_brief(d2)
        .await
        .unwrap();
    assert_eq!(
        brief2.rebuttal_summary.recommended_action,
        "CONTEST_WITH_FULFILLMENT_PROOF_REQUIRED"
    );
    assert!(brief2.rebuttal_summary.primary_arguments.iter().any(|a| a
        .claim
        .contains("Physical shipping tracking numbers are unavailable")));
}

#[tokio::test]
async fn test_evidence_brief_webhook_transmission_anchoring() {
    let ctx = setup_test_context().await;
    let dispute_id = "PP-D-WEBHOOK-ANCHOR";

    // Insert entry into webhook_events
    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            event_id, event_type, resource_type, resource_id, transmission_id,
            processing_status, retry_count, max_retries, raw_payload, payload_hash,
            created_at
        ) VALUES (
            'WH-EVT-999', 'PAYMENT.DISPUTE.CREATED', 'dispute', ?, 'TRANS-ID-ABC-123',
            'PROCESSED', 0, 5, X'0102', 'hash', '2026-11-09T18:00:00Z'
        );
        "#,
    )
    .bind(dispute_id)
    .execute(&ctx.pool)
    .await
    .unwrap();

    let payload = make_upsert_payload(
        dispute_id,
        None,
        None,
        None,
        None,
        Some("UNAUTHORISED_TRANSACTION"),
        "100.00",
        CorrelationStatus::NotFound,
        None,
    );

    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    let brief = ctx
        .evidence_service
        .generate_evidence_brief(dispute_id)
        .await
        .unwrap();

    let src = brief
        .dispute_summary
        .source
        .expect("Source must be present");
    assert_eq!(
        src.webhook_transmission_id.as_deref(),
        Some("TRANS-ID-ABC-123")
    );
    assert_eq!(src.paypal_record_id.as_deref(), Some("WH-EVT-999"));
}

#[tokio::test]
async fn test_evidence_brief_disclosures_and_integrity_rules() {
    let ctx = setup_test_context().await;
    let order_id = "ORD-DISC-001";
    let capture_id = "CAP-DISC-001";
    let dispute_id = "PP-D-DISC-001";

    let (s, r) = create_complete_purchase_session(&ctx.ledger_service, order_id, capture_id).await;

    let payload = make_upsert_payload(
        dispute_id,
        Some(s),
        Some(r),
        Some(order_id),
        Some(capture_id),
        Some("UNAUTHORISED_TRANSACTION"),
        "278.00",
        CorrelationStatus::Matched,
        Some(MatchedBy::CaptureId),
    );

    ctx.dispute_service.upsert_dispute(&payload).await.unwrap();

    let brief = ctx
        .evidence_service
        .generate_evidence_brief(dispute_id)
        .await
        .unwrap();

    // Verify unavailable evidence disclaimers are present
    assert_eq!(
        brief.missing_evidence.category,
        EpistemicCategory::UnavailableEvidence
    );
    assert!(brief
        .missing_evidence
        .missing_items
        .iter()
        .any(|i| i.evidence_type
            == UnavailableEvidenceType::PhysicalShippingTrackingAndProofOfDelivery));
    assert!(brief
        .missing_evidence
        .missing_items
        .iter()
        .any(|i| i.evidence_type == UnavailableEvidenceType::BuyerPhysicalIdentityProof));
    assert!(brief
        .missing_evidence
        .missing_items
        .iter()
        .any(|i| i.evidence_type == UnavailableEvidenceType::CardholderBankCommunications));

    // Verify authorization identity disclaimer
    let auth = brief
        .agent_audit_trail
        .unwrap()
        .purchase_authorization
        .unwrap();
    assert!(auth
        .identity_disclaimer
        .contains("does not independently establish the physical identity"));

    // Verify hash chain scope statement
    assert!(brief
        .cryptographic_proof
        .integrity_scope_statement
        .contains("does not prevent an attacker with full database access"));
}

#[tokio::test]
async fn test_evidence_brief_nonexistent_dispute_returns_error() {
    let ctx = setup_test_context().await;
    let res = ctx
        .evidence_service
        .generate_evidence_brief("NONEXISTENT_DISPUTE_ID")
        .await;

    assert!(res.is_err());
    match res.unwrap_err() {
        AppError::DisputeNotFound(_) => (),
        other => panic!("Expected DisputeNotFound, got {:?}", other),
    }
}
