//! `library.lock`: the exact version of every library item a project uses.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Symbol,
    Part,
}

impl ItemKind {
    pub fn dir(self) -> &'static str {
        match self {
            ItemKind::Symbol => "symbols",
            ItemKind::Part => "parts",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockEntry {
    /// Name of the library source the item came from.
    pub source: String,
    /// Content hash of the vendored version.
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(default)]
    pub symbols: BTreeMap<String, LockEntry>,
    #[serde(default)]
    pub parts: BTreeMap<String, LockEntry>,
}

impl Lockfile {
    pub fn get(&self, kind: ItemKind, id: &str) -> Option<&LockEntry> {
        self.map(kind).get(id)
    }

    pub fn map(&self, kind: ItemKind) -> &BTreeMap<String, LockEntry> {
        match kind {
            ItemKind::Symbol => &self.symbols,
            ItemKind::Part => &self.parts,
        }
    }

    pub fn map_mut(&mut self, kind: ItemKind) -> &mut BTreeMap<String, LockEntry> {
        match kind {
            ItemKind::Symbol => &mut self.symbols,
            ItemKind::Part => &mut self.parts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str, hash: &str) -> LockEntry {
        LockEntry {
            source: source.to_string(),
            hash: hash.to_string(),
        }
    }

    #[test]
    fn kind_dirs_match_library_folders() {
        assert_eq!(ItemKind::Symbol.dir(), "symbols");
        assert_eq!(ItemKind::Part.dir(), "parts");
    }

    #[test]
    fn kind_serializes_lowercase() {
        let Ok(text) = serde_json::to_string(&[ItemKind::Symbol, ItemKind::Part]) else {
            unreachable!()
        };
        assert_eq!(text, r#"["symbol","part"]"#);
        let Ok(back) = serde_json::from_str::<ItemKind>(r#""part""#) else {
            unreachable!()
        };
        assert_eq!(back, ItemKind::Part);
        assert!(serde_json::from_str::<ItemKind>(r#""Part""#).is_err());
    }

    #[test]
    fn get_looks_only_in_the_map_of_its_kind() {
        let mut lock = Lockfile::default();
        lock.map_mut(ItemKind::Symbol)
            .insert("relay".into(), entry("std", "aa"));
        assert_eq!(lock.get(ItemKind::Symbol, "relay"), Some(&entry("std", "aa")));
        assert_eq!(lock.get(ItemKind::Part, "relay"), None);
        assert!(lock.parts.is_empty());
        assert_eq!(lock.map(ItemKind::Symbol).len(), 1);
    }

    #[test]
    fn same_id_can_be_pinned_as_symbol_and_part() {
        let mut lock = Lockfile::default();
        lock.map_mut(ItemKind::Symbol).insert("k1".into(), entry("std", "11"));
        lock.map_mut(ItemKind::Part).insert("k1".into(), entry("vendor", "22"));
        assert_eq!(lock.get(ItemKind::Symbol, "k1"), Some(&entry("std", "11")));
        assert_eq!(lock.get(ItemKind::Part, "k1"), Some(&entry("vendor", "22")));
    }

    #[test]
    fn empty_or_partial_toml_defaults_missing_tables() {
        let Ok(empty) = toml::from_str::<Lockfile>("") else {
            unreachable!()
        };
        assert_eq!(empty, Lockfile::default());
        let text = "[parts.contactor]\nsource = \"std\"\nhash = \"beef\"\n";
        let Ok(partial) = toml::from_str::<Lockfile>(text) else {
            unreachable!()
        };
        assert!(partial.symbols.is_empty());
        assert_eq!(partial.get(ItemKind::Part, "contactor"), Some(&entry("std", "beef")));
    }

    #[test]
    fn entry_without_hash_is_refused() {
        let text = "[symbols.relay]\nsource = \"std\"\n";
        assert!(toml::from_str::<Lockfile>(text).is_err());
    }

    #[test]
    fn toml_round_trip_is_sorted_and_stable() {
        let mut lock = Lockfile::default();
        for (id, hash) in [("zeta", "03"), ("alpha", "01"), ("mid", "02")] {
            lock.map_mut(ItemKind::Symbol).insert(id.into(), entry("std", hash));
        }
        lock.map_mut(ItemKind::Part)
            .insert("p/with.dots".into(), entry("vendor", "ff"));
        let Ok(text) = toml::to_string_pretty(&lock) else {
            unreachable!()
        };
        let (Some(a), Some(m), Some(z)) = (
            text.find("[symbols.alpha]"),
            text.find("[symbols.mid]"),
            text.find("[symbols.zeta]"),
        ) else {
            unreachable!("missing table in {text}")
        };
        assert!(a < m && m < z, "ids not sorted: {text}");
        let Ok(back) = toml::from_str::<Lockfile>(&text) else {
            unreachable!()
        };
        assert_eq!(back, lock);
        let Ok(again) = toml::to_string_pretty(&back) else {
            unreachable!()
        };
        assert_eq!(again, text);
    }
}
