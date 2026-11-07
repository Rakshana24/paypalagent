use serde::{Deserialize, Serialize};

/// Detailed specifications for a catalog product.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProductSpecification {
    pub battery_life_hours: u32,
    pub noise_cancelling: bool,
    pub anc_type: String,
    pub wireless: bool,
    pub bluetooth_version: String,
    pub weight_grams: u32,
    pub microphone: bool,
    pub water_resistant: bool,
}

/// Catalog product entity representing an item available for purchase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Product {
    pub product_id: String,
    pub name: String,
    pub brand: String,
    pub category: String,
    pub price: f64,
    pub currency: String,
    pub rating: f64,
    pub available_stock: u32,
    pub description: String,
    pub specifications: ProductSpecification,
    pub features: Vec<String>,
}
