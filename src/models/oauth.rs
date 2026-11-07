use serde::{Deserialize, Serialize};

/// Request configuration parameters for PayPal OAuth token generation (/v1/oauth2/token).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalOAuthRequest {
    /// OAuth grant type. Defaults to "client_credentials".
    pub grant_type: String,

    /// Optional directive to bypass PayPal token caching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_cache: Option<bool>,
}

impl Default for PayPalOAuthRequest {
    fn default() -> Self {
        Self {
            grant_type: "client_credentials".to_string(),
            ignore_cache: None,
        }
    }
}

/// Successful OAuth 2.0 token response from PayPal (/v1/oauth2/token).
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalTokenResponse {
    /// Granted scopes.
    #[serde(default)]
    pub scope: Option<String>,

    /// Access token string.
    pub access_token: String,

    /// Token type (e.g. "Bearer").
    pub token_type: String,

    /// PayPal Application ID.
    #[serde(default)]
    pub app_id: Option<String>,

    /// Token lifetime in seconds.
    pub expires_in: u64,

    /// Nonce string returned by PayPal.
    #[serde(default)]
    pub nonce: Option<String>,
}

/// Custom Debug implementation ensuring access tokens are never printed in logs.
impl std::fmt::Debug for PayPalTokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayPalTokenResponse")
            .field("scope", &self.scope)
            .field("access_token", &"[REDACTED]")
            .field("token_type", &self.token_type)
            .field("app_id", &self.app_id)
            .field("expires_in", &self.expires_in)
            .field("nonce", &self.nonce)
            .finish()
    }
}

/// Response payload for GET /api/paypal/auth-check endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthCheckResponse {
    pub success: bool,
    pub message: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_request_default() {
        let req = PayPalOAuthRequest::default();
        assert_eq!(req.grant_type, "client_credentials");
        assert_eq!(req.ignore_cache, None);

        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"grant_type":"client_credentials"}"#);
    }

    #[test]
    fn test_token_response_deserialization() {
        let json_data = r#"{
            "scope": "https://uri.paypal.com/services/invoicing",
            "access_token": "A21AAK_secret_access_token_12345",
            "token_type": "Bearer",
            "app_id": "APP-80W284485P519543T",
            "expires_in": 32400,
            "nonce": "2026-11-07T14:00:00Z"
        }"#;

        let token_resp: PayPalTokenResponse = serde_json::from_str(json_data).unwrap();
        assert_eq!(token_resp.token_type, "Bearer");
        assert_eq!(token_resp.expires_in, 32400);
        assert_eq!(token_resp.access_token, "A21AAK_secret_access_token_12345");

        // Verify Debug hides access token
        let debug_str = format!("{:?}", token_resp);
        assert!(!debug_str.contains("A21AAK_secret_access_token_12345"));
        assert!(debug_str.contains("[REDACTED]"));
    }

    #[test]
    fn test_auth_check_response_serialization() {
        let resp = AuthCheckResponse {
            success: true,
            message: "PayPal Sandbox authentication successful".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 300,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""success":true"#));
        assert!(json.contains(r#""message":"PayPal Sandbox authentication successful""#));
        assert!(json.contains(r#""token_type":"Bearer""#));
        assert!(json.contains(r#""expires_in":300"#));
        // Verify access_token is NOT part of AuthCheckResponse
        assert!(!json.contains("access_token"));
    }
}
