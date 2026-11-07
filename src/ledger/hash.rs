use sha2::{Digest, Sha256};

/// 64-character hexadecimal genesis hash for sequence index 0.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Format/version identifier for the hash-chain encoding scheme.
pub const HASH_VERSION: &str = "AgentAudit-v1";

/// Normalizes a JSON value into a canonical `serde_json::Value` according to RFC 8785 (JCS).
pub fn canonicalize_json_value(value: &serde_json::Value) -> serde_json::Value {
    let s = serde_jcs::to_string(value).unwrap_or_default();
    serde_json::from_str(&s).unwrap_or_else(|_| value.clone())
}

/// Serializes any serializable value into deterministic RFC 8785 (JCS) canonical JSON bytes.
pub fn to_canonical_json_bytes<T: serde::Serialize + ?Sized>(
    value: &T,
) -> Result<Vec<u8>, serde_json::Error> {
    serde_jcs::to_vec(value)
}

/// Serializes any serializable value into a deterministic RFC 8785 (JCS) canonical compact JSON string.
pub fn to_canonical_json_string<T: serde::Serialize + ?Sized>(
    value: &T,
) -> Result<String, serde_json::Error> {
    serde_jcs::to_string(value)
}

/// Computes a deterministic SHA-256 hash digest for a ledger event.
/// Uses unambiguous length-prefixed byte encoding across all dynamic-length inputs.
pub fn compute_event_hash(
    sequence_index: i64,
    action_type: &str,
    payload_bytes: &[u8],
    previous_hash: &str,
) -> String {
    let mut hasher = Sha256::new();

    // 1. Hash scheme format/version identifier: 4-byte BE length + UTF-8 bytes
    let version_bytes = HASH_VERSION.as_bytes();
    hasher.update(&(version_bytes.len() as u32).to_be_bytes());
    hasher.update(version_bytes);

    // 2. Sequence index: 8-byte signed big-endian integer
    hasher.update(&sequence_index.to_be_bytes());

    // 3. Action type: 4-byte BE length + UTF-8 bytes
    let action_bytes = action_type.as_bytes();
    hasher.update(&(action_bytes.len() as u32).to_be_bytes());
    hasher.update(action_bytes);

    // 4. Previous hash: 4-byte BE length + lowercase hex bytes
    let prev_hash_bytes = previous_hash.to_lowercase();
    let prev_bytes = prev_hash_bytes.as_bytes();
    hasher.update(&(prev_bytes.len() as u32).to_be_bytes());
    hasher.update(prev_bytes);

    // 5. Canonical payload: 8-byte BE length + raw payload bytes
    hasher.update(&(payload_bytes.len() as u64).to_be_bytes());
    hasher.update(payload_bytes);

    let digest = hasher.finalize();
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_genesis_hash_format() {
        assert_eq!(GENESIS_HASH.len(), 64);
        assert_eq!(
            GENESIS_HASH,
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn test_canonicalize_json_key_order_independence() {
        let json_a = json!({
            "z": 100,
            "a": "first",
            "m": { "nested_b": 2, "nested_a": 1 }
        });

        let json_b = json!({
            "a": "first",
            "m": { "nested_a": 1, "nested_b": 2 },
            "z": 100
        });

        let bytes_a = to_canonical_json_bytes(&json_a).unwrap();
        let bytes_b = to_canonical_json_bytes(&json_b).unwrap();
        assert_eq!(bytes_a, bytes_b);

        let str_a = to_canonical_json_string(&json_a).unwrap();
        assert_eq!(
            str_a,
            r#"{"a":"first","m":{"nested_a":1,"nested_b":2},"z":100}"#
        );
    }

    #[test]
    fn test_rfc8785_utf16_code_unit_sorting() {
        // RFC 8785 Section 3.2.3:
        // Supplementary characters (surrogate pairs like U+1F600 '😀') must sort BEFORE
        // BMP Private Use characters like U+E000 ('\uE000') because 0xD83D < 0xE000 in UTF-16.
        let json_input: serde_json::Value = serde_json::from_str(
            r#"{
                "\uE000": "bmp_pua",
                "\uD83D\uDE00": "supplementary_emoji",
                "a": 1
            }"#,
        )
        .unwrap();

        let canonical = to_canonical_json_string(&json_input).unwrap();
        // Expected order: "a", then "😀" (0xD83D), then "\uE000" (0xE000)
        let expected = format!(
            "{{\"a\":1,\"😀\":\"supplementary_emoji\",\"{tag}\":\"bmp_pua\"}}",
            tag = '\u{E000}'
        );
        assert_eq!(canonical, expected);
    }

    #[test]
    fn test_rfc8785_number_serialization() {
        // Integer numbers
        assert_eq!(to_canonical_json_string(&json!(100)).unwrap(), "100");
        assert_eq!(to_canonical_json_string(&json!(0)).unwrap(), "0");
        assert_eq!(to_canonical_json_string(&json!(-42)).unwrap(), "-42");

        // Floats with whole number values serialize without decimal point in RFC 8785
        assert_eq!(to_canonical_json_string(&json!(1.0)).unwrap(), "1");
        assert_eq!(to_canonical_json_string(&json!(0.0)).unwrap(), "0");

        // Fractional floats
        assert_eq!(
            to_canonical_json_string(&json!(3.14159)).unwrap(),
            "3.14159"
        );

        // Scientific notation range per RFC 8785 (ECMAScript ToString)
        assert_eq!(
            to_canonical_json_string(&json!(1e20)).unwrap(),
            "100000000000000000000"
        );
        assert_eq!(to_canonical_json_string(&json!(1e21)).unwrap(), "1e+21");
        assert_eq!(to_canonical_json_string(&json!(1e-6)).unwrap(), "0.000001");
        assert_eq!(to_canonical_json_string(&json!(1e-7)).unwrap(), "1e-7");
    }

    #[test]
    fn test_rfc8785_string_escaping_and_unicode() {
        // Accented characters must remain literal UTF-8 (not escaped as \u00e9)
        let json_french = json!({
            "éléphant": "éléphant"
        });
        assert_eq!(
            to_canonical_json_string(&json_french).unwrap(),
            r#"{"éléphant":"éléphant"}"#
        );

        // Solidus / must NEVER be escaped
        let json_url = json!({
            "url": "https://api.sandbox.paypal.com/v1/oauth2/token"
        });
        assert_eq!(
            to_canonical_json_string(&json_url).unwrap(),
            r#"{"url":"https://api.sandbox.paypal.com/v1/oauth2/token"}"#
        );

        // Control characters MUST be escaped
        let json_ctrl = json!({
            "newline": "line1\nline2",
            "tab": "col1\tcol2",
            "null_byte": "\u{0000}"
        });
        assert_eq!(
            to_canonical_json_string(&json_ctrl).unwrap(),
            r#"{"newline":"line1\nline2","null_byte":"\u0000","tab":"col1\tcol2"}"#
        );
    }

    #[test]
    fn test_rfc8785_nested_arrays_and_objects() {
        let json_nested = json!({
            "array": [
                { "z": 9, "a": 1 },
                { "b": 2, "a": 3 }
            ],
            "obj": {
                "inner_b": { "y": 2, "x": 1 },
                "inner_a": [3, 2, 1]
            }
        });

        let canonical = to_canonical_json_string(&json_nested).unwrap();
        assert_eq!(
            canonical,
            r#"{"array":[{"a":1,"z":9},{"a":3,"b":2}],"obj":{"inner_a":[3,2,1],"inner_b":{"x":1,"y":2}}}"#
        );
    }

    #[test]
    fn test_deterministic_hashing() {
        let payload = b"{\"prompt\":\"test prompt\"}";
        let hash1 = compute_event_hash(0, "INITIAL_PROMPT", payload, GENESIS_HASH);
        let hash2 = compute_event_hash(0, "INITIAL_PROMPT", payload, GENESIS_HASH);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
        // Must be valid lowercase hexadecimal
        assert!(hash1
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn test_different_payload_produces_different_hash() {
        let p1 = b"{\"item\":\"A\"}";
        let p2 = b"{\"item\":\"B\"}";

        let h1 = compute_event_hash(1, "ADD_ITEM", p1, GENESIS_HASH);
        let h2 = compute_event_hash(1, "ADD_ITEM", p2, GENESIS_HASH);

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_different_action_produces_different_hash() {
        let p = b"{\"amount\":100}";

        let h1 = compute_event_hash(1, "ACTION_ONE", p, GENESIS_HASH);
        let h2 = compute_event_hash(1, "ACTION_TWO", p, GENESIS_HASH);

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_different_sequence_produces_different_hash() {
        let p = b"{\"amount\":100}";

        let h1 = compute_event_hash(1, "ACTION", p, GENESIS_HASH);
        let h2 = compute_event_hash(2, "ACTION", p, GENESIS_HASH);

        assert_ne!(h1, h2);
    }
}
