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
    /// Draw wires between pins that line up when a symbol is placed or moved.
    pub auto_wire: bool,
    /// Snap step in mm.
    pub snap: f64,
    pub recent: Vec<PathBuf>,
    /// The assistant panel is shown.
    pub show_assistant: bool,
    /// The assistant's model, effort and (when not in the environment) API key.
    pub assistant: wc_agent::Settings,
    /// Editor layers this user hid or locked on the canvas.
    pub layers: crate::layers::Layers,
    /// Run the project check again after each edit and mark what it finds on the canvas.
    pub live_check: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            chrome: Chrome::Classic,
            theme: crate::theme::Mode::Light,
            show_grid: true,
            auto_wire: true,
            snap: 2.5,
            recent: Vec::new(),
            show_assistant: false,
            assistant: wc_agent::Settings::default(),
            layers: crate::layers::Layers::default(),
            live_check: true,
        }
    }
}

pub(crate) fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("wiringcraft"))
}

/// The user's home folder, as the system knows it.
fn home_dir() -> Option<PathBuf> {
    dirs::home_dir().filter(|h| h.is_absolute())
}

/// Where new projects go by default: `WiringCraft` in the user's real Documents folder (where
/// OneDrive moved it, or its localized name on Linux), else in `Documents` or the home folder,
/// or the current folder when there is no home.
pub fn projects_dir() -> PathBuf {
    let documents = dirs::document_dir().filter(|d| d.is_absolute() && d.is_dir());
    projects_dir_in(documents.as_deref(), home_dir().as_deref())
}

fn projects_dir_in(documents: Option<&Path>, home: Option<&Path>) -> PathBuf {
    match (documents, home) {
        (Some(d), _) => d.join("WiringCraft"),
        (None, Some(h)) if h.join("Documents").is_dir() => h.join("Documents").join("WiringCraft"),
        (None, Some(h)) => h.join("WiringCraft"),
        (None, None) => absolute(Path::new("WiringCraft")).unwrap_or_else(|e| {
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
pub fn new_project_dir(location: &Path, name: &str) -> Result<PathBuf, String> {
    let leaf = if name.trim().is_empty() {
        "new-project".to_owned()
    } else {
        folder_name(name).ok_or_else(|| format!("\"{}\" cannot be a folder name.", name.trim()))?
    };
    Ok(location.join(leaf))
}

/// A folder field in a dialog: what is typed, and the folder last picked with Browse (or from
/// the recent list). A picked folder is used exactly as picked, spaces and all; typed text
/// follows [`project_path`]. Editing the text after picking makes it typed again.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderField {
    pub text: String,
    picked: Option<PathBuf>,
}

impl FolderField {
    pub fn typed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            picked: None,
        }
    }

    /// Take a folder from the picker. A name that is not valid text (possible on Linux) cannot
    /// be shown in the field without changing it, so it is refused with the reason.
    pub fn pick(&mut self, dir: PathBuf) -> Result<(), String> {
        let text = dir
            .to_str()
            .ok_or_else(|| {
                format!(
                    "The folder name {} is not valid text; rename it or type another.",
                    dir.display()
                )
            })?
            .to_owned();
        self.text = text;
        self.picked = Some(dir);
        Ok(())
    }

    /// The folder this field means now, or why typed text cannot be used.
    pub fn path(&self) -> Result<PathBuf, String> {
        match &self.picked {
            Some(p) if p.to_str() == Some(self.text.as_str()) => Ok(p.clone()),
            _ => project_path(&self.text),
        }
    }

    /// Nothing is in the field. A folder picked earlier does not count once its text is gone.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// A folder typed into a dialog: a relative one (just `test`) is taken inside
/// [`projects_dir`], never the folder the app happened to start in (on Windows that can be
/// `C:\Windows\System32` or the install folder). `~` at the start is the home folder. On
/// Windows a path that still depends on the current folder (`C:foo`, `\foo`) is refused with
/// the reason, since the dialogs cannot know which folder that would be.
pub fn project_path(typed: &str) -> Result<PathBuf, String> {
    resolve_typed(typed, home_dir().as_deref(), cfg!(windows), &projects_dir())
}

fn resolve_typed(typed: &str, home: Option<&Path>, windows: bool, projects: &Path) -> Result<PathBuf, String> {
    let t = typed.trim();
    let sep = |c: char| c == '/' || (windows && c == '\\');
    if let Some(rest) = t.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with(sep))
    {
        let home = home.ok_or("~ means the home folder, and this system has none; type a full path.")?;
        let rest = rest.trim_start_matches(sep);
        // `~\C:foo` would join as `C:foo` and leave the home folder behind.
        let b = rest.as_bytes();
        if windows && b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return Err(format!(
                "{t} names a drive after ~; type either ~\\folder or a full path."
            ));
        }
        return Ok(if rest.is_empty() {
            home.to_path_buf()
        } else {
            home.join(rest)
        });
    }
    if windows {
        let b = t.as_bytes();
        if b.len() >= 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && !b.get(2).is_some_and(|&c| c == b'\\' || c == b'/')
        {
            let drive = char::from(b[0]).to_ascii_uppercase();
            return Err(format!(
                "{t} is relative to the current folder on drive {drive}:, which can be anything. Type a full path, such as {drive}:\\{}.",
                &t[2..]
            ));
        }
        let unc = t.starts_with("\\\\") || t.starts_with("//");
        if t.starts_with(['\\', '/']) && !unc {
            return Err(format!(
                "{t} has no drive, so it means the current drive, which can be anything. Type the drive too, such as C:{t}."
            ));
        }
    }
    let p = PathBuf::from(t);
    Ok(if p.is_absolute() { p } else { projects.join(p) })
}

#[cfg(not(test))]
fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("prefs.toml"))
}

/// Tests never read or write the settings of the person running them: opening a project
/// remembers it and saves the preferences.
#[cfg(test)]
fn path() -> Option<PathBuf> {
    None
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
        // A test's preferences never replace the ones of whoever runs the tests.
        if cfg!(test) {
            return;
        }
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
        assert_eq!(project_path(&shown), Ok(projects_dir()));
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
        let l = FolderField::typed(l).path().unwrap();
        assert_eq!(new_project_dir(&l, "").unwrap(), loc.join("new-project"));
        assert_eq!(new_project_dir(&l, "Motor").unwrap(), loc.join("Motor"));
        assert_eq!(new_project_dir(&l, "../Motor").unwrap(), loc.join("-Motor"));
        assert!(new_project_dir(&l, "..").is_err());
    }

    #[test]
    fn a_picked_folder_is_used_exactly_and_typing_makes_it_typed_again() {
        let picked = std::env::temp_dir().join("Motor ");
        let mut f = FolderField::typed("");
        f.pick(picked.clone()).unwrap();
        assert_eq!(f.path(), Ok(picked.clone()), "the trailing space stays");
        f.text.push('x');
        assert_eq!(f.path(), project_path(&f.text), "edited text follows the typed rule");
        // Browse, then clear the field: it is empty again, so Create is off.
        f.pick(picked).unwrap();
        assert!(!f.is_empty());
        f.text.clear();
        assert!(f.is_empty(), "a cleared field is empty even after a pick");
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_name_that_is_not_text_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let odd = std::env::temp_dir().join(std::ffi::OsStr::from_bytes(b"bad\xff"));
        let mut f = FolderField::typed("keep");
        assert!(f.pick(odd).is_err());
        assert_eq!(f.text, "keep", "the field is left as it was");
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
        assert_eq!(project_path(" test "), Ok(projects_dir().join("test")));
        let full = std::env::temp_dir().join("wc-elsewhere");
        assert_eq!(project_path(&full.display().to_string()), Ok(full));
    }

    #[test]
    fn a_tilde_is_the_home_folder() {
        let home = std::env::temp_dir().join("wc-home");
        let projects = std::env::temp_dir().join("wc-projects");
        let r = |t: &str, windows: bool| resolve_typed(t, Some(&home), windows, &projects);
        assert_eq!(r("~", false), Ok(home.clone()));
        assert_eq!(r(" ~/work/plant ", false), Ok(home.join("work/plant")));
        assert_eq!(r("~\\work", true), Ok(home.join("work")));
        assert!(r("~\\C:foo", true).is_err());
        assert!(r("~/d:\\x", true).is_err());
        // Only `~` alone or before a separator: `~work` is an ordinary name.
        assert_eq!(r("~work", false), Ok(projects.join("~work")));
        assert!(resolve_typed("~/x", None, false, &projects).is_err());
    }

    #[test]
    fn windows_paths_that_depend_on_the_current_folder_are_refused() {
        let projects = std::env::temp_dir().join("wc-projects");
        let r = |t: &str| resolve_typed(t, None, true, &projects);
        let e = r("C:foo").unwrap_err();
        assert!(e.contains("drive C:") && e.contains("C:\\foo"), "{e}");
        assert!(r("d:").unwrap_err().contains("drive D:"));
        let e = r("\\foo").unwrap_err();
        assert!(e.contains("C:\\foo"), "{e}");
        assert!(r("/foo").is_err());
        // A full path, a share and a plain name are fine.
        assert!(r("C:\\foo").is_ok());
        assert!(r("\\\\server\\share").is_ok());
        assert_eq!(r("plant"), Ok(projects.join("plant")));
        // Elsewhere `C:foo` is an ordinary name and `/foo` a full path.
        assert_eq!(
            resolve_typed("C:foo", None, false, &projects),
            Ok(projects.join("C:foo"))
        );
    }

    #[test]
    fn the_projects_folder_follows_the_real_documents_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let moved = tmp.path().join("OneDrive").join("Dokumenter");
        let home = tmp.path().join("home");
        assert_eq!(projects_dir_in(Some(&moved), Some(&home)), moved.join("WiringCraft"));
        // No Documents known: `Documents` in the home folder when it is there, else the home.
        assert_eq!(projects_dir_in(None, Some(&home)), home.join("WiringCraft"));
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        assert_eq!(
            projects_dir_in(None, Some(&home)),
            home.join("Documents").join("WiringCraft")
        );
        assert!(projects_dir_in(None, None).is_absolute());
    }
}
