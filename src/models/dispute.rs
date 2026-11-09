use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::models::correlation::{CorrelationStatus, MatchedBy};

/// Standard 64-zero SHA-256 genesis hash used for unlinked/uncorrelated dispute chains.
pub const GENESIS_HASH_DEFAULT: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// The anchor type used to initialize a dispute's cryptographic audit chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GenesisAnchorType {
    /// Anchored to a verified purchase session root hash.
    PurchaseRootHash,
    /// Anchored to a default 64-zero SHA-256 genesis hash (for unlinked/uncorrelated disputes).
    GenesisHash,
}

impl GenesisAnchorType {
    /// Returns the database string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PurchaseRootHash => "PURCHASE_ROOT_HASH",
            Self::GenesisHash => "GENESIS_HASH",
        }
    }

    /// Parses string into GenesisAnchorType.
    pub fn from_str(s: &str) -> Result<Self, AppError> {
        match s.trim().to_uppercase().as_str() {
            "PURCHASE_ROOT_HASH" => Ok(Self::PurchaseRootHash),
            "GENESIS_HASH" => Ok(Self::GenesisHash),
            other => Err(AppError::InvalidDisputeData(format!(
                "Invalid genesis anchor type: '{}'",
                other
            ))),
        }
    }
}

impl std::fmt::Display for GenesisAnchorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Persistent dispute entity stored in the `disputes` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct DisputeRecord {
    /// PayPal dispute ID (e.g. "PP-D-12345" or "DP-12345"), acts as primary key.
    pub dispute_id: String,

    /// Associated purchase session ID (if correlated).
    pub session_id: Option<String>,

    /// Genesis anchor type indicating how this chain was anchored.
    pub genesis_anchor_type: GenesisAnchorType,

    /// Genesis anchor hash (either the original purchase session root hash or 64 zeros).
    pub genesis_anchor_hash: String,

    /// Purchase root hash linked at creation or populated later during late correlation.
    pub purchase_root_hash: Option<String>,

    /// PayPal Capture ID (seller_transaction_id) from the dispute resource.
    pub seller_transaction_id: Option<String>,

    /// PayPal Order ID associated with the dispute.
    pub paypal_order_id: Option<String>,

    /// Current PayPal dispute status (e.g. "UNDER_REVIEW", "RESOLVED").
    pub status: String,

    /// PayPal dispute lifecycle stage (e.g. "INQUIRY", "CHARGEBACK").
    pub dispute_lifecycle_stage: Option<String>,

    /// Channel through which the dispute was submitted (e.g. "INTERNAL", "EXTERNAL").
    pub dispute_channel: Option<String>,

    /// Dispute reason code (e.g. "MERCHANDISE_OR_SERVICE_NOT_RECEIVED").
    pub reason: Option<String>,

    /// Disputed amount value (e.g. "199.99").
    pub dispute_amount_value: Option<String>,

    /// Disputed amount currency (e.g. "USD").
    pub dispute_amount_currency: Option<String>,

    /// Outcome of the correlation engine.
    pub correlation_status: CorrelationStatus,

    /// Correlation tier / matching mechanism if correlated.
    pub matched_by: Option<MatchedBy>,

    /// Root hash of this dispute's audit chain (latest audit step hash).
    pub root_hash: Option<String>,

    /// Authoritative creation timestamp provided by PayPal.
    pub dispute_create_time: Option<String>,

    /// Authoritative update timestamp provided by PayPal (nullable if omitted in webhook).
    pub dispute_update_time: Option<String>,

    /// Record creation timestamp in AgentAudit (ISO 8601 UTC).
    pub created_at: String,

    /// Record last update timestamp in AgentAudit (ISO 8601 UTC).
    pub updated_at: String,
}

impl DisputeRecord {
    /// Returns true if the dispute has been correlated with a purchase session.
    pub fn is_correlated(&self) -> bool {
        self.session_id.is_some() && self.correlation_status == CorrelationStatus::Matched
    }

    /// Returns true if the genesis anchor is an original purchase root hash.
    pub fn is_anchored_to_purchase(&self) -> bool {
        self.genesis_anchor_type == GenesisAnchorType::PurchaseRootHash
    }

    /// Returns true if the dispute requires human/manual review.
    pub fn is_manual_review_required(&self) -> bool {
        self.correlation_status == CorrelationStatus::ManualReviewRequired
            || self.dispute_update_time.is_none()
    }
}

/// An append-only audit step in a dispute's cryptographic hash chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct DisputeAuditStep {
    /// Unique identifier for this step (UUID v4).
    pub id: String,

    /// Foreign key referencing the parent dispute.
    pub dispute_id: String,

    /// Monotonically increasing 0-based sequence index.
    pub sequence_index: i64,

    /// Type of action performed (e.g. "DISPUTE_IMPORTED", "DISPUTE_CORRELATED").
    pub action_type: String,

    /// Canonical JSON payload serialized as a string.
    pub payload: String,

    /// SHA-256 hash of the preceding step, or the genesis anchor hash if sequence_index == 0.
    pub previous_hash: String,

    /// SHA-256 hash of this step: sha256(previous_hash || sequence_index || action_type || canonical_payload).
    pub current_hash: String,

    /// Step creation timestamp (ISO 8601 UTC).
    pub created_at: String,
}

impl DisputeAuditStep {
    /// Deserializes the JSON payload into a `serde_json::Value`.
    pub fn parse_payload(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(&self.payload)
    }
}

/// Compact summary representation of a dispute for queries and listings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisputeSummary {
    pub dispute_id: String,
    pub session_id: Option<String>,
    pub status: String,
    pub reason: Option<String>,
    pub dispute_amount_value: Option<String>,
    pub dispute_amount_currency: Option<String>,
    pub correlation_status: CorrelationStatus,
    pub matched_by: Option<MatchedBy>,
    pub dispute_update_time: Option<String>,
    pub root_hash: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&DisputeRecord> for DisputeSummary {
    fn from(r: &DisputeRecord) -> Self {
        Self {
            dispute_id: r.dispute_id.clone(),
            session_id: r.session_id.clone(),
            status: r.status.clone(),
            reason: r.reason.clone(),
            dispute_amount_value: r.dispute_amount_value.clone(),
            dispute_amount_currency: r.dispute_amount_currency.clone(),
            correlation_status: r.correlation_status,
            matched_by: r.matched_by,
            dispute_update_time: r.dispute_update_time.clone(),
            root_hash: r.root_hash.clone(),
            created_at: r.created_at.clone(),
            updated_at: r.updated_at.clone(),
        }
    }
}

/// Cryptographic verification result for a dispute audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisputeVerificationResult {
    pub dispute_id: String,
    pub valid: bool,
    pub total_steps: usize,
    pub genesis_anchor_type: GenesisAnchorType,
    pub genesis_anchor_hash: String,
    pub purchase_root_hash: Option<String>,
    pub current_root_hash: String,
    pub computed_root_hash: String,
    pub errors: Vec<String>,
}

impl DisputeVerificationResult {
    /// Creates a valid verification result.
    pub fn valid(
        dispute_id: impl Into<String>,
        total_steps: usize,
        genesis_anchor_type: GenesisAnchorType,
        genesis_anchor_hash: impl Into<String>,
        purchase_root_hash: Option<String>,
        root_hash: impl Into<String>,
    ) -> Self {
        let h = root_hash.into();
        Self {
            dispute_id: dispute_id.into(),
            valid: true,
            total_steps,
            genesis_anchor_type,
            genesis_anchor_hash: genesis_anchor_hash.into(),
            purchase_root_hash,
            current_root_hash: h.clone(),
            computed_root_hash: h,
            errors: Vec::new(),
        }
    }

    /// Creates an invalid verification result with error details.
    pub fn invalid(
        dispute_id: impl Into<String>,
        total_steps: usize,
        genesis_anchor_type: GenesisAnchorType,
        genesis_anchor_hash: impl Into<String>,
        purchase_root_hash: Option<String>,
        current_root_hash: impl Into<String>,
        computed_root_hash: impl Into<String>,
        errors: Vec<String>,
    ) -> Self {
        Self {
            dispute_id: dispute_id.into(),
            valid: false,
            total_steps,
            genesis_anchor_type,
            genesis_anchor_hash: genesis_anchor_hash.into(),
            purchase_root_hash,
            current_root_hash: current_root_hash.into(),
            computed_root_hash: computed_root_hash.into(),
            errors,
        }
    }
}

/// Standard dispute audit-trail action types.
pub struct DisputeActionType;

impl DisputeActionType {
    pub const DISPUTE_IMPORTED: &'static str = "DISPUTE_IMPORTED";
    pub const DISPUTE_UPDATED: &'static str = "DISPUTE_UPDATED";
    pub const DISPUTE_CORRELATED: &'static str = "DISPUTE_CORRELATED";
    pub const DISPUTE_LATE_LINKED: &'static str = "DISPUTE_LATE_LINKED";
    pub const DISPUTE_MANUAL_REVIEW_FLAGGED: &'static str = "DISPUTE_MANUAL_REVIEW_FLAGGED";
    pub const DISPUTE_RESOLVED: &'static str = "DISPUTE_RESOLVED";
    pub const DISPUTE_OUT_OF_ORDER_RECEIVED: &'static str = "DISPUTE_OUT_OF_ORDER_RECEIVED";
    pub const DISPUTE_CONCURRENT_CONFLICT_DETECTED: &'static str =
        "DISPUTE_CONCURRENT_CONFLICT_DETECTED";
    pub const DISPUTE_TIMESTAMP_AMBIGUOUS_RECEIVED: &'static str =
        "DISPUTE_TIMESTAMP_AMBIGUOUS_RECEIVED";
}

/// Standard actors in the dispute audit trail.
pub struct DisputeActor;

impl DisputeActor {
    pub const SYSTEM_WEBHOOK: &'static str = "SYSTEM_WEBHOOK";
    pub const WORKER_DAEMON: &'static str = "WORKER_DAEMON";
    pub const ADMIN_MANUAL: &'static str = "ADMIN_MANUAL";
    pub const SYSTEM_SCHEDULER: &'static str = "SYSTEM_SCHEDULER";
}

/// Input payload container used when creating or upserting a dispute from a webhook or manual trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertDisputePayload {
    pub dispute_id: String,
    pub session_id: Option<String>,
    pub genesis_anchor_type: GenesisAnchorType,
    pub genesis_anchor_hash: String,
    pub purchase_root_hash: Option<String>,
    pub seller_transaction_id: Option<String>,
    pub paypal_order_id: Option<String>,
    pub status: String,
    pub dispute_lifecycle_stage: Option<String>,
    pub dispute_channel: Option<String>,
    pub reason: Option<String>,
    pub dispute_amount_value: Option<String>,
    pub dispute_amount_currency: Option<String>,
    pub correlation_status: CorrelationStatus,
    pub matched_by: Option<MatchedBy>,
    pub dispute_create_time: Option<String>,
    pub dispute_update_time: Option<String>,
    pub raw_event_payload: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_anchor_type_serde_and_str() {
        assert_eq!(
            GenesisAnchorType::PurchaseRootHash.as_str(),
            "PURCHASE_ROOT_HASH"
        );
        assert_eq!(GenesisAnchorType::GenesisHash.as_str(), "GENESIS_HASH");

        assert_eq!(
            GenesisAnchorType::from_str("PURCHASE_ROOT_HASH").unwrap(),
            GenesisAnchorType::PurchaseRootHash
        );
        assert_eq!(
            GenesisAnchorType::from_str("genesis_hash").unwrap(),
            GenesisAnchorType::GenesisHash
        );
        assert!(GenesisAnchorType::from_str("INVALID_TYPE").is_err());

        let json = serde_json::to_string(&GenesisAnchorType::PurchaseRootHash).unwrap();
        assert_eq!(json, "\"PURCHASE_ROOT_HASH\"");
        let deserialized: GenesisAnchorType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, GenesisAnchorType::PurchaseRootHash);
    }

    #[test]
    fn test_dispute_record_serde_with_optional_and_null_fields() {
        let record = DisputeRecord {
            dispute_id: "PP-D-12345".to_string(),
            session_id: Some("sess-1".to_string()),
            genesis_anchor_type: GenesisAnchorType::PurchaseRootHash,
            genesis_anchor_hash: "abcd1234abcd1234".to_string(),
            purchase_root_hash: Some("abcd1234abcd1234".to_string()),
            seller_transaction_id: Some("tx-capture-1".to_string()),
            paypal_order_id: Some("ord-1".to_string()),
            status: "UNDER_REVIEW".to_string(),
            dispute_lifecycle_stage: Some("INQUIRY".to_string()),
            dispute_channel: Some("INTERNAL".to_string()),
            reason: Some("MERCHANDISE_OR_SERVICE_NOT_RECEIVED".to_string()),
            dispute_amount_value: Some("150.00".to_string()),
            dispute_amount_currency: Some("USD".to_string()),
            correlation_status: CorrelationStatus::Matched,
            matched_by: Some(MatchedBy::CaptureId),
            root_hash: Some("roothash123".to_string()),
            dispute_create_time: Some("2026-11-08T00:00:00Z".to_string()),
            dispute_update_time: None, // Nullable update time
            created_at: "2026-11-08T00:00:01Z".to_string(),
            updated_at: "2026-11-08T00:00:01Z".to_string(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: DisputeRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, deserialized);
        assert!(deserialized.is_correlated());
        assert!(deserialized.is_anchored_to_purchase());
        // Since dispute_update_time is None, manual review is flagged
        assert!(deserialized.is_manual_review_required());
    }

    #[test]
    fn test_dispute_audit_step_serde_and_payload_parsing() {
        let step = DisputeAuditStep {
            id: "step-uuid-1".to_string(),
            dispute_id: "PP-D-12345".to_string(),
            sequence_index: 0,
            action_type: DisputeActionType::DISPUTE_IMPORTED.to_string(),
            payload: r#"{"status":"UNDER_REVIEW","reason":"TEST"}"#.to_string(),
            previous_hash: GENESIS_HASH_DEFAULT.to_string(),
            current_hash: "step-hash-1".to_string(),
            created_at: "2026-11-08T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&step).unwrap();
        let deserialized: DisputeAuditStep = serde_json::from_str(&json).unwrap();
        assert_eq!(step, deserialized);

        let parsed_payload = deserialized.parse_payload().unwrap();
        assert_eq!(parsed_payload["status"], "UNDER_REVIEW");
        assert_eq!(parsed_payload["reason"], "TEST");
    }

    #[test]
    fn test_dispute_summary_conversion() {
        let record = DisputeRecord {
            dispute_id: "PP-D-999".to_string(),
            session_id: None,
            genesis_anchor_type: GenesisAnchorType::GenesisHash,
            genesis_anchor_hash: GENESIS_HASH_DEFAULT.to_string(),
            purchase_root_hash: None,
            seller_transaction_id: None,
            paypal_order_id: None,
            status: "OPEN".to_string(),
            dispute_lifecycle_stage: None,
            dispute_channel: None,
            reason: None,
            dispute_amount_value: None,
            dispute_amount_currency: None,
            correlation_status: CorrelationStatus::NotFound,
            matched_by: None,
            root_hash: None,
            dispute_create_time: None,
            dispute_update_time: None,
            created_at: "2026-11-08T00:00:00Z".to_string(),
            updated_at: "2026-11-08T00:00:00Z".to_string(),
        };

        let summary = DisputeSummary::from(&record);
        assert_eq!(summary.dispute_id, "PP-D-999");
        assert_eq!(summary.status, "OPEN");
        assert_eq!(summary.correlation_status, CorrelationStatus::NotFound);
        assert_eq!(summary.session_id, None);
        assert_eq!(summary.dispute_update_time, None);
    }

    #[test]
    fn test_dispute_verification_result_constructors() {
        let valid = DisputeVerificationResult::valid(
            "PP-D-123",
            3,
            GenesisAnchorType::PurchaseRootHash,
            "root-hash-0",
            Some("root-hash-0".to_string()),
            "root-hash-3",
        );
        assert!(valid.valid);
        assert_eq!(valid.total_steps, 3);
        assert_eq!(valid.computed_root_hash, "root-hash-3");
        assert!(valid.errors.is_empty());

        let invalid = DisputeVerificationResult::invalid(
            "PP-D-123",
            3,
            GenesisAnchorType::PurchaseRootHash,
            "root-hash-0",
            Some("root-hash-0".to_string()),
            "expected-hash",
            "computed-hash",
            vec!["Step 2 hash mismatch".to_string()],
        );
        assert!(!invalid.valid);
        assert_eq!(invalid.errors.len(), 1);
        assert_eq!(invalid.errors[0], "Step 2 hash mismatch");
    }

    #[test]
    fn test_upsert_dispute_payload_serde() {
        let payload = UpsertDisputePayload {
            dispute_id: "PP-D-555".to_string(),
            session_id: Some("sess-abc".to_string()),
            genesis_anchor_type: GenesisAnchorType::PurchaseRootHash,
            genesis_anchor_hash: "hash0".to_string(),
            purchase_root_hash: Some("hash0".to_string()),
            seller_transaction_id: Some("tx-1".to_string()),
            paypal_order_id: Some("ord-1".to_string()),
            status: "WAITING_FOR_SELLER_RESPONSE".to_string(),
            dispute_lifecycle_stage: Some("INQUIRY".to_string()),
            dispute_channel: Some("INTERNAL".to_string()),
            reason: Some("UNAUTHORISED".to_string()),
            dispute_amount_value: Some("250.00".to_string()),
            dispute_amount_currency: Some("USD".to_string()),
            correlation_status: CorrelationStatus::Matched,
            matched_by: Some(MatchedBy::CustomId),
            dispute_create_time: Some("2026-11-08T00:00:00Z".to_string()),
            dispute_update_time: Some("2026-11-08T01:00:00Z".to_string()),
            raw_event_payload: Some(serde_json::json!({"id": "PP-D-555"})),
        };

        let json = serde_json::to_string(&payload).unwrap();
        let deserialized: UpsertDisputePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, deserialized);
    }
}
