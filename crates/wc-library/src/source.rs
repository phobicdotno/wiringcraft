use std::path::{Path, PathBuf};
use std::process::Command;

use wc_core::lock::ItemKind;
use wc_core::project::LibrarySource;

use crate::{Error, Result};

/// A library source resolved to a folder on this computer.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub name: String,
    pub root: PathBuf,
}

impl Source {
    pub fn resolve(project_dir: &Path, s: &LibrarySource) -> Result<Self> {
        let root = match s.kind.as_str() {
            "folder" => {
                let p = PathBuf::from(&s.location);
                if p.is_absolute() { p } else { project_dir.join(p) }
            }
            "git" => git_cache_dir(&s.name)?,
            other => {
                return Err(Error::Source {
                    name: s.name.clone(),
                    message: format!("unknown kind {other:?} (folder, git)"),
                });
            }
        };
        Ok(Self {
            name: s.name.clone(),
            root,
        })
    }

    /// Ids of all items of a kind in this source, sorted.
    pub fn list(&self, kind: ItemKind) -> Vec<String> {
        let mut ids: Vec<String> = std::fs::read_dir(self.root.join(kind.dir()))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_suffix(".json"))
                    .map(str::to_owned)
            })
            .collect();
        ids.sort();
        ids
    }
}

/// Per-user cache for git sources: `$WIRINGCRAFT_CACHE`, else `$XDG_CACHE_HOME/wiringcraft`,
/// else `~/.cache/wiringcraft`.
pub fn cache_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WIRINGCRAFT_CACHE") {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(p).join("wiringcraft"));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".cache").join("wiringcraft"))
}

fn git_cache_dir(name: &str) -> Result<PathBuf> {
    wc_core::store::check_id(name).map_err(|e| Error::Source {
        name: name.into(),
        message: e.to_string(),
    })?;
    let root = cache_root().ok_or_else(|| Error::Source {
        name: name.into(),
        message: "no cache folder (set WIRINGCRAFT_CACHE)".into(),
    })?;
    Ok(root.join("git").join(name))
}

fn git(args: &[&str], cwd: Option<&Path>, name: &str) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let out = cmd.output().map_err(|e| Error::Source {
        name: name.into(),
        message: format!("cannot run git: {e}"),
    })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Source {
            name: name.into(),
            message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

pub(crate) fn sync_one(project_dir: &Path, s: &LibrarySource) -> Result<String> {
    match s.kind.as_str() {
        "folder" => {
            let src = Source::resolve(project_dir, s)?;
            Ok(if src.root.is_dir() {
                format!("{}: folder {}", s.name, src.root.display())
            } else {
                format!("{}: folder {} is not reachable", s.name, src.root.display())
            })
        }
        "git" => {
            let dir = git_cache_dir(&s.name)?;
            let reference = s.reference.as_deref().unwrap_or("HEAD");
            if dir.join(".git").is_dir() {
                git(&["fetch", "--depth", "1", "origin", reference], Some(&dir), &s.name)?;
                git(&["reset", "--hard", "FETCH_HEAD"], Some(&dir), &s.name)?;
                Ok(format!("{}: updated from {}", s.name, s.location))
            } else {
                if let Some(parent) = dir.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| Error::Core(wc_core::Error::io(parent, e)))?;
                }
                let dir_s = dir.to_string_lossy().into_owned();
                let mut args = vec!["clone", "--depth", "1"];
                if let Some(r) = s.reference.as_deref() {
                    args.extend(["--branch", r]);
                }
                args.extend([s.location.as_str(), dir_s.as_str()]);
                git(&args, None, &s.name)?;
                Ok(format!("{}: cloned {}", s.name, s.location))
            }
        }
        other => Err(Error::Source {
            name: s.name.clone(),
            message: format!("unknown kind {other:?}"),
        }),
    }
}
