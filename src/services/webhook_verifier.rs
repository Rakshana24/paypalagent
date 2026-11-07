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
        let status = self.verify_signature_status(headers, webhook_event).await?;

        if status.is_success() {
            info!(
                transmission_id = %headers.transmission_id,
                "PayPal webhook signature successfully verified"
            );
            Ok(())
        } else {
            warn!(
                transmission_id = %headers.transmission_id,
                "PayPal webhook signature verification REJECTED by PayPal"
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
        // 1. Validate configured webhook ID
        let webhook_id = self.config.validate_webhook_id()?;

        // 2. Validate certificate URL structure
        headers.validate_cert_url()?;

        // 3. Obtain valid OAuth access token from existing auth service
        let access_token = self.auth_service.get_valid_access_token().await?;

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
                    "Network error during webhook signature verification"
                );
                AppError::HttpClient(err)
            })?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            error!(
                status = %status,
                transmission_id = %headers.transmission_id,
                body = %err_body,
                "PayPal webhook signature verification API returned HTTP error"
            );
            return Err(AppError::PayPalApi(format!(
                "PayPal webhook verification API error ({}): {}",
                status, err_body
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

        let verify_resp: VerifyWebhookSignatureResponse = serde_json::from_str(&body_text)
            .map_err(|err| {
                error!(
                    transmission_id = %headers.transmission_id,
                    body = %body_text,
                    error = %err,
                    "Failed to deserialize PayPal webhook verification response"
                );
                AppError::Serialization(err)
            })?;

        Ok(verify_resp.verification_status)
    }
}
