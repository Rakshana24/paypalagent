use crate::error::AppError;
use crate::models::shopping::ShoppingRequirements;

/// Deterministic parser and extractor for customer natural language shopping requests.
pub struct RequirementExtractor;

impl RequirementExtractor {
    /// Extracts structured shopping requirements from a natural language prompt.
    pub fn extract(prompt: &str) -> Result<ShoppingRequirements, AppError> {
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidPayload(
                "Shopping request prompt cannot be empty".to_string(),
            ));
        }

        let lower = trimmed.to_lowercase();

        // 1. Currency validation
        let currency = if lower.contains("inr") || lower.contains('₹') {
            return Err(AppError::InvalidPayload(
                "Currency 'INR' is not supported for PayPal Sandbox checkout in this integration; please specify USD ($)".to_string(),
            ));
        } else if lower.contains("eur") || lower.contains('€') {
            return Err(AppError::InvalidPayload(
                "Currency 'EUR' is not supported; please specify USD ($)".to_string(),
            ));
        } else if lower.contains("gbp") || lower.contains('£') {
            return Err(AppError::InvalidPayload(
                "Currency 'GBP' is not supported; please specify USD ($)".to_string(),
            ));
        } else {
            "USD".to_string()
        };

        // 2. Maximum budget extraction
        let maximum_budget = Self::extract_budget(&lower)?;
        if maximum_budget <= 0.0 {
            return Err(AppError::InvalidPayload(
                "Maximum budget must be greater than zero".to_string(),
            ));
        }

        // 3. Category extraction
        let category = if lower.contains("earbuds") {
            "earbuds".to_string()
        } else if lower.contains("headset") {
            "headset".to_string()
        } else {
            "headphones".to_string()
        };

        // 4. Battery hours requirement
        let minimum_battery_hours = Self::extract_battery_hours(&lower);

        // 5. Required features
        let mut required_features = Vec::new();
        if lower.contains("noise-canceling")
            || lower.contains("noise-cancelling")
            || lower.contains("noise canceling")
            || lower.contains("noise cancelling")
            || lower.contains("anc")
        {
            required_features.push("noise-canceling".to_string());
        }
        if lower.contains("wireless") || lower.contains("bluetooth") {
            required_features.push("wireless".to_string());
        }
        if lower.contains("microphone") || lower.contains("mic") {
            required_features.push("microphone".to_string());
        }
        if lower.contains("waterproof") || lower.contains("water-resistant") {
            required_features.push("water-resistant".to_string());
        }

        // 6. Preferred brand extraction
        let preferred_brand = if lower.contains("sony") {
            Some("Sony".to_string())
        } else if lower.contains("bose") {
            Some("Bose".to_string())
        } else if lower.contains("anker") || lower.contains("soundcore") {
            Some("Anker".to_string())
        } else if lower.contains("sennheiser") {
            Some("Sennheiser".to_string())
        } else if lower.contains("jbl") {
            Some("JBL".to_string())
        } else if lower.contains("skullcandy") {
            Some("Skullcandy".to_string())
        } else if lower.contains("audio-technica") || lower.contains("audio technica") {
            Some("Audio-Technica".to_string())
        } else {
            None
        };

        Ok(ShoppingRequirements {
            category,
            maximum_budget,
            currency,
            required_features,
            minimum_battery_hours,
            preferred_brand,
        })
    }

    /// Helper to extract budget value from prompt.
    fn extract_budget(text: &str) -> Result<f64, AppError> {
        let words: Vec<&str> = text.split_whitespace().collect();

        // Check patterns like "$100", "under $100", "under 100", "below 100", "budget 100", "max 100"
        for (i, word) in words.iter().enumerate() {
            let clean = word.trim_matches(|c: char| !c.is_numeric() && c != '.' && c != '$');

            // Pattern with leading dollar sign e.g. "$100" or "$100.50"
            if clean.starts_with('$') {
                let num_str = &clean[1..];
                if let Ok(val) = num_str.parse::<f64>() {
                    return Ok(val);
                }
            }

            // Keyword followed by number e.g. "under 100", "below 100", "budget 100", "max 100", "< 100"
            let is_budget_keyword = *word == "under"
                || *word == "below"
                || *word == "budget"
                || *word == "max"
                || *word == "maximum"
                || *word == "<"
                || *word == "<=";

            if is_budget_keyword && i + 1 < words.len() {
                let next_word =
                    words[i + 1].trim_matches(|c: char| !c.is_numeric() && c != '.' && c != '$');
                let num_str = if next_word.starts_with('$') {
                    &next_word[1..]
                } else {
                    next_word
                };
                if let Ok(val) = num_str.parse::<f64>() {
                    return Ok(val);
                }
            }
        }

        // Fallback: look for any number followed by "dollars" or "usd"
        for (i, word) in words.iter().enumerate() {
            if (*word == "dollars" || *word == "dollar" || *word == "usd") && i > 0 {
                let prev_word = words[i - 1].trim_matches(|c: char| !c.is_numeric() && c != '.');
                if let Ok(val) = prev_word.parse::<f64>() {
                    return Ok(val);
                }
            }
        }

        Err(AppError::InvalidPayload(
            "Could not determine maximum budget from shopping prompt (e.g. 'under $100')"
                .to_string(),
        ))
    }

    /// Helper to extract minimum battery hours from prompt.
    fn extract_battery_hours(text: &str) -> Option<u32> {
        let words: Vec<&str> = text.split_whitespace().collect();

        for (i, word) in words.iter().enumerate() {
            // E.g. "30h", "30hr", "30hrs"
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
            if let Some(num_part) = clean
                .strip_suffix("h")
                .or_else(|| clean.strip_suffix("hr"))
                .or_else(|| clean.strip_suffix("hrs"))
            {
                if let Ok(h) = num_part.parse::<u32>() {
                    return Some(h);
                }
            }

            // E.g. "30 hours", "30 hrs", "30 hour"
            if (clean == "hours" || clean == "hrs" || clean == "hour") && i > 0 {
                let prev = words[i - 1].trim_matches(|c: char| !c.is_numeric());
                if let Ok(h) = prev.parse::<u32>() {
                    return Some(h);
                }
            }
        }

        None
    }
}
