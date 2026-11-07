use serde::{Deserialize, Serialize};

/// Standard PayPal OAuth2 error response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalOAuthError {
    /// Error code (e.g. "invalid_client", "unsupported_grant_type").
    pub error: String,

    /// Human-readable description of the error.
    #[serde(default)]
    pub error_description: Option<String>,
}

/// Detailed validation or processing issue in PayPal REST API v2 error response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalErrorDetail {
    /// Name of the field causing the issue.
    #[serde(default)]
    pub field: Option<String>,

    /// Value that caused the issue.
    #[serde(default)]
    pub value: Option<String>,

    /// Location of the field (e.g. "body", "path", "query").
    #[serde(default)]
    pub location: Option<String>,

    /// Specific reason for the error.
    pub issue: String,

    /// Human-readable explanation of the issue.
    #[serde(default)]
    pub description: Option<String>,
}

/// Link navigation object in PayPal error response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalLinkDescription {
    /// Target URL.
    pub href: String,

    /// Link relation type (e.g. "information_link").
    pub rel: String,

    /// HTTP method (e.g. "GET").
    #[serde(default)]
    pub method: Option<String>,
}

/// Standard PayPal REST API v2 error response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalApiError {
    /// High-level error classification (e.g. "INVALID_REQUEST", "UNPROCESSABLE_ENTITY").
    pub name: String,

    /// Human-readable error message.
    pub message: String,

    /// PayPal internal debugging ID.
    #[serde(default)]
    pub debug_id: Option<String>,

    /// Detailed field-level errors if present.
    #[serde(default)]
    pub details: Option<Vec<PayPalErrorDetail>>,

    /// HATEOAS links related to the error.
    #[serde(default)]
    pub links: Option<Vec<PayPalLinkDescription>>,
}

/// Unified PayPal error response that can deserialize either OAuth or API v2 errors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PayPalErrorResponse {
    OAuth(PayPalOAuthError),
    Api(PayPalApiError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_oauth_error() {
        let json_data = r#"{
            "error": "invalid_client",
            "error_description": "Client Authentication failed"
        }"#;

        let parsed: PayPalOAuthError = serde_json::from_str(json_data).unwrap();
        assert_eq!(parsed.error, "invalid_client");
        assert_eq!(
            parsed.error_description.as_deref(),
            Some("Client Authentication failed")
        );

        let untagged: PayPalErrorResponse = serde_json::from_str(json_data).unwrap();
        match untagged {
            PayPalErrorResponse::OAuth(err) => assert_eq!(err.error, "invalid_client"),
            _ => panic!("Expected OAuth error variant"),
        }
    }

    #[test]
    fn test_deserialize_api_error() {
        let json_data = r#"{
            "name": "INVALID_REQUEST",
            "message": "Request is not well-formed, syntactically incorrect, or violates schema.",
            "debug_id": "9f5a113bc9490",
            "details": [
                {
                    "field": "/purchase_units/@reference_id=='default'/amount/value",
                    "value": "-10.00",
                    "issue": "CANNOT_BE_NEGATIVE",
                    "description": "The amount cannot be negative"
                }
            ],
            "links": [
                {
                    "href": "https://developer.paypal.com/docs/api/orders/v2/#error-CANNOT_BE_NEGATIVE",
                    "rel": "information_link",
                    "method": "GET"
                }
            ]
        }"#;

        let parsed: PayPalApiError = serde_json::from_str(json_data).unwrap();
        assert_eq!(parsed.name, "INVALID_REQUEST");
        assert_eq!(parsed.debug_id.as_deref(), Some("9f5a113bc9490"));
        let details = parsed.details.unwrap();
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].issue, "CANNOT_BE_NEGATIVE");

        let untagged: PayPalErrorResponse = serde_json::from_str(json_data).unwrap();
        match untagged {
            PayPalErrorResponse::Api(err) => assert_eq!(err.name, "INVALID_REQUEST"),
            _ => panic!("Expected Api error variant"),
        }
    }
}
