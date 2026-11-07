use serde::{Deserialize, Serialize};

/// Public application configuration response contract (excluding any secrets).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfigDto {
    pub port: u16,
    pub paypal_base_url: String,
    pub paypal_client_id_configured: bool,
    pub paypal_client_secret_configured: bool,
}
