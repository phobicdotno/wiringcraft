//! User preferences, stored as TOML in the user's config folder.

use std::path::{Path, PathBuf};

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
        None => absolute(Path::new("WiringCraft")).unwrap_or_else(|e| {
            eprintln!("wiringcraft-app: no home folder, and {e}; new projects go in ./WiringCraft");
            PathBuf::from("WiringCraft")
        }),
    }
}

/// `p` as an absolute path (against the current folder), without touching the disk. Fails for
/// an empty path or when the current folder cannot be read.
pub fn absolute(p: &Path) -> Result<PathBuf, String> {
    std::path::absolute(p).map_err(|e| format!("{} cannot be made a full path: {e}", p.display()))
}

/// A project name as one folder name: separators and characters Windows does not allow become
/// `-`, and leading or trailing dots and spaces go. `None` when nothing usable is left (empty,
/// `.` or `..`), so a name can never point outside the chosen location.
pub fn folder_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

/// The folder a new project goes in: `location` (resolved like any typed folder) with the
/// project name as the last folder, or `new-project` when there is no name yet.
pub fn new_project_dir(location: &str, name: &str) -> Result<PathBuf, String> {
    let leaf = if name.trim().is_empty() {
        "new-project".to_owned()
    } else {
        folder_name(name).ok_or_else(|| format!("\"{}\" cannot be a folder name.", name.trim()))?
    };
    Ok(project_path(location).join(leaf))
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

    /// Put `dir` first in the recent projects. Kept absolute, so a recent entry means the same
    /// folder however the app was started; a folder that cannot be made absolute is not kept.
    pub fn remember(&mut self, dir: PathBuf) {
        let dir = match absolute(&dir) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("wiringcraft-app: not kept in the recent projects: {e}");
                return;
            }
        };
        self.recent.retain(|d| d != &dir);
        self.recent.insert(0, dir);
        self.recent.truncate(8);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_default_projects_folder_is_absolute() {
        assert!(projects_dir().is_absolute());
        // Shown as the default location, then typed back: the same folder, not nested twice.
        let shown = projects_dir().display().to_string();
        assert_eq!(project_path(&shown), projects_dir());
    }

    #[test]
    fn a_project_name_is_one_safe_folder_name() {
        assert_eq!(folder_name(" Motor control ").as_deref(), Some("Motor control"));
        assert_eq!(folder_name("../Motor").as_deref(), Some("-Motor"));
        assert_eq!(folder_name("/tmp/Motor").as_deref(), Some("-tmp-Motor"));
        assert_eq!(folder_name("a:b*c").as_deref(), Some("a-b-c"));
        assert_eq!(folder_name(".."), None);
        assert_eq!(folder_name("  "), None);
    }

    #[test]
    fn the_new_project_folder_follows_the_name_whenever_it_is_typed() {
        let loc = std::env::temp_dir().join("wc-loc");
        let l = loc.display().to_string();
        assert_eq!(new_project_dir(&l, "").unwrap(), loc.join("new-project"));
        assert_eq!(new_project_dir(&l, "Motor").unwrap(), loc.join("Motor"));
        assert_eq!(new_project_dir(&l, "../Motor").unwrap(), loc.join("-Motor"));
        assert!(new_project_dir(&l, "..").is_err());
    }

    #[test]
    fn recent_projects_are_kept_absolute() {
        let mut p = Prefs::default();
        p.remember(PathBuf::from("projects/demo"));
        assert!(p.recent[0].is_absolute());
        // An empty path cannot be made absolute: it is left out, with no relative entry kept.
        p.remember(PathBuf::new());
        assert_eq!(p.recent.len(), 1);
        assert!(absolute(Path::new("")).is_err());
    }

    #[test]
    fn a_typed_name_goes_into_the_projects_folder_and_a_full_path_stays() {
        assert_eq!(project_path(" test "), projects_dir().join("test"));
        let full = std::env::temp_dir().join("wc-elsewhere");
        assert_eq!(project_path(&full.display().to_string()), full);
    }
}
