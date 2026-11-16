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
