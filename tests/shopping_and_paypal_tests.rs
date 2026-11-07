use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::post;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower::ServiceExt;

use agentic_commerce::config::Config;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::{
    AuthorizePurchaseRequest, AuthorizePurchaseResponse, CaptureOrderResponse, CreateOrderResponse,
    Product, ShoppingSessionDetail, ShoppingSessionResponse, VerificationStatus,
};
use agentic_commerce::services::{
    CatalogService, LedgerService, PayPalAuthService, PayPalOrdersService, RequirementExtractor,
    ShoppingAgentService, VerificationService,
};
use agentic_commerce::{create_router, AppState};

// =========================================================================
// MOCK PAYPAL SERVER FOR ORDERS & CAPTURE TESTS
// =========================================================================

#[derive(Clone)]
struct MockPayPalState {
    order_create_count: Arc<AtomicUsize>,
    order_capture_count: Arc<AtomicUsize>,
    last_bearer_token: Arc<RwLock<Option<String>>>,
    last_order_payload: Arc<RwLock<Option<serde_json::Value>>>,
    order_create_response_status: Arc<RwLock<StatusCode>>,
    order_create_response_body: Arc<RwLock<String>>,
    capture_response_status: Arc<RwLock<StatusCode>>,
    capture_response_body: Arc<RwLock<String>>,
}

async fn start_mock_paypal_orders_server(
    create_status: StatusCode,
    create_body: String,
    capture_status: StatusCode,
    capture_body: String,
) -> (String, MockPayPalState) {
    let state = MockPayPalState {
        order_create_count: Arc::new(AtomicUsize::new(0)),
        order_capture_count: Arc::new(AtomicUsize::new(0)),
        last_bearer_token: Arc::new(RwLock::new(None)),
        last_order_payload: Arc::new(RwLock::new(None)),
        order_create_response_status: Arc::new(RwLock::new(create_status)),
        order_create_response_body: Arc::new(RwLock::new(create_body)),
        capture_response_status: Arc::new(RwLock::new(capture_status)),
        capture_response_body: Arc::new(RwLock::new(capture_body)),
    };

    // Handler for OAuth token retrieval (for authentication)
    let token_handler = |_req: Request| async {
        let body = json!({
            "access_token": "mock_paypal_orders_token_9999",
            "token_type": "Bearer",
            "expires_in": 3600
        });
        (
            StatusCode::OK,
            [("content-type", "application/json")],
            body.to_string(),
        )
    };

    // Handler for order creation: POST /v2/checkout/orders
    let orders_state = state.clone();
    let orders_handler = move |req: Request| {
        let s = orders_state.clone();
        async move {
            s.order_create_count.fetch_add(1, Ordering::SeqCst);

            if let Some(auth) = req.headers().get(header::AUTHORIZATION) {
                *s.last_bearer_token.write().await =
                    Some(auth.to_str().unwrap_or_default().to_string());
            }

            let bytes = to_bytes(req.into_body(), 1024 * 64)
                .await
                .unwrap_or_default();
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                *s.last_order_payload.write().await = Some(val);
            }

            let status = *s.order_create_response_status.read().await;
            let body = s.order_create_response_body.read().await.clone();

            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap()
        }
    };

    // Handler for capture: POST /v2/checkout/orders/:id/capture
    let capture_state = state.clone();
    let capture_handler = move |_req: Request| {
        let s = capture_state.clone();
        async move {
            s.order_capture_count.fetch_add(1, Ordering::SeqCst);

            let status = *s.capture_response_status.read().await;
            let body = s.capture_response_body.read().await.clone();

            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap()
        }
    };

    let router = axum::Router::new()
        .route("/v1/oauth2/token", post(token_handler))
        .route("/v2/checkout/orders", post(orders_handler))
        .route("/v2/checkout/orders/:id/capture", post(capture_handler));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    (url, state)
}

/// Helper to set up an in-memory application connected to a mock PayPal base URL.
async fn setup_app_with_paypal_url(base_url: &str) -> (axum::Router, AppState) {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let config = Arc::new(Config {
        paypal_client_id: "test_client_id".to_string(),
        paypal_client_secret: "test_client_secret".to_string(),
        paypal_base_url: base_url.to_string(),
        port: 5000,
        rust_log: "info".to_string(),
        database_url: "sqlite::memory:".to_string(),
        paypal_webhook_id: None,
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

    let state = AppState {
        config,
        http_client,
        auth_service,
        ledger_service,
        verification_service,
        catalog_service,
        shopping_agent,
        paypal_orders,
    };

    let router = create_router(state.clone());
    (router, state)
}

// =========================================================================
// 1. PRODUCT CATALOG TESTS (TASK 3.1)
// =========================================================================

#[test]
fn test_catalog_listing_and_counts() {
    let catalog = CatalogService::new();
    let products = catalog.list_products();
    assert!(
        products.len() >= 8,
        "Catalog must contain at least 8 seeded headphone products (found {})",
        products.len()
    );

    // Verify all products are in USD currency
    for p in &products {
        assert_eq!(p.currency, "USD");
        assert!(p.price > 0.0);
        assert!(!p.name.is_empty());
        assert!(!p.brand.is_empty());
        assert!(p.available_stock > 0);
    }
}

#[test]
fn test_catalog_product_retrieval() {
    let catalog = CatalogService::new();

    // Valid product ID
    let product = catalog.get_product("hp-anker-q30");
    assert!(product.is_some());
    let p = product.unwrap();
    assert_eq!(p.brand, "Anker");
    assert_eq!(p.price, 79.99);
    assert!(p.specifications.noise_cancelling);
    assert_eq!(p.specifications.battery_life_hours, 40);

    // Missing product ID
    let missing = catalog.get_product("non-existent-product-id");
    assert!(missing.is_none());
}

#[test]
fn test_catalog_search_behavior() {
    let catalog = CatalogService::new();

    // Search by brand
    let sony_products = catalog.search_products("Sony", None);
    assert!(sony_products.len() >= 2);
    assert!(sony_products.iter().all(|p| p.brand == "Sony"));

    // Search by keyword "noise"
    let anc_products = catalog.search_products("noise", Some("headphones"));
    assert!(!anc_products.is_empty());

    // Search by specific feature
    let jbl = catalog.search_products("pure-bass", None);
    assert_eq!(jbl.len(), 1);
    assert_eq!(jbl[0].brand, "JBL");
}

// =========================================================================
// 2. REQUIREMENT EXTRACTION TESTS (TASK 3.2)
// =========================================================================

#[test]
fn test_requirement_extraction_canonical_prompt() {
    let prompt = "Buy noise-canceling headphones under $100 with at least 30 hours battery life.";
    let req = RequirementExtractor::extract(prompt).unwrap();

    assert_eq!(req.category, "headphones");
    assert_eq!(req.maximum_budget, 100.0);
    assert_eq!(req.currency, "USD");
    assert_eq!(req.minimum_battery_hours, Some(30));
    assert!(req
        .required_features
        .contains(&"noise-canceling".to_string()));
}

#[test]
fn test_requirement_extraction_brand_and_price_variations() {
    let prompt1 = "I need Sony wireless headphones below $150 with 35h battery";
    let req1 = RequirementExtractor::extract(prompt1).unwrap();
    assert_eq!(req1.preferred_brand, Some("Sony".to_string()));
    assert_eq!(req1.maximum_budget, 150.0);
    assert_eq!(req1.minimum_battery_hours, Some(35));
    assert!(req1.required_features.contains(&"wireless".to_string()));

    let prompt2 = "Anker headset max 80 dollars";
    let req2 = RequirementExtractor::extract(prompt2).unwrap();
    assert_eq!(req2.preferred_brand, Some("Anker".to_string()));
    assert_eq!(req2.maximum_budget, 80.0);
    assert_eq!(req2.category, "headset");
}

#[test]
fn test_requirement_extraction_unsupported_currency_rejected() {
    let prompt_inr = "Buy headphones under ₹5000";
    let res_inr = RequirementExtractor::extract(prompt_inr);
    assert!(
        res_inr.is_err(),
        "INR currency must be rejected with helpful guidance"
    );

    let prompt_eur = "Buy headphones under 100 EUR";
    let res_eur = RequirementExtractor::extract(prompt_eur);
    assert!(res_eur.is_err(), "EUR currency must be rejected");
}

#[test]
fn test_requirement_extraction_invalid_inputs_rejected() {
    // Empty prompt
    assert!(RequirementExtractor::extract("   ").is_err());

    // Missing budget
    assert!(RequirementExtractor::extract("Buy some headphones").is_err());

    // Zero budget
    assert!(RequirementExtractor::extract("Buy headphones under $0").is_err());
}

// =========================================================================
// 3. REFERENCE SHOPPING AGENT TESTS (TASK 3.3)
// =========================================================================

#[tokio::test]
async fn test_shopping_agent_canonical_selection() {
    let (_app, state) = setup_app_with_paypal_url("http://mock-paypal.local").await;

    // Run shopping agent with canonical user prompt
    let prompt = "Buy noise-canceling headphones under $100 with at least 30 hours battery life.";
    let res = state
        .shopping_agent
        .process_shopping_request(prompt)
        .await
        .unwrap();

    assert!(res.success);
    assert!(res.selected_product.is_some());
    let selected = res.selected_product.unwrap();

    // Verify selected product is hp-anker-q30 ($79.99, 40h battery, ANC)
    assert_eq!(selected.product_id, "hp-anker-q30");
    assert_eq!(selected.brand, "Anker");
    assert!(selected.price <= 100.0);
    assert!(selected.specifications.battery_life_hours >= 30);
    assert!(selected.specifications.noise_cancelling);

    // Verify rejection records exist for products failing constraints
    assert!(!res.rejected_candidates.is_empty());

    // Verify Sony XM5 ($399.99) rejected for price
    let xm5_rejection = res
        .rejected_candidates
        .iter()
        .find(|r| r.product_id == "hp-sony-wh1000xm5");
    assert!(xm5_rejection.is_some());
    assert!(xm5_rejection
        .unwrap()
        .rejection_reason
        .contains("exceeds customer budget limit"));

    // Verify Audio-Technica M20x rejected for missing ANC
    let m20x_rejection = res
        .rejected_candidates
        .iter()
        .find(|r| r.product_id == "hp-audio-technica-m20x");
    assert!(m20x_rejection.is_some());
    assert!(m20x_rejection
        .unwrap()
        .rejection_reason
        .contains("lacks active noise cancellation"));

    // Verify Skullcandy Hesh ANC rejected for battery (22h < 30h)
    let hesh_rejection = res
        .rejected_candidates
        .iter()
        .find(|r| r.product_id == "hp-skullcandy-hesh");
    assert!(hesh_rejection.is_some());
    assert!(hesh_rejection
        .unwrap()
        .rejection_reason
        .contains("is below required minimum"));

    // Verify cryptographic ledger integrity of the shopping session
    let verification = state
        .verification_service
        .verify_session(&res.session_id)
        .await
        .unwrap();
    assert_eq!(verification.status, VerificationStatus::Valid);

    // Verify all milestones were recorded in correct sequence
    let events = state
        .ledger_service
        .get_session_events(&res.session_id)
        .await
        .unwrap();
    let action_types: Vec<&str> = events.iter().map(|e| e.action_type.as_str()).collect();
    assert_eq!(action_types[0], "INITIAL_PROMPT");
    assert_eq!(action_types[1], "REQUIREMENTS_EXTRACTED");
    assert_eq!(action_types[2], "CATALOG_SEARCH");
    assert!(action_types.contains(&"PRODUCT_REJECTED"));
    assert!(action_types.contains(&"PRODUCT_COMPARISON"));
    assert_eq!(*action_types.last().unwrap(), "PRODUCT_SELECTED");
}

#[tokio::test]
async fn test_shopping_agent_no_matching_product_preserves_session() {
    let (_, state) = setup_app_with_paypal_url("http://mock-paypal.local").await;

    // Budget of $20 is below all catalog products
    let prompt = "Buy headphones under $20";
    let res = state
        .shopping_agent
        .process_shopping_request(prompt)
        .await
        .unwrap();

    assert!(res.success);
    assert!(res.selected_product.is_none());
    assert!(res
        .selection_rationale
        .unwrap()
        .contains("No catalog products matched"));

    // Audit session must be preserved and valid
    let session = state
        .ledger_service
        .get_session(&res.session_id)
        .await
        .unwrap();
    assert!(
        session.root_hash.is_none(),
        "Session should remain open without false finalization"
    );

    let verification = state
        .verification_service
        .verify_session(&res.session_id)
        .await
        .unwrap();
    assert_eq!(verification.status, VerificationStatus::Valid);
}

// =========================================================================
// 4. PURCHASE AUTHORIZATION TESTS (TASK 3.3.1)
// =========================================================================

#[tokio::test]
async fn test_purchase_authorization_lifecycle() {
    let (_, state) = setup_app_with_paypal_url("http://mock-paypal.local").await;

    let shop_res = state
        .shopping_agent
        .process_shopping_request(
            "Buy noise-canceling headphones under $100 with at least 30 hours battery life.",
        )
        .await
        .unwrap();
    let session_id = shop_res.session_id;

    // Case 1: Customer authorizes with sufficient spending limit -> SUCCESS
    let auth_req = AuthorizePurchaseRequest {
        selected_product_id: "hp-anker-q30".to_string(),
        maximum_authorized_amount: 85.00, // Price is $79.99
        currency: "USD".to_string(),
        approved: true,
    };
    let auth_res = state
        .shopping_agent
        .authorize_purchase(&session_id, auth_req)
        .await
        .unwrap();

    assert!(auth_res.success);
    assert_eq!(auth_res.authorization.selected_product_id, "hp-anker-q30");
    assert_eq!(auth_res.authorization.maximum_authorized_amount, 85.00);

    // Verify PURCHASE_AUTHORIZED was recorded in hash-chain
    let events = state
        .ledger_service
        .get_session_events(&session_id)
        .await
        .unwrap();
    assert_eq!(events.last().unwrap().action_type, "PURCHASE_AUTHORIZED");

    // Case 2: Duplicate authorization attempt -> REJECTED
    let dup_req = AuthorizePurchaseRequest {
        selected_product_id: "hp-anker-q30".to_string(),
        maximum_authorized_amount: 85.00,
        currency: "USD".to_string(),
        approved: true,
    };
    let dup_res = state
        .shopping_agent
        .authorize_purchase(&session_id, dup_req)
        .await;
    assert!(dup_res.is_err(), "Duplicate authorization must be rejected");
}

#[tokio::test]
async fn test_purchase_authorization_rejected_cases() {
    let (_, state) = setup_app_with_paypal_url("http://mock-paypal.local").await;

    let shop_res = state
        .shopping_agent
        .process_shopping_request("Buy headphones under $100")
        .await
        .unwrap();
    let session_id = shop_res.session_id;

    // Case A: Customer declines approval (`approved: false`)
    let decline_req = AuthorizePurchaseRequest {
        selected_product_id: "hp-anker-q30".to_string(),
        maximum_authorized_amount: 100.0,
        currency: "USD".to_string(),
        approved: false,
    };
    let decline_res = state
        .shopping_agent
        .authorize_purchase(&session_id, decline_req)
        .await;
    assert!(decline_res.is_err());

    // Case B: Authorized amount is LESS than product price ($70 < $79.99)
    let underfund_req = AuthorizePurchaseRequest {
        selected_product_id: "hp-anker-q30".to_string(),
        maximum_authorized_amount: 70.00,
        currency: "USD".to_string(),
        approved: true,
    };
    let underfund_res = state
        .shopping_agent
        .authorize_purchase(&session_id, underfund_req)
        .await;
    assert!(underfund_res.is_err());

    // Case C: Authorized product ID does not match selected product
    let mismatch_req = AuthorizePurchaseRequest {
        selected_product_id: "hp-bose-qc45".to_string(),
        maximum_authorized_amount: 500.00,
        currency: "USD".to_string(),
        approved: true,
    };
    let mismatch_res = state
        .shopping_agent
        .authorize_purchase(&session_id, mismatch_req)
        .await;
    assert!(mismatch_res.is_err());
}

// =========================================================================
// 5. REAL PAYPAL ORDERS v2 INTEGRATION & LIFECYCLE (TASK 3.4)
// =========================================================================

#[tokio::test]
async fn test_paypal_order_creation_requires_prior_authorization() {
    let (_, state) = setup_app_with_paypal_url("http://mock-paypal.local").await;

    let shop_res = state
        .shopping_agent
        .process_shopping_request(
            "Buy noise-canceling headphones under $100 with at least 30 hours battery life.",
        )
        .await
        .unwrap();
    let session_id = shop_res.session_id;

    // Attempting to create a PayPal order BEFORE customer authorization must fail
    let order_res = state.paypal_orders.create_order(&session_id).await;
    assert!(
        order_res.is_err(),
        "Creating a PayPal order without prior customer authorization must be rejected"
    );
}

#[tokio::test]
async fn test_paypal_order_creation_and_capture_full_lifecycle() {
    let mock_order_id = "PAYPAL-ORDER-123456789";
    let mock_approval_url =
        "https://www.sandbox.paypal.com/checkoutnow?token=PAYPAL-ORDER-123456789";
    let mock_capture_id = "CAPTURE-987654321";

    let create_response_json = json!({
        "id": mock_order_id,
        "status": "CREATED",
        "links": [
            { "href": "https://api-m.sandbox.paypal.com/v2/checkout/orders/PAYPAL-ORDER-123456789", "rel": "self", "method": "GET" },
            { "href": mock_approval_url, "rel": "approve", "method": "GET" },
            { "href": "https://api-m.sandbox.paypal.com/v2/checkout/orders/PAYPAL-ORDER-123456789/capture", "rel": "capture", "method": "POST" }
        ]
    }).to_string();

    let capture_response_json = json!({
        "id": mock_order_id,
        "status": "COMPLETED",
        "purchase_units": [
            {
                "reference_id": "session-ref",
                "payments": {
                    "captures": [
                        {
                            "id": mock_capture_id,
                            "status": "COMPLETED",
                            "amount": { "currency_code": "USD", "value": "79.99" }
                        }
                    ]
                }
            }
        ]
    })
    .to_string();

    let (mock_url, mock_state) = start_mock_paypal_orders_server(
        StatusCode::CREATED,
        create_response_json,
        StatusCode::CREATED,
        capture_response_json,
    )
    .await;

    let (_, state) = setup_app_with_paypal_url(&mock_url).await;

    // Step 1: Shopping agent execution
    let shop_res = state
        .shopping_agent
        .process_shopping_request(
            "Buy noise-canceling headphones under $100 with at least 30 hours battery life.",
        )
        .await
        .unwrap();
    let session_id = shop_res.session_id;

    // Step 2: Customer purchase authorization
    state
        .shopping_agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: "hp-anker-q30".to_string(),
                maximum_authorized_amount: 80.00,
                currency: "USD".to_string(),
                approved: true,
            },
        )
        .await
        .unwrap();

    // Step 3: Create PayPal order
    let order_res = state.paypal_orders.create_order(&session_id).await.unwrap();
    assert!(order_res.success);
    assert_eq!(order_res.order_id, mock_order_id);
    assert_eq!(order_res.status, "CREATED");
    assert_eq!(order_res.approval_url, Some(mock_approval_url.to_string()));

    // Verify mock received correct payload and Bearer token
    assert_eq!(mock_state.order_create_count.load(Ordering::SeqCst), 1);
    let bearer = mock_state.last_bearer_token.read().await.clone().unwrap();
    assert!(bearer.starts_with("Bearer "));

    let payload = mock_state.last_order_payload.read().await.clone().unwrap();
    assert_eq!(payload["intent"], "CAPTURE");
    assert_eq!(payload["purchase_units"][0]["amount"]["value"], "79.99");
    assert_eq!(
        payload["purchase_units"][0]["amount"]["currency_code"],
        "USD"
    );

    // Verify session updated with PayPal Order ID
    let session_mid = state.ledger_service.get_session(&session_id).await.unwrap();
    assert_eq!(session_mid.paypal_order_id, Some(mock_order_id.to_string()));
    assert!(
        session_mid.root_hash.is_none(),
        "Session must not be finalized before capture"
    );

    // Attempting duplicate order creation must fail
    assert!(state.paypal_orders.create_order(&session_id).await.is_err());

    // Step 4: Capture PayPal order
    let capture_res = state
        .paypal_orders
        .capture_order(&session_id)
        .await
        .unwrap();
    assert!(capture_res.success);
    assert_eq!(capture_res.status, "COMPLETED");
    assert_eq!(capture_res.capture_id, Some(mock_capture_id.to_string()));

    // Verify session is now finalized and root hash matches capture event
    let session_final = state.ledger_service.get_session(&session_id).await.unwrap();
    assert_eq!(
        session_final.root_hash,
        Some(capture_res.final_root_hash.clone())
    );

    // Verify cryptographic integrity of entire session chain
    let verification = state
        .verification_service
        .verify_session(&session_id)
        .await
        .unwrap();
    assert_eq!(verification.status, VerificationStatus::Valid);
    assert_eq!(
        verification.calculated_root_hash,
        Some(capture_res.final_root_hash)
    );

    // Attempting to append after finalization must return conflict
    let post_capture_append = state
        .ledger_service
        .append_event(
            &session_id,
            agentic_commerce::models::AppendEventRequest {
                action_type: "EXTRA_STEP".to_string(),
                payload: json!({}),
            },
        )
        .await;
    assert!(post_capture_append.is_err());
}

#[tokio::test]
async fn test_paypal_order_capture_failure_does_not_finalize() {
    let (mock_url, _) = start_mock_paypal_orders_server(
        StatusCode::CREATED,
        json!({
            "id": "PAYPAL-ORDER-FAIL",
            "status": "CREATED",
            "links": []
        })
        .to_string(),
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({
            "name": "UNPROCESSABLE_ENTITY",
            "message": "Order is not approved by buyer yet"
        })
        .to_string(),
    )
    .await;

    let (_, state) = setup_app_with_paypal_url(&mock_url).await;

    let shop_res = state
        .shopping_agent
        .process_shopping_request(
            "Buy noise-canceling headphones under $100 with at least 30 hours battery life.",
        )
        .await
        .unwrap();
    let session_id = shop_res.session_id;

    state
        .shopping_agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: "hp-anker-q30".to_string(),
                maximum_authorized_amount: 80.00,
                currency: "USD".to_string(),
                approved: true,
            },
        )
        .await
        .unwrap();

    state.paypal_orders.create_order(&session_id).await.unwrap();

    // Capture attempt fails
    let cap_res = state.paypal_orders.capture_order(&session_id).await;
    assert!(
        cap_res.is_err(),
        "Capture should fail when buyer has not approved"
    );

    // Session must NOT be finalized as paid
    let session = state.ledger_service.get_session(&session_id).await.unwrap();
    assert!(
        session.root_hash.is_none(),
        "Failed capture must not finalize session"
    );

    // Chain integrity should still be valid with the failure event recorded
    let verify = state
        .verification_service
        .verify_session(&session_id)
        .await
        .unwrap();
    assert_eq!(verify.status, VerificationStatus::Valid);
}

// =========================================================================
// 6. REST API INTEGRATION TESTS (TASK 3.5)
// =========================================================================

#[tokio::test]
async fn test_api_catalog_products() {
    let (app, _) = setup_app_with_paypal_url("http://mock-paypal.local").await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/catalog/products")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let products: Vec<Product> = serde_json::from_slice(&bytes).unwrap();
    assert!(products.len() >= 8);
}

#[tokio::test]
async fn test_api_shopping_and_lifecycle_end_to_end() {
    let mock_order_id = "PAYPAL-REST-777";
    let mock_approval_url = "https://www.sandbox.paypal.com/checkoutnow?token=PAYPAL-REST-777";

    let (mock_url, _) = start_mock_paypal_orders_server(
        StatusCode::CREATED,
        json!({
            "id": mock_order_id,
            "status": "CREATED",
            "links": [{ "href": mock_approval_url, "rel": "approve" }]
        }).to_string(),
        StatusCode::OK,
        json!({
            "id": mock_order_id,
            "status": "COMPLETED",
            "purchase_units": [{ "payments": { "captures": [{ "id": "CAP-777", "status": "COMPLETED" }] } }]
        }).to_string(),
    )
    .await;

    let (app, _) = setup_app_with_paypal_url(&mock_url).await;

    // 1. POST /api/shopping/sessions -> 201 Created
    let shop_req_body = json!({
        "prompt": "Buy noise-canceling headphones under $100 with at least 30 hours battery life."
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/shopping/sessions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&shop_req_body).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let shop_res: ShoppingSessionResponse = serde_json::from_slice(&body).unwrap();
    let session_id = shop_res.session_id;

    // 2. GET /api/shopping/sessions/{session_id} -> 200 OK
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/shopping/sessions/{}", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let detail: ShoppingSessionDetail = serde_json::from_slice(&body).unwrap();
    assert_eq!(detail.session_id, session_id);
    assert_eq!(detail.verification.status, VerificationStatus::Valid);

    // 3. POST /api/shopping/sessions/{session_id}/authorize -> 200 OK
    let auth_req_body = json!({
        "selected_product_id": "hp-anker-q30",
        "maximum_authorized_amount": 80.00,
        "currency": "USD",
        "approved": true
    });
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/shopping/sessions/{}/authorize", session_id))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&auth_req_body).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let auth_res: AuthorizePurchaseResponse = serde_json::from_slice(&body).unwrap();
    assert!(auth_res.success);

    // 4. POST /api/shopping/sessions/{session_id}/orders -> 201 Created
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/shopping/sessions/{}/orders", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let order_res: CreateOrderResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(order_res.order_id, mock_order_id);

    // 5. POST /api/shopping/sessions/{session_id}/capture -> 200 OK
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/shopping/sessions/{}/capture", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let cap_res: CaptureOrderResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(cap_res.status, "COMPLETED");
    assert_eq!(cap_res.capture_id, Some("CAP-777".to_string()));

    // 6. Verify final session details
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/shopping/sessions/{}", session_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let final_detail: ShoppingSessionDetail = serde_json::from_slice(&body).unwrap();
    assert!(final_detail.is_finalized);
    assert_eq!(
        final_detail.paypal_order_id,
        Some(mock_order_id.to_string())
    );
    assert_eq!(final_detail.verification.status, VerificationStatus::Valid);
    assert_eq!(final_detail.root_hash, Some(cap_res.final_root_hash));
}
