use serde::{Deserialize, Serialize};

/// Response schema for GET /health endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthResponse {
    pub success: bool,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_response_serialization() {
        let resp = HealthResponse {
            success: true,
            message: "AgenticCommerce backend is running".to_string(),
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r#"{"success":true,"message":"AgenticCommerce backend is running"}"#
        );
    }
}
