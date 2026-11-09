use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::models::paypal_orders::PayPalLink;

/// Expected header keys sent with PayPal webhook deliveries.
pub const PAYPAL_AUTH_ALGO_HEADER: &str = "paypal-auth-algo";
pub const PAYPAL_CERT_URL_HEADER: &str = "paypal-cert-url";
pub const PAYPAL_TRANSMISSION_ID_HEADER: &str = "paypal-transmission-id";
pub const PAYPAL_TRANSMISSION_SIG_HEADER: &str = "paypal-transmission-sig";
pub const PAYPAL_TRANSMISSION_TIME_HEADER: &str = "paypal-transmission-time";

/// Trusted PayPal certificate hostnames for Sandbox and Production.
pub const TRUSTED_PAYPAL_CERT_HOSTS: &[&str] = &[
    "api.paypal.com",
    "api.sandbox.paypal.com",
    "api-m.paypal.com",
    "api-m.sandbox.paypal.com",
    "notify.paypal.com",
    "notify.sandbox.paypal.com",
];

/// Strongly typed container for PayPal webhook transmission headers.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayPalWebhookHeaders {
    /// The algorithm used to generate the signature (e.g. SHA256withRSA).
    pub auth_algo: String,
    /// The URL of the certificate used to sign the webhook event.
    pub cert_url: String,
    /// A unique ID for each webhook event transmission.
    pub transmission_id: String,
    /// The base64-encoded signature of the webhook payload.
    pub transmission_sig: String,
    /// The ISO 8601 timestamp of when the transmission occurred.
    pub transmission_time: String,
}

impl std::fmt::Debug for PayPalWebhookHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayPalWebhookHeaders")
            .field("auth_algo", &self.auth_algo)
            .field("cert_url", &self.cert_url)
            .field("transmission_id", &self.transmission_id)
            .field("transmission_sig", &"[REDACTED_SIG]")
            .field("transmission_time", &self.transmission_time)
            .finish()
    }
}

impl PayPalWebhookHeaders {
    /// Creates a new `PayPalWebhookHeaders` directly from values.
    pub fn new(
        auth_algo: impl Into<String>,
        cert_url: impl Into<String>,
        transmission_id: impl Into<String>,
        transmission_sig: impl Into<String>,
        transmission_time: impl Into<String>,
    ) -> Self {
        Self {
            auth_algo: auth_algo.into(),
            cert_url: cert_url.into(),
            transmission_id: transmission_id.into(),
            transmission_sig: transmission_sig.into(),
            transmission_time: transmission_time.into(),
        }
    }

    /// Extracts and validates required transmission headers from an HTTP `HeaderMap`.
    pub fn from_header_map(headers: &HeaderMap) -> Result<Self, AppError> {
        let auth_algo = headers
            .get(PAYPAL_AUTH_ALGO_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| AppError::MissingWebhookHeader(PAYPAL_AUTH_ALGO_HEADER.to_string()))?;

        let cert_url = headers
            .get(PAYPAL_CERT_URL_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| AppError::MissingWebhookHeader(PAYPAL_CERT_URL_HEADER.to_string()))?;

        let transmission_id = headers
            .get(PAYPAL_TRANSMISSION_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                AppError::MissingWebhookHeader(PAYPAL_TRANSMISSION_ID_HEADER.to_string())
            })?;

        let transmission_sig = headers
            .get(PAYPAL_TRANSMISSION_SIG_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                AppError::MissingWebhookHeader(PAYPAL_TRANSMISSION_SIG_HEADER.to_string())
            })?;

        let transmission_time = headers
            .get(PAYPAL_TRANSMISSION_TIME_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                AppError::MissingWebhookHeader(PAYPAL_TRANSMISSION_TIME_HEADER.to_string())
            })?;

        Ok(Self {
            auth_algo: auth_algo.trim().to_string(),
            cert_url: cert_url.trim().to_string(),
            transmission_id: transmission_id.trim().to_string(),
            transmission_sig: transmission_sig.trim().to_string(),
            transmission_time: transmission_time.trim().to_string(),
        })
    }

    /// Validates the certificate URL format against PayPal domain rules.
    pub fn validate_cert_url(&self) -> Result<(), AppError> {
        let parsed_url = reqwest::Url::parse(&self.cert_url).map_err(|e| {
            AppError::WebhookVerificationFailed(format!(
                "Invalid PayPal certificate URL format: {}",
                e
            ))
        })?;

        // 1. Must use HTTPS protocol
        if parsed_url.scheme() != "https" {
            return Err(AppError::WebhookVerificationFailed(
                "PayPal certificate URL must use HTTPS protocol".to_string(),
            ));
        }

        // 2. Must not contain user credentials (username or password)
        if !parsed_url.username().is_empty() || parsed_url.password().is_some() {
            return Err(AppError::WebhookVerificationFailed(
                "PayPal certificate URL must not contain user credentials".to_string(),
            ));
        }

        // 3. Port must be standard HTTPS (443) or omitted
        if let Some(port) = parsed_url.port() {
            if port != 443 {
                return Err(AppError::WebhookVerificationFailed(format!(
                    "PayPal certificate URL specifies unauthorized port: {}",
                    port
                )));
            }
        }

        // 4. Exact hostname validation against trusted PayPal hosts (no string suffix matching)
        let host = match parsed_url.host_str() {
            Some(h) if !h.is_empty() => h.to_ascii_lowercase(),
            _ => {
                return Err(AppError::WebhookVerificationFailed(
                    "PayPal certificate URL is missing a valid hostname".to_string(),
                ));
            }
        };

        if !TRUSTED_PAYPAL_CERT_HOSTS.contains(&host.as_str()) {
            return Err(AppError::WebhookVerificationFailed(format!(
                "PayPal certificate URL host '{}' is not a trusted PayPal certificate domain",
                host
            )));
        }

        Ok(())
    }
}

/// Top-level envelope for PayPal Webhook events.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct PayPalWebhookEvent {
    /// The unique ID of the webhook event notification.
    pub id: String,

    /// Time the notification was created (ISO 8601).
    #[serde(default)]
    pub create_time: Option<String>,

    /// The name of the resource type contained in the event (e.g. "dispute", "capture").
    pub resource_type: String,

    /// Event schema version (e.g. "1.0").
    #[serde(default)]
    pub event_version: Option<String>,

    /// The event name (e.g. "CUSTOMER.DISPUTE.CREATED").
    pub event_type: String,

    /// Summary description of the event.
    #[serde(default)]
    pub summary: Option<String>,

    /// Version of the resource schema.
    #[serde(default)]
    pub resource_version: Option<String>,

    /// Flexible representation of the event resource payload.
    pub resource: serde_json::Value,

    /// HATEOAS navigational links.
    #[serde(default)]
    pub links: Option<Vec<PayPalLink>>,
}

impl std::fmt::Debug for PayPalWebhookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayPalWebhookEvent")
            .field("id", &self.id)
            .field("event_version", &self.event_version)
            .field("create_time", &self.create_time)
            .field("resource_type", &self.resource_type)
            .field("event_type", &self.event_type)
            .field("summary", &self.summary)
            .field("resource", &"[PROTECTED_RESOURCE_PAYLOAD]")
            .finish()
    }
}

impl PayPalWebhookEvent {
    /// Attempts to extract the underlying resource identifier from the flexible resource payload.
    ///
    /// Checks common PayPal ID keys:
    /// - "dispute_id" (Customer Disputes API)
    /// - "order_id" (Orders API)
    /// - "id" (Generic / Captures / Payments / Invoices)
    pub fn extract_resource_id(&self) -> Option<String> {
        self.resource
            .get("dispute_id")
            .or_else(|| self.resource.get("order_id"))
            .or_else(|| self.resource.get("id"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
    }
}

/// Request payload sent to PayPal's `POST /v1/notifications/verify-webhook-signature`.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct VerifyWebhookSignatureRequest {
    /// The algorithm used to generate the signature (e.g. SHA256withRSA).
    pub auth_algo: String,

    /// The URL of the certificate.
    pub cert_url: String,

    /// The transmission ID from headers.
    pub transmission_id: String,

    /// The transmission signature from headers.
    pub transmission_sig: String,

    /// The transmission time from headers.
    pub transmission_time: String,

    /// The ID of the webhook configured in PayPal Developer Dashboard.
    pub webhook_id: String,

    /// The raw event payload as received.
    pub webhook_event: serde_json::Value,
}

impl std::fmt::Debug for VerifyWebhookSignatureRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifyWebhookSignatureRequest")
            .field("auth_algo", &self.auth_algo)
            .field("cert_url", &self.cert_url)
            .field("transmission_id", &self.transmission_id)
            .field("transmission_sig", &"[REDACTED_SIG]")
            .field("transmission_time", &self.transmission_time)
            .field("webhook_id", &self.webhook_id)
            .field("webhook_event", &"[PROTECTED_EVENT_PAYLOAD]")
            .finish()
    }
}

/// Verification status returned by PayPal's signature verification API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebhookVerificationStatus {
    /// The signature is valid and authentic.
    #[serde(rename = "SUCCESS")]
    Success,

    /// The signature is invalid or verification failed.
    #[serde(rename = "FAILURE")]
    Failure,
}

impl WebhookVerificationStatus {
    /// Returns true if the status indicates successful verification.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Response payload from PayPal's `POST /v1/notifications/verify-webhook-signature`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifyWebhookSignatureResponse {
    /// The outcome of the verification check.
    pub verification_status: WebhookVerificationStatus,
}

/// Processing lifecycle states for durably persisted PayPal webhook events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebhookProcessingStatus {
    /// Event has been durably stored and is awaiting asynchronous processing.
    Pending,
    /// Event is currently being processed by a worker or handler.
    Processing,
    /// Event has completed business processing successfully.
    Processed,
    /// Event failed transiently and has been scheduled for a backoff retry.
    Retry,
    /// Event exceeded maximum retries or encountered a fatal, non-retryable error.
    Failed,
    /// Event was explicitly ignored (e.g. unhandled event type or obsolete notification).
    Ignored,
}

impl WebhookProcessingStatus {
    /// Returns the uppercase SQLite database representation of the status.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Processing => "PROCESSING",
            Self::Processed => "PROCESSED",
            Self::Retry => "RETRY",
            Self::Failed => "FAILED",
            Self::Ignored => "IGNORED",
        }
    }

    /// Parses a string into `WebhookProcessingStatus`.
    pub fn from_str(s: &str) -> Result<Self, AppError> {
        match s.trim().to_uppercase().as_str() {
            "PENDING" => Ok(Self::Pending),
            "PROCESSING" => Ok(Self::Processing),
            "PROCESSED" => Ok(Self::Processed),
            "RETRY" => Ok(Self::Retry),
            "FAILED" => Ok(Self::Failed),
            "IGNORED" => Ok(Self::Ignored),
            other => Err(AppError::InvalidPayload(format!(
                "Invalid webhook processing status: '{}'",
                other
            ))),
        }
    }

    /// Indicates whether this status is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Processed | Self::Failed | Self::Ignored)
    }

    /// Validates whether a state transition from `self` to `next` is safe and permitted.
    pub fn can_transition_to(&self, next: Self) -> bool {
        match self {
            Self::Pending => matches!(next, Self::Processing | Self::Ignored),
            Self::Processing => {
                matches!(
                    next,
                    Self::Processed | Self::Retry | Self::Failed | Self::Ignored
                )
            }
            Self::Retry => matches!(next, Self::Processing | Self::Failed | Self::Ignored),
            Self::Processed | Self::Failed | Self::Ignored => false,
        }
    }
}

/// Computes the hex-encoded SHA-256 hash over arbitrary raw payload bytes.
pub fn compute_payload_hash(raw_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_bytes);
    hex::encode(hasher.finalize())
}

/// Calculates exponential backoff retry delay in seconds.
///
/// Starts at 5 seconds, doubles with each retry, and is capped at 3600 seconds (1 hour).
/// * retry_count = 0 -> 5s
/// * retry_count = 1 -> 10s
/// * retry_count = 2 -> 20s
/// * retry_count = 3 -> 40s
/// * ... capped at 3600s
pub fn calculate_retry_delay_seconds(retry_count: u32) -> u64 {
    const INITIAL_DELAY_SECS: u64 = 5;
    const MAX_DELAY_SECS: u64 = 3600;

    let multiplier = 1u64.checked_shl(retry_count).unwrap_or(u64::MAX);
    INITIAL_DELAY_SECS
        .saturating_mul(multiplier)
        .min(MAX_DELAY_SECS)
}

/// Durable database record representation of a PayPal webhook event.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookEventRecord {
    pub event_id: String,
    pub event_type: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub transmission_id: String,
    pub processing_status: WebhookProcessingStatus,
    pub retry_count: i64,
    pub max_retries: i64,
    pub last_error: Option<String>,
    pub raw_payload: Vec<u8>,
    pub payload_hash: String,
    pub next_retry_at: Option<String>,
    pub created_at: String,
    pub processed_at: Option<String>,
    pub processing_started_at: Option<String>,
    pub lease_token: Option<String>,
    pub processing_worker_id: Option<String>,
}

impl std::fmt::Debug for WebhookEventRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookEventRecord")
            .field("event_id", &self.event_id)
            .field("event_type", &self.event_type)
            .field("resource_type", &self.resource_type)
            .field("resource_id", &self.resource_id)
            .field("transmission_id", &self.transmission_id)
            .field("processing_status", &self.processing_status)
            .field("retry_count", &self.retry_count)
            .field("max_retries", &self.max_retries)
            .field("last_error", &self.last_error)
            .field(
                "raw_payload",
                &format!("[PROTECTED_RAW_PAYLOAD: {} bytes]", self.raw_payload.len()),
            )
            .field("payload_hash", &self.payload_hash)
            .field("next_retry_at", &self.next_retry_at)
            .field("created_at", &self.created_at)
            .field("processed_at", &self.processed_at)
            .field("processing_started_at", &self.processing_started_at)
            .field("lease_token", &self.lease_token)
            .field("processing_worker_id", &self.processing_worker_id)
            .finish()
    }
}

/// Result of an idempotent webhook event persistence operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookPersistenceResult {
    /// Newly received event was successfully persisted into the database.
    Inserted(WebhookEventRecord),
    /// The event ID has already been persisted; duplicate delivery was safely recognized.
    Duplicate { event_id: String },
}

impl WebhookPersistenceResult {
    /// Returns true if the event was newly inserted.
    pub fn is_inserted(&self) -> bool {
        matches!(self, Self::Inserted(_))
    }

    /// Returns true if the delivery was recognized as a duplicate.
    pub fn is_duplicate(&self) -> bool {
        matches!(self, Self::Duplicate { .. })
    }

    /// Returns the event ID regardless of insertion or duplicate status.
    pub fn event_id(&self) -> &str {
        match self {
            Self::Inserted(record) => &record.event_id,
            Self::Duplicate { event_id } => event_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn test_webhook_headers_from_header_map_success() {
        let mut map = HeaderMap::new();
        map.insert(
            PAYPAL_AUTH_ALGO_HEADER,
            HeaderValue::from_static("SHA256withRSA"),
        );
        map.insert(
            PAYPAL_CERT_URL_HEADER,
            HeaderValue::from_static(
                "https://api.sandbox.paypal.com/v1/notifications/certs/CERT-1234",
            ),
        );
        map.insert(
            PAYPAL_TRANSMISSION_ID_HEADER,
            HeaderValue::from_static("trans-id-abc"),
        );
        map.insert(
            PAYPAL_TRANSMISSION_SIG_HEADER,
            HeaderValue::from_static("base64sig=="),
        );
        map.insert(
            PAYPAL_TRANSMISSION_TIME_HEADER,
            HeaderValue::from_static("2026-11-07T12:00:00Z"),
        );

        let headers = PayPalWebhookHeaders::from_header_map(&map).unwrap();
        assert_eq!(headers.auth_algo, "SHA256withRSA");
        assert_eq!(headers.transmission_id, "trans-id-abc");
        assert!(headers.validate_cert_url().is_ok());
    }

    #[test]
    fn test_webhook_headers_missing_required() {
        let mut map = HeaderMap::new();
        map.insert(
            PAYPAL_AUTH_ALGO_HEADER,
            HeaderValue::from_static("SHA256withRSA"),
        );
        // cert_url is missing

        let res = PayPalWebhookHeaders::from_header_map(&map);
        assert!(res.is_err());
        assert!(
            matches!(res.err().unwrap(), AppError::MissingWebhookHeader(h) if h == PAYPAL_CERT_URL_HEADER)
        );
    }

    #[test]
    fn test_webhook_headers_untrusted_cert_url() {
        // Non-HTTPS
        let headers_http = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "http://api.paypal.com/cert.pem",
            "trans-1",
            "sig-1",
            "2026-11-07T12:00:00Z",
        );
        assert!(headers_http.validate_cert_url().is_err());

        // FTP
        let headers_ftp = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "ftp://api.paypal.com/cert.pem",
            "trans-1",
            "sig-1",
            "2026-11-07T12:00:00Z",
        );
        assert!(headers_ftp.validate_cert_url().is_err());

        // Malformed URL
        let headers_malformed = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "not-a-valid-url",
            "trans-1",
            "sig-1",
            "2026-11-07T12:00:00Z",
        );
        assert!(headers_malformed.validate_cert_url().is_err());

        // Misleading host (lookalike)
        let headers_bad_host = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "https://evil-paypal.com/cert.pem",
            "trans-1",
            "sig-1",
            "2026-11-07T12:00:00Z",
        );
        assert!(headers_bad_host.validate_cert_url().is_err());

        // Suffix attack domain
        let headers_suffix = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "https://api.paypal.com.attacker.com/cert.pem",
            "trans-1",
            "sig-1",
            "2026-11-07T12:00:00Z",
        );
        assert!(headers_suffix.validate_cert_url().is_err());

        // Embedded credentials / userinfo
        let headers_credentials = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "https://user:password@api.sandbox.paypal.com/cert.pem",
            "trans-1",
            "sig-1",
            "2026-11-07T12:00:00Z",
        );
        assert!(headers_credentials.validate_cert_url().is_err());

        // Userinfo tricking host
        let headers_userinfo_host = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "https://api.sandbox.paypal.com@evil.com/cert.pem",
            "trans-1",
            "sig-1",
            "2026-11-07T12:00:00Z",
        );
        assert!(headers_userinfo_host.validate_cert_url().is_err());

        // Non-standard port
        let headers_port = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "https://api.sandbox.paypal.com:8443/cert.pem",
            "trans-1",
            "sig-1",
            "2026-11-07T12:00:00Z",
        );
        assert!(headers_port.validate_cert_url().is_err());

        // Valid hosts: production & sandbox variants
        let valid_hosts = [
            "https://api.paypal.com/v1/notifications/certs/CERT-1",
            "https://api.sandbox.paypal.com/v1/notifications/certs/CERT-2",
            "https://api-m.paypal.com/v1/notifications/certs/CERT-3",
            "https://api-m.sandbox.paypal.com/v1/notifications/certs/CERT-4",
            "https://notify.paypal.com/v1/notifications/certs/CERT-5",
            "https://notify.sandbox.paypal.com/v1/notifications/certs/CERT-6",
            "https://API.SANDBOX.PAYPAL.COM/v1/notifications/certs/CERT-7",
        ];

        for host_url in valid_hosts {
            let headers_valid = PayPalWebhookHeaders::new(
                "SHA256withRSA",
                host_url,
                "trans-1",
                "sig-1",
                "2026-11-07T12:00:00Z",
            );
            assert!(
                headers_valid.validate_cert_url().is_ok(),
                "Failed for valid host: {}",
                host_url
            );
        }
    }

    #[test]
    fn test_webhook_debug_redacts_sensitive_fields() {
        let headers = PayPalWebhookHeaders::new(
            "SHA256withRSA",
            "https://api.sandbox.paypal.com/cert",
            "trans-123",
            "super_secret_raw_signature_bytes",
            "2026-11-07T12:00:00Z",
        );
        let debug_hdr = format!("{:?}", headers);
        assert!(!debug_hdr.contains("super_secret_raw_signature_bytes"));
        assert!(debug_hdr.contains("[REDACTED_SIG]"));

        let event = PayPalWebhookEvent {
            id: "WH-1".to_string(),
            create_time: None,
            resource_type: "dispute".to_string(),
            event_version: None,
            event_type: "CUSTOMER.DISPUTE.CREATED".to_string(),
            summary: None,
            resource_version: None,
            resource: serde_json::json!({
                "buyer_email": "sensitive_buyer@example.com",
                "buyer_claim": "unauthorized transaction details"
            }),
            links: None,
        };
        let debug_event = format!("{:?}", event);
        assert!(!debug_event.contains("sensitive_buyer@example.com"));
        assert!(debug_event.contains("[PROTECTED_RESOURCE_PAYLOAD]"));

        let req = VerifyWebhookSignatureRequest {
            auth_algo: "SHA256withRSA".to_string(),
            cert_url: "https://api.sandbox.paypal.com/cert".to_string(),
            transmission_id: "trans-123".to_string(),
            transmission_sig: "secret_sig_string".to_string(),
            transmission_time: "2026-11-07T12:00:00Z".to_string(),
            webhook_id: "WH-ID-1".to_string(),
            webhook_event: serde_json::json!({ "private_data": "secret" }),
        };
        let debug_req = format!("{:?}", req);
        assert!(!debug_req.contains("secret_sig_string"));
        assert!(!debug_req.contains("private_data"));
        assert!(debug_req.contains("[REDACTED_SIG]"));
        assert!(debug_req.contains("[PROTECTED_EVENT_PAYLOAD]"));
    }

    #[test]
    fn test_webhook_verification_status_serde() {
        let json_success = r#"{"verification_status":"SUCCESS"}"#;
        let resp: VerifyWebhookSignatureResponse = serde_json::from_str(json_success).unwrap();
        assert_eq!(resp.verification_status, WebhookVerificationStatus::Success);
        assert!(resp.verification_status.is_success());

        let json_failure = r#"{"verification_status":"FAILURE"}"#;
        let resp_fail: VerifyWebhookSignatureResponse = serde_json::from_str(json_failure).unwrap();
        assert_eq!(
            resp_fail.verification_status,
            WebhookVerificationStatus::Failure
        );
        assert!(!resp_fail.verification_status.is_success());
    }

    #[test]
    fn test_webhook_event_envelope_deserialization() {
        let json_str = r#"{
            "id": "WH-90812345",
            "create_time": "2026-11-07T14:44:47Z",
            "resource_type": "dispute",
            "event_type": "CUSTOMER.DISPUTE.CREATED",
            "summary": "Dispute created by customer",
            "resource": {
                "dispute_id": "PP-D-12345",
                "dispute_amount": { "currency_code": "USD", "value": "79.99" },
                "status": "WAITING_FOR_SELLER_RESPONSE"
            }
        }"#;

        let event: PayPalWebhookEvent = serde_json::from_str(json_str).unwrap();
        assert_eq!(event.id, "WH-90812345");
        assert_eq!(event.resource_type, "dispute");
        assert_eq!(event.event_type, "CUSTOMER.DISPUTE.CREATED");
        assert_eq!(event.resource["dispute_id"], "PP-D-12345");
        assert_eq!(event.extract_resource_id(), Some("PP-D-12345".to_string()));
    }

    #[test]
    fn test_webhook_processing_status_transitions() {
        use WebhookProcessingStatus::*;

        // PENDING transitions
        assert!(Pending.can_transition_to(Processing));
        assert!(Pending.can_transition_to(Ignored));
        assert!(!Pending.can_transition_to(Processed));
        assert!(!Pending.can_transition_to(Retry));
        assert!(!Pending.can_transition_to(Failed));

        // PROCESSING transitions
        assert!(Processing.can_transition_to(Processed));
        assert!(Processing.can_transition_to(Retry));
        assert!(Processing.can_transition_to(Failed));
        assert!(Processing.can_transition_to(Ignored));
        assert!(!Processing.can_transition_to(Pending));

        // RETRY transitions
        assert!(Retry.can_transition_to(Processing));
        assert!(Retry.can_transition_to(Failed));
        assert!(Retry.can_transition_to(Ignored));
        assert!(!Retry.can_transition_to(Pending));
        assert!(!Retry.can_transition_to(Processed));

        // Terminal states cannot transition to anything
        for terminal in [Processed, Failed, Ignored] {
            assert!(terminal.is_terminal());
            for next in [Pending, Processing, Processed, Retry, Failed, Ignored] {
                assert!(!terminal.can_transition_to(next));
            }
        }

        // Serialization & parsing
        assert_eq!(Pending.as_str(), "PENDING");
        assert_eq!(
            WebhookProcessingStatus::from_str("pending").unwrap(),
            Pending
        );
        assert_eq!(
            WebhookProcessingStatus::from_str("PROCESSED").unwrap(),
            Processed
        );
        assert!(WebhookProcessingStatus::from_str("UNKNOWN").is_err());
    }

    #[test]
    fn test_exponential_backoff_delay_calculation() {
        assert_eq!(calculate_retry_delay_seconds(0), 5);
        assert_eq!(calculate_retry_delay_seconds(1), 10);
        assert_eq!(calculate_retry_delay_seconds(2), 20);
        assert_eq!(calculate_retry_delay_seconds(3), 40);
        assert_eq!(calculate_retry_delay_seconds(4), 80);
        assert_eq!(calculate_retry_delay_seconds(5), 160);
        assert_eq!(calculate_retry_delay_seconds(6), 320);
        assert_eq!(calculate_retry_delay_seconds(7), 640);
        assert_eq!(calculate_retry_delay_seconds(8), 1280);
        assert_eq!(calculate_retry_delay_seconds(9), 2560);
        // Capped at 3600 (1 hour)
        assert_eq!(calculate_retry_delay_seconds(10), 3600);
        assert_eq!(calculate_retry_delay_seconds(50), 3600);
    }

    #[test]
    fn test_compute_payload_hash() {
        let payload = b"{\"hello\":\"world\"}";
        let hash = compute_payload_hash(payload);
        assert_eq!(
            hash,
            "93a23971a914e5eacbf0a8d25154cda309c3c1c72fbb9914d47c60f3cb681588"
        );

        // Determinism check
        let hash2 = compute_payload_hash(payload);
        assert_eq!(hash, hash2);

        // Sensitivity check
        let hash_diff = compute_payload_hash(b"{\"hello\":\"world \"}");
        assert_ne!(hash, hash_diff);
    }

    #[test]
    fn test_webhook_event_record_debug_redaction() {
        let record = WebhookEventRecord {
            event_id: "WH-EVT-1".to_string(),
            event_type: "CUSTOMER.DISPUTE.CREATED".to_string(),
            resource_type: Some("dispute".to_string()),
            resource_id: Some("PP-D-123".to_string()),
            transmission_id: "trans-1".to_string(),
            processing_status: WebhookProcessingStatus::Pending,
            retry_count: 0,
            max_retries: 5,
            last_error: None,
            raw_payload: b"sensitive raw payload bytes containing buyer email".to_vec(),
            payload_hash: "abcd1234hash".to_string(),
            next_retry_at: None,
            created_at: "2026-11-07T12:00:00Z".to_string(),
            processed_at: None,
            processing_started_at: None,
            lease_token: None,
            processing_worker_id: None,
        };

        let debug_str = format!("{:?}", record);
        assert!(!debug_str.contains("sensitive raw payload bytes"));
        assert!(debug_str.contains(&format!(
            "[PROTECTED_RAW_PAYLOAD: {} bytes]",
            record.raw_payload.len()
        )));
        assert!(debug_str.contains("WH-EVT-1"));
    }
}
