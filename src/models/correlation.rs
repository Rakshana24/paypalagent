use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;

/// Outcome status of the deterministic dispute-to-session correlation engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CorrelationStatus {
    /// Deterministic match verified and cryptographically consistent.
    Matched,
    /// Ambiguous, conflicting, tampered, unfinalized, or insufficient data requiring human review.
    ManualReviewRequired,
    /// Identifier queried was not found in the database.
    NotFound,
}

impl CorrelationStatus {
    /// Returns the uppercase string representation used in database constraints.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Matched => "MATCHED",
            Self::ManualReviewRequired => "MANUAL_REVIEW_REQUIRED",
            Self::NotFound => "NOT_FOUND",
        }
    }

    /// Parses a string into `CorrelationStatus`.
    pub fn from_str(s: &str) -> Result<Self, AppError> {
        match s.trim().to_uppercase().as_str() {
            "MATCHED" => Ok(Self::Matched),
            "MANUAL_REVIEW_REQUIRED" => Ok(Self::ManualReviewRequired),
            "NOT_FOUND" => Ok(Self::NotFound),
            other => Err(AppError::InvalidPayload(format!(
                "Invalid correlation status: '{}'",
                other
            ))),
        }
    }
}

impl std::fmt::Display for CorrelationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Mechanism/tier by which the session was correlated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MatchedBy {
    /// Tier 1: Matched directly by PayPal Capture ID (seller_transaction_id).
    CaptureId,
    /// Tier 2: Matched by Custom ID / Reference ID with verified PayPal transaction.
    CustomId,
    /// Tier 3: Matched by PayPal Order ID (sessions.paypal_order_id).
    OrderId,
    /// Tier 4a: Matched by Capture ID retrieved from PayPal Show Dispute Details API.
    DisputeDetailsCaptureId,
    /// Tier 4b: Matched by Custom ID retrieved from PayPal Show Dispute Details API.
    DisputeDetailsCustomId,
    /// Tier 4c: Matched by Order ID retrieved from PayPal Show Dispute Details API.
    DisputeDetailsOrderId,
}

impl std::fmt::Display for MatchedBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CaptureId => write!(f, "CAPTURE_ID"),
            Self::CustomId => write!(f, "CUSTOM_ID"),
            Self::OrderId => write!(f, "ORDER_ID"),
            Self::DisputeDetailsCaptureId => write!(f, "DISPUTE_DETAILS_CAPTURE_ID"),
            Self::DisputeDetailsCustomId => write!(f, "DISPUTE_DETAILS_CUSTOM_ID"),
            Self::DisputeDetailsOrderId => write!(f, "DISPUTE_DETAILS_ORDER_ID"),
        }
    }
}

/// Strongly typed result produced by the dispute correlation engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrelationResult {
    /// Correlation status outcome.
    pub status: CorrelationStatus,

    /// Matched AgentAudit purchase session ID (if matched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Associated PayPal Order ID (if matched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paypal_order_id: Option<String>,

    /// Associated PayPal Capture ID (if matched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paypal_capture_id: Option<String>,

    /// Matching tier / mechanism (if matched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_by: Option<MatchedBy>,

    /// ISO 8601 timestamp of when correlation occurred (if matched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_timestamp: Option<String>,

    /// Explanation detailing match justification or failure cause.
    pub explanation: String,
}

impl CorrelationResult {
    /// Creates a successful match result.
    pub fn matched(
        session_id: impl Into<String>,
        paypal_order_id: impl Into<String>,
        paypal_capture_id: impl Into<String>,
        matched_by: MatchedBy,
        explanation: impl Into<String>,
    ) -> Self {
        Self {
            status: CorrelationStatus::Matched,
            session_id: Some(session_id.into()),
            paypal_order_id: Some(paypal_order_id.into()),
            paypal_capture_id: Some(paypal_capture_id.into()),
            matched_by: Some(matched_by),
            correlation_timestamp: Some(chrono::Utc::now().to_rfc3339()),
            explanation: explanation.into(),
        }
    }

    /// Creates a manual-review-required result.
    pub fn manual_review(explanation: impl Into<String>) -> Self {
        Self {
            status: CorrelationStatus::ManualReviewRequired,
            session_id: None,
            paypal_order_id: None,
            paypal_capture_id: None,
            matched_by: None,
            correlation_timestamp: None,
            explanation: explanation.into(),
        }
    }

    /// Creates a not-found result.
    pub fn not_found(explanation: impl Into<String>) -> Self {
        Self {
            status: CorrelationStatus::NotFound,
            session_id: None,
            paypal_order_id: None,
            paypal_capture_id: None,
            matched_by: None,
            correlation_timestamp: None,
            explanation: explanation.into(),
        }
    }

    /// Returns true if the correlation produced a confirmed match.
    pub fn is_matched(&self) -> bool {
        self.status == CorrelationStatus::Matched
    }

    /// Returns true if manual review is required.
    pub fn is_manual_review_required(&self) -> bool {
        self.status == CorrelationStatus::ManualReviewRequired
    }

    /// Returns true if the queried transaction was not found.
    pub fn is_not_found(&self) -> bool {
        self.status == CorrelationStatus::NotFound
    }
}

/// Identifiers extracted from a PayPal dispute resource or API payload.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExtractedDisputeIdentifiers {
    /// Dispute identifier (e.g. "PP-D-12345" or "DP-12345").
    pub dispute_id: Option<String>,
    /// Unique PayPal capture IDs / seller_transaction_ids.
    pub capture_ids: Vec<String>,
    /// Unique session / custom identifiers.
    pub custom_ids: Vec<String>,
    /// Unique PayPal Order IDs.
    pub order_ids: Vec<String>,
}

/// Extracts transaction identifiers from a PayPal dispute resource payload.
pub fn extract_dispute_identifiers(resource: &serde_json::Value) -> ExtractedDisputeIdentifiers {
    let mut dispute_id = None;
    let mut capture_ids = Vec::new();
    let mut custom_ids = Vec::new();
    let mut order_ids = Vec::new();

    // 1. Extract dispute_id
    if let Some(id) = resource.get("dispute_id").or_else(|| resource.get("id")) {
        if let Some(s) = id.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                dispute_id = Some(trimmed.to_string());
            }
        }
    }

    // Helper to push non-empty unique string
    let push_unique = |vec: &mut Vec<String>, val: &str| {
        let t = val.trim();
        if !t.is_empty() && !vec.iter().any(|existing| existing == t) {
            vec.push(t.to_string());
        }
    };

    // Helper for custom_id: strips "session-" if present
    let push_custom = |vec: &mut Vec<String>, val: &str| {
        let t = val.trim();
        if !t.is_empty() {
            let candidate = t.strip_prefix("session-").unwrap_or(t).trim();
            if !candidate.is_empty() && !vec.iter().any(|existing| existing == candidate) {
                vec.push(candidate.to_string());
            }
        }
    };

    // 2. Extract root-level identifier fields
    if let Some(s) = resource
        .get("seller_transaction_id")
        .and_then(|v| v.as_str())
    {
        push_unique(&mut capture_ids, s);
    }
    if let Some(s) = resource.get("capture_id").and_then(|v| v.as_str()) {
        push_unique(&mut capture_ids, s);
    }
    if let Some(s) = resource.get("custom").and_then(|v| v.as_str()) {
        push_custom(&mut custom_ids, s);
    }
    if let Some(s) = resource.get("custom_id").and_then(|v| v.as_str()) {
        push_custom(&mut custom_ids, s);
    }
    if let Some(s) = resource.get("reference_id").and_then(|v| v.as_str()) {
        push_custom(&mut custom_ids, s);
    }
    if let Some(s) = resource.get("order_id").and_then(|v| v.as_str()) {
        push_unique(&mut order_ids, s);
    }

    // 3. Extract from disputed_transactions array
    if let Some(tx_array) = resource
        .get("disputed_transactions")
        .and_then(|v| v.as_array())
    {
        for tx in tx_array {
            if let Some(s) = tx.get("seller_transaction_id").and_then(|v| v.as_str()) {
                push_unique(&mut capture_ids, s);
            }
            if let Some(s) = tx.get("capture_id").and_then(|v| v.as_str()) {
                push_unique(&mut capture_ids, s);
            }
            if let Some(s) = tx.get("custom").and_then(|v| v.as_str()) {
                push_custom(&mut custom_ids, s);
            }
            if let Some(s) = tx.get("custom_id").and_then(|v| v.as_str()) {
                push_custom(&mut custom_ids, s);
            }
            if let Some(s) = tx.get("reference_id").and_then(|v| v.as_str()) {
                push_custom(&mut custom_ids, s);
            }
            if let Some(s) = tx.get("order_id").and_then(|v| v.as_str()) {
                push_unique(&mut order_ids, s);
            }
        }
    }

    // 4. Extract order_id from links if present
    if let Some(links) = resource.get("links").and_then(|v| v.as_array()) {
        for link in links {
            if let (Some(rel), Some(href)) = (
                link.get("rel").and_then(|v| v.as_str()),
                link.get("href").and_then(|v| v.as_str()),
            ) {
                if rel == "order" {
                    if let Some(order_id) = href.split('/').last() {
                        push_unique(&mut order_ids, order_id);
                    }
                }
            }
        }
    }

    ExtractedDisputeIdentifiers {
        dispute_id,
        capture_ids,
        custom_ids,
        order_ids,
    }
}

/// Validates whether a candidate string conforms to a valid UUID format.
pub fn is_valid_session_uuid(candidate: &str) -> bool {
    Uuid::parse_str(candidate).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_identifiers_root_and_array() {
        let dispute = json!({
            "dispute_id": "PP-D-1001",
            "seller_transaction_id": "CAP-ROOT-1",
            "custom": "session-11111111-1111-1111-1111-111111111111",
            "order_id": "ORD-ROOT-1",
            "disputed_transactions": [
                {
                    "seller_transaction_id": "CAP-ARRAY-1",
                    "custom_id": "22222222-2222-2222-2222-222222222222",
                    "reference_id": "session-33333333-3333-3333-3333-333333333333",
                    "order_id": "ORD-ARRAY-1"
                }
            ]
        });

        let extracted = extract_dispute_identifiers(&dispute);
        assert_eq!(extracted.dispute_id.as_deref(), Some("PP-D-1001"));
        assert_eq!(extracted.capture_ids, vec!["CAP-ROOT-1", "CAP-ARRAY-1"]);
        assert_eq!(
            extracted.custom_ids,
            vec![
                "11111111-1111-1111-1111-111111111111",
                "22222222-2222-2222-2222-222222222222",
                "33333333-3333-3333-3333-333333333333"
            ]
        );
        assert_eq!(extracted.order_ids, vec!["ORD-ROOT-1", "ORD-ARRAY-1"]);
    }

    #[test]
    fn test_uuid_validation() {
        assert!(is_valid_session_uuid(
            "123e4567-e89b-12d3-a456-426614174000"
        ));
        assert!(!is_valid_session_uuid("not-a-valid-uuid"));
        assert!(!is_valid_session_uuid(""));
    }

    #[test]
    fn test_correlation_result_constructors() {
        let matched = CorrelationResult::matched(
            "s1",
            "o1",
            "c1",
            MatchedBy::CaptureId,
            "Matched via capture",
        );
        assert!(matched.is_matched());
        assert_eq!(matched.status, CorrelationStatus::Matched);
        assert_eq!(matched.session_id.as_deref(), Some("s1"));
        assert_eq!(matched.paypal_order_id.as_deref(), Some("o1"));
        assert_eq!(matched.paypal_capture_id.as_deref(), Some("c1"));
        assert_eq!(matched.matched_by, Some(MatchedBy::CaptureId));
        assert!(matched.correlation_timestamp.is_some());

        let manual = CorrelationResult::manual_review("Review reason");
        assert!(manual.is_manual_review_required());
        assert_eq!(manual.status, CorrelationStatus::ManualReviewRequired);
        assert!(manual.session_id.is_none());

        let not_found = CorrelationResult::not_found("Not found reason");
        assert!(not_found.is_not_found());
        assert_eq!(not_found.status, CorrelationStatus::NotFound);
        assert!(not_found.session_id.is_none());
    }
}
