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

/// A source location as it may be shown: the user name and password of a URL are replaced by
/// `***`, and so are its query and fragment, which can hold a token:
/// `https://user:token@host/path?key=x#f` is shown as `https://***@host/path?***`.
pub fn redact_location(location: &str) -> String {
    let Some((scheme, rest)) = location.split_once("://") else {
        return location.to_owned();
    };
    let (userinfo, host_path, secret) = split_url(rest);
    let userinfo = if userinfo.is_some() { "***@" } else { "" };
    let tail = match secret.chars().next() {
        Some(mark) => format!("{mark}***"),
        None => String::new(),
    };
    format!("{scheme}://{userinfo}{host_path}{tail}")
}

/// The part of a URL after `scheme://` split into its user name and password (without the `@`;
/// `None` when there are none), its host and path, and its query and fragment (from the first
/// `?` or `#`, empty when there is none).
fn split_url(rest: &str) -> (Option<&str>, &str, &str) {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let (userinfo, host_path) = match authority.rsplit_once('@') {
        Some((userinfo, _)) => (Some(userinfo), rest.get(userinfo.len() + 1..).unwrap_or_default()),
        None => (None, rest),
    };
    let (host_path, secret) = match host_path.find(['?', '#']) {
        Some(at) => host_path.split_at_checked(at).unwrap_or((host_path, "")),
        None => (host_path, ""),
    };
    (userinfo, host_path, secret)
}

/// `text` with every form of `location` that shows a secret replaced by [`redact_location`]:
/// the location as given, and (when it has a query or fragment) the location without its user
/// name and password and its bare host and path, as git may print it. The user name and
/// password are then replaced by `***` wherever they are still followed by `@` (see
/// [`mask_before_at`]), as in git's credential prompts (`https://user@host`), which leave out
/// the path and the password.
fn redact_text(text: &str, location: &str) -> String {
    let shown = redact_location(location);
    let mut text = text.replace(location, &shown);
    if let Some((scheme, rest)) = location.split_once("://") {
        let (userinfo, host_path, secret) = split_url(rest);
        if !secret.is_empty() {
            let bare = format!("{host_path}{secret}");
            text = text
                .replace(&format!("{scheme}://{bare}"), &shown)
                .replace(&bare, &shown);
        }
        if let Some(userinfo) = userinfo {
            text = mask_before_at(&text, &credential_forms(userinfo));
        }
    }
    text
}

/// The ways a URL's `user:password` may be printed: the user name and password together, then
/// the user name alone, each as configured, percent-encoded (every character outside RFC 3986's
/// unreserved set as `%XX`, with uppercase and with lowercase hex) and percent-decoded.
fn credential_forms(userinfo: &str) -> Vec<String> {
    let parts: Vec<&str> = match userinfo.split_once(':') {
        Some((user, password)) => vec![user, password],
        None => vec![userinfo],
    };
    let ways: [fn(&str) -> String; 4] = [
        str::to_owned,
        |s| percent_encode(s, true),
        |s| percent_encode(s, false),
        percent_decode,
    ];
    let mut forms = Vec::new();
    for way in ways {
        forms.push(parts.iter().map(|p| way(p)).collect::<Vec<_>>().join(":"));
    }
    if let Some(user) = parts.first().filter(|_| parts.len() > 1) {
        forms.extend(ways.iter().map(|way| way(user)));
    }
    forms
}

/// `s` with every byte outside RFC 3986's unreserved characters (`A-Z a-z 0-9 - . _ ~`) written
/// as `%XX`, with uppercase or lowercase hex.
fn percent_encode(s: &str, upper: bool) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                char::from(b).to_string()
            } else if upper {
                format!("%{b:02X}")
            } else {
                format!("%{b:02x}")
            }
        })
        .collect()
}

/// `s` with each `%XX` turned back into its byte (left as it is where it is not valid hex, or
/// when the result is not UTF-8).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match hex.filter(|_| b == b'%').and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(decoded) => {
                out.push(decoded);
                i += 3;
            }
            None => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// `text` with each `secret` that is followed by `@` replaced by `***@`, in one pass from left
/// to right: at each place the longest secret that matches is taken, so the `***` put in is
/// never looked at again. An empty secret hides nothing and is not looked for.
fn mask_before_at(text: &str, secrets: &[String]) -> String {
    let mut forms: Vec<String> = secrets
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| format!("{s}@"))
        .collect();
    forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(form) = forms.iter().find(|f| rest.starts_with(f.as_str())) {
            out.push_str("***@");
            rest = rest.get(form.len()..).unwrap_or_default();
        } else {
            out.push(c);
            rest = rest.get(c.len_utf8()..).unwrap_or_default();
        }
    }
    out
}

/// Sync every source in order, one outcome each, going on past a source that fails.
/// Two sources with the same name would share one cache folder, so a list that has them is
/// refused before anything is synced.
pub(crate) fn sync_all(project_dir: &Path, sources: &[LibrarySource]) -> Result<Vec<Result<String>>> {
    sync_all_in(project_dir, sources, cache_root().as_deref())
}

/// [`sync_all`] with the git cache folder given instead of read from the environment.
fn sync_all_in(project_dir: &Path, sources: &[LibrarySource], cache: Option<&Path>) -> Result<Vec<Result<String>>> {
    check_unique_names(sources)?;
    Ok(sources.iter().map(|s| sync_one_in(project_dir, s, cache)).collect())
}

/// Refuses two sources whose names are the same, ignoring ASCII case (the cache folder is named
/// after the source, and Windows and macOS file names ignore case).
fn check_unique_names(sources: &[LibrarySource]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for s in sources {
        if !seen.insert(s.name.to_ascii_lowercase()) {
            return Err(Error::Source {
                name: s.name.clone(),
                message: "two library sources have this name; give each source its own name".into(),
            });
        }
    }
    Ok(())
}

/// Sync one source. A failure's message has the location redacted as [`redact_location`] does.
fn sync_one_in(project_dir: &Path, s: &LibrarySource, cache: Option<&Path>) -> Result<String> {
    sync_one_raw(project_dir, s, cache).map_err(|e| match e {
        Error::Source { name, message } if s.location.contains("://") => Error::Source {
            name,
            message: redact_text(&message, &s.location),
        },
        other => other,
    })
}

/// Sync one source, with the git cache folder given instead of read from the environment.
fn sync_one_raw(project_dir: &Path, s: &LibrarySource, cache: Option<&Path>) -> Result<String> {
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
                Ok(format!("{}: updated from {}", s.name, redact_location(&s.location)))
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
                Ok(format!("{}: cloned {}", s.name, redact_location(&s.location)))
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
