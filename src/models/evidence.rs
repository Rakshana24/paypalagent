use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::models::correlation::{CorrelationStatus, MatchedBy};
use crate::models::dispute::GenesisAnchorType;

/// Default evaluator identifier declaring local AgentAudit correlation engine origin.
pub const DEFAULT_CORRELATION_EVALUATOR: &str = "AGENTAUDIT_LOCAL_CORRELATION_ENGINE";

/// Epistemic categories classifying the epistemic warrant and origin of evidence claims.
///
/// Every statement in an evidence brief must be tagged with its epistemic category.
/// Inferences, recorded external data, and unavailable facts must never be conflated
/// with internally verified cryptographic facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EpistemicCategory {
    /// Directly verified against AgentAudit's internal cryptographic hash chain and decision steps.
    /// Note: Hash chain verification confirms internal sequence consistency and tamper-detection,
    /// but does not prevent an attacker with full database access from recalculating the chain
    /// unless verified against an external anchor.
    VerifiedFact,

    /// Authoritative data received via verified PayPal webhooks or PayPal REST APIs
    /// (e.g. dispute reasons, dispute amounts, PayPal capture IDs, PayPal timestamps).
    RecordedPaypalData,

    /// Logical conclusions derived deterministically from verified facts and recorded PayPal data
    /// (e.g. comparing authorized spend limit with actual capture amount).
    /// Must never be treated as independently verified ground truth.
    DeterministicInference,

    /// Relevant evidentiary categories that AgentAudit does not possess or track
    /// (e.g. physical shipping tracking numbers, carrier proof of delivery, cardholder physical identity).
    UnavailableEvidence,
}

impl EpistemicCategory {
    /// Returns the uppercase string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::VerifiedFact => "VERIFIED_FACT",
            Self::RecordedPaypalData => "RECORDED_PAYPAL_DATA",
            Self::DeterministicInference => "DETERMINISTIC_INFERENCE",
            Self::UnavailableEvidence => "UNAVAILABLE_EVIDENCE",
        }
    }
}

impl std::fmt::Display for EpistemicCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Provenance of recorded PayPal settlement data.
///
/// Distinguishes live, authoritative records retrieved directly from PayPal REST APIs from
/// records merely replayed or extracted from the JSON payloads of internal ledger steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SettlementProvenance {
    /// Retrieved directly from an authoritative live PayPal REST API call (e.g. GET /v2/payments/captures/{id}).
    LivePaypalApi,

    /// Extracted / replayed from the recorded JSON payload of an internal ledger step without an independent PayPal API query.
    ReplayedFromLedgerPayload,
}

impl SettlementProvenance {
    /// Returns the uppercase string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LivePaypalApi => "LIVE_PAYPAL_API",
            Self::ReplayedFromLedgerPayload => "REPLAYED_FROM_LEDGER_PAYLOAD",
        }
    }
}

impl std::fmt::Display for SettlementProvenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Closed enum representing specific categories of evidence that AgentAudit does NOT possess or record.
///
/// Enforces strong typing so that missing evidence cannot be arbitrarily or inconsistently labeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UnavailableEvidenceType {
    /// Physical shipping carrier tracking numbers, postal scans, or delivery signature images.
    PhysicalShippingTrackingAndProofOfDelivery,

    /// Biometric, government ID, or physical confirmation of the human operating the account.
    BuyerPhysicalIdentityProof,

    /// Out-of-band email, phone, or merchant support ticketing communications.
    BuyerMerchantCommunications,

    /// Warehouse physical return tracking, RMA receipt scans, or package inspection reports.
    ReturnMerchandiseReceipt,

    /// Confidential cardholder communications with issuing banks or card schemes.
    CardholderBankCommunications,
}

impl UnavailableEvidenceType {
    /// Returns the uppercase string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PhysicalShippingTrackingAndProofOfDelivery => {
                "PHYSICAL_SHIPPING_TRACKING_AND_PROOF_OF_DELIVERY"
            }
            Self::BuyerPhysicalIdentityProof => "BUYER_PHYSICAL_IDENTITY_PROOF",
            Self::BuyerMerchantCommunications => "BUYER_MERCHANT_COMMUNICATIONS",
            Self::ReturnMerchandiseReceipt => "RETURN_MERCHANDISE_RECEIPT",
            Self::CardholderBankCommunications => "CARDHOLDER_BANK_COMMUNICATIONS",
        }
    }
}

impl std::fmt::Display for UnavailableEvidenceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Precise source reference anchoring an evidence claim to an immutable ledger step, session record, or PayPal record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSourceReference {
    /// AgentAudit purchase session ID if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// PayPal dispute ID for dispute audit trail steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_id: Option<String>,

    /// Monotonically increasing sequence index within the session ledger or dispute audit trail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_index: Option<i64>,

    /// Action type recorded in the ledger (e.g. "PURCHASE_AUTHORIZED", "PAYPAL_PAYMENT_CAPTURED").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,

    /// SHA-256 hash of the corresponding ledger step or sealed root hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_hash: Option<String>,

    /// PayPal identifier (e.g. Order ID, Capture ID, or Dispute ID).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paypal_record_id: Option<String>,

    /// PayPal Webhook Transmission ID from the HTTP webhook delivery headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_transmission_id: Option<String>,

    /// Authoritative timestamp of the event or step (ISO 8601 UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl EvidenceSourceReference {
    /// Validates that the reference contains valid values and at least one meaningful anchor.
    pub fn validate(&self) -> Result<(), AppError> {
        let has_step_anchor = self
            .step_hash
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let has_paypal_anchor = self
            .paypal_record_id
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let has_webhook_anchor = self
            .webhook_transmission_id
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        if !has_step_anchor && !has_paypal_anchor && !has_webhook_anchor {
            return Err(AppError::InvalidPayload(
                "EvidenceSourceReference must contain at least one meaningful anchor (step_hash, paypal_record_id, or webhook_transmission_id)"
                    .to_string(),
            ));
        }

        if let Some(seq) = self.sequence_index {
            if seq < 0 {
                return Err(AppError::InvalidPayload(format!(
                    "EvidenceSourceReference sequence_index cannot be negative: {}",
                    seq
                )));
            }
        }

        if let Some(ref act) = self.action_type {
            if act.trim().is_empty() {
                return Err(AppError::InvalidPayload(
                    "EvidenceSourceReference action_type cannot be empty or whitespace".to_string(),
                ));
            }
        }

        if let Some(ref s_id) = self.session_id {
            if s_id.trim().is_empty() {
                return Err(AppError::InvalidPayload(
                    "EvidenceSourceReference session_id cannot be empty or whitespace".to_string(),
                ));
            }
        }

        if let Some(ref d_id) = self.dispute_id {
            if d_id.trim().is_empty() {
                return Err(AppError::InvalidPayload(
                    "EvidenceSourceReference dispute_id cannot be empty or whitespace".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Creates a validated source reference anchored to an AgentAudit purchase session ledger step.
    pub fn ledger_step(
        session_id: impl Into<String>,
        sequence_index: i64,
        action_type: impl Into<String>,
        step_hash: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Result<Self, AppError> {
        let sid = session_id.into();
        let act = action_type.into();
        let shash = step_hash.into();
        let ts = timestamp.into();

        if sid.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "session_id cannot be empty".to_string(),
            ));
        }
        if sequence_index < 0 {
            return Err(AppError::InvalidPayload(format!(
                "sequence_index cannot be negative: {}",
                sequence_index
            )));
        }
        if act.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "action_type cannot be empty".to_string(),
            ));
        }
        if shash.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "step_hash cannot be empty".to_string(),
            ));
        }
        if ts.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "timestamp cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            session_id: Some(sid),
            dispute_id: None,
            sequence_index: Some(sequence_index),
            action_type: Some(act),
            step_hash: Some(shash),
            paypal_record_id: None,
            webhook_transmission_id: None,
            timestamp: Some(ts),
        })
    }

    /// Creates a validated source reference anchored to a step in a dispute's cryptographic audit trail.
    pub fn dispute_step(
        dispute_id: impl Into<String>,
        sequence_index: i64,
        action_type: impl Into<String>,
        step_hash: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Result<Self, AppError> {
        let did = dispute_id.into();
        let act = action_type.into();
        let shash = step_hash.into();
        let ts = timestamp.into();

        if did.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "dispute_id cannot be empty".to_string(),
            ));
        }
        if sequence_index < 0 {
            return Err(AppError::InvalidPayload(format!(
                "sequence_index cannot be negative: {}",
                sequence_index
            )));
        }
        if act.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "action_type cannot be empty".to_string(),
            ));
        }
        if shash.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "step_hash cannot be empty".to_string(),
            ));
        }
        if ts.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "timestamp cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            session_id: None,
            dispute_id: Some(did),
            sequence_index: Some(sequence_index),
            action_type: Some(act),
            step_hash: Some(shash),
            paypal_record_id: None,
            webhook_transmission_id: None,
            timestamp: Some(ts),
        })
    }

    /// Creates a validated source reference anchored to the purchase session finalization record in `sessions`.
    ///
    /// Accurately reflects that session finalization in AgentAudit locks `root_hash` and `updated_at` on the
    /// session record itself, rather than appending a distinct decision step.
    pub fn session_finalization(
        session_id: impl Into<String>,
        root_hash: impl Into<String>,
        finalized_at: impl Into<String>,
    ) -> Result<Self, AppError> {
        let sid = session_id.into();
        let r_hash = root_hash.into();
        let ts = finalized_at.into();

        if sid.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "session_id cannot be empty".to_string(),
            ));
        }
        if r_hash.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "root_hash cannot be empty".to_string(),
            ));
        }
        if ts.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "finalized_at timestamp cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            session_id: Some(sid),
            dispute_id: None,
            sequence_index: None,
            action_type: None,
            step_hash: Some(r_hash),
            paypal_record_id: None,
            webhook_transmission_id: None,
            timestamp: Some(ts),
        })
    }

    /// Creates a validated source reference anchored to an external PayPal record or API response.
    pub fn paypal_record(
        paypal_record_id: impl Into<String>,
        timestamp: Option<String>,
    ) -> Result<Self, AppError> {
        let pid = paypal_record_id.into();
        if pid.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "paypal_record_id cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            session_id: None,
            dispute_id: None,
            sequence_index: None,
            action_type: None,
            step_hash: None,
            paypal_record_id: Some(pid),
            webhook_transmission_id: None,
            timestamp,
        })
    }

    /// Creates a validated source reference anchored to a PayPal Webhook event notification.
    pub fn webhook_event(
        transmission_id: impl Into<String>,
        event_id: impl Into<String>,
        timestamp: Option<String>,
    ) -> Result<Self, AppError> {
        let tid = transmission_id.into();
        let eid = event_id.into();

        if tid.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "transmission_id cannot be empty".to_string(),
            ));
        }
        if eid.trim().is_empty() {
            return Err(AppError::InvalidPayload(
                "event_id cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            session_id: None,
            dispute_id: None,
            sequence_index: None,
            action_type: None,
            step_hash: None,
            paypal_record_id: Some(eid),
            webhook_transmission_id: Some(tid),
            timestamp,
        })
    }
}

/// Represents an agent-generated statement or evaluation recorded in a ledger step.
///
/// Epistemic note: The recording of this statement in the cryptographic ledger is a `VERIFIED_FACT`,
/// but the semantic content is an agent-generated rationale or heuristic evaluation, NOT an
/// independently verified objective fact about the external world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatementFact {
    /// The text of the statement or rationale produced by the agent.
    pub statement: String,

    /// Explicit disclaimer that the content is agent-generated.
    pub disclaimer: String,
}

impl AgentStatementFact {
    /// Standard disclaimer clarifying that rationale is agent evaluation rather than physical proof.
    pub const DEFAULT_RATIONALE_DISCLAIMER: &'static str =
        "This statement is an agent-generated evaluation recorded in the immutable ledger. \
        Its presence in the audit trail is verified, but its content reflects autonomous agent \
        heuristics rather than an independently verified physical fact.";

    /// Creates a new agent statement fact with the standard disclaimer.
    pub fn new(statement: impl Into<String>) -> Self {
        Self {
            statement: statement.into(),
            disclaimer: Self::DEFAULT_RATIONALE_DISCLAIMER.to_string(),
        }
    }
}

/// Authoritative summary facts regarding the dispute entity itself, received directly from PayPal.
///
/// Contains strictly PayPal-provided data. Locally computed correlation outputs must never be
/// included in this struct to prevent mischaracterizing local results as external facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisputeSummaryFact {
    /// Categorized strictly as recorded PayPal data.
    pub category: EpistemicCategory,

    /// PayPal Dispute ID (e.g. "PP-D-12345").
    pub dispute_id: String,

    /// PayPal dispute status (e.g. "UNDER_REVIEW", "RESOLVED").
    pub status: String,

    /// PayPal dispute lifecycle stage (e.g. "INQUIRY", "CHARGEBACK").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_lifecycle_stage: Option<String>,

    /// Channel through which the dispute was submitted (e.g. "INTERNAL", "EXTERNAL").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_channel: Option<String>,

    /// Authoritative PayPal dispute reason (e.g. "MERCHANDISE_OR_SERVICE_NOT_RECEIVED", "UNAUTHORISED_TRANSACTION").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Disputed amount value (e.g. "199.99").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_amount_value: Option<String>,

    /// Disputed currency code (e.g. "USD").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_amount_currency: Option<String>,

    /// Associated PayPal Capture ID (seller_transaction_id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seller_transaction_id: Option<String>,

    /// Associated PayPal Order ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paypal_order_id: Option<String>,

    /// Authoritative creation timestamp provided by PayPal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_create_time: Option<String>,

    /// Authoritative update timestamp provided by PayPal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_update_time: Option<String>,

    /// Source reference linking to the webhook transmission or PayPal API call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<EvidenceSourceReference>,
}

impl DisputeSummaryFact {
    /// Validates epistemic category safety.
    pub fn validate_category(&self) -> Result<(), AppError> {
        if self.category != EpistemicCategory::RecordedPaypalData {
            return Err(AppError::InvalidPayload(format!(
                "DisputeSummaryFact must have category RECORDED_PAYPAL_DATA, found {:?}",
                self.category
            )));
        }
        Ok(())
    }
}

/// Deterministic correlation assessment produced locally by AgentAudit's correlation engine.
///
/// Categorized strictly as deterministic inference to distinguish local engine decisions from
/// external PayPal data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrelationSummary {
    /// Categorized strictly as deterministic inference.
    pub category: EpistemicCategory,

    /// Identifier of the evaluator / engine that computed the correlation.
    pub evaluator: String,

    /// Correlation status outcome (single authoritative location).
    pub status: CorrelationStatus,

    /// Mechanism / tier by which the correlation was established (if matched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_by: Option<MatchedBy>,

    /// Correlated AgentAudit purchase session ID (if matched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Detailed explanation of the deterministic match or review justification.
    pub explanation: String,

    /// Source reference linking to the correlation decision or audit step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<EvidenceSourceReference>,
}

impl CorrelationSummary {
    /// Creates a new correlation summary with enforced DETERMINISTIC_INFERENCE category and default evaluator.
    pub fn new(
        status: CorrelationStatus,
        matched_by: Option<MatchedBy>,
        session_id: Option<String>,
        explanation: impl Into<String>,
        source: Option<EvidenceSourceReference>,
    ) -> Self {
        Self {
            category: EpistemicCategory::DeterministicInference,
            evaluator: DEFAULT_CORRELATION_EVALUATOR.to_string(),
            status,
            matched_by,
            session_id,
            explanation: explanation.into(),
            source,
        }
    }

    /// Validates epistemic category safety.
    pub fn validate_category(&self) -> Result<(), AppError> {
        if self.category != EpistemicCategory::DeterministicInference {
            return Err(AppError::InvalidPayload(format!(
                "CorrelationSummary must have category DETERMINISTIC_INFERENCE, found {:?}",
                self.category
            )));
        }
        Ok(())
    }
}

/// Cryptographic integrity assessment and hash-chain audit summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CryptographicProofSummary {
    /// Categorized strictly as verified fact.
    pub category: EpistemicCategory,

    /// Linked purchase session ID (if correlated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purchase_session_id: Option<String>,

    /// Purchase session root hash (if finalized).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purchase_root_hash: Option<String>,

    /// Source reference pointing to the purchase session finalization record in `sessions`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purchase_root_hash_source: Option<EvidenceSourceReference>,

    /// Whether the purchase session hash chain is strictly continuous and valid.
    pub purchase_chain_valid: bool,

    /// Total decision steps in the purchase session.
    pub purchase_total_steps: usize,

    /// Dispute audit chain root hash (if initialized).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_root_hash: Option<String>,

    /// Source reference pointing to the latest dispute audit step sealing the chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_root_hash_source: Option<EvidenceSourceReference>,

    /// Whether the dispute audit chain is strictly continuous and valid.
    pub dispute_chain_valid: bool,

    /// Total steps in the dispute audit trail.
    pub dispute_total_steps: usize,

    /// How the dispute audit trail was initialized.
    pub genesis_anchor_type: GenesisAnchorType,

    /// Genesis anchor hash (either purchase root hash or 64-zero SHA-256 hash).
    pub genesis_anchor_hash: String,

    /// Indicates whether dual-chain cryptographic verification succeeded.
    ///
    /// Specifically verifies the conjunction of three conditions:
    /// 1. The correlated purchase session ledger hash chain is complete, continuous, and matches `purchase_root_hash`.
    /// 2. The dispute audit trail hash chain is complete, continuous, and matches `dispute_root_hash`.
    /// 3. The dispute's genesis anchor matches the purchase root hash (`genesis_anchor_hash == purchase_root_hash`).
    pub dual_chain_integrity_verified: bool,

    /// Integrity scope statement explicitly explaining the cryptographic boundary.
    pub integrity_scope_statement: String,
}

impl CryptographicProofSummary {
    /// Standard integrity scope statement explicitly clarifying hash-chain guarantees.
    pub const DEFAULT_SCOPE_STATEMENT: &'static str =
        "The cryptographic hash chain confirms internal sequence consistency, append-only order, \
        and tamper-detection across recorded decision steps. It does not prevent an attacker with \
        full database access from recomputing the chain unless externally anchored, nor does it \
        independently verify physical real-world events outside the system boundary.";

    /// Validates epistemic category safety.
    pub fn validate_category(&self) -> Result<(), AppError> {
        if self.category != EpistemicCategory::VerifiedFact {
            return Err(AppError::InvalidPayload(format!(
                "CryptographicProofSummary must have category VERIFIED_FACT, found {:?}",
                self.category
            )));
        }
        Ok(())
    }
}

/// Verified fact of the user's initial prompt submitted to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAuditFact {
    pub prompt: String,
    pub source: EvidenceSourceReference,
}

/// Verified fact of extracted purchasing constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequirementsAuditFact {
    pub category: String,
    pub maximum_budget: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_battery_hours: Option<u32>,
    pub required_features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intended_use: Option<String>,
    pub source: EvidenceSourceReference,
}

/// Verified fact of catalog candidates evaluated during catalog search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSearchAuditFact {
    pub candidates_evaluated_count: usize,
    pub source: EvidenceSourceReference,
}

/// Verified fact of a product rejected during autonomous agent evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RejectedProductAuditFact {
    pub product_id: String,
    pub product_name: String,
    pub price: f64,
    pub currency: String,
    pub rejection_rationale: AgentStatementFact,
    pub source: EvidenceSourceReference,
}

/// Verified fact of the catalog product autonomously selected by the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectedProductAuditFact {
    pub product_id: String,
    pub product_name: String,
    pub brand: String,
    pub category: String,
    pub price: f64,
    pub currency: String,
    pub selection_rationale: AgentStatementFact,
    pub source: EvidenceSourceReference,
}

/// Verified fact of the customer purchase authorization.
///
/// Important: This proves that an authorization payload was submitted and logged in the session
/// before PayPal checkout was initiated. It does NOT independently prove the physical identity
/// of the human who approved it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationAuditFact {
    pub selected_product_id: String,
    pub maximum_authorized_amount: f64,
    pub user_confirmed: bool,
    pub authorized_at: String,
    pub identity_disclaimer: String,
    pub source: EvidenceSourceReference,
}

impl AuthorizationAuditFact {
    /// Standard disclaimer clarifying identity verification limits.
    pub const DEFAULT_IDENTITY_DISCLAIMER: &'static str =
        "This record establishes that customer purchase authorization was recorded in the session \
        prior to PayPal checkout initiation. It verifies session authorization compliance but does not \
        independently establish the physical identity of the human operator.";
}

/// Complete summary of the autonomous shopping agent's decision steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentAuditTrailSummary {
    /// Categorized strictly as verified fact.
    pub category: EpistemicCategory,

    /// Initial user prompt fact.
    pub initial_prompt: PromptAuditFact,

    /// Structured requirements extracted from the prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requirements: Option<RequirementsAuditFact>,

    /// Catalog search evaluation step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_search: Option<CatalogSearchAuditFact>,

    /// Products rejected during evaluation with recorded agent rationale.
    pub rejected_candidates: Vec<RejectedProductAuditFact>,

    /// The product selected by the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_product: Option<SelectedProductAuditFact>,

    /// Customer authorization step recorded prior to checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purchase_authorization: Option<AuthorizationAuditFact>,
}

impl AgentAuditTrailSummary {
    /// Validates epistemic category safety.
    pub fn validate_category(&self) -> Result<(), AppError> {
        if self.category != EpistemicCategory::VerifiedFact {
            return Err(AppError::InvalidPayload(format!(
                "AgentAuditTrailSummary must have category VERIFIED_FACT, found {:?}",
                self.category
            )));
        }
        Ok(())
    }
}

/// Authoritative settlement attributes received from PayPal APIs or webhooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedPaypalSettlementFact {
    /// Categorized strictly as recorded PayPal data.
    pub category: EpistemicCategory,

    /// Explicit provenance distinguishing live PayPal API data from values copied from internal ledger payloads.
    pub provenance: SettlementProvenance,

    /// Required source reference establishing the exact API call, webhook event, or ledger payload origin.
    pub source: EvidenceSourceReference,

    /// PayPal Order ID (e.g. "5O190127TN364715T").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paypal_order_id: Option<String>,

    /// Order status at creation (e.g. "CREATED", "APPROVED").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_status: Option<String>,

    /// Order currency (e.g. "USD").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_currency: Option<String>,

    /// Order amount value (e.g. "199.99").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_amount: Option<String>,

    /// PayPal Capture ID (seller_transaction_id, e.g. "9AB12345CD678901E").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paypal_capture_id: Option<String>,

    /// Payment capture status (e.g. "COMPLETED").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_status: Option<String>,

    /// Capture completion timestamp from PayPal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_timestamp: Option<String>,
}

impl RecordedPaypalSettlementFact {
    /// Validates epistemic category safety.
    pub fn validate_category(&self) -> Result<(), AppError> {
        if self.category != EpistemicCategory::RecordedPaypalData {
            return Err(AppError::InvalidPayload(format!(
                "RecordedPaypalSettlementFact must have category RECORDED_PAYPAL_DATA, found {:?}",
                self.category
            )));
        }
        Ok(())
    }
}

/// Verified settlement records stored within AgentAudit's cryptographic ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedLedgerSettlementFact {
    /// Categorized strictly as verified fact.
    pub category: EpistemicCategory,

    /// Source reference for the `PAYPAL_ORDER_CREATED` ledger step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_created_step: Option<EvidenceSourceReference>,

    /// Source reference for the `PAYPAL_PAYMENT_CAPTURED` ledger step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_captured_step: Option<EvidenceSourceReference>,
}

impl VerifiedLedgerSettlementFact {
    /// Validates epistemic category safety.
    pub fn validate_category(&self) -> Result<(), AppError> {
        if self.category != EpistemicCategory::VerifiedFact {
            return Err(AppError::InvalidPayload(format!(
                "VerifiedLedgerSettlementFact must have category VERIFIED_FACT, found {:?}",
                self.category
            )));
        }
        Ok(())
    }
}

/// Transaction proof cleanly separating external PayPal settlement data from internal ledger records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionProof {
    /// Authoritative settlement data received from PayPal.
    pub paypal_settlement: RecordedPaypalSettlementFact,

    /// Internally verified ledger steps confirming order creation and capture events.
    pub ledger_settlement: VerifiedLedgerSettlementFact,
}

/// A single argument or claim in the dispute rebuttal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebuttalPoint {
    /// Epistemic category of this specific point (e.g. DeterministicInference, VerifiedFact).
    pub category: EpistemicCategory,

    /// Headline claim statement.
    pub claim: String,

    /// Detailed factual explanation supporting the claim.
    pub explanation: String,

    /// Specific source references backing this rebuttal point.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub source_references: Vec<EvidenceSourceReference>,
}

/// Factual rebuttal synthesis tailored to the dispute reason code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebuttalSummary {
    /// Categorized strictly as deterministic inference.
    pub category: EpistemicCategory,

    /// Authoritative PayPal dispute reason code being addressed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispute_reason: Option<String>,

    /// Deterministic recommendation based on evidentiary strength.
    pub recommended_action: String,

    /// Key factual arguments rebutting the dispute claim.
    pub primary_arguments: Vec<RebuttalPoint>,

    /// Reason-specific synthesis detailing how the verified audit trail addresses the claim.
    pub reason_specific_analysis: String,
}

impl RebuttalSummary {
    /// Validates epistemic category safety.
    pub fn validate_category(&self) -> Result<(), AppError> {
        if self.category != EpistemicCategory::DeterministicInference {
            return Err(AppError::InvalidPayload(format!(
                "RebuttalSummary must have category DETERMINISTIC_INFERENCE, found {:?}",
                self.category
            )));
        }
        Ok(())
    }
}

/// A specific item of evidence that is unavailable in the AgentAudit system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingEvidenceItem {
    /// Strongly typed category of unavailable evidence.
    pub evidence_type: UnavailableEvidenceType,

    /// Explanation of why this evidence is not available in AgentAudit.
    pub reason_unavailable: String,

    /// Impact of this missing evidence on dispute adjudication.
    pub adjudication_impact: String,
}

/// Explicit accounting of evidence categories that AgentAudit does NOT track or possess.
///
/// Under no circumstances should missing physical or identity evidence be fabricated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingEvidenceDisclaimer {
    /// Categorized strictly as unavailable evidence.
    pub category: EpistemicCategory,

    /// List of explicit evidence items that cannot be provided.
    pub missing_items: Vec<MissingEvidenceItem>,

    /// Overarching disclaimer text.
    pub general_disclaimer: String,
}

impl MissingEvidenceDisclaimer {
    /// Generates standard disclaimers for common commerce gaps (e.g. physical shipping, biometric identity).
    pub fn standard_disclaimers(has_physical_goods: bool) -> Self {
        let mut items = vec![
            MissingEvidenceItem {
                evidence_type: UnavailableEvidenceType::BuyerPhysicalIdentityProof,
                reason_unavailable: "AgentAudit records session-level prompts and authorizations, \
                    but does not perform biometric, government ID, or physical identity verification."
                    .to_string(),
                adjudication_impact: "Proves internal authorization compliance, but does not \
                    independently disprove account takeover or physical device compromise."
                    .to_string(),
            },
            MissingEvidenceItem {
                evidence_type: UnavailableEvidenceType::BuyerMerchantCommunications,
                reason_unavailable: "Out-of-band email, phone, or live chat customer service communications \
                    are not ingested into AgentAudit's ledger.".to_string(),
                adjudication_impact: "External communications between buyer and merchant cannot be \
                    verified from this brief alone; this does not assert whether such records exist in merchant systems."
                    .to_string(),
            },
            MissingEvidenceItem {
                evidence_type: UnavailableEvidenceType::CardholderBankCommunications,
                reason_unavailable: "AgentAudit does not interface with or store private communications \
                    between the cardholder and their card-issuing bank or payment card scheme."
                    .to_string(),
                adjudication_impact: "Direct communications between cardholder and issuing bank are unavailable \
                    within this brief; this does not assert whether such records exist externally with financial institutions."
                    .to_string(),
            },
        ];

        if has_physical_goods {
            items.push(MissingEvidenceItem {
                evidence_type: UnavailableEvidenceType::PhysicalShippingTrackingAndProofOfDelivery,
                reason_unavailable:
                    "AgentAudit governs agent shopping and PayPal checkout capture, \
                    but does not integrate with warehouse shipping carriers (FedEx, UPS, USPS) \
                    or store carrier delivery signature scans."
                        .to_string(),
                adjudication_impact:
                    "For INR (Item Not Received) claims, physical carrier tracking \
                    and delivery confirmation must be obtained from the shipping provider."
                        .to_string(),
            });
            items.push(MissingEvidenceItem {
                evidence_type: UnavailableEvidenceType::ReturnMerchandiseReceipt,
                reason_unavailable:
                    "Physical return tracking, warehouse RMA receipt, and package condition \
                    inspection logs are not tracked by AgentAudit."
                        .to_string(),
                adjudication_impact:
                    "If buyer claims merchandise was returned, physical warehouse \
                    verification is required outside this system."
                        .to_string(),
            });
        }

        Self {
            category: EpistemicCategory::UnavailableEvidence,
            missing_items: items,
            general_disclaimer: "AgentAudit provides verifiable cryptographic records of autonomous agent \
                decisions and PayPal order/capture transactions. It does not replace physical logistics \
                documentation, shipping carrier delivery confirmation, or external identity verification."
                .to_string(),
        }
    }
}

/// Comprehensive, factual dispute evidence brief.
///
/// Designed for automated dispute adjudication review and merchant defense packages.
/// Combines verified ledger steps, recorded PayPal transaction data, deterministic
/// inferences, and explicit missing evidence disclaimers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisputeEvidenceBrief {
    /// Unique brief identifier (UUID v4).
    pub brief_id: String,

    /// Schema version for evidence brief consumer compatibility.
    pub schema_version: String,

    /// PayPal Dispute ID.
    pub dispute_id: String,

    /// ISO 8601 UTC timestamp of when this evidence brief was synthesized.
    pub generated_at: String,

    /// Single authoritative location for deterministic correlation outcome and details.
    pub correlation_summary: CorrelationSummary,

    /// Authoritative dispute metadata from PayPal.
    pub dispute_summary: DisputeSummaryFact,

    /// Cryptographic proof and dual-chain verification assessment.
    pub cryptographic_proof: CryptographicProofSummary,

    /// Autonomous agent decision ledger audit trail (None if uncorrelated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_audit_trail: Option<AgentAuditTrailSummary>,

    /// Verified PayPal checkout and capture transaction proof.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_proof: Option<TransactionProof>,

    /// Reason-tailored factual rebuttal arguments.
    pub rebuttal_summary: RebuttalSummary,

    /// Explicit accounting of missing or unavailable evidence categories.
    pub missing_evidence: MissingEvidenceDisclaimer,
}

fn format_source_ref(src: &EvidenceSourceReference) -> String {
    let mut parts = Vec::new();
    if let Some(ref session_id) = src.session_id {
        parts.push(format!("session: `{}`", session_id));
    }
    if let Some(ref dispute_id) = src.dispute_id {
        parts.push(format!("dispute: `{}`", dispute_id));
    }
    if let Some(seq) = src.sequence_index {
        parts.push(format!("seq: {}", seq));
    }
    if let Some(ref action) = src.action_type {
        parts.push(format!("action: `{}`", action));
    }
    if let Some(ref hash) = src.step_hash {
        let short_hash = if hash.len() > 16 {
            &hash[..16]
        } else {
            hash.as_str()
        };
        parts.push(format!("hash: `{}...`", short_hash));
    }
    if let Some(ref trans_id) = src.webhook_transmission_id {
        parts.push(format!("transmission_id: `{}`", trans_id));
    }
    if let Some(ref rec_id) = src.paypal_record_id {
        parts.push(format!("paypal_record_id: `{}`", rec_id));
    }
    if let Some(ref ts) = src.timestamp {
        parts.push(format!("timestamp: `{}`", ts));
    }
    if parts.is_empty() {
        "Unspecified".to_string()
    } else {
        parts.join(", ")
    }
}

impl DisputeEvidenceBrief {
    /// Formats the complete evidence brief into an audit-grade, human-readable Markdown report.
    ///
    /// Distinguishes epistemic categories (`VERIFIED_FACT`, `RECORDED_PAYPAL_DATA`,
    /// `DETERMINISTIC_INFERENCE`, `UNAVAILABLE_EVIDENCE`) and includes all required disclaimers.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();

        // 1. Title & Metadata
        md.push_str(&format!(
            "# Dispute Evidence Brief: {}\n\n",
            self.dispute_id
        ));
        md.push_str(&format!("- **Brief ID**: `{}`\n", self.brief_id));
        md.push_str(&format!(
            "- **Schema Version**: `{}`\n",
            self.schema_version
        ));
        md.push_str(&format!("- **Generated At**: {}\n\n", self.generated_at));

        // 2. Executive Summary / Rebuttal Recommendation
        md.push_str("## Executive Summary & Recommended Action\n\n");
        md.push_str(&format!(
            "> **Epistemic Classification**: `{}`\n\n",
            self.rebuttal_summary.category
        ));
        md.push_str(&format!(
            "- **Dispute Reason**: {}\n",
            self.rebuttal_summary
                .dispute_reason
                .as_deref()
                .unwrap_or("UNKNOWN")
        ));
        md.push_str(&format!(
            "- **Recommended Action**: **{}**\n",
            self.rebuttal_summary.recommended_action
        ));
        md.push_str(&format!(
            "- **Analysis**: {}\n\n",
            self.rebuttal_summary.reason_specific_analysis
        ));

        // 3. Dispute Summary (Recorded PayPal Data)
        md.push_str("## 1. PayPal Dispute Summary\n\n");
        md.push_str(&format!(
            "> **Epistemic Classification**: `{}`\n\n",
            self.dispute_summary.category
        ));
        md.push_str("| Attribute | Value |\n");
        md.push_str("|---|---|\n");
        md.push_str(&format!(
            "| Dispute ID | `{}` |\n",
            self.dispute_summary.dispute_id
        ));
        md.push_str(&format!("| Status | `{}` |\n", self.dispute_summary.status));
        if let Some(ref stage) = self.dispute_summary.dispute_lifecycle_stage {
            md.push_str(&format!("| Lifecycle Stage | `{}` |\n", stage));
        }
        if let Some(ref channel) = self.dispute_summary.dispute_channel {
            md.push_str(&format!("| Channel | `{}` |\n", channel));
        }
        if let Some(ref reason) = self.dispute_summary.reason {
            md.push_str(&format!("| Reason Code | `{}` |\n", reason));
        }
        if let (Some(ref val), Some(ref cur)) = (
            &self.dispute_summary.dispute_amount_value,
            &self.dispute_summary.dispute_amount_currency,
        ) {
            md.push_str(&format!("| Disputed Amount | {} {} |\n", val, cur));
        }
        if let Some(ref ord) = self.dispute_summary.paypal_order_id {
            md.push_str(&format!("| PayPal Order ID | `{}` |\n", ord));
        }
        if let Some(ref cap) = self.dispute_summary.seller_transaction_id {
            md.push_str(&format!(
                "| Seller Transaction ID (Capture) | `{}` |\n",
                cap
            ));
        }
        if let Some(ref created) = self.dispute_summary.dispute_create_time {
            md.push_str(&format!("| Dispute Opened | {} |\n", created));
        }
        if let Some(ref updated) = self.dispute_summary.dispute_update_time {
            md.push_str(&format!("| Dispute Updated | {} |\n", updated));
        }
        if let Some(ref src) = self.dispute_summary.source {
            md.push_str(&format!(
                "| Source Reference | {} |\n",
                format_source_ref(src)
            ));
        }
        md.push('\n');

        // 4. Correlation Summary (Deterministic Inference)
        md.push_str("## 2. Session Correlation Assessment\n\n");
        md.push_str(&format!(
            "> **Epistemic Classification**: `{}`\n\n",
            self.correlation_summary.category
        ));
        md.push_str(&format!(
            "- **Correlation Status**: `{:?}`\n",
            self.correlation_summary.status
        ));
        md.push_str(&format!(
            "- **Correlation Engine**: `{}`\n",
            self.correlation_summary.evaluator
        ));
        if let Some(matched) = self.correlation_summary.matched_by {
            md.push_str(&format!("- **Matched By**: `{:?}`\n", matched));
        }
        if let Some(ref s_id) = self.correlation_summary.session_id {
            md.push_str(&format!("- **Correlated Purchase Session**: `{}`\n", s_id));
        }
        md.push_str(&format!(
            "- **Assessment**: {}\n",
            self.correlation_summary.explanation
        ));
        if let Some(ref src) = self.correlation_summary.source {
            md.push_str(&format!(
                "- **Source Reference**: {}\n",
                format_source_ref(src)
            ));
        }
        md.push('\n');

        // 5. Cryptographic Verification (Verified Fact)
        md.push_str("## 3. Cryptographic Proof & Ledger Integrity\n\n");
        md.push_str(&format!(
            "> **Epistemic Classification**: `{}`\n\n",
            self.cryptographic_proof.category
        ));
        md.push_str("| Verification Parameter | Status / Value |\n");
        md.push_str("|---|---|\n");
        md.push_str(&format!(
            "| Dispute Audit Chain Valid | **{}** ({} steps) |\n",
            if self.cryptographic_proof.dispute_chain_valid {
                "VALID"
            } else {
                "FAILED / TAMPERED"
            },
            self.cryptographic_proof.dispute_total_steps
        ));
        if let Some(ref p_id) = self.cryptographic_proof.purchase_session_id {
            md.push_str(&format!("| Purchase Session ID | `{}` |\n", p_id));
            md.push_str(&format!(
                "| Purchase Ledger Chain Valid | **{}** ({} steps) |\n",
                if self.cryptographic_proof.purchase_chain_valid {
                    "VALID"
                } else {
                    "FAILED / TAMPERED"
                },
                self.cryptographic_proof.purchase_total_steps
            ));
        } else {
            md.push_str("| Purchase Session Ledger | N/A (Uncorrelated) |\n");
        }
        md.push_str(&format!(
            "| Genesis Anchor Type | `{:?}` |\n",
            self.cryptographic_proof.genesis_anchor_type
        ));
        md.push_str(&format!(
            "| Genesis Anchor Hash | `{}` |\n",
            self.cryptographic_proof.genesis_anchor_hash
        ));
        md.push_str(&format!(
            "| Dual-Chain Integrity Verified | **{}** |\n",
            if self.cryptographic_proof.dual_chain_integrity_verified {
                "YES"
            } else {
                "NO"
            }
        ));
        if let Some(ref src) = self.cryptographic_proof.dispute_root_hash_source {
            md.push_str(&format!(
                "| Dispute Root Hash Source | {} |\n",
                format_source_ref(src)
            ));
        }
        if let Some(ref src) = self.cryptographic_proof.purchase_root_hash_source {
            md.push_str(&format!(
                "| Purchase Root Hash Source | {} |\n",
                format_source_ref(src)
            ));
        }
        md.push_str("\n> **Cryptographic Scope Statement**:\n");
        md.push_str(&format!(
            "> {}\n\n",
            self.cryptographic_proof.integrity_scope_statement
        ));

        // 6. Agent Audit Trail (Verified Fact)
        if let Some(ref trail) = self.agent_audit_trail {
            md.push_str("## 4. Agent Decision Audit Trail\n\n");
            md.push_str(&format!(
                "> **Epistemic Classification**: `{}`\n\n",
                trail.category
            ));
            md.push_str(&format!(
                "### 4.1 Initial Customer Prompt\n- **Prompt**: \"{}\"\n- **Source**: {}\n\n",
                trail.initial_prompt.prompt,
                format_source_ref(&trail.initial_prompt.source)
            ));

            if let Some(ref req) = trail.requirements {
                md.push_str("### 4.2 Extracted Requirements\n");
                md.push_str(&format!("- Category: `{}`\n", req.category));
                md.push_str(&format!("- Maximum Budget: ${:.2}\n", req.maximum_budget));
                if let Some(ref brand) = req.preferred_brand {
                    md.push_str(&format!("- Preferred Brand: `{}`\n", brand));
                }
                if let Some(hours) = req.minimum_battery_hours {
                    md.push_str(&format!("- Minimum Battery: {} hours\n", hours));
                }
                if !req.required_features.is_empty() {
                    md.push_str(&format!(
                        "- Required Features: {:?}\n",
                        req.required_features
                    ));
                }
                md.push_str(&format!("- Source: {}\n\n", format_source_ref(&req.source)));
            }

            if let Some(ref cat) = trail.catalog_search {
                md.push_str("### 4.3 Catalog Search\n");
                md.push_str(&format!(
                    "- Evaluated Candidates Count: {}\n",
                    cat.candidates_evaluated_count
                ));
                md.push_str(&format!("- Source: {}\n\n", format_source_ref(&cat.source)));
            }

            if !trail.rejected_candidates.is_empty() {
                md.push_str("### 4.4 Rejected Candidates\n");
                for rej in &trail.rejected_candidates {
                    md.push_str(&format!(
                        "- **{}** (`{}`) - ${:.2} {}\n",
                        rej.product_name, rej.product_id, rej.price, rej.currency
                    ));
                    md.push_str(&format!(
                        "  - Agent Rejection Rationale: \"{}\"\n",
                        rej.rejection_rationale.statement
                    ));
                    md.push_str(&format!(
                        "  - Notice: *{}*\n",
                        rej.rejection_rationale.disclaimer
                    ));
                    md.push_str(&format!("  - Source: {}\n", format_source_ref(&rej.source)));
                }
                md.push('\n');
            }

            if let Some(ref sel) = trail.selected_product {
                md.push_str("### 4.5 Selected Product\n");
                md.push_str(&format!(
                    "- **Product**: {} (`{}`)\n",
                    sel.product_name, sel.product_id
                ));
                md.push_str(&format!(
                    "- **Brand / Category**: {} / {}\n",
                    sel.brand, sel.category
                ));
                md.push_str(&format!(
                    "- **Price**: ${:.2} {}\n",
                    sel.price, sel.currency
                ));
                md.push_str(&format!(
                    "- **Agent Selection Rationale**: \"{}\"\n",
                    sel.selection_rationale.statement
                ));
                md.push_str(&format!(
                    "- Notice: *{}*\n",
                    sel.selection_rationale.disclaimer
                ));
                md.push_str(&format!(
                    "- **Source**: {}\n\n",
                    format_source_ref(&sel.source)
                ));
            }

            if let Some(ref auth) = trail.purchase_authorization {
                md.push_str("### 4.6 Purchase Authorization\n");
                md.push_str(&format!(
                    "- Selected Product ID: `{}`\n",
                    auth.selected_product_id
                ));
                md.push_str(&format!(
                    "- Maximum Authorized Amount: ${:.2}\n",
                    auth.maximum_authorized_amount
                ));
                md.push_str(&format!("- User Confirmed: `{}`\n", auth.user_confirmed));
                md.push_str(&format!("- Authorized At: {}\n", auth.authorized_at));
                md.push_str(&format!("- Source: {}\n", format_source_ref(&auth.source)));
                md.push_str(&format!(
                    "\n> **Identity Disclaimer**: {}\n\n",
                    auth.identity_disclaimer
                ));
            }
        }

        // 7. Transaction Proof
        if let Some(ref trans) = self.transaction_proof {
            md.push_str("## 5. Transaction Proof & Settlement\n\n");
            md.push_str("### 5.1 Recorded PayPal Settlement\n");
            md.push_str(&format!(
                "> **Epistemic Classification**: `{}` | **Provenance**: `{}`\n\n",
                trans.paypal_settlement.category, trans.paypal_settlement.provenance
            ));
            if let Some(ref ord) = trans.paypal_settlement.paypal_order_id {
                md.push_str(&format!("- PayPal Order ID: `{}`\n", ord));
            }
            if let Some(ref st) = trans.paypal_settlement.order_status {
                md.push_str(&format!("- Order Status: `{}`\n", st));
            }
            if let (Some(ref amt), Some(ref cur)) = (
                &trans.paypal_settlement.order_amount,
                &trans.paypal_settlement.order_currency,
            ) {
                md.push_str(&format!("- Order Amount: {} {}\n", amt, cur));
            }
            if let Some(ref cap) = trans.paypal_settlement.paypal_capture_id {
                md.push_str(&format!("- PayPal Capture ID: `{}`\n", cap));
            }
            if let Some(ref st) = trans.paypal_settlement.capture_status {
                md.push_str(&format!("- Capture Status: `{}`\n", st));
            }
            if let Some(ref ts) = trans.paypal_settlement.capture_timestamp {
                md.push_str(&format!("- Capture Timestamp: {}\n", ts));
            }
            md.push_str(&format!(
                "- Source: {}\n\n",
                format_source_ref(&trans.paypal_settlement.source)
            ));

            md.push_str("### 5.2 Verified Ledger Settlement\n");
            md.push_str(&format!(
                "> **Epistemic Classification**: `{}`\n\n",
                trans.ledger_settlement.category
            ));
            if let Some(ref os) = trans.ledger_settlement.order_created_step {
                md.push_str(&format!(
                    "- Order Created Step: {}\n",
                    format_source_ref(os)
                ));
            }
            if let Some(ref cs) = trans.ledger_settlement.payment_captured_step {
                md.push_str(&format!(
                    "- Payment Captured Step: {}\n",
                    format_source_ref(cs)
                ));
            }
            md.push('\n');
        }

        // 8. Rebuttal Arguments
        md.push_str("## 6. Factual Rebuttal Arguments\n\n");
        md.push_str(&format!(
            "> **Epistemic Classification**: `{}`\n\n",
            self.rebuttal_summary.category
        ));
        for (i, arg) in self.rebuttal_summary.primary_arguments.iter().enumerate() {
            md.push_str(&format!("### 6.{} {}\n", i + 1, arg.claim));
            md.push_str(&format!("> **Classification**: `{}`\n\n", arg.category));
            md.push_str(&format!("{}\n\n", arg.explanation));
            if !arg.source_references.is_empty() {
                md.push_str("**Source References**:\n");
                for src in &arg.source_references {
                    md.push_str(&format!("- {}\n", format_source_ref(src)));
                }
                md.push('\n');
            }
        }

        // 9. Unavailable Evidence Disclaimers
        md.push_str("## 7. Unavailable Evidence Disclaimers\n\n");
        md.push_str(&format!(
            "> **Epistemic Classification**: `{}`\n\n",
            self.missing_evidence.category
        ));
        md.push_str(&format!("{}\n\n", self.missing_evidence.general_disclaimer));
        md.push_str("| Evidence Category | Reason Unavailable | Adjudication Impact |\n");
        md.push_str("|---|---|---|\n");
        for item in &self.missing_evidence.missing_items {
            md.push_str(&format!(
                "| `{:?}` | {} | {} |\n",
                item.evidence_type, item.reason_unavailable, item.adjudication_impact
            ));
        }
        md.push('\n');

        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epistemic_category_serialization_and_display() {
        assert_eq!(
            serde_json::to_string(&EpistemicCategory::VerifiedFact).unwrap(),
            "\"VERIFIED_FACT\""
        );
        assert_eq!(
            serde_json::to_string(&EpistemicCategory::RecordedPaypalData).unwrap(),
            "\"RECORDED_PAYPAL_DATA\""
        );
        assert_eq!(
            serde_json::to_string(&EpistemicCategory::DeterministicInference).unwrap(),
            "\"DETERMINISTIC_INFERENCE\""
        );
        assert_eq!(
            serde_json::to_string(&EpistemicCategory::UnavailableEvidence).unwrap(),
            "\"UNAVAILABLE_EVIDENCE\""
        );

        assert_eq!(EpistemicCategory::VerifiedFact.as_str(), "VERIFIED_FACT");
        assert_eq!(
            format!("{}", EpistemicCategory::VerifiedFact),
            "VERIFIED_FACT"
        );
        assert_eq!(
            format!("{}", EpistemicCategory::UnavailableEvidence),
            "UNAVAILABLE_EVIDENCE"
        );
    }

    #[test]
    fn test_invalid_json_category_fails_deserialization() {
        let invalid_json = "\"INVALID_CATEGORY\"";
        let res: Result<EpistemicCategory, _> = serde_json::from_str(invalid_json);
        assert!(
            res.is_err(),
            "Invalid category string must fail deserialization"
        );
    }

    #[test]
    fn test_unavailable_evidence_type_enum() {
        assert_eq!(
            serde_json::to_string(
                &UnavailableEvidenceType::PhysicalShippingTrackingAndProofOfDelivery
            )
            .unwrap(),
            "\"PHYSICAL_SHIPPING_TRACKING_AND_PROOF_OF_DELIVERY\""
        );
        assert_eq!(
            UnavailableEvidenceType::BuyerPhysicalIdentityProof.as_str(),
            "BUYER_PHYSICAL_IDENTITY_PROOF"
        );
        assert_eq!(
            format!("{}", UnavailableEvidenceType::ReturnMerchandiseReceipt),
            "RETURN_MERCHANDISE_RECEIPT"
        );
        assert_eq!(
            format!("{}", UnavailableEvidenceType::CardholderBankCommunications),
            "CARDHOLDER_BANK_COMMUNICATIONS"
        );
    }

    #[test]
    fn test_evidence_source_reference_constructors_and_validation() {
        // Valid ledger step
        let ledger_ref = EvidenceSourceReference::ledger_step(
            "sess-123",
            4,
            "PURCHASE_AUTHORIZED",
            "hash_abc",
            "2026-11-09T12:00:00Z",
        )
        .unwrap();
        assert_eq!(ledger_ref.session_id.as_deref(), Some("sess-123"));
        assert!(ledger_ref.dispute_id.is_none());
        assert_eq!(ledger_ref.sequence_index, Some(4));
        assert_eq!(
            ledger_ref.action_type.as_deref(),
            Some("PURCHASE_AUTHORIZED")
        );
        assert_eq!(ledger_ref.step_hash.as_deref(), Some("hash_abc"));
        assert!(ledger_ref.validate().is_ok());

        // Valid dispute step uses dispute_id, leaves session_id None
        let dispute_ref = EvidenceSourceReference::dispute_step(
            "PP-D-55555",
            1,
            "DISPUTE_CORRELATED",
            "disp_hash_1",
            "2026-11-09T18:05:00Z",
        )
        .unwrap();
        assert_eq!(dispute_ref.dispute_id.as_deref(), Some("PP-D-55555"));
        assert!(dispute_ref.session_id.is_none());
        assert_eq!(dispute_ref.sequence_index, Some(1));
        assert!(dispute_ref.validate().is_ok());

        // Valid session finalization source
        let finalization_ref = EvidenceSourceReference::session_finalization(
            "sess-123",
            "final_root_hash_abc",
            "2026-11-09T12:05:00Z",
        )
        .unwrap();
        assert_eq!(finalization_ref.session_id.as_deref(), Some("sess-123"));
        assert!(finalization_ref.sequence_index.is_none());
        assert!(finalization_ref.action_type.is_none());
        assert_eq!(
            finalization_ref.step_hash.as_deref(),
            Some("final_root_hash_abc")
        );
        assert!(finalization_ref.validate().is_ok());

        // Valid paypal record
        let paypal_ref = EvidenceSourceReference::paypal_record(
            "CAP-999",
            Some("2026-11-09T12:05:00Z".to_string()),
        )
        .unwrap();
        assert_eq!(paypal_ref.paypal_record_id.as_deref(), Some("CAP-999"));
        assert!(paypal_ref.validate().is_ok());

        // Valid webhook event
        let webhook_ref = EvidenceSourceReference::webhook_event(
            "trans-111",
            "WH-222",
            Some("2026-11-09T12:06:00Z".to_string()),
        )
        .unwrap();
        assert_eq!(
            webhook_ref.webhook_transmission_id.as_deref(),
            Some("trans-111")
        );
        assert_eq!(webhook_ref.paypal_record_id.as_deref(), Some("WH-222"));
        assert!(webhook_ref.validate().is_ok());
    }

    #[test]
    fn test_evidence_source_reference_validation_rejects_malformed_references() {
        // 1. Negative sequence index in ledger_step rejected
        let res = EvidenceSourceReference::ledger_step(
            "sess-1",
            -1,
            "TEST",
            "hash",
            "2026-11-09T12:00:00Z",
        );
        assert!(res.is_err(), "Negative sequence index must be rejected");

        // 2. Empty session_id rejected
        let res =
            EvidenceSourceReference::ledger_step("   ", 0, "TEST", "hash", "2026-11-09T12:00:00Z");
        assert!(res.is_err(), "Blank session_id must be rejected");

        // 3. Empty dispute_id rejected
        let res =
            EvidenceSourceReference::dispute_step("", 0, "TEST", "hash", "2026-11-09T12:00:00Z");
        assert!(res.is_err(), "Empty dispute_id must be rejected");

        // 4. Empty paypal_record_id rejected
        let res = EvidenceSourceReference::paypal_record("", None);
        assert!(res.is_err(), "Empty paypal_record_id must be rejected");

        // 5. Empty webhook transmission ID rejected
        let res = EvidenceSourceReference::webhook_event("", "EVT-1", None);
        assert!(res.is_err(), "Empty transmission_id must be rejected");

        // 6. Unanchored (all-None) reference fails validate()
        let unanchored = EvidenceSourceReference {
            session_id: Some("sess-1".to_string()),
            dispute_id: None,
            sequence_index: Some(0),
            action_type: Some("STEP".to_string()),
            step_hash: None,
            paypal_record_id: None,
            webhook_transmission_id: None,
            timestamp: None,
        };
        assert!(
            unanchored.validate().is_err(),
            "Reference with no cryptographic or external anchor must fail validation"
        );
    }

    #[test]
    fn test_missing_evidence_disclaimer_generation_includes_cardholder_comms() {
        let physical_disclaimers = MissingEvidenceDisclaimer::standard_disclaimers(true);
        assert_eq!(
            physical_disclaimers.category,
            EpistemicCategory::UnavailableEvidence
        );
        assert_eq!(physical_disclaimers.missing_items.len(), 5);
        assert!(physical_disclaimers
            .missing_items
            .iter()
            .any(|i| i.evidence_type
                == UnavailableEvidenceType::PhysicalShippingTrackingAndProofOfDelivery));
        assert!(physical_disclaimers
            .missing_items
            .iter()
            .any(|i| i.evidence_type == UnavailableEvidenceType::BuyerPhysicalIdentityProof));
        assert!(physical_disclaimers
            .missing_items
            .iter()
            .any(|i| i.evidence_type == UnavailableEvidenceType::CardholderBankCommunications));

        // Digital goods disclaimers must also include cardholder bank communications
        let digital_disclaimers = MissingEvidenceDisclaimer::standard_disclaimers(false);
        assert_eq!(digital_disclaimers.missing_items.len(), 3);
        assert!(digital_disclaimers
            .missing_items
            .iter()
            .any(|i| i.evidence_type == UnavailableEvidenceType::CardholderBankCommunications));
    }

    #[test]
    fn test_settlement_provenance_distinction_and_serialization() {
        let api_source = EvidenceSourceReference::paypal_record(
            "CAP-LIVE-1",
            Some("2026-11-09T17:02:00Z".to_string()),
        )
        .unwrap();

        let live_fact = RecordedPaypalSettlementFact {
            category: EpistemicCategory::RecordedPaypalData,
            provenance: SettlementProvenance::LivePaypalApi,
            source: api_source,
            paypal_order_id: Some("ORD-1".to_string()),
            order_status: Some("COMPLETED".to_string()),
            order_currency: Some("USD".to_string()),
            order_amount: Some("150.00".to_string()),
            paypal_capture_id: Some("CAP-LIVE-1".to_string()),
            capture_status: Some("COMPLETED".to_string()),
            capture_timestamp: Some("2026-11-09T17:02:00Z".to_string()),
        };

        let json = serde_json::to_string(&live_fact).unwrap();
        assert!(json.contains("\"provenance\":\"LIVE_PAYPAL_API\""));
        assert!(json.contains("\"category\":\"RECORDED_PAYPAL_DATA\""));

        // Replayed provenance
        let ledger_source = EvidenceSourceReference::ledger_step(
            "sess-1",
            7,
            "PAYPAL_PAYMENT_CAPTURED",
            "hash-7",
            "2026-11-09T17:02:00Z",
        )
        .unwrap();

        let replayed_fact = RecordedPaypalSettlementFact {
            category: EpistemicCategory::RecordedPaypalData,
            provenance: SettlementProvenance::ReplayedFromLedgerPayload,
            source: ledger_source,
            paypal_order_id: Some("ORD-1".to_string()),
            order_status: Some("COMPLETED".to_string()),
            order_currency: Some("USD".to_string()),
            order_amount: Some("150.00".to_string()),
            paypal_capture_id: Some("CAP-REPLAY-1".to_string()),
            capture_status: Some("COMPLETED".to_string()),
            capture_timestamp: Some("2026-11-09T17:02:00Z".to_string()),
        };

        let json_replayed = serde_json::to_string(&replayed_fact).unwrap();
        assert!(json_replayed.contains("\"provenance\":\"REPLAYED_FROM_LEDGER_PAYLOAD\""));
    }

    #[test]
    fn test_correlation_summary_evaluator_identification() {
        let summary = CorrelationSummary::new(
            CorrelationStatus::Matched,
            Some(MatchedBy::CaptureId),
            Some("sess-1".to_string()),
            "Matched via Capture ID CAP-123",
            None,
        );

        assert_eq!(summary.category, EpistemicCategory::DeterministicInference);
        assert_eq!(summary.evaluator, DEFAULT_CORRELATION_EVALUATOR);

        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"evaluator\":\"AGENTAUDIT_LOCAL_CORRELATION_ENGINE\""));
        assert!(json.contains("\"category\":\"DETERMINISTIC_INFERENCE\""));
    }

    #[test]
    fn test_epistemic_category_validation_guards() {
        // CorrelationSummary must reject non-DeterministicInference
        let bad_correlation = CorrelationSummary {
            category: EpistemicCategory::VerifiedFact,
            evaluator: DEFAULT_CORRELATION_EVALUATOR.to_string(),
            status: CorrelationStatus::Matched,
            matched_by: None,
            session_id: None,
            explanation: "test".to_string(),
            source: None,
        };
        assert!(bad_correlation.validate_category().is_err());

        // DisputeSummaryFact must reject non-RecordedPaypalData
        let bad_dispute = DisputeSummaryFact {
            category: EpistemicCategory::DeterministicInference,
            dispute_id: "PP-D-1".to_string(),
            status: "OPEN".to_string(),
            dispute_lifecycle_stage: None,
            dispute_channel: None,
            reason: None,
            dispute_amount_value: None,
            dispute_amount_currency: None,
            seller_transaction_id: None,
            paypal_order_id: None,
            dispute_create_time: None,
            dispute_update_time: None,
            source: None,
        };
        assert!(bad_dispute.validate_category().is_err());
    }

    #[test]
    fn test_optional_field_omission_in_serialization() {
        let minimal_dispute = DisputeSummaryFact {
            category: EpistemicCategory::RecordedPaypalData,
            dispute_id: "PP-D-999".to_string(),
            status: "OPEN".to_string(),
            dispute_lifecycle_stage: None,
            dispute_channel: None,
            reason: None,
            dispute_amount_value: None,
            dispute_amount_currency: None,
            seller_transaction_id: None,
            paypal_order_id: None,
            dispute_create_time: None,
            dispute_update_time: None,
            source: None,
        };

        let json = serde_json::to_string(&minimal_dispute).unwrap();
        assert!(!json.contains("\"reason\":null"));
        assert!(!json.contains("\"dispute_lifecycle_stage\":null"));
        assert!(!json.contains("\"seller_transaction_id\":null"));
        assert!(!json.contains("\"source\":null"));
    }

    #[test]
    fn test_dispute_evidence_brief_serde_roundtrip() {
        let brief = DisputeEvidenceBrief {
            brief_id: "brief-uuid-1".to_string(),
            schema_version: "1.0.0".to_string(),
            dispute_id: "PP-D-12345".to_string(),
            generated_at: "2026-11-09T19:00:00Z".to_string(),
            correlation_summary: CorrelationSummary::new(
                CorrelationStatus::Matched,
                Some(MatchedBy::CaptureId),
                Some("sess-abc".to_string()),
                "Deterministically matched via Capture ID CAP-123",
                Some(
                    EvidenceSourceReference::ledger_step(
                        "sess-abc",
                        7,
                        "PAYPAL_PAYMENT_CAPTURED",
                        "hash-7",
                        "2026-11-09T17:02:00Z",
                    )
                    .unwrap(),
                ),
            ),
            dispute_summary: DisputeSummaryFact {
                category: EpistemicCategory::RecordedPaypalData,
                dispute_id: "PP-D-12345".to_string(),
                status: "UNDER_REVIEW".to_string(),
                dispute_lifecycle_stage: Some("INQUIRY".to_string()),
                dispute_channel: Some("INTERNAL".to_string()),
                reason: Some("UNAUTHORISED_TRANSACTION".to_string()),
                dispute_amount_value: Some("179.99".to_string()),
                dispute_amount_currency: Some("USD".to_string()),
                seller_transaction_id: Some("CAP-123".to_string()),
                paypal_order_id: Some("ORD-456".to_string()),
                dispute_create_time: Some("2026-11-09T18:00:00Z".to_string()),
                dispute_update_time: Some("2026-11-09T18:00:00Z".to_string()),
                source: Some(
                    EvidenceSourceReference::webhook_event(
                        "trans-wh-1",
                        "WH-EVT-1",
                        Some("2026-11-09T18:00:01Z".to_string()),
                    )
                    .unwrap(),
                ),
            },
            cryptographic_proof: CryptographicProofSummary {
                category: EpistemicCategory::VerifiedFact,
                purchase_session_id: Some("sess-abc".to_string()),
                purchase_root_hash: Some("root-hash-123".to_string()),
                purchase_root_hash_source: Some(
                    EvidenceSourceReference::session_finalization(
                        "sess-abc",
                        "root-hash-123",
                        "2026-11-09T17:02:01Z",
                    )
                    .unwrap(),
                ),
                purchase_chain_valid: true,
                purchase_total_steps: 8,
                dispute_root_hash: Some("disp-root-456".to_string()),
                dispute_root_hash_source: Some(
                    EvidenceSourceReference::dispute_step(
                        "PP-D-12345",
                        1,
                        "DISPUTE_CORRELATED",
                        "disp-root-456",
                        "2026-11-09T18:05:00Z",
                    )
                    .unwrap(),
                ),
                dispute_chain_valid: true,
                dispute_total_steps: 2,
                genesis_anchor_type: GenesisAnchorType::PurchaseRootHash,
                genesis_anchor_hash: "root-hash-123".to_string(),
                dual_chain_integrity_verified: true,
                integrity_scope_statement:
                    CryptographicProofSummary::DEFAULT_SCOPE_STATEMENT.to_string(),
            },
            agent_audit_trail: Some(AgentAuditTrailSummary {
                category: EpistemicCategory::VerifiedFact,
                initial_prompt: PromptAuditFact {
                    prompt: "Find wireless headphones with ANC under $200".to_string(),
                    source: EvidenceSourceReference::ledger_step(
                        "sess-abc",
                        0,
                        "INITIAL_PROMPT",
                        "hash-0",
                        "2026-11-09T17:00:00Z",
                    )
                    .unwrap(),
                },
                requirements: Some(RequirementsAuditFact {
                    category: "Headphones".to_string(),
                    maximum_budget: 200.0,
                    preferred_brand: None,
                    minimum_battery_hours: Some(20),
                    required_features: vec!["anc".to_string()],
                    intended_use: None,
                    source: EvidenceSourceReference::ledger_step(
                        "sess-abc",
                        1,
                        "REQUIREMENTS_EXTRACTED",
                        "hash-1",
                        "2026-11-09T17:00:05Z",
                    )
                    .unwrap(),
                }),
                catalog_search: Some(CatalogSearchAuditFact {
                    candidates_evaluated_count: 5,
                    source: EvidenceSourceReference::ledger_step(
                        "sess-abc",
                        2,
                        "CATALOG_SEARCH",
                        "hash-2",
                        "2026-11-09T17:00:10Z",
                    )
                    .unwrap(),
                }),
                rejected_candidates: vec![RejectedProductAuditFact {
                    product_id: "prod-expensive".to_string(),
                    product_name: "Overbudget ANC".to_string(),
                    price: 249.99,
                    currency: "USD".to_string(),
                    rejection_rationale: AgentStatementFact::new(
                        "Price $249.99 exceeds customer budget limit of $200.00",
                    ),
                    source: EvidenceSourceReference::ledger_step(
                        "sess-abc",
                        3,
                        "PRODUCT_REJECTED",
                        "hash-3",
                        "2026-11-09T17:00:15Z",
                    )
                    .unwrap(),
                }],
                selected_product: Some(SelectedProductAuditFact {
                    product_id: "prod-best".to_string(),
                    product_name: "Budget ANC Pro".to_string(),
                    brand: "SoundTech".to_string(),
                    category: "Headphones".to_string(),
                    price: 179.99,
                    currency: "USD".to_string(),
                    selection_rationale: AgentStatementFact::new(
                        "Meets all criteria within budget",
                    ),
                    source: EvidenceSourceReference::ledger_step(
                        "sess-abc",
                        4,
                        "PRODUCT_SELECTED",
                        "hash-4",
                        "2026-11-09T17:00:20Z",
                    )
                    .unwrap(),
                }),
                purchase_authorization: Some(AuthorizationAuditFact {
                    selected_product_id: "prod-best".to_string(),
                    maximum_authorized_amount: 200.0,
                    user_confirmed: true,
                    authorized_at: "2026-11-09T17:01:00Z".to_string(),
                    identity_disclaimer:
                        AuthorizationAuditFact::DEFAULT_IDENTITY_DISCLAIMER.to_string(),
                    source: EvidenceSourceReference::ledger_step(
                        "sess-abc",
                        5,
                        "PURCHASE_AUTHORIZED",
                        "hash-5",
                        "2026-11-09T17:01:00Z",
                    )
                    .unwrap(),
                }),
            }),
            transaction_proof: Some(TransactionProof {
                paypal_settlement: RecordedPaypalSettlementFact {
                    category: EpistemicCategory::RecordedPaypalData,
                    provenance: SettlementProvenance::LivePaypalApi,
                    source: EvidenceSourceReference::paypal_record(
                        "CAP-123",
                        Some("2026-11-09T17:02:00Z".to_string()),
                    )
                    .unwrap(),
                    paypal_order_id: Some("ORD-456".to_string()),
                    order_status: Some("CREATED".to_string()),
                    order_currency: Some("USD".to_string()),
                    order_amount: Some("179.99".to_string()),
                    paypal_capture_id: Some("CAP-123".to_string()),
                    capture_status: Some("COMPLETED".to_string()),
                    capture_timestamp: Some("2026-11-09T17:02:00Z".to_string()),
                },
                ledger_settlement: VerifiedLedgerSettlementFact {
                    category: EpistemicCategory::VerifiedFact,
                    order_created_step: Some(
                        EvidenceSourceReference::ledger_step(
                            "sess-abc",
                            6,
                            "PAYPAL_ORDER_CREATED",
                            "hash-6",
                            "2026-11-09T17:01:30Z",
                        )
                        .unwrap(),
                    ),
                    payment_captured_step: Some(
                        EvidenceSourceReference::ledger_step(
                            "sess-abc",
                            7,
                            "PAYPAL_PAYMENT_CAPTURED",
                            "hash-7",
                            "2026-11-09T17:02:00Z",
                        )
                        .unwrap(),
                    ),
                },
            }),
            rebuttal_summary: RebuttalSummary {
                category: EpistemicCategory::DeterministicInference,
                dispute_reason: Some("UNAUTHORISED_TRANSACTION".to_string()),
                recommended_action: "CONTEST_WITH_EVIDENCE".to_string(),
                primary_arguments: vec![RebuttalPoint {
                    category: EpistemicCategory::DeterministicInference,
                    claim: "Customer explicitly authorized purchase before checkout".to_string(),
                    explanation: "Session ledger records explicit authorization at sequence 5 for product prod-best covering up to $200.00.".to_string(),
                    source_references: vec![EvidenceSourceReference::ledger_step(
                        "sess-abc",
                        5,
                        "PURCHASE_AUTHORIZED",
                        "hash-5",
                        "2026-11-09T17:01:00Z",
                    ).unwrap()],
                }],
                reason_specific_analysis: "Transaction matches authorized parameters exactly.".to_string(),
            },
            missing_evidence: MissingEvidenceDisclaimer::standard_disclaimers(true),
        };

        // Serialize to JSON
        let json_str = serde_json::to_string_pretty(&brief).unwrap();
        assert!(json_str.contains("\"VERIFIED_FACT\""));
        assert!(json_str.contains("\"RECORDED_PAYPAL_DATA\""));
        assert!(json_str.contains("\"DETERMINISTIC_INFERENCE\""));
        assert!(json_str.contains("\"UNAVAILABLE_EVIDENCE\""));
        assert!(json_str.contains("\"PHYSICAL_SHIPPING_TRACKING_AND_PROOF_OF_DELIVERY\""));
        assert!(json_str.contains("\"CARDHOLDER_BANK_COMMUNICATIONS\""));
        assert!(json_str.contains("\"evaluator\": \"AGENTAUDIT_LOCAL_CORRELATION_ENGINE\""));
        assert!(json_str.contains("\"provenance\": \"LIVE_PAYPAL_API\""));

        // Deserialize back
        let deserialized: DisputeEvidenceBrief = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.brief_id, "brief-uuid-1");
        assert_eq!(deserialized.dispute_id, "PP-D-12345");
        assert_eq!(
            deserialized.correlation_summary.status,
            CorrelationStatus::Matched
        );
        assert_eq!(
            deserialized.correlation_summary.category,
            EpistemicCategory::DeterministicInference
        );
        assert_eq!(
            deserialized.correlation_summary.evaluator,
            DEFAULT_CORRELATION_EVALUATOR
        );
        assert_eq!(
            deserialized.dispute_summary.category,
            EpistemicCategory::RecordedPaypalData
        );
        assert_eq!(
            deserialized.cryptographic_proof.category,
            EpistemicCategory::VerifiedFact
        );
        assert_eq!(
            deserialized
                .transaction_proof
                .as_ref()
                .unwrap()
                .paypal_settlement
                .provenance,
            SettlementProvenance::LivePaypalApi
        );
        assert_eq!(
            deserialized
                .transaction_proof
                .as_ref()
                .unwrap()
                .paypal_settlement
                .category,
            EpistemicCategory::RecordedPaypalData
        );
        assert_eq!(
            deserialized
                .transaction_proof
                .as_ref()
                .unwrap()
                .ledger_settlement
                .category,
            EpistemicCategory::VerifiedFact
        );
    }

    #[test]
    fn test_epistemic_integrity_rules() {
        // Rule: Authorization disclaimer must not claim identity proof
        let disclaimer = AuthorizationAuditFact::DEFAULT_IDENTITY_DISCLAIMER;
        assert!(disclaimer.contains("does not"));
        assert!(disclaimer.contains("identity"));

        // Rule: Hash chain scope statement must clarify tamper-detection vs database access
        let scope = CryptographicProofSummary::DEFAULT_SCOPE_STATEMENT;
        assert!(scope.contains("tamper-detection"));
        assert!(scope.contains("does not prevent an attacker with full database access"));

        // Rule: Rationale disclaimer must clarify agent-generated heuristics
        let rationale_disclaimer = AgentStatementFact::DEFAULT_RATIONALE_DISCLAIMER;
        assert!(rationale_disclaimer.contains("agent-generated"));
        assert!(rationale_disclaimer.contains("heuristics"));
    }

    #[test]
    fn test_dispute_evidence_brief_to_markdown() {
        let brief = DisputeEvidenceBrief {
            brief_id: "brief-md-test".to_string(),
            schema_version: "1.0.0".to_string(),
            dispute_id: "PP-D-MD-TEST".to_string(),
            generated_at: "2026-11-09T20:00:00Z".to_string(),
            correlation_summary: CorrelationSummary::new(
                CorrelationStatus::Matched,
                Some(MatchedBy::CaptureId),
                Some("sess-md-1".to_string()),
                "Matched via Capture ID".to_string(),
                None,
            ),
            dispute_summary: DisputeSummaryFact {
                category: EpistemicCategory::RecordedPaypalData,
                dispute_id: "PP-D-MD-TEST".to_string(),
                status: "UNDER_REVIEW".to_string(),
                dispute_lifecycle_stage: Some("INQUIRY".to_string()),
                dispute_channel: Some("INTERNAL".to_string()),
                reason: Some("UNAUTHORISED_TRANSACTION".to_string()),
                dispute_amount_value: Some("250.00".to_string()),
                dispute_amount_currency: Some("USD".to_string()),
                seller_transaction_id: Some("CAP-MD-1".to_string()),
                paypal_order_id: Some("ORD-MD-1".to_string()),
                dispute_create_time: Some("2026-11-09T18:00:00Z".to_string()),
                dispute_update_time: Some("2026-11-09T18:00:00Z".to_string()),
                source: None,
            },
            cryptographic_proof: CryptographicProofSummary {
                category: EpistemicCategory::VerifiedFact,
                purchase_session_id: Some("sess-md-1".to_string()),
                purchase_root_hash: Some("root-hash-1".to_string()),
                purchase_root_hash_source: None,
                purchase_chain_valid: true,
                purchase_total_steps: 8,
                dispute_root_hash: Some("disp-root-1".to_string()),
                dispute_root_hash_source: None,
                dispute_chain_valid: true,
                dispute_total_steps: 2,
                genesis_anchor_type: GenesisAnchorType::PurchaseRootHash,
                genesis_anchor_hash: "root-hash-1".to_string(),
                dual_chain_integrity_verified: true,
                integrity_scope_statement: CryptographicProofSummary::DEFAULT_SCOPE_STATEMENT
                    .to_string(),
            },
            agent_audit_trail: None,
            transaction_proof: None,
            rebuttal_summary: RebuttalSummary {
                category: EpistemicCategory::DeterministicInference,
                dispute_reason: Some("UNAUTHORISED_TRANSACTION".to_string()),
                recommended_action: "CONTEST_WITH_EVIDENCE".to_string(),
                primary_arguments: vec![RebuttalPoint {
                    category: EpistemicCategory::DeterministicInference,
                    claim: "Pre-authorized transaction".to_string(),
                    explanation: "Customer authorized purchase in session.".to_string(),
                    source_references: Vec::new(),
                }],
                reason_specific_analysis: "Transaction authorized.".to_string(),
            },
            missing_evidence: MissingEvidenceDisclaimer::standard_disclaimers(true),
        };

        let md = brief.to_markdown();
        assert!(md.contains("# Dispute Evidence Brief: PP-D-MD-TEST"));
        assert!(md.contains("VERIFIED_FACT"));
        assert!(md.contains("RECORDED_PAYPAL_DATA"));
        assert!(md.contains("DETERMINISTIC_INFERENCE"));
        assert!(md.contains("UNAVAILABLE_EVIDENCE"));
        assert!(md.contains("does not prevent an attacker with full database access"));
        assert!(md.contains("CONTEST_WITH_EVIDENCE"));
    }
}
