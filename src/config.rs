use serde::{Deserialize, Serialize};

/// Default PayPal sandbox base URL.
pub const DEFAULT_PAYPAL_BASE_URL: &str = "https://api-m.sandbox.paypal.com";

/// Default application port.
pub const DEFAULT_PORT: u16 = 5000;

/// Default RUST_LOG filter directive.
pub const DEFAULT_RUST_LOG: &str = "agentic_commerce=info,tower_http=info,axum=info";

/// Default database URL.
pub const DEFAULT_DATABASE_URL: &str = "sqlite:data/agentic_commerce.db?mode=rwc";

/// Centralized application configuration.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// Port on which the HTTP server listens.
    pub port: u16,

    /// Tracing/logging directive string.
    pub rust_log: String,

    /// Base URL for the PayPal API (sandbox or live).
    pub paypal_base_url: String,

    /// Database connection URL.
    pub database_url: String,

    /// PayPal client ID for authentication.
    pub paypal_client_id: String,

    /// PayPal client secret (kept private; skipped in serialization).
    #[serde(skip_serializing)]
    pub paypal_client_secret: String,

    /// PayPal Webhook ID for signature verification (optional until webhook processing is enabled).
    #[serde(default)]
    pub paypal_webhook_id: Option<String>,
}

impl Config {
    /// Loads configuration from environment variables with sensible defaults.
    pub fn from_env() -> Self {
        // Attempt to load .env file if available
        dotenvy::dotenv().ok();

        let port = std::env::var("PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);

        let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_RUST_LOG.to_string());

        let paypal_base_url = std::env::var("PAYPAL_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_PAYPAL_BASE_URL.to_string());

        let database_url =
            std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());

        let paypal_client_id = std::env::var("PAYPAL_CLIENT_ID").unwrap_or_default();

        let paypal_client_secret = std::env::var("PAYPAL_CLIENT_SECRET").unwrap_or_default();

        let paypal_webhook_id = std::env::var("PAYPAL_WEBHOOK_ID")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Self {
            port,
            rust_log,
            paypal_base_url,
            database_url,
            paypal_client_id,
            paypal_client_secret,
            paypal_webhook_id,
        }
    }

    /// Validates that PayPal Webhook ID is configured.
    pub fn validate_webhook_id(&self) -> Result<&str, crate::error::AppError> {
        match &self.paypal_webhook_id {
            Some(id) if !id.trim().is_empty() => Ok(id.as_str()),
            _ => Err(crate::error::AppError::Config(
                "PAYPAL_WEBHOOK_ID is not configured".to_string(),
            )),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            rust_log: DEFAULT_RUST_LOG.to_string(),
            paypal_base_url: DEFAULT_PAYPAL_BASE_URL.to_string(),
            database_url: DEFAULT_DATABASE_URL.to_string(),
            paypal_client_id: String::new(),
            paypal_client_secret: String::new(),
            paypal_webhook_id: None,
        }
    }
}

/// Custom Debug implementation ensuring secret values are never printed or leaked.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("port", &self.port)
            .field("rust_log", &self.rust_log)
            .field("paypal_base_url", &self.paypal_base_url)
            .field("database_url", &self.database_url)
            .field("paypal_client_id", &self.paypal_client_id)
            .field("paypal_client_secret", &"[REDACTED]")
            .field("paypal_webhook_id", &self.paypal_webhook_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.port, 5000);
        assert_eq!(config.paypal_base_url, "https://api-m.sandbox.paypal.com");
        assert_eq!(config.database_url, DEFAULT_DATABASE_URL);
        assert_eq!(config.rust_log, DEFAULT_RUST_LOG);
        assert!(config.paypal_client_id.is_empty());
        assert!(config.paypal_client_secret.is_empty());
    }

    #[test]
    fn test_secret_redaction_in_debug() {
        let config = Config {
            port: 5000,
            rust_log: "info".to_string(),
            paypal_base_url: DEFAULT_PAYPAL_BASE_URL.to_string(),
            database_url: DEFAULT_DATABASE_URL.to_string(),
            paypal_client_id: "test_client_id".to_string(),
            paypal_client_secret: "super_secret_key_12345".to_string(),
            paypal_webhook_id: Some("WH-TEST-12345".to_string()),
        };

        let debug_output = format!("{:?}", config);
        assert!(!debug_output.contains("super_secret_key_12345"));
        assert!(debug_output.contains("[REDACTED]"));
        assert!(debug_output.contains("WH-TEST-12345"));
    }

    #[test]
    fn test_secret_skip_serializing() {
        let config = Config {
            port: 5000,
            rust_log: "info".to_string(),
            paypal_base_url: DEFAULT_PAYPAL_BASE_URL.to_string(),
            database_url: DEFAULT_DATABASE_URL.to_string(),
            paypal_client_id: "test_client_id".to_string(),
            paypal_client_secret: "super_secret_key_12345".to_string(),
            paypal_webhook_id: None,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("super_secret_key_12345"));
        assert!(!json.contains("paypal_client_secret"));
    }

    #[test]
    fn test_webhook_id_validation() {
        let mut config = Config::default();
        assert!(config.validate_webhook_id().is_err());

        config.paypal_webhook_id = Some("   ".to_string());
        assert!(config.validate_webhook_id().is_err());

        config.paypal_webhook_id = Some("WH-12345".to_string());
        assert_eq!(config.validate_webhook_id().unwrap(), "WH-12345");
    }
}
