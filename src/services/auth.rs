use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info};

use crate::config::Config;
use crate::error::AppError;
use crate::models::{AuthCheckResponse, PayPalOAuthError, PayPalTokenResponse};

/// Safety margin in seconds prior to token expiration to trigger automatic refresh.
pub const TOKEN_EXPIRY_SAFETY_MARGIN_SECS: u64 = 60;

/// Internal in-memory representation of a cached PayPal OAuth token.
#[derive(Clone)]
pub struct CachedToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_at: Instant,
    pub initial_expires_in: u64,
}

impl CachedToken {
    /// Determines whether the cached token is still valid outside the safety margin.
    pub fn is_valid(&self) -> bool {
        let now = Instant::now();
        let margin = Duration::from_secs(TOKEN_EXPIRY_SAFETY_MARGIN_SECS);
        self.expires_at > now + margin
    }

    /// Calculates remaining lifetime in seconds, or 0 if expired.
    pub fn remaining_secs(&self) -> u64 {
        let now = Instant::now();
        if self.expires_at > now {
            (self.expires_at - now).as_secs()
        } else {
            0
        }
    }
}

/// Thread-safe authentication service managing the PayPal OAuth 2.0 token lifecycle.
pub struct PayPalAuthService {
    config: Arc<Config>,
    http_client: reqwest::Client,
    cache: RwLock<Option<CachedToken>>,
    refresh_lock: Mutex<()>,
}

impl PayPalAuthService {
    /// Creates a new `PayPalAuthService` instance with the given configuration and HTTP client.
    pub fn new(config: Arc<Config>, http_client: reqwest::Client) -> Self {
        Self {
            config,
            http_client,
            cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    /// Retrieves a valid access token.
    /// Reuses the cached token if valid. Automatically refreshes prior to expiration.
    /// Uses double-checked locking to ensure concurrent requests do not initiate redundant refreshes.
    pub async fn get_valid_access_token(&self) -> Result<String, AppError> {
        // Fast path: check valid token under read lock
        {
            let read_guard = self.cache.read().await;
            if let Some(ref cached) = *read_guard {
                if cached.is_valid() {
                    debug!("Using valid cached PayPal OAuth token");
                    return Ok(cached.access_token.clone());
                }
            }
        }

        // Slow path: acquire refresh lock to prevent concurrent refresh stampedes
        let _refresh_guard = self.refresh_lock.lock().await;

        // Double-check: another task may have completed refresh while we were waiting for the lock
        {
            let read_guard = self.cache.read().await;
            if let Some(ref cached) = *read_guard {
                if cached.is_valid() {
                    debug!("Using freshly refreshed cached PayPal OAuth token");
                    return Ok(cached.access_token.clone());
                }
            }
        }

        // Request a new token from PayPal
        let token_response = self.fetch_oauth_token().await?;

        let cached = CachedToken {
            access_token: token_response.access_token.clone(),
            token_type: token_response.token_type.clone(),
            expires_at: Instant::now() + Duration::from_secs(token_response.expires_in),
            initial_expires_in: token_response.expires_in,
        };

        let mut write_guard = self.cache.write().await;
        *write_guard = Some(cached);

        Ok(token_response.access_token)
    }

    /// Provides safe public authentication status without exposing tokens or credentials.
    pub async fn get_auth_status(&self) -> Result<AuthCheckResponse, AppError> {
        // Ensure valid token is obtained or refreshed
        let _ = self.get_valid_access_token().await?;

        let read_guard = self.cache.read().await;
        if let Some(ref cached) = *read_guard {
            Ok(AuthCheckResponse {
                success: true,
                message: "PayPal Sandbox authentication successful".to_string(),
                token_type: cached.token_type.clone(),
                expires_in: cached.remaining_secs(),
            })
        } else {
            Err(AppError::PayPalAuth(
                "Unable to retrieve token status".to_string(),
            ))
        }
    }

    /// Executes the HTTP POST request to /v1/oauth2/token with Basic Authentication and client credentials.
    async fn fetch_oauth_token(&self) -> Result<PayPalTokenResponse, AppError> {
        if self.config.paypal_client_id.trim().is_empty()
            || self.config.paypal_client_secret.trim().is_empty()
        {
            return Err(AppError::Config(
                "PayPal Client ID or Client Secret is not configured".to_string(),
            ));
        }

        let token_url = format!(
            "{}/v1/oauth2/token",
            self.config.paypal_base_url.trim_end_matches('/')
        );

        debug!("Requesting OAuth token from {}", token_url);

        let response = self
            .http_client
            .post(&token_url)
            .basic_auth(
                &self.config.paypal_client_id,
                Some(&self.config.paypal_client_secret),
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en_US")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(|err| {
                error!("Network failure during PayPal OAuth request: {}", err);
                AppError::HttpClient(err)
            })?;

        let status = response.status();
        if status.is_success() {
            let body_text = response.text().await.map_err(|err| {
                error!("Failed to read PayPal OAuth response body: {}", err);
                AppError::HttpClient(err)
            })?;

            let token_response: PayPalTokenResponse =
                serde_json::from_str(&body_text).map_err(|err| {
                    error!("Failed to parse successful PayPal OAuth response: {}", err);
                    AppError::Serialization(err)
                })?;

            info!(
                "Successfully acquired PayPal OAuth token (token_type: {}, expires_in: {}s)",
                token_response.token_type, token_response.expires_in
            );

            Ok(token_response)
        } else {
            let error_bytes = response.bytes().await.unwrap_or_default();
            let error_message =
                if let Ok(oauth_err) = serde_json::from_slice::<PayPalOAuthError>(&error_bytes) {
                    let desc = oauth_err.error_description.unwrap_or_default();
                    if desc.is_empty() {
                        format!("OAuth error: {}", oauth_err.error)
                    } else {
                        format!("OAuth error {}: {}", oauth_err.error, desc)
                    }
                } else if let Ok(text) = std::str::from_utf8(&error_bytes) {
                    format!(
                        "HTTP {} error from PayPal: {}",
                        status,
                        text.chars().take(200).collect::<String>()
                    )
                } else {
                    format!("HTTP {} error from PayPal OAuth endpoint", status)
                };

            error!("PayPal OAuth authentication failed: {}", error_message);
            Err(AppError::PayPalAuth(error_message))
        }
    }

    /// Test helper to inspect cache contents without breaking encapsulation.
    #[doc(hidden)]
    pub async fn set_cached_token_for_test(&self, token: CachedToken) {
        let mut guard = self.cache.write().await;
        *guard = Some(token);
    }

    /// Test helper to get cache count.
    #[doc(hidden)]
    pub async fn has_cached_token(&self) -> bool {
        self.cache.read().await.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cached_token_validity() {
        let token_valid = CachedToken {
            access_token: "test_token".to_string(),
            token_type: "Bearer".to_string(),
            expires_at: Instant::now() + Duration::from_secs(120),
            initial_expires_in: 120,
        };
        assert!(token_valid.is_valid());
        assert!(token_valid.remaining_secs() > 60);

        // Token within 60-second safety margin should be considered invalid
        let token_expiring_soon = CachedToken {
            access_token: "test_token".to_string(),
            token_type: "Bearer".to_string(),
            expires_at: Instant::now() + Duration::from_secs(30),
            initial_expires_in: 30,
        };
        assert!(!token_expiring_soon.is_valid());
    }
}
