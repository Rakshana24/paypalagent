use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::error::AppError;
use crate::models::webhook::{
    PayPalWebhookHeaders, VerifyWebhookSignatureRequest, VerifyWebhookSignatureResponse,
    WebhookVerificationStatus,
};
use crate::services::auth::PayPalAuthService;

/// Service for verifying PayPal webhook transmission signatures against PayPal API.
#[derive(Clone)]
pub struct WebhookVerifierService {
    config: Arc<Config>,
    http_client: reqwest::Client,
    auth_service: Arc<PayPalAuthService>,
}

impl WebhookVerifierService {
    /// Creates a new `WebhookVerifierService`.
    pub fn new(
        config: Arc<Config>,
        http_client: reqwest::Client,
        auth_service: Arc<PayPalAuthService>,
    ) -> Self {
        Self {
            config,
            http_client,
            auth_service,
        }
    }

    /// Verifies the authenticity of a PayPal webhook notification using PayPal's verification API.
    ///
    /// # Arguments
    /// * `headers` - Validated transmission headers sent with the webhook delivery.
    /// * `webhook_event` - The raw or deserialized event JSON payload.
    ///
    /// # Returns
    /// * `Ok(())` if PayPal responds with `verification_status: "SUCCESS"`.
    /// * `Err(AppError::WebhookVerificationFailed)` if PayPal responds with `verification_status: "FAILURE"`.
    /// * `Err(AppError)` if configuration, authentication, network, or server errors occur.
    pub async fn verify_signature(
        &self,
        headers: &PayPalWebhookHeaders,
        webhook_event: &serde_json::Value,
    ) -> Result<(), AppError> {
        self.verify_signature_with_raw(headers, webhook_event, None)
            .await
    }

    /// Verifies the authenticity of a PayPal webhook notification, preserving raw request bytes for diagnostic comparison.
    pub async fn verify_signature_with_raw(
        &self,
        headers: &PayPalWebhookHeaders,
        webhook_event: &serde_json::Value,
        raw_body: Option<&[u8]>,
    ) -> Result<(), AppError> {
        let status = self
            .verify_signature_status_with_raw(headers, webhook_event, raw_body)
            .await?;

        if status.is_success() {
            info!(
                transmission_id = %headers.transmission_id,
                auth_algo = %headers.auth_algo,
                "PayPal webhook signature successfully verified (verification_status=SUCCESS)"
            );
            Ok(())
        } else {
            warn!(
                transmission_id = %headers.transmission_id,
                auth_algo = %headers.auth_algo,
                webhook_id = %self.config.validate_webhook_id().unwrap_or("UNCONFIGURED"),
                rejection_reason = "PAYPAL_SIGNATURE_VERIFICATION_REJECTED",
                verification_status = ?status,
                "PayPal webhook signature verification REJECTED by PayPal: verification_status=FAILURE"
            );
            Err(AppError::WebhookVerificationFailed(
                "PayPal webhook signature verification failed (status: FAILURE)".to_string(),
            ))
        }
    }

    /// Verifies the webhook signature and returns the raw `WebhookVerificationStatus` from PayPal.
    pub async fn verify_signature_status(
        &self,
        headers: &PayPalWebhookHeaders,
        webhook_event: &serde_json::Value,
    ) -> Result<WebhookVerificationStatus, AppError> {
        self.verify_signature_status_with_raw(headers, webhook_event, None)
            .await
    }

    /// Verifies the webhook signature and returns the raw `WebhookVerificationStatus` from PayPal,
    /// recording diagnostic telemetry on raw request bytes preservation and distinguishing rejection causes.
    pub async fn verify_signature_status_with_raw(
        &self,
        headers: &PayPalWebhookHeaders,
        webhook_event: &serde_json::Value,
        raw_body: Option<&[u8]>,
    ) -> Result<WebhookVerificationStatus, AppError> {
        // Diagnostic telemetry: Inspect preserved raw request bytes vs re-serialized value
        if let Some(bytes) = raw_body {
            let reserialized = serde_json::to_vec(webhook_event).unwrap_or_default();
            if reserialized != bytes {
                debug!(
                    transmission_id = %headers.transmission_id,
                    raw_len = bytes.len(),
                    reserialized_len = reserialized.len(),
                    "Diagnostic note: parsed serde_json::Value serialization differs from original raw request bytes"
                );
            } else {
                debug!(
                    transmission_id = %headers.transmission_id,
                    raw_len = bytes.len(),
                    "Diagnostic note: parsed serde_json::Value matches raw request bytes byte-for-byte"
                );
            }
        }

        // 1. Validate configured webhook ID
        let webhook_id = self.config.validate_webhook_id()?;

        // 2. Validate certificate URL structure (distinguishing local cert validation rejection)
        if let Err(err) = headers.validate_cert_url() {
            warn!(
                transmission_id = %headers.transmission_id,
                cert_url = %headers.cert_url,
                error = %err,
                rejection_reason = "LOCAL_CERT_URL_VALIDATION_FAILED",
                "PayPal webhook rejected: Local certificate URL validation failed before calling PayPal API"
            );
            return Err(err);
        }

        // 3. Obtain valid OAuth access token from existing auth service (distinguishing OAuth failure)
        let access_token = match self.auth_service.get_valid_access_token().await {
            Ok(token) => token,
            Err(err) => {
                error!(
                    transmission_id = %headers.transmission_id,
                    error = %err,
                    rejection_reason = "OAUTH_TOKEN_ACQUISITION_FAILED",
                    "PayPal webhook verification failed: Unable to acquire OAuth access token for PayPal API"
                );
                return Err(err);
            }
        };

        // 4. Build verification request payload
        let verify_req = VerifyWebhookSignatureRequest {
            auth_algo: headers.auth_algo.clone(),
            cert_url: headers.cert_url.clone(),
            transmission_id: headers.transmission_id.clone(),
            transmission_sig: headers.transmission_sig.clone(),
            transmission_time: headers.transmission_time.clone(),
            webhook_id: webhook_id.to_string(),
            webhook_event: webhook_event.clone(),
        };

        let url = format!(
            "{}/v1/notifications/verify-webhook-signature",
            self.config.paypal_base_url.trim_end_matches('/')
        );

        debug!(
            transmission_id = %headers.transmission_id,
            url = %url,
            "Sending webhook signature verification request to PayPal"
        );

        // 5. Send POST request with Bearer authorization
        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(access_token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&verify_req)
            .send()
            .await
            .map_err(|err| {
                error!(
                    transmission_id = %headers.transmission_id,
                    error = %err,
                    rejection_reason = "PAYPAL_VERIFY_NETWORK_ERROR",
                    "Network error during webhook signature verification"
                );
                AppError::HttpClient(err)
            })?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            let sanitized_err = sanitize_paypal_response(&err_body);
            error!(
                status = %status,
                transmission_id = %headers.transmission_id,
                rejection_reason = "PAYPAL_VERIFY_API_HTTP_ERROR",
                body = %sanitized_err,
                "PayPal webhook signature verification API returned HTTP error"
            );
            return Err(AppError::PayPalApi(format!(
                "PayPal webhook verification API error ({}): {}",
                status, sanitized_err
            )));
        }

        // 6. Parse response and evaluate verification status
        let body_text = resp.text().await.map_err(|err| {
            error!(
                transmission_id = %headers.transmission_id,
                error = %err,
                "Failed to read PayPal webhook verification response body"
            );
            AppError::HttpClient(err)
        })?;

        let sanitized_resp = sanitize_paypal_response(&body_text);
        debug!(
            transmission_id = %headers.transmission_id,
            status = %status,
            body = %sanitized_resp,
            "Received PayPal webhook verification response"
        );

        let verify_resp: VerifyWebhookSignatureResponse = serde_json::from_str(&body_text)
            .map_err(|err| {
                error!(
                    transmission_id = %headers.transmission_id,
                    body = %sanitized_resp,
                    error = %err,
                    "Failed to deserialize PayPal webhook verification response"
                );
                AppError::Serialization(err)
            })?;

        Ok(verify_resp.verification_status)
    }
}

/// Sanitizes PayPal API response bodies for safe diagnostic logging.
/// Redacts sensitive credential, token, and signature fields; truncates arbitrary text.
pub fn sanitize_paypal_response(body: &str) -> String {
    if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(body) {
        sanitize_json_value(&mut json_val);
        serde_json::to_string(&json_val).unwrap_or_else(|_| "[REDACTED_JSON]".to_string())
    } else {
        let truncated: String = body.chars().take(300).collect();
        truncated.replace('\n', " ").replace('\r', "")
    }
}

fn sanitize_json_value(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let lower = k.to_ascii_lowercase();
                if lower.contains("token")
                    || lower.contains("secret")
                    || lower.contains("sig")
                    || lower.contains("cert")
                    || lower.contains("authorization")
                    || lower.contains("password")
                {
                    *v = serde_json::json!("[REDACTED]");
                } else {
                    sanitize_json_value(v);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                sanitize_json_value(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_paypal_response_redacts_sensitive_keys() {
        let input = r#"{"verification_status":"FAILURE","access_token":"secret123","transmission_sig":"sig456","nested":{"client_secret":"shh","status":"error"}}"#;
        let sanitized = sanitize_paypal_response(input);
        assert!(!sanitized.contains("secret123"));
        assert!(!sanitized.contains("sig456"));
        assert!(!sanitized.contains("shh"));
        assert!(sanitized.contains("[REDACTED]"));
        assert!(sanitized.contains("FAILURE"));
        assert!(sanitized.contains("error"));
    }

    #[test]
    fn test_sanitize_paypal_response_plain_text() {
        let plain = "Service Unavailable\nInternal Error";
        let sanitized = sanitize_paypal_response(plain);
        assert_eq!(sanitized, "Service Unavailable Internal Error");
    }
}
