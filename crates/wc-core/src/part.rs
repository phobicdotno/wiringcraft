//! Parts: orderable articles with commercial and technical data.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hash::content_hash;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Part {
    /// Stable library id, usually `<manufacturer>.<order number>` normalised.
    pub id: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub order_number: String,
    #[serde(default)]
    pub description: String,
    /// Symbol id this part is usually drawn with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// Free technical properties: voltage, current, width, ...
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

impl Part {
    pub fn hash(&self) -> String {
        content_hash(self)
    }

    /// A file- and id-safe form of `manufacturer.order_number`.
    pub fn make_id(manufacturer: &str, order_number: &str) -> String {
        let norm = |s: &str| -> String {
            s.trim()
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>()
                .trim_matches('-')
                .to_owned()
        };
        let m = norm(manufacturer);
        let o = norm(order_number);
        if m.is_empty() { o } else { format!("{m}.{o}") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_safe() {
        assert_eq!(Part::make_id("Acme Corp", "AB 12/34"), "acme-corp.ab-12-34");
        assert_eq!(Part::make_id("", "X1"), "x1");
    }
}
