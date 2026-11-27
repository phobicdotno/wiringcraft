use std::ffi::OsString;
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
        Self::resolve_in(project_dir, s, cache_root().as_deref())
    }

    /// [`Source::resolve`] with the git cache folder given instead of read from the environment.
    fn resolve_in(project_dir: &Path, s: &LibrarySource, cache: Option<&Path>) -> Result<Self> {
        let root = match s.kind.as_str() {
            "folder" => {
                let p = PathBuf::from(&s.location);
                if p.is_absolute() { p } else { project_dir.join(p) }
            }
            "git" => git_cache_dir(&s.name, cache)?,
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
    cache_root_from(
        std::env::var_os("WIRINGCRAFT_CACHE"),
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
    )
}

/// [`cache_root`] from the given variable values (`home` is `HOME`, else `USERPROFILE`).
fn cache_root_from(wc_cache: Option<OsString>, xdg_cache: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = wc_cache {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = xdg_cache {
        return Some(PathBuf::from(p).join("wiringcraft"));
    }
    Some(PathBuf::from(home?).join(".cache").join("wiringcraft"))
}

fn git_cache_dir(name: &str, cache: Option<&Path>) -> Result<PathBuf> {
    wc_core::store::check_id(name).map_err(|e| Error::Source {
        name: name.into(),
        message: e.to_string(),
    })?;
    let root = cache.ok_or_else(|| Error::Source {
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
    sync_one_in(project_dir, s, cache_root().as_deref())
}

/// [`sync_one`] with the git cache folder given instead of read from the environment.
fn sync_one_in(project_dir: &Path, s: &LibrarySource, cache: Option<&Path>) -> Result<String> {
    match s.kind.as_str() {
        "folder" => {
            let src = Source::resolve_in(project_dir, s, cache)?;
            Ok(if src.root.is_dir() {
                format!("{}: folder {}", s.name, src.root.display())
            } else {
                format!("{}: folder {} is not reachable", s.name, src.root.display())
            })
        }
        "git" => {
            let dir = git_cache_dir(&s.name, cache)?;
            let reference = s.reference.as_deref().unwrap_or("HEAD");
            if dir.join(".git").is_dir() {
                // Fetch from the location as it is now, not the one the cache was cloned from.
                // A local relative path is made absolute, as the clone resolved it.
                let loc = Path::new(&s.location);
                let url = match std::path::absolute(loc) {
                    Ok(abs) if loc.is_relative() && loc.exists() => abs.to_string_lossy().into_owned(),
                    _ => s.location.clone(),
                };
                git(&["remote", "set-url", "origin", &url], Some(&dir), &s.name)?;
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

#[cfg(test)]
mod tests;
