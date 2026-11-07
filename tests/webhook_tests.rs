use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use agentic_commerce::config::Config;
use agentic_commerce::error::AppError;
use agentic_commerce::http_client::{create_http_client, HttpClientConfig};
use agentic_commerce::models::webhook::{
    PayPalWebhookHeaders, VerifyWebhookSignatureRequest, WebhookVerificationStatus,
    PAYPAL_AUTH_ALGO_HEADER, PAYPAL_CERT_URL_HEADER, PAYPAL_TRANSMISSION_ID_HEADER,
    PAYPAL_TRANSMISSION_SIG_HEADER, PAYPAL_TRANSMISSION_TIME_HEADER,
};
use agentic_commerce::services::auth::PayPalAuthService;
use agentic_commerce::services::webhook_verifier::WebhookVerifierService;

#[derive(Clone)]
struct MockPayPalServerState {
    pub token_request_count: Arc<AtomicUsize>,
    pub verify_request_count: Arc<AtomicUsize>,
    pub last_verify_auth_header: Arc<RwLock<Option<String>>>,
    pub last_verify_payload: Arc<RwLock<Option<String>>>,
    pub verify_response_status: Arc<RwLock<StatusCode>>,
    pub verify_response_body: Arc<RwLock<String>>,
    pub verify_delay_ms: Arc<RwLock<u64>>,
}

async fn mock_token_handler(
    State(state): State<MockPayPalServerState>,
    _req: Request,
) -> Result<Response, StatusCode> {
    state.token_request_count.fetch_add(1, Ordering::SeqCst);
    let token_json = json!({
        "access_token": "mock-access-token-webhook-test",
        "token_type": "Bearer",
        "expires_in": 3600
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(token_json.to_string()))
        .unwrap())
}

async fn mock_verify_handler(
    State(state): State<MockPayPalServerState>,
    req: Request,
) -> Result<Response, StatusCode> {
    state.verify_request_count.fetch_add(1, Ordering::SeqCst);

    if let Some(auth) = req.headers().get(header::AUTHORIZATION) {
        *state.last_verify_auth_header.write().await = Some(auth.to_str().unwrap().to_string());
    }

    let bytes = to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let body_str = String::from_utf8_lossy(&bytes).to_string();
    *state.last_verify_payload.write().await = Some(body_str);

    let delay = *state.verify_delay_ms.read().await;
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    let status = *state.verify_response_status.read().await;
    let body = state.verify_response_body.read().await.clone();

    Ok(Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap())
}

async fn spawn_mock_paypal_server(
    initial_verify_status: StatusCode,
    initial_verify_body: &str,
) -> (String, MockPayPalServerState) {
    let state = MockPayPalServerState {
        token_request_count: Arc::new(AtomicUsize::new(0)),
        verify_request_count: Arc::new(AtomicUsize::new(0)),
        last_verify_auth_header: Arc::new(RwLock::new(None)),
        last_verify_payload: Arc::new(RwLock::new(None)),
        verify_response_status: Arc::new(RwLock::new(initial_verify_status)),
        verify_response_body: Arc::new(RwLock::new(initial_verify_body.to_string())),
        verify_delay_ms: Arc::new(RwLock::new(0)),
    };

    let app = Router::new()
        .route("/v1/oauth2/token", post(mock_token_handler))
        .route(
            "/v1/notifications/verify-webhook-signature",
            post(mock_verify_handler),
        )
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mock_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (mock_url, state)
}

fn valid_test_headers() -> PayPalWebhookHeaders {
    PayPalWebhookHeaders::new(
        "SHA256withRSA",
        "https://api.sandbox.paypal.com/v1/notifications/certs/CERT-12345",
        "trans-uuid-8899",
        "base64signature==",
        "2026-11-07T14:44:47Z",
    )
}

fn sample_webhook_event_json() -> serde_json::Value {
    json!({
        "id": "WH-90812345",
        "create_time": "2026-11-07T14:44:47Z",
        "resource_type": "dispute",
        "event_type": "CUSTOMER.DISPUTE.CREATED",
        "summary": "Customer opened dispute",
        "resource": {
            "dispute_id": "PP-D-12345",
            "status": "WAITING_FOR_SELLER_RESPONSE"
        }
    })
}

#[tokio::test]
async fn test_successful_webhook_signature_verification() {
    let (mock_url, mock_state) =
        spawn_mock_paypal_server(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-CONFIGURED-ID-123".to_string()),
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config.clone(), client, auth_service);

    let headers = valid_test_headers();
    let event = sample_webhook_event_json();

    let result = verifier.verify_signature(&headers, &event).await;
    assert!(result.is_ok(), "Verification should succeed with Ok(())");

    // Verify correct calls were made to mock PayPal API
    assert_eq!(mock_state.token_request_count.load(Ordering::SeqCst), 1);
    assert_eq!(mock_state.verify_request_count.load(Ordering::SeqCst), 1);

    // Verify Bearer authorization header
    let auth_hdr = mock_state.last_verify_auth_header.read().await;
    assert_eq!(
        auth_hdr.as_deref(),
        Some("Bearer mock-access-token-webhook-test")
    );

    // Verify complete payload structure sent to PayPal (all 7 required fields)
    let payload_str = mock_state.last_verify_payload.read().await;
    let sent_req: VerifyWebhookSignatureRequest =
        serde_json::from_str(payload_str.as_ref().unwrap()).unwrap();
    assert_eq!(sent_req.webhook_id, "WH-CONFIGURED-ID-123");
    assert_eq!(sent_req.auth_algo, "SHA256withRSA");
    assert_eq!(
        sent_req.cert_url,
        "https://api.sandbox.paypal.com/v1/notifications/certs/CERT-12345"
    );
    assert_eq!(sent_req.transmission_id, "trans-uuid-8899");
    assert_eq!(sent_req.transmission_sig, "base64signature==");
    assert_eq!(sent_req.transmission_time, "2026-11-07T14:44:47Z");
    assert_eq!(sent_req.webhook_event["id"], "WH-90812345");
}

#[tokio::test]
async fn test_failed_webhook_signature_verification() {
    let (mock_url, mock_state) =
        spawn_mock_paypal_server(StatusCode::OK, r#"{"verification_status":"FAILURE"}"#).await;

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-CONFIGURED-ID-123".to_string()),
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config, client, auth_service);

    let headers = valid_test_headers();
    let event = sample_webhook_event_json();

    let result = verifier.verify_signature(&headers, &event).await;
    assert!(
        result.is_err(),
        "Must return Err when PayPal reports FAILURE, never Ok(false)"
    );

    match result.err().unwrap() {
        AppError::WebhookVerificationFailed(msg) => {
            assert!(msg.contains("FAILURE"));
        }
        other => panic!("Expected WebhookVerificationFailed, got {:?}", other),
    }

    assert_eq!(mock_state.verify_request_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_missing_required_headers() {
    let mut header_map = HeaderMap::new();
    header_map.insert(
        PAYPAL_AUTH_ALGO_HEADER,
        HeaderValue::from_static("SHA256withRSA"),
    );
    header_map.insert(
        PAYPAL_CERT_URL_HEADER,
        HeaderValue::from_static("https://api.sandbox.paypal.com/cert"),
    );
    // PAYPAL_TRANSMISSION_ID_HEADER is missing

    let res = PayPalWebhookHeaders::from_header_map(&header_map);
    assert!(res.is_err());
    match res.err().unwrap() {
        AppError::MissingWebhookHeader(h) => {
            assert_eq!(h, PAYPAL_TRANSMISSION_ID_HEADER);
        }
        other => panic!("Expected MissingWebhookHeader, got {:?}", other),
    }

    // When all headers are present
    header_map.insert(
        PAYPAL_TRANSMISSION_ID_HEADER,
        HeaderValue::from_static("id-123"),
    );
    header_map.insert(
        PAYPAL_TRANSMISSION_SIG_HEADER,
        HeaderValue::from_static("sig-123"),
    );
    header_map.insert(
        PAYPAL_TRANSMISSION_TIME_HEADER,
        HeaderValue::from_static("2026-11-07T12:00:00Z"),
    );

    let ok_res = PayPalWebhookHeaders::from_header_map(&header_map);
    assert!(ok_res.is_ok());
}

#[tokio::test]
async fn test_invalid_verification_response_payload() {
    // PayPal returns malformed / unexpected JSON
    let (mock_url, _) =
        spawn_mock_paypal_server(StatusCode::OK, r#"{"unexpected_key": "not-valid"}"#).await;

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-123".to_string()),
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config, client, auth_service);

    let headers = valid_test_headers();
    let event = sample_webhook_event_json();

    let res = verifier.verify_signature(&headers, &event).await;
    assert!(res.is_err());
    assert!(matches!(res.err().unwrap(), AppError::Serialization(_)));
}

#[tokio::test]
async fn test_paypal_verification_api_error() {
    // PayPal returns 400 Bad Request
    let (mock_url, _) = spawn_mock_paypal_server(
        StatusCode::BAD_REQUEST,
        r#"{"name":"INVALID_REQUEST","message":"Invalid payload"}"#,
    )
    .await;

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-123".to_string()),
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config, client, auth_service);

    let headers = valid_test_headers();
    let event = sample_webhook_event_json();

    let res = verifier.verify_signature(&headers, &event).await;
    assert!(res.is_err());
    match res.err().unwrap() {
        AppError::PayPalApi(msg) => {
            assert!(msg.contains("400"));
            assert!(msg.contains("INVALID_REQUEST"));
        }
        other => panic!("Expected PayPalApi, got {:?}", other),
    }
}

#[tokio::test]
async fn test_network_timeout() {
    let (mock_url, mock_state) =
        spawn_mock_paypal_server(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    // Set delay to 300ms
    *mock_state.verify_delay_ms.write().await = 300;

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-123".to_string()),
    });

    // Configure HTTP client with tight 50ms timeout
    let client = create_http_client(&HttpClientConfig {
        timeout: Duration::from_millis(50),
        connect_timeout: Duration::from_millis(50),
        user_agent: "test-agent".to_string(),
    })
    .unwrap();

    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config, client, auth_service);

    let headers = valid_test_headers();
    let event = sample_webhook_event_json();

    let res = verifier.verify_signature(&headers, &event).await;
    assert!(res.is_err());
    assert!(matches!(res.err().unwrap(), AppError::HttpClient(_)));
}

#[tokio::test]
async fn test_existing_oauth_service_compatibility() {
    let (mock_url, mock_state) =
        spawn_mock_paypal_server(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-123".to_string()),
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config, client, auth_service);

    let headers = valid_test_headers();
    let event = sample_webhook_event_json();

    // First call: requests token and verifies
    let res1 = verifier.verify_signature(&headers, &event).await;
    assert!(res1.is_ok());
    assert_eq!(mock_state.token_request_count.load(Ordering::SeqCst), 1);
    assert_eq!(mock_state.verify_request_count.load(Ordering::SeqCst), 1);

    // Second call: reuses cached token, no duplicate token request!
    let res2 = verifier.verify_signature(&headers, &event).await;
    assert!(res2.is_ok());
    assert_eq!(
        mock_state.token_request_count.load(Ordering::SeqCst),
        1,
        "OAuth token must be reused from cache"
    );
    assert_eq!(mock_state.verify_request_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_unconfigured_webhook_id_fails_fast() {
    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: "http://127.0.0.1:9".to_string(),
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: None, // Missing
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config, client, auth_service);

    let headers = valid_test_headers();
    let event = sample_webhook_event_json();

    let res = verifier.verify_signature(&headers, &event).await;
    assert!(res.is_err());
    match res.err().unwrap() {
        AppError::Config(msg) => {
            assert!(msg.contains("PAYPAL_WEBHOOK_ID"));
        }
        other => panic!("Expected AppError::Config, got {:?}", other),
    }
}

#[tokio::test]
async fn test_webhook_verification_failure_maps_to_http_401() {
    let err = AppError::WebhookVerificationFailed(
        "PayPal webhook signature verification failed (status: FAILURE)".to_string(),
    );
    let resp = err.into_response();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_missing_webhook_header_maps_to_http_400() {
    let err = AppError::MissingWebhookHeader("paypal-transmission-sig".to_string());
    let resp = err.into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_verify_signature_raw_status_returns_enum_directly() {
    let (mock_url, _) =
        spawn_mock_paypal_server(StatusCode::OK, r#"{"verification_status":"FAILURE"}"#).await;

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-123".to_string()),
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config, client, auth_service);

    let headers = valid_test_headers();
    let event = sample_webhook_event_json();

    let status = verifier
        .verify_signature_status(&headers, &event)
        .await
        .unwrap();
    assert_eq!(status, WebhookVerificationStatus::Failure);
    assert!(!status.is_success());
}

#[tokio::test]
async fn test_untrusted_cert_url_rejected_before_calling_paypal() {
    let (mock_url, mock_state) =
        spawn_mock_paypal_server(StatusCode::OK, r#"{"verification_status":"SUCCESS"}"#).await;

    let config = Arc::new(Config {
        port: 5000,
        rust_log: "info".to_string(),
        paypal_base_url: mock_url,
        database_url: "sqlite::memory:".to_string(),
        paypal_client_id: "test-client-id".to_string(),
        paypal_client_secret: "test-client-secret".to_string(),
        paypal_webhook_id: Some("WH-123".to_string()),
    });

    let client = create_http_client(&HttpClientConfig::default()).unwrap();
    let auth_service = Arc::new(PayPalAuthService::new(config.clone(), client.clone()));
    let verifier = WebhookVerifierService::new(config, client, auth_service);

    let mut headers = valid_test_headers();
    headers.cert_url = "https://evil.com/fake-cert".to_string();
    let event = sample_webhook_event_json();

    let res = verifier.verify_signature(&headers, &event).await;
    assert!(res.is_err());
    match res.err().unwrap() {
        AppError::WebhookVerificationFailed(msg) => {
            assert!(msg.contains("not a trusted PayPal certificate domain"));
        }
        other => panic!("Expected WebhookVerificationFailed, got {:?}", other),
    }

    // PayPal API must not have been reached
    assert_eq!(mock_state.verify_request_count.load(Ordering::SeqCst), 0);
}
