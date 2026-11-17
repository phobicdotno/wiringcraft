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
    /// Light or dark interface.
    pub theme: crate::theme::Mode,
    pub show_grid: bool,
    /// Snap step in mm.
    pub snap: f64,
    pub recent: Vec<PathBuf>,
    /// The assistant panel is shown.
    pub show_assistant: bool,
    /// The assistant's model, effort and (when not in the environment) API key.
    pub assistant: wc_agent::Settings,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            chrome: Chrome::Classic,
            theme: crate::theme::Mode::Light,
            show_grid: true,
            snap: 2.5,
            recent: Vec::new(),
            show_assistant: false,
            assistant: wc_agent::Settings::default(),
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("wiringcraft"))
}

/// Where new projects go by default: `Documents/WiringCraft` in the user's home folder (the
/// home folder itself when it has no Documents), or the current folder when there is no home.
pub fn projects_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    match home {
        Some(h) if h.join("Documents").is_dir() => h.join("Documents").join("WiringCraft"),
        Some(h) => h.join("WiringCraft"),
        None => PathBuf::from("WiringCraft"),
    }
}

/// A folder typed into a dialog: a relative one (just `test`) is taken inside
/// [`projects_dir`], never the folder the app happened to start in (on Windows that can be
/// `C:\Windows\System32` or the install folder).
pub fn project_path(typed: &str) -> PathBuf {
    let p = PathBuf::from(typed.trim());
    if p.is_absolute() { p } else { projects_dir().join(p) }
}

fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("prefs.toml"))
}

/// WiringCraft's own folder for Claude Code to start in. Never the project folder: a project
/// from someone else could carry settings (`.npmrc`, `.claude/`) that run code when the
/// connector or Claude Code starts there. The assistant reaches the project only through
/// WiringCraft's tools, so it needs no folder of its own.
pub fn assistant_dir() -> Result<PathBuf, String> {
    let dir = config_dir()
        .map(|d| d.join("assistant"))
        .ok_or_else(|| "No folder for WiringCraft's settings (HOME is not set).".to_owned())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not make {}: {e}", dir.display()))?;
    let dir = dir.canonicalize().or_else(|_| std::path::absolute(&dir)).unwrap_or(dir);
    Ok(dir)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_name_goes_into_the_projects_folder_and_a_full_path_stays() {
        assert_eq!(project_path(" test "), projects_dir().join("test"));
        let full = std::env::temp_dir().join("wc-elsewhere");
        assert_eq!(project_path(&full.display().to_string()), full);
    }
}
