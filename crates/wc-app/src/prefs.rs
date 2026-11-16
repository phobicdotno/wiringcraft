//! User preferences, stored as TOML in the user's config folder.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How commands are presented: classic menus with icon toolbars (default), or a ribbon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chrome {
    #[default]
    Classic,
    Ribbon,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub chrome: Chrome,
    pub show_grid: bool,
    /// Snap step in mm.
    pub snap: f64,
    pub recent: Vec<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            chrome: Chrome::Classic,
            show_grid: true,
            snap: 2.5,
            recent: Vec::new(),
        }
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("wiringcraft").join("prefs.toml"))
}

impl Prefs {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = wc_core::store::write_if_changed(&p, &text);
        }
    }

    pub fn remember(&mut self, dir: PathBuf) {
        self.recent.retain(|d| d != &dir);
        self.recent.insert(0, dir);
        self.recent.truncate(8);
    }
}
