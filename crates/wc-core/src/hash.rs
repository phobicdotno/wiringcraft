//! Content hashes for library items: the identity of one version of a symbol or part.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// SHA-256 of the canonical JSON form, as `sha256:<first 16 hex digits>`.
///
/// Canonical means `serde_json::Value` with sorted object keys and no whitespace, so the hash
/// does not change when a file is reformatted.
pub fn content_hash<T: Serialize>(value: &T) -> String {
    let canonical = serde_json::to_value(value).map(|v| v.to_string()).unwrap_or_default();
    let digest = Sha256::digest(canonical.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_ignores_key_order() {
        let a: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":[1,2]}"#).unwrap_or_default();
        let b: serde_json::Value = serde_json::from_str(r#"{ "b": [1,2], "a": 1 }"#).unwrap_or_default();
        assert_eq!(content_hash(&a), content_hash(&b));
        assert!(content_hash(&a).starts_with("sha256:"));
    }
}
