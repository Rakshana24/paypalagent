use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, StatusCode},
    response::Response,
    routing::post,
    Router,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use agentic_commerce::config::Config;
use agentic_commerce::error::AppError;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::AuthCheckResponse;
use agentic_commerce::services::auth::{CachedToken, PayPalAuthService};
use agentic_commerce::{create_router, AppState};

#[derive(Clone)]
struct MockServerState {
    request_count: Arc<AtomicUsize>,
    last_auth_header: Arc<RwLock<Option<String>>>,
    last_content_type: Arc<RwLock<Option<String>>>,
    last_body: Arc<RwLock<Option<String>>>,
    response_status: Arc<RwLock<StatusCode>>,
    response_body: Arc<RwLock<String>>,
    delay_ms: Arc<RwLock<u64>>,
}

async fn mock_token_handler(
    State(state): State<MockServerState>,
    req: Request,
) -> Result<Response, StatusCode> {
    state.request_count.fetch_add(1, Ordering::SeqCst);

    if let Some(auth) = req.headers().get(header::AUTHORIZATION) {
        let auth_str = auth.to_str().unwrap_or_default().to_string();
        *state.last_auth_header.write().await = Some(auth_str);
    }

    if let Some(ct) = req.headers().get(header::CONTENT_TYPE) {
        let ct_str = ct.to_str().unwrap_or_default().to_string();
        *state.last_content_type.write().await = Some(ct_str);
    }

    let bytes = to_bytes(req.into_body(), 1024 * 16)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let body_str = String::from_utf8_lossy(&bytes).to_string();
    *state.last_body.write().await = Some(body_str);

    let delay = *state.delay_ms.read().await;
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    let status = *state.response_status.read().await;
    let body = state.response_body.read().await.clone();

    let resp = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(resp)
}

async fn start_mock_paypal_server(status: StatusCode, body: &str) -> (String, MockServerState) {
    let state = MockServerState {
        request_count: Arc::new(AtomicUsize::new(0)),
        last_auth_header: Arc::new(RwLock::new(None)),
        last_content_type: Arc::new(RwLock::new(None)),
        last_body: Arc::new(RwLock::new(None)),
        response_status: Arc::new(RwLock::new(status)),
        response_body: Arc::new(RwLock::new(body.to_string())),
        delay_ms: Arc::new(RwLock::new(0)),
    };

    let router = Router::new()
        .route("/v1/oauth2/token", post(mock_token_handler))
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    (url, state)
}

fn create_test_config(base_url: &str) -> Config {
    Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: base_url.to_string(),
        paypal_client_id: "test_client_id_123".to_string(),
        paypal_client_secret: "test_client_secret_xyz".to_string(),
        database_url: "sqlite::memory:".to_string(),
        paypal_webhook_id: None,
    }
}

#[tokio::test]
async fn test_successful_oauth_token_retrieval_and_parsing() {
    let success_json = r#"{
        "scope": "https://uri.paypal.com/services/invoicing",
        "access_token": "mock_access_token_12345",
        "token_type": "Bearer",
        "app_id": "APP-TEST1234",
        "expires_in": 3600,
        "nonce": "2026-11-07T12:00:00Z"
    }"#;

    let (mock_url, mock_state) = start_mock_paypal_server(StatusCode::OK, success_json).await;

    let config = Arc::new(create_test_config(&mock_url));
    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = PayPalAuthService::new(config, http_client);

    let token = auth_service.get_valid_access_token().await.unwrap();
    assert_eq!(token, "mock_access_token_12345");

    // Verify request headers and payload
    let auth_header = mock_state.last_auth_header.read().await.clone().unwrap();
    assert!(auth_header.starts_with("Basic "));

    let content_type = mock_state.last_content_type.read().await.clone().unwrap();
    assert!(content_type.contains("application/x-www-form-urlencoded"));

    let body = mock_state.last_body.read().await.clone().unwrap();
    assert!(body.contains("grant_type=client_credentials"));
}

#[tokio::test]
async fn test_token_reuse_from_cache() {
    let success_json = r#"{
        "access_token": "reused_cached_token_999",
        "token_type": "Bearer",
        "expires_in": 3600
    }"#;

    let (mock_url, mock_state) = start_mock_paypal_server(StatusCode::OK, success_json).await;

    let config = Arc::new(create_test_config(&mock_url));
    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = PayPalAuthService::new(config, http_client);

    // Call 5 times consecutively
    for _ in 0..5 {
        let token = auth_service.get_valid_access_token().await.unwrap();
        assert_eq!(token, "reused_cached_token_999");
    }

    // Must have made only 1 network request
    assert_eq!(mock_state.request_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_automatic_refresh_near_expiration() {
    let success_json = r#"{
        "access_token": "refreshed_token_abc",
        "token_type": "Bearer",
        "expires_in": 3600
    }"#;

    let (mock_url, mock_state) = start_mock_paypal_server(StatusCode::OK, success_json).await;

    let config = Arc::new(create_test_config(&mock_url));
    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = PayPalAuthService::new(config, http_client);

    // Manually seed the cache with an expired/expiring token (30s remaining is within 60s safety margin)
    let expiring_token = CachedToken {
        access_token: "old_expiring_token".to_string(),
        token_type: "Bearer".to_string(),
        expires_at: Instant::now() + Duration::from_secs(30),
        initial_expires_in: 30,
    };
    auth_service.set_cached_token_for_test(expiring_token).await;

    // Call should detect invalid cache and automatically refresh
    let new_token = auth_service.get_valid_access_token().await.unwrap();
    assert_eq!(new_token, "refreshed_token_abc");
    assert_eq!(mock_state.request_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_concurrent_requests_no_duplicate_token_requests() {
    let success_json = r#"{
        "access_token": "concurrent_test_token",
        "token_type": "Bearer",
        "expires_in": 3600
    }"#;

    let (mock_url, mock_state) = start_mock_paypal_server(StatusCode::OK, success_json).await;
    // Add artificial delay to simulate real network latency
    *mock_state.delay_ms.write().await = 50;

    let config = Arc::new(create_test_config(&mock_url));
    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config, http_client));

    // Spawn 10 concurrent requests
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let service = auth_service.clone();
        tasks.push(tokio::spawn(async move {
            service.get_valid_access_token().await
        }));
    }

    for task in tasks {
        let res = task.await.unwrap();
        assert_eq!(res.unwrap(), "concurrent_test_token");
    }

    // Even with 10 concurrent calls, exactly 1 network request occurred
    assert_eq!(mock_state.request_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_paypal_authentication_failure() {
    let error_json = r#"{
        "error": "invalid_client",
        "error_description": "Client Authentication failed"
    }"#;

    let (mock_url, mock_state) =
        start_mock_paypal_server(StatusCode::UNAUTHORIZED, error_json).await;

    let config = Arc::new(create_test_config(&mock_url));
    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = PayPalAuthService::new(config, http_client);

    let err = auth_service.get_valid_access_token().await.unwrap_err();
    match err {
        AppError::PayPalAuth(msg) => {
            assert!(msg.contains("invalid_client"));
            assert!(msg.contains("Client Authentication failed"));
        }
        other => panic!("Expected AppError::PayPalAuth, got: {:?}", other),
    }

    assert_eq!(mock_state.request_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_invalid_json_response() {
    let (mock_url, _) = start_mock_paypal_server(StatusCode::OK, "MALFORMED_NON_JSON").await;

    let config = Arc::new(create_test_config(&mock_url));
    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = PayPalAuthService::new(config, http_client);

    let err = auth_service.get_valid_access_token().await.unwrap_err();
    match err {
        AppError::Serialization(_) => {}
        other => panic!("Expected AppError::Serialization, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_network_connection_failure() {
    // Unreachable loopback port
    let config = Arc::new(create_test_config("http://127.0.0.1:59998"));
    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = PayPalAuthService::new(config, http_client);

    let err = auth_service.get_valid_access_token().await.unwrap_err();
    match err {
        AppError::HttpClient(client_err) => {
            assert!(client_err.is_connect());
        }
        other => panic!("Expected AppError::HttpClient, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_network_timeout() {
    let (mock_url, mock_state) =
        start_mock_paypal_server(StatusCode::OK, r#"{"access_token":"t","expires_in":3600}"#).await;
    // Delay longer than client timeout
    *mock_state.delay_ms.write().await = 300;

    let config = Arc::new(create_test_config(&mock_url));
    let http_client = create_http_client(&HttpClientConfig {
        timeout: Duration::from_millis(50),
        connect_timeout: Duration::from_millis(50),
        user_agent: "test".to_string(),
    })
    .unwrap();
    let auth_service = PayPalAuthService::new(config, http_client);

    let err = auth_service.get_valid_access_token().await.unwrap_err();
    match err {
        AppError::HttpClient(client_err) => {
            assert!(client_err.is_timeout());
        }
        other => panic!("Expected timeout HttpClient error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_no_credential_leakage_in_errors_and_logs() {
    let canary_secret = "CANARY_SECRET_LEAK_DETECTION_VAL_999";
    let (mock_url, _) = start_mock_paypal_server(
        StatusCode::UNAUTHORIZED,
        r#"{"error":"invalid_client","error_description":"Bad credentials"}"#,
    )
    .await;

    let mut cfg = create_test_config(&mock_url);
    cfg.paypal_client_secret = canary_secret.to_string();
    let config = Arc::new(cfg);

    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = PayPalAuthService::new(config.clone(), http_client);

    let err = auth_service.get_valid_access_token().await.unwrap_err();
    let display_str = format!("{}", err);
    let debug_str = format!("{:?}", err);

    assert!(
        !display_str.contains(canary_secret),
        "Error display leaked secret!"
    );
    assert!(
        !debug_str.contains(canary_secret),
        "Error debug leaked secret!"
    );

    let config_debug = format!("{:?}", config);
    assert!(
        !config_debug.contains(canary_secret),
        "Config debug leaked secret!"
    );
    assert!(config_debug.contains("[REDACTED]"));
}

#[tokio::test]
async fn test_auth_check_endpoint_integration() {
    let success_json = r#"{
        "access_token": "endpoint_token_secret_123",
        "token_type": "Bearer",
        "expires_in": 7200
    }"#;

    let (mock_url, _) = start_mock_paypal_server(StatusCode::OK, success_json).await;

    let config = Arc::new(create_test_config(&mock_url));
    let http_client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), http_client.clone()));

    let db_pool = agentic_commerce::db::init_db("sqlite::memory:")
        .await
        .unwrap();
    let ledger_service = Arc::new(agentic_commerce::services::LedgerService::new(
        db_pool.clone(),
    ));
    let verification_service = Arc::new(agentic_commerce::services::VerificationService::new(
        db_pool,
    ));
    let catalog_service = agentic_commerce::services::CatalogService::new();
    let shopping_agent = Arc::new(agentic_commerce::services::ShoppingAgentService::new(
        catalog_service.clone(),
        ledger_service.clone(),
        verification_service.clone(),
    ));
    let paypal_orders = Arc::new(agentic_commerce::services::PayPalOrdersService::new(
        config.clone(),
        http_client.clone(),
        auth_service.clone(),
        ledger_service.clone(),
    ));

    let state = AppState {
        config: config.clone(),
        http_client,
        auth_service,
        ledger_service,
        verification_service,
        catalog_service,
        shopping_agent,
        paypal_orders,
    };

    let router = create_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/paypal/auth-check", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let body_text = resp.text().await.unwrap();
    let auth_resp: AuthCheckResponse = serde_json::from_str(&body_text).unwrap();

    assert!(auth_resp.success);
    assert_eq!(
        auth_resp.message,
        "PayPal Sandbox authentication successful"
    );
    assert_eq!(auth_resp.token_type, "Bearer");
    assert!(auth_resp.expires_in > 7100);

    // CRITICAL SECURITY ASSERTION: Access token is NEVER returned to public client
    assert!(
        !body_text.contains("endpoint_token_secret_123"),
        "Endpoint leaked access_token!"
    );
}
