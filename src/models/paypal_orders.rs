use serde::{Deserialize, Serialize};

use crate::models::authorization::PurchaseAuthorization;
use crate::models::catalog::Product;
use crate::models::ledger::VerificationResult;
use crate::models::shopping::ShoppingRequirements;

/// PayPal link reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalLink {
    pub href: String,
    pub rel: String,
    #[serde(default)]
    pub method: Option<String>,
}

/// PayPal money amount.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalAmount {
    pub currency_code: String,
    pub value: String,
}

/// PayPal order purchase unit request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalPurchaseUnitRequest {
    pub reference_id: String,
    pub description: String,
    pub custom_id: String,
    pub amount: PayPalAmount,
}

/// PayPal web experience context for checkout UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalExperienceContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landing_page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_method_preference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_url: Option<String>,
}

/// PayPal wrapper for paypal experience context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalExperienceWrapper {
    pub experience_context: PayPalExperienceContext,
}

/// Payment source structure for PayPal Orders v2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalPaymentSource {
    pub paypal: PayPalExperienceWrapper,
}

/// PayPal Orders v2 creation request payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalCreateOrderRequest {
    pub intent: String,
    pub purchase_units: Vec<PayPalPurchaseUnitRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_source: Option<PayPalPaymentSource>,
}

/// PayPal Orders v2 creation response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalCreateOrderResponse {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub links: Vec<PayPalLink>,
}

/// Detailed capture transaction from PayPal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalCaptureDetail {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub amount: Option<PayPalAmount>,
    #[serde(default)]
    pub create_time: Option<String>,
    #[serde(default)]
    pub update_time: Option<String>,
}

/// Payments object inside a captured purchase unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PayPalUnitPayments {
    #[serde(default)]
    pub captures: Vec<PayPalCaptureDetail>,
}

/// Purchase unit in capture response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalCapturedPurchaseUnit {
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub payments: Option<PayPalUnitPayments>,
}

/// PayPal Orders v2 capture response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayPalCaptureOrderResponse {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub purchase_units: Vec<PayPalCapturedPurchaseUnit>,
    #[serde(default)]
    pub links: Vec<PayPalLink>,
}

/// Response returned from POST /api/shopping/sessions/:session_id/orders.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateOrderResponse {
    pub success: bool,
    pub session_id: String,
    pub order_id: String,
    pub status: String,
    pub approval_url: Option<String>,
}

/// Response returned from POST /api/shopping/sessions/:session_id/capture.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureOrderResponse {
    pub success: bool,
    pub session_id: String,
    pub order_id: String,
    pub capture_id: Option<String>,
    pub status: String,
    pub final_root_hash: String,
}

/// Comprehensive details of a shopping session lifecycle (GET /api/shopping/sessions/:session_id).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShoppingSessionDetail {
    pub session_id: String,
    pub initial_prompt: String,
    pub requirements: Option<ShoppingRequirements>,
    pub selected_product: Option<Product>,
    pub authorization: Option<PurchaseAuthorization>,
    pub paypal_order_id: Option<String>,
    pub is_finalized: bool,
    pub root_hash: Option<String>,
    pub event_count: usize,
    pub verification: VerificationResult,
}
