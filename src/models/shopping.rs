use serde::{Deserialize, Serialize};

use crate::models::catalog::Product;

/// Structured customer shopping requirements extracted from natural language.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShoppingRequirements {
    pub category: String,
    pub maximum_budget: f64,
    pub currency: String,
    pub required_features: Vec<String>,
    pub minimum_battery_hours: Option<u32>,
    pub preferred_brand: Option<String>,
}

/// Request body to initiate an AI shopping session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShoppingRequest {
    pub prompt: String,
}

/// Summary of a candidate product explored during catalog search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateSummary {
    pub product_id: String,
    pub name: String,
    pub brand: String,
    pub price: f64,
    pub currency: String,
    pub battery_life_hours: u32,
    pub noise_cancelling: bool,
    pub rating: f64,
}

/// Structured record of a product rejection with explicit reason.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RejectionRecord {
    pub product_id: String,
    pub product_name: String,
    pub price: f64,
    pub currency: String,
    pub rejection_reason: String,
}

/// Detailed evaluation and scoring for a matching product candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateEvaluation {
    pub product_id: String,
    pub product_name: String,
    pub score: f64,
    pub price: f64,
    pub battery_hours: u32,
    pub rating: f64,
    pub brand_matched: bool,
}

/// Selection record committed to the cryptographic ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelectionRecord {
    pub selected_product: Product,
    pub price: f64,
    pub currency: String,
    pub selection_rationale: String,
}

/// Response returned from POST /api/shopping/sessions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShoppingSessionResponse {
    pub success: bool,
    pub session_id: String,
    pub prompt: String,
    pub requirements: ShoppingRequirements,
    pub candidates_found: usize,
    pub rejected_candidates: Vec<RejectionRecord>,
    pub selected_product: Option<Product>,
    pub selection_rationale: Option<String>,
    pub current_sequence_index: i64,
    pub latest_hash: String,
}
