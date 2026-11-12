use std::sync::Arc;

use agentic_commerce::config::Config;
use agentic_commerce::db::init_db;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::{AuthorizePurchaseRequest, VerificationStatus};
use agentic_commerce::services::{
    CatalogService, LedgerService, PayPalAuthService, PayPalOrdersService, RequirementExtractor,
    ShoppingAgentService, VerificationService,
};

/// Helper to initialize test services backed by SQLite in-memory and real environment config.
async fn setup_real_env_services() -> (
    Arc<Config>,
    Arc<LedgerService>,
    Arc<VerificationService>,
    CatalogService,
    Arc<ShoppingAgentService>,
    Arc<PayPalOrdersService>,
    Arc<PayPalAuthService>,
) {
    dotenvy::dotenv().ok();
    let mut config = Config::from_env();
    config.database_url = "sqlite::memory:".to_string();
    let config = Arc::new(config);

    let pool = init_db("sqlite::memory:").await.unwrap();
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

    (
        config,
        ledger_service,
        verification_service,
        catalog_service,
        shopping_agent,
        paypal_orders,
        auth_service,
    )
}

// =========================================================================
// SECTION 3: SHOPPING AGENT SCENARIOS (A THROUGH F)
// =========================================================================

#[tokio::test]
async fn test_scenario_a_valid_request() {
    let (_, ledger, verifier, _, agent, _, _) = setup_real_env_services().await;

    let prompt = "Buy noise-canceling headphones under $100 with at least 30 hours battery life.";
    let res = agent.process_shopping_request(prompt).await.unwrap();

    assert!(res.success);
    assert!(res.selected_product.is_some());
    let selected = res.selected_product.unwrap();

    println!("\n=== SCENARIO A: VALID REQUEST ===");
    println!("Selected: {} (${:.2})", selected.name, selected.price);
    println!(
        "Rationale: {}",
        res.selection_rationale.as_deref().unwrap_or("")
    );

    assert_eq!(selected.product_id, "hp-anker-q30");
    assert!(selected.price <= 100.0);
    assert!(selected.specifications.battery_life_hours >= 30);
    assert!(selected.specifications.noise_cancelling);

    // Verify ledger integrity
    let verify = verifier.verify_session(&res.session_id).await.unwrap();
    assert_eq!(verify.status, VerificationStatus::Valid);

    let events = ledger.get_session_events(&res.session_id).await.unwrap();
    assert!(events.len() >= 6);
}

#[tokio::test]
async fn test_scenario_b_budget_violation() {
    let (_, ledger, verifier, _, agent, _, _) = setup_real_env_services().await;

    let prompt = "Buy headphones under $40.";
    let res = agent.process_shopping_request(prompt).await.unwrap();

    println!("\n=== SCENARIO B: BUDGET VIOLATION ===");
    println!(
        "Selected: {:?}",
        res.selected_product.as_ref().map(|p| &p.name)
    );
    println!(
        "Rationale: {}",
        res.selection_rationale.as_deref().unwrap_or("")
    );

    // Lowest catalog price is $49.99 (hp-anker-q20); no product should be selected
    assert!(
        res.selected_product.is_none(),
        "No product should be selected under $40"
    );
    assert!(!res.rejected_candidates.is_empty());
    assert!(res
        .rejected_candidates
        .iter()
        .all(|r| r.rejection_reason.contains("exceeds customer budget limit")));

    // Verify session remains open and ledger is intact
    let session = ledger.get_session(&res.session_id).await.unwrap();
    assert!(session.root_hash.is_none());

    let verify = verifier.verify_session(&res.session_id).await.unwrap();
    assert_eq!(verify.status, VerificationStatus::Valid);
}

#[tokio::test]
async fn test_scenario_c_battery_violation() {
    let (_, _, verifier, _, agent, _, _) = setup_real_env_services().await;

    let prompt = "Buy headphones with at least 50 hours battery life under $100.";
    let res = agent.process_shopping_request(prompt).await.unwrap();

    println!("\n=== SCENARIO C: BATTERY REQUIREMENT ===");
    println!(
        "Selected: {:?}",
        res.selected_product.as_ref().map(|p| &p.name)
    );

    assert!(res.selected_product.is_some());
    let selected = res.selected_product.unwrap();

    // hp-audio-technica-m20x has 60 hours battery life and is $79.00
    assert_eq!(selected.product_id, "hp-audio-technica-m20x");
    assert!(selected.specifications.battery_life_hours >= 50);

    let verify = verifier.verify_session(&res.session_id).await.unwrap();
    assert_eq!(verify.status, VerificationStatus::Valid);
}

#[tokio::test]
async fn test_scenario_d_conflicting_requirements() {
    let (_, _, verifier, _, agent, _, _) = setup_real_env_services().await;

    // Sony headphones in catalog are $148.00 and $399.99. None are under $50.
    let prompt = "Buy Sony headphones under $50.";
    let res = agent.process_shopping_request(prompt).await.unwrap();

    println!("\n=== SCENARIO D: CONFLICTING REQUIREMENTS ===");
    println!(
        "Selected: {:?}",
        res.selected_product.as_ref().map(|p| &p.name)
    );

    // Agent must NOT silently pick a non-Sony product or violate the $50 budget
    assert!(
        res.selected_product.is_none(),
        "Must not select any product when Sony headphones exceed $50"
    );

    let verify = verifier.verify_session(&res.session_id).await.unwrap();
    assert_eq!(verify.status, VerificationStatus::Valid);
}

#[tokio::test]
async fn test_scenario_e_invalid_currency() {
    let res_eur = RequirementExtractor::extract("Buy headphones under 100 EUR");
    assert!(res_eur.is_err());
    let err_eur = res_eur.err().unwrap().to_string();
    println!("\n=== SCENARIO E: UNSUPPORTED CURRENCY ===");
    println!("EUR error: {}", err_eur);
    assert!(err_eur.contains("Currency 'EUR' is not supported"));

    let res_inr = RequirementExtractor::extract("Buy headphones under ₹5000");
    assert!(res_inr.is_err());
    let err_inr = res_inr.err().unwrap().to_string();
    println!("INR error: {}", err_inr);
    assert!(err_inr.contains("Currency 'INR' is not supported"));
}

#[tokio::test]
async fn test_scenario_f_empty_or_malformed_prompt() {
    let (_, _, _, _, agent, _, _) = setup_real_env_services().await;

    // Empty prompt
    let res_empty = agent.process_shopping_request("   ").await;
    assert!(res_empty.is_err());

    // Prompt missing budget constraint
    let res_no_budget = agent
        .process_shopping_request("Buy noise-canceling headphones")
        .await;
    assert!(res_no_budget.is_err());
    assert!(res_no_budget
        .err()
        .unwrap()
        .to_string()
        .contains("Could not determine maximum budget"));

    println!("\n=== SCENARIO F: MALFORMED PROMPTS REJECTED SAFELY ===");
}

// =========================================================================
// SECTION 4: AUTHORIZATION SECURITY TESTS
// =========================================================================

#[tokio::test]
async fn test_authorization_security_vectors() {
    let (_, ledger, _, _, agent, orders, _) = setup_real_env_services().await;

    let shop_res = agent
        .process_shopping_request(
            "Buy noise-canceling headphones under $100 with at least 30 hours battery life.",
        )
        .await
        .unwrap();
    let session_id = shop_res.session_id;

    // 1. Order creation WITHOUT prior authorization must fail
    let order_without_auth = orders.create_order(&session_id).await;
    assert!(order_without_auth.is_err());
    assert!(order_without_auth
        .err()
        .unwrap()
        .to_string()
        .contains("customer purchase authorization is required"));

    // 2. Authorization with amount lower than product price ($50 < $79.99)
    let underfund = agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: "hp-anker-q30".to_string(),
                maximum_authorized_amount: 50.00,
                currency: "USD".to_string(),
                approved: true,
            },
        )
        .await;
    assert!(underfund.is_err());
    assert!(underfund
        .err()
        .unwrap()
        .to_string()
        .contains("is less than product price"));

    // 3. Authorization for a different product ID
    let mismatch_prod = agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: "hp-sony-wh1000xm5".to_string(),
                maximum_authorized_amount: 100.00,
                currency: "USD".to_string(),
                approved: true,
            },
        )
        .await;
    assert!(mismatch_prod.is_err());
    assert!(mismatch_prod
        .err()
        .unwrap()
        .to_string()
        .contains("does not match selected product"));

    // 4. Customer explicitly declines (approved: false)
    let declined = agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: "hp-anker-q30".to_string(),
                maximum_authorized_amount: 100.00,
                currency: "USD".to_string(),
                approved: false,
            },
        )
        .await;
    assert!(declined.is_err());
    assert!(declined
        .err()
        .unwrap()
        .to_string()
        .contains("declined purchase authorization"));

    // 5. Valid authorization succeeds
    let valid_auth = agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: "hp-anker-q30".to_string(),
                maximum_authorized_amount: 85.00,
                currency: "USD".to_string(),
                approved: true,
            },
        )
        .await;
    assert!(valid_auth.is_ok());

    // 6. Duplicate authorization attempt on the same session must fail
    let dup_auth = agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: "hp-anker-q30".to_string(),
                maximum_authorized_amount: 85.00,
                currency: "USD".to_string(),
                approved: true,
            },
        )
        .await;
    assert!(dup_auth.is_err());
    assert!(dup_auth
        .err()
        .unwrap()
        .to_string()
        .contains("has already been recorded"));

    // 7. Authorization after session finalization must fail
    ledger.finalize_session(&session_id).await.unwrap();

    let post_fin_auth = agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: "hp-anker-q30".to_string(),
                maximum_authorized_amount: 85.00,
                currency: "USD".to_string(),
                approved: true,
            },
        )
        .await;
    assert!(post_fin_auth.is_err());

    println!("\n=== AUTHORIZATION SECURITY VECTORS: ALL PASSED ===");
}

// =========================================================================
// SECTION 6: LIVE PAYPAL SANDBOX INTEGRATION TEST (ORDERS V2)
// =========================================================================

#[tokio::test]
async fn test_live_paypal_sandbox_oauth_and_order_creation() {
    let (config, ledger, verifier, _, agent, orders, auth) = setup_real_env_services().await;

    // Check if real PayPal credentials are configured in .env
    if config.paypal_client_id.is_empty()
        || config.paypal_client_secret.is_empty()
        || config.paypal_client_id.starts_with("test_")
    {
        println!("\n[SKIPPED] Live PayPal test skipped: real credentials not configured in .env");
        return;
    }

    println!("\n========================================================");
    println!(">>> LIVE PAYPAL SANDBOX INTEGRATION TEST IN PROGRESS <<<");
    println!("========================================================");
    println!("PayPal Base URL: {}", config.paypal_base_url);

    // Step 1: Real OAuth token check against PayPal Sandbox
    let auth_status = auth.get_auth_status().await;
    if let Err(ref e) = auth_status {
        let err_str = format!("{:?}", e);
        if err_str.contains("failed to lookup address information")
            || err_str.contains("dns error")
            || err_str.contains("Could not resolve host")
        {
            println!(
                "\n[SKIPPED] Live PayPal test skipped: network/DNS is unreachable in this environment"
            );
            return;
        }
    }
    assert!(
        auth_status.is_ok(),
        "Live PayPal OAuth token retrieval failed: {:?}",
        auth_status.err()
    );
    let token_info = auth_status.unwrap();
    println!("✓ Live PayPal Sandbox OAuth Successful!");
    println!("  Token Type: {}", token_info.token_type);
    println!("  Expires In: {} seconds", token_info.expires_in);

    // Step 2: Create a real shopping session
    let shop_res = agent
        .process_shopping_request(
            "Buy noise-canceling headphones under $100 with at least 30 hours battery life.",
        )
        .await
        .unwrap();
    let session_id = shop_res.session_id;
    let selected = shop_res
        .selected_product
        .expect("Must have selected product");
    println!("✓ AI Shopping Session Created: {}", session_id);
    println!(
        "  Selected Product: {} (${:.2})",
        selected.name, selected.price
    );

    // Step 3: Authorize the purchase
    let auth_res = agent
        .authorize_purchase(
            &session_id,
            AuthorizePurchaseRequest {
                selected_product_id: selected.product_id.clone(),
                maximum_authorized_amount: 85.00,
                currency: "USD".to_string(),
                approved: true,
            },
        )
        .await
        .unwrap();
    println!(
        "✓ Purchase Authorization Recorded in Hash Chain (Seq: {})",
        auth_res.sequence_index
    );

    // Step 4: Create REAL PayPal Sandbox Order via Orders API v2
    let order_res = orders.create_order(&session_id).await;
    assert!(
        order_res.is_ok(),
        "Real PayPal Sandbox order creation failed: {:?}",
        order_res.err()
    );
    let order = order_res.unwrap();

    println!("========================================================");
    println!("✓ REAL PAYPAL SANDBOX ORDER CREATED SUCCESSFULLY!");
    println!("  PayPal Order ID : {}", order.order_id);
    println!("  Order Status    : {}", order.status);
    println!("  Session ID      : {}", order.session_id);
    println!(
        "  Buyer Approval  : {}",
        order.approval_url.as_deref().unwrap_or("N/A")
    );
    println!("========================================================");

    // Verify database association
    let session = ledger.get_session(&session_id).await.unwrap();
    assert_eq!(session.paypal_order_id, Some(order.order_id.clone()));
    assert!(
        session.root_hash.is_none(),
        "Session must not be finalized before buyer approval and capture"
    );

    // Verify hash chain integrity
    let verify = verifier.verify_session(&session_id).await.unwrap();
    assert_eq!(verify.status, VerificationStatus::Valid);
    println!("✓ Cryptographic Ledger Integrity Verified up to PAYPAL_ORDER_CREATED!");

    // Capture attempt BEFORE buyer approval must fail safely
    let premature_capture = orders.capture_order(&session_id).await;
    println!(
        "✓ Tested premature capture before buyer approval: {:?}",
        premature_capture.as_ref().err().map(|e| e.to_string())
    );
    assert!(
        premature_capture.is_err(),
        "Premature capture before buyer approval must be rejected by PayPal"
    );

    // Session remains unfinalized and ledger remains valid with PAYPAL_PAYMENT_FAILED recorded
    let session_after_premature = ledger.get_session(&session_id).await.unwrap();
    assert!(
        session_after_premature.root_hash.is_none(),
        "Failed capture must not finalize session"
    );

    let verify_after = verifier.verify_session(&session_id).await.unwrap();
    assert_eq!(verify_after.status, VerificationStatus::Valid);
    println!("✓ Ledger Integrity Maintained After Premature Capture Failure!");
}

// =========================================================================
// SECTION 7: LIVE PAYPAL SANDBOX WEBHOOK REGISTRATION LOOKUP
// =========================================================================

#[tokio::test]
async fn test_live_paypal_sandbox_webhook_lookup() {
    let (config, _, _, _, _, _, auth) = setup_real_env_services().await;

    // Guard 1: Skip if real OAuth credentials are absent or placeholder.
    if config.paypal_client_id.is_empty()
        || config.paypal_client_secret.is_empty()
        || config.paypal_client_id.starts_with("test_")
    {
        println!(
            "\n[SKIPPED] Webhook lookup skipped: real PayPal credentials not configured in .env"
        );
        return;
    }

    // Guard 2: Skip if PAYPAL_WEBHOOK_ID is absent.
    let webhook_id = match config.validate_webhook_id() {
        Ok(id) => id.to_string(),
        Err(_) => {
            println!(
                "\n[SKIPPED] Webhook lookup skipped: PAYPAL_WEBHOOK_ID is not configured in .env"
            );
            return;
        }
    };

    println!("\n========================================================");
    println!(">>> LIVE PAYPAL SANDBOX WEBHOOK LOOKUP IN PROGRESS <<<");
    println!("========================================================");
    println!("PayPal Base URL : {}", config.paypal_base_url);

    // Step 1: Obtain a live OAuth Bearer token via the existing auth service.
    // The token value is never printed.
    let access_token = match auth.get_valid_access_token().await {
        Ok(token) => token,
        Err(ref e) => {
            let err_str = format!("{:?}", e);
            if err_str.contains("failed to lookup address information")
                || err_str.contains("dns error")
                || err_str.contains("Could not resolve host")
            {
                println!(
                    "\n[SKIPPED] Live PayPal webhook lookup skipped: network/DNS is unreachable in this environment"
                );
                return;
            }
            panic!("Live PayPal OAuth token retrieval failed: {:?}", e);
        }
    };

    println!("✓ Live PayPal Sandbox OAuth token obtained.");

    // Step 2: Build the read-only GET URL for the registered webhook.
    let url = format!(
        "{}/v1/notifications/webhooks/{}",
        config.paypal_base_url.trim_end_matches('/'),
        webhook_id,
    );

    // Step 3: Send exactly one GET request. No body, no mutation, no database writes.
    let http_client = create_http_client(&HttpClientConfig::default())
        .expect("Failed to create HTTP client for webhook lookup");

    let response = http_client
        .get(&url)
        .bearer_auth(access_token) // token used for auth only; never printed
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .expect("HTTP request to PayPal webhook lookup endpoint failed");

    let status = response.status();
    println!("  HTTP Status     : {}", status);

    // Step 4: Assert HTTP 200 — confirms PAYPAL_WEBHOOK_ID belongs to this Sandbox app.
    assert!(
        status.is_success(),
        "PayPal webhook lookup returned non-200 status {}. \
         Check that PAYPAL_WEBHOOK_ID belongs to the same Sandbox REST app as PAYPAL_CLIENT_ID.",
        status
    );

    // Step 5: Parse and print only the safe, non-secret fields from the response.
    let body: serde_json::Value = response
        .json()
        .await
        .expect("Failed to parse PayPal webhook lookup response as JSON");

    // Registered public webhook URL (the ngrok / tunnel endpoint you configured).
    if let Some(url_field) = body.get("url").and_then(|v| v.as_str()) {
        println!("  Registered URL  : {}", url_field);
    }

    // Active event subscriptions.
    if let Some(event_types) = body.get("event_types").and_then(|v| v.as_array()) {
        println!("  Event Types     : {} subscribed", event_types.len());
        for et in event_types {
            let name = et
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            let desc = et.get("description").and_then(|v| v.as_str()).unwrap_or("");
            if desc.is_empty() {
                println!("    - {}", name);
            } else {
                println!("    - {} ({})", name, desc);
            }
        }
    }

    // Optional status field (present on some PayPal responses).
    if let Some(wh_status) = body.get("status").and_then(|v| v.as_str()) {
        println!("  Webhook Status  : {}", wh_status);
    }

    println!("========================================================");
    println!("✓ Webhook registration confirmed and event types verified!");
    println!("========================================================");
}
