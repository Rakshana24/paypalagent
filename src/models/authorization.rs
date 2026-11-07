use serde::{Deserialize, Serialize};

/// Status of purchase approval by customer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum AuthorizationStatus {
    Approved,
    Rejected,
}

/// Request to authorize purchasing the selected item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthorizePurchaseRequest {
    pub selected_product_id: String,
    pub maximum_authorized_amount: f64,
    pub currency: String,
    pub approved: bool,
}

/// Purchase authorization entity committed to the ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PurchaseAuthorization {
    pub session_id: String,
    pub selected_product_id: String,
    pub maximum_authorized_amount: f64,
    pub currency: String,
    pub authorization_status: AuthorizationStatus,
    pub authorization_timestamp: String,
    pub authorization_method: String,
}

/// Response returned from POST /api/shopping/sessions/:session_id/authorize.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthorizePurchaseResponse {
    pub success: bool,
    pub session_id: String,
    pub authorization: PurchaseAuthorization,
    pub sequence_index: i64,
    pub current_hash: String,
}
