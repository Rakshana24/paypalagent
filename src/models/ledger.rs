use serde::{Deserialize, Serialize};

/// Session database entity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Session {
    pub session_id: String,
    pub initial_prompt: String,
    pub paypal_order_id: Option<String>,
    pub root_hash: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Decision step / ledger event database entity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct DecisionStep {
    pub id: String,
    pub session_id: String,
    pub sequence_index: i64,
    pub action_type: String,
    pub payload: String,
    pub previous_hash: String,
    pub current_hash: String,
    pub created_at: String,
}

/// Request body for creating an audit session (POST /api/sessions).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSessionRequest {
    pub initial_prompt: String,
}

/// Response returned on session creation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSessionResponse {
    pub success: bool,
    pub session_id: String,
    pub initial_hash: String,
    pub created_at: String,
}

/// Request body for appending an observable event (POST /api/sessions/{session_id}/events).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppendEventRequest {
    pub action_type: String,
    pub payload: serde_json::Value,
}

/// Response returned on appending an event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppendEventResponse {
    pub success: bool,
    pub step_id: String,
    pub sequence_index: i64,
    pub previous_hash: String,
    pub current_hash: String,
    pub created_at: String,
}

/// Response returned on finalizing a session (POST /api/sessions/{session_id}/finalize).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinalizeSessionResponse {
    pub success: bool,
    pub session_id: String,
    pub root_hash: String,
    pub total_steps: usize,
    pub finalized_at: String,
}

/// Verification status classification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum VerificationStatus {
    Valid,
    Tampered,
}

/// Complete cryptographic hash-chain verification report (GET /api/sessions/{session_id}/verify).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationResult {
    pub session_id: String,
    pub status: VerificationStatus,
    pub total_steps: usize,
    pub verified_steps: usize,
    pub calculated_root_hash: Option<String>,
    pub stored_root_hash: Option<String>,
    pub first_invalid_sequence_index: Option<i64>,
    pub error_code: Option<String>,
    pub explanation: String,
}
