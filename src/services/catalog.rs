use std::sync::Arc;

use crate::models::catalog::{Product, ProductSpecification};

/// Thread-safe product catalog service providing deterministic product lookups and searches.
#[derive(Clone)]
pub struct CatalogService {
    products: Arc<Vec<Product>>,
}

impl Default for CatalogService {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogService {
    /// Creates a new catalog service seeded with realistic headphone products.
    pub fn new() -> Self {
        let products = vec![
            Product {
                product_id: "hp-sony-wh1000xm5".to_string(),
                name: "Sony WH-1000XM5 Wireless Noise-Canceling Headphones".to_string(),
                brand: "Sony".to_string(),
                category: "headphones".to_string(),
                price: 399.99,
                currency: "USD".to_string(),
                rating: 4.8,
                available_stock: 45,
                description: "Industry-leading noise canceling with two processors and 8 microphones. Ultra-comfortable lightweight design.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 30,
                    noise_cancelling: true,
                    anc_type: "Hybrid Active".to_string(),
                    wireless: true,
                    bluetooth_version: "5.2".to_string(),
                    weight_grams: 250,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "noise-canceling".to_string(),
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "fast-charge".to_string(),
                    "multipoint".to_string(),
                ],
            },
            Product {
                product_id: "hp-bose-qc45".to_string(),
                name: "Bose QuietComfort 45 Bluetooth Wireless Headphones".to_string(),
                brand: "Bose".to_string(),
                category: "headphones".to_string(),
                price: 329.00,
                currency: "USD".to_string(),
                rating: 4.7,
                available_stock: 30,
                description: "Iconic quiet, comfort, and sound. TriPort acoustic architecture offers depth and fullness.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 24,
                    noise_cancelling: true,
                    anc_type: "Acoustic Noise Cancelling".to_string(),
                    wireless: true,
                    bluetooth_version: "5.1".to_string(),
                    weight_grams: 240,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "noise-canceling".to_string(),
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "aware-mode".to_string(),
                ],
            },
            Product {
                product_id: "hp-anker-q30".to_string(),
                name: "Anker Soundcore Life Q30 Hybrid Active Noise Cancelling Headphones".to_string(),
                brand: "Anker".to_string(),
                category: "headphones".to_string(),
                price: 79.99,
                currency: "USD".to_string(),
                rating: 4.5,
                available_stock: 120,
                description: "Advanced noise cancellation technology with 40-hour battery life and custom EQ via app.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 40,
                    noise_cancelling: true,
                    anc_type: "Hybrid Active".to_string(),
                    wireless: true,
                    bluetooth_version: "5.0".to_string(),
                    weight_grams: 260,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "noise-canceling".to_string(),
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "transparency-mode".to_string(),
                    "app-control".to_string(),
                ],
            },
            Product {
                product_id: "hp-anker-q20".to_string(),
                name: "Anker Soundcore Life Q20 Hybrid Active Noise Cancelling Headphones".to_string(),
                brand: "Anker".to_string(),
                category: "headphones".to_string(),
                price: 49.99,
                currency: "USD".to_string(),
                rating: 4.4,
                available_stock: 200,
                description: "High-resolution audio certified with BassUp technology and 30-hour wireless playtime in ANC mode.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 30,
                    noise_cancelling: true,
                    anc_type: "Hybrid Active".to_string(),
                    wireless: true,
                    bluetooth_version: "5.0".to_string(),
                    weight_grams: 268,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "noise-canceling".to_string(),
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "bassup".to_string(),
                ],
            },
            Product {
                product_id: "hp-jbl-tune760".to_string(),
                name: "JBL Tune 760NC Wireless Over-Ear Active Noise Cancelling Headphones".to_string(),
                brand: "JBL".to_string(),
                category: "headphones".to_string(),
                price: 89.95,
                currency: "USD".to_string(),
                rating: 4.3,
                available_stock: 85,
                description: "Punchy Pure Bass Sound with active noise cancellation and up to 35 hours battery life with ANC on.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 35,
                    noise_cancelling: true,
                    anc_type: "Active".to_string(),
                    wireless: true,
                    bluetooth_version: "5.0".to_string(),
                    weight_grams: 220,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "noise-canceling".to_string(),
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "pure-bass".to_string(),
                    "hands-free-calls".to_string(),
                ],
            },
            Product {
                product_id: "hp-senn-hd450".to_string(),
                name: "Sennheiser HD 450BT Wireless Noise-Cancelling Headphones".to_string(),
                brand: "Sennheiser".to_string(),
                category: "headphones".to_string(),
                price: 129.95,
                currency: "USD".to_string(),
                rating: 4.2,
                available_stock: 40,
                description: "Great wireless sound with deep dynamic bass and active noise cancellation for uninterrupted listening.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 30,
                    noise_cancelling: true,
                    anc_type: "Active".to_string(),
                    wireless: true,
                    bluetooth_version: "5.0".to_string(),
                    weight_grams: 238,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "noise-canceling".to_string(),
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "aac-aptx".to_string(),
                ],
            },
            Product {
                product_id: "hp-audio-technica-m20x".to_string(),
                name: "Audio-Technica ATH-M20xBT Wireless Over-Ear Headphones".to_string(),
                brand: "Audio-Technica".to_string(),
                category: "headphones".to_string(),
                price: 79.00,
                currency: "USD".to_string(),
                rating: 4.4,
                available_stock: 65,
                description: "Critically acclaimed studio sound with 60 hours battery life. Passive noise isolation without active circuitry.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 60,
                    noise_cancelling: false,
                    anc_type: "Passive Isolation".to_string(),
                    wireless: true,
                    bluetooth_version: "5.0".to_string(),
                    weight_grams: 216,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "long-battery".to_string(),
                    "low-latency".to_string(),
                ],
            },
            Product {
                product_id: "hp-sony-ch720n".to_string(),
                name: "Sony WH-CH720N Noise-Canceling Wireless Headphones".to_string(),
                brand: "Sony".to_string(),
                category: "headphones".to_string(),
                price: 148.00,
                currency: "USD".to_string(),
                rating: 4.5,
                available_stock: 55,
                description: "Lightweight wireless noise-canceling headphones powered by Sony Integrated Processor V1.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 35,
                    noise_cancelling: true,
                    anc_type: "Dual Noise Sensor".to_string(),
                    wireless: true,
                    bluetooth_version: "5.2".to_string(),
                    weight_grams: 192,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "noise-canceling".to_string(),
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "lightweight".to_string(),
                ],
            },
            Product {
                product_id: "hp-skullcandy-hesh".to_string(),
                name: "Skullcandy Hesh ANC Wireless Noise-Canceling Headphones".to_string(),
                brand: "Skullcandy".to_string(),
                category: "headphones".to_string(),
                price: 99.99,
                currency: "USD".to_string(),
                rating: 4.1,
                available_stock: 75,
                description: "4-mic digital active noise canceling with Tile finding technology and 22-hour battery life.".to_string(),
                specifications: ProductSpecification {
                    battery_life_hours: 22,
                    noise_cancelling: true,
                    anc_type: "Active".to_string(),
                    wireless: true,
                    bluetooth_version: "5.0".to_string(),
                    weight_grams: 228,
                    microphone: true,
                    water_resistant: false,
                },
                features: vec![
                    "noise-canceling".to_string(),
                    "wireless".to_string(),
                    "bluetooth".to_string(),
                    "tile-tracker".to_string(),
                ],
            },
        ];

        Self {
            products: Arc::new(products),
        }
    }

    /// Lists all available products in the catalog.
    pub fn list_products(&self) -> Vec<Product> {
        (*self.products).clone()
    }

    /// Looks up a product by its unique product ID.
    pub fn get_product(&self, id: &str) -> Option<Product> {
        self.products.iter().find(|p| p.product_id == id).cloned()
    }

    /// Searches products by keyword in name, brand, or features, optionally filtering by category.
    pub fn search_products(&self, query: &str, category: Option<&str>) -> Vec<Product> {
        let q_lower = query.to_lowercase();
        let terms: Vec<&str> = q_lower.split_whitespace().collect();

        self.products
            .iter()
            .filter(|p| {
                if let Some(cat) = category {
                    if !p.category.eq_ignore_ascii_case(cat) {
                        return false;
                    }
                }

                if terms.is_empty() {
                    return true;
                }

                let p_name = p.name.to_lowercase();
                let p_brand = p.brand.to_lowercase();
                let p_desc = p.description.to_lowercase();

                terms.iter().any(|term| {
                    p_name.contains(term)
                        || p_brand.contains(term)
                        || p_desc.contains(term)
                        || p.features.iter().any(|f| f.contains(term))
                })
            })
            .cloned()
            .collect()
    }
}
