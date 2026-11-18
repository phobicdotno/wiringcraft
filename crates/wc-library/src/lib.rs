//! The global library: stacked sources of symbols and parts, pinned per project.
//!
//! A source is a folder with `symbols/*.json` and `parts/*.json`. It can be a local or network
//! folder, or a git repository that [`sync`] clones into a cache. Sources listed first in
//! `project.toml` win when two sources hold the same id.
//!
//! A project never reads the library directly while drawing: it reads its own `vendor/` copies,
//! pinned by `library.lock`. [`status`] compares those pins with the sources, and [`update`]
//! takes newer versions on request. That keeps projects stable, reproducible and offline-safe.

mod diff;
mod source;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wc_core::lock::ItemKind;
use wc_core::{LockEntry, Part, Project, Symbol, store};

pub use diff::{diff_parts, diff_symbols};
pub use source::{Source, cache_root};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] wc_core::Error),
    #[error("library source {name:?}: {message}")]
    Source { name: String, message: String },
    #[error("{kind:?} {id:?} is not in any library source")]
    NotFound { kind: ItemKind, id: String },
    /// An update failed and could not put back every file it had changed.
    #[error("{error}; putting back the files the update changed failed too: {failed}")]
    NotPutBack { error: Box<Error>, failed: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Where one item version lives.
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    pub source: String,
    pub path: PathBuf,
}

/// All sources of a project, opened (read from disk; no network).
#[derive(Debug, Clone)]
pub struct Library {
    pub sources: Vec<Source>,
}

impl Library {
    /// Open the sources listed in the project. Git sources must have been synced once.
    pub fn open(project_dir: &Path, project: &Project) -> Result<Self> {
        let sources = project
            .meta
            .libraries
            .iter()
            .map(|s| Source::resolve(project_dir, s))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { sources })
    }

    /// The first source holding `id` wins.
    pub fn locate(&self, kind: ItemKind, id: &str) -> Option<Located> {
        store::check_id(id).ok()?;
        self.sources.iter().find_map(|s| {
            let path = s.root.join(kind.dir()).join(format!("{id}.json"));
            path.is_file().then(|| Located {
                source: s.name.clone(),
                path,
            })
        })
    }

    pub fn symbol(&self, id: &str) -> Result<(Symbol, String)> {
        let loc = self.locate(ItemKind::Symbol, id).ok_or_else(|| Error::NotFound {
            kind: ItemKind::Symbol,
            id: id.into(),
        })?;
        let s: Symbol = store::read_json(&loc.path)?;
        Ok((s, loc.source))
    }

    pub fn part(&self, id: &str) -> Result<(Part, String)> {
        let loc = self.locate(ItemKind::Part, id).ok_or_else(|| Error::NotFound {
            kind: ItemKind::Part,
            id: id.into(),
        })?;
        let p: Part = store::read_json(&loc.path)?;
        Ok((p, loc.source))
    }

    /// Every item id per kind, with the source that wins for it.
    pub fn index(&self, kind: ItemKind) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for s in self.sources.iter().rev() {
            for id in s.list(kind) {
                out.insert(id, s.name.clone());
            }
        }
        out
    }

    /// Macros in the sources' `macros/` folders: (source name, macro id, file), in source order.
    pub fn macros(&self) -> Vec<(String, String, PathBuf)> {
        self.sources
            .iter()
            .flat_map(|s| {
                wc_core::macros::list_macros(&s.root)
                    .into_iter()
                    .map(|(id, path)| (s.name.clone(), id, path))
            })
            .collect()
    }

    /// Case-insensitive search over symbol and part ids, names and descriptions, and part
    /// manufacturer, order number and properties.
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        let q = query.to_lowercase();
        let mut hits = Vec::new();
        for (id, source) in self.index(ItemKind::Symbol) {
            if let Ok((s, _)) = self.symbol(&id) {
                let hay = format!("{} {} {} {}", s.id, s.name, s.category, s.description).to_lowercase();
                if hay.contains(&q) {
                    hits.push(SearchHit {
                        kind: ItemKind::Symbol,
                        id,
                        source,
                        title: s.name,
                    });
                }
            }
        }
        for (id, source) in self.index(ItemKind::Part) {
            if let Ok((p, _)) = self.part(&id) {
                let props: Vec<String> = p.properties.iter().map(|(k, v)| format!("{k}={v}")).collect();
                let hay = format!(
                    "{} {} {} {} {}",
                    p.id,
                    p.manufacturer,
                    p.order_number,
                    p.description,
                    props.join(" ")
                )
                .to_lowercase();
                if hay.contains(&q) {
                    let title = format!("{} {} {}", p.manufacturer, p.order_number, p.description)
                        .trim()
                        .to_owned();
                    hits.push(SearchHit {
                        kind: ItemKind::Part,
                        id,
                        source,
                        title,
                    });
                }
            }
        }
        hits
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub kind: ItemKind,
    pub id: String,
    pub source: String,
    pub title: String,
}

/// Fetch or refresh every git source into the local cache. Folder sources need nothing.
/// Returns one line of outcome per source.
pub fn sync(project_dir: &Path, project: &Project) -> Result<Vec<String>> {
    project
        .meta
        .libraries
        .iter()
        .map(|s| source::sync_one(project_dir, s))
        .collect()
}

/// How a project's pinned item compares to the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    UpToDate,
    /// The library holds a different version than the project pins.
    Changed,
    /// Used by the project but not pinned yet (for example after a hand edit).
    NotLocked,
    /// Pinned, but no source holds it any more. The vendored copy keeps working.
    GoneFromLibrary,
    /// The library's file for it declares another id, so it cannot be taken (see `changes`).
    /// The vendored copy, if any, keeps working.
    WrongId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemStatus {
    pub kind: ItemKind,
    pub id: String,
    pub state: State,
    pub locked: Option<LockEntry>,
    pub library_hash: Option<String>,
    pub library_source: Option<String>,
    /// Display numbers of the pages using it (symbols) or the device tags (parts).
    pub used_by: Vec<String>,
    /// Human-readable differences between the pinned and the library version.
    pub changes: Vec<String>,
}

/// The library's version of an item: (hash, source, the id its file declares).
fn library_version(lib: &Library, kind: ItemKind, id: &str) -> Option<(String, String, String)> {
    match kind {
        ItemKind::Symbol => lib.symbol(id).ok().map(|(s, src)| (s.hash(), src, s.id)),
        ItemKind::Part => lib.part(id).ok().map(|(p, src)| (p.hash(), src, p.id)),
    }
}

/// Compare every item the project uses or pins with the library.
pub fn status(project_dir: &Path, project: &Project, lib: &Library) -> Vec<ItemStatus> {
    let mut out = Vec::new();
    for kind in [ItemKind::Symbol, ItemKind::Part] {
        let used: std::collections::BTreeSet<String> = match kind {
            ItemKind::Symbol => project.used_symbols(),
            ItemKind::Part => project.used_parts(),
        };
        let mut ids: std::collections::BTreeSet<String> = used.clone();
        ids.extend(project.lock.map(kind).keys().cloned());
        for id in ids {
            let locked = project.lock.get(kind, &id).cloned();
            let lib_v = library_version(lib, kind, &id);
            // A file declaring another id is no version of this item: `take` refuses it, so it
            // is neither an update nor something to pin.
            let wrong_id = lib_v
                .as_ref()
                .filter(|(_, _, inside)| *inside != id)
                .map(|(_, src, inside)| {
                    let what = if kind == ItemKind::Symbol { "symbol" } else { "part" };
                    format!("the {what} file for {id:?} in {src:?} declares id {inside:?}; fix the library file")
                });
            let library_source = lib_v.as_ref().map(|v| v.1.clone());
            let lib_v = lib_v.filter(|_| wrong_id.is_none());
            let state = match (&locked, &lib_v) {
                _ if wrong_id.is_some() => State::WrongId,
                (None, _) => State::NotLocked,
                (Some(_), None) => State::GoneFromLibrary,
                (Some(l), Some((h, _, _))) if &l.hash == h => State::UpToDate,
                _ => State::Changed,
            };
            let used_by = match kind {
                ItemKind::Symbol => project
                    .pages_using_symbol(&id)
                    .iter()
                    .map(|p| p.number.clone())
                    .collect(),
                ItemKind::Part => project
                    .devices
                    .iter()
                    .filter(|(_, d)| d.uses_part(&id))
                    .map(|(t, _)| t.to_string())
                    .chain(
                        project
                            .meta
                            .jumpers
                            .iter()
                            .filter(|j| j.part.as_deref() == Some(id.as_str()))
                            .map(|j| format!("{} jumper {}", j.strip, j.terminals.join("-"))),
                    )
                    .collect(),
            };
            let changes = if state == State::Changed {
                match kind {
                    ItemKind::Symbol => match (store::read_vendored_symbol(project_dir, &id), lib.symbol(&id)) {
                        (Ok(old), Ok((new, _))) => diff_symbols(&old, &new),
                        _ => vec!["vendored copy missing".into()],
                    },
                    ItemKind::Part => match (store::read_vendored_part(project_dir, &id), lib.part(&id)) {
                        (Ok(old), Ok((new, _))) => diff_parts(&old, &new),
                        _ => vec!["vendored copy missing".into()],
                    },
                }
            } else {
                wrong_id.into_iter().collect()
            };
            out.push(ItemStatus {
                kind,
                id,
                state,
                locked,
                library_hash: lib_v.as_ref().map(|v| v.0.clone()),
                library_source,
                used_by,
                changes,
            });
        }
    }
    out
}

/// Copy the library's current version of an item into the project and pin it.
/// Returns the new lock entry.
pub fn take(project_dir: &Path, project: &mut Project, lib: &Library, kind: ItemKind, id: &str) -> Result<LockEntry> {
    let item = fetch(lib, kind, id, None)?;
    store_item(project_dir, project, id, item)
}

/// An item read from the library to be written into a project, with its lock entry.
enum Fetched {
    Symbol(Symbol, LockEntry),
    Part(Part, LockEntry),
}

/// Read item `id` from the library and check that its file holds that id and, when `hash` is
/// given, that it is that version.
fn fetch(lib: &Library, kind: ItemKind, id: &str, hash: Option<&str>) -> Result<Fetched> {
    let moved = |source: &str, what: &str| Error::Source {
        name: source.to_owned(),
        message: format!("the {what} {id} changed in the library while it was being updated; update again"),
    };
    Ok(match kind {
        ItemKind::Symbol => {
            let (s, source) = lib.symbol(id)?;
            same_id(&source, "symbol", id, &s.id)?;
            let h = s.hash();
            if hash.is_some_and(|want| want != h) {
                return Err(moved(&source, "symbol"));
            }
            Fetched::Symbol(s, LockEntry { source, hash: h })
        }
        ItemKind::Part => {
            let (p, source) = lib.part(id)?;
            same_id(&source, "part", id, &p.id)?;
            let h = p.hash();
            if hash.is_some_and(|want| want != h) {
                return Err(moved(&source, "part"));
            }
            Fetched::Part(p, LockEntry { source, hash: h })
        }
    })
}

/// Write a fetched item into the project folder and pin it.
fn store_item(project_dir: &Path, project: &mut Project, id: &str, item: Fetched) -> Result<LockEntry> {
    let (kind, entry) = match item {
        Fetched::Symbol(s, entry) => {
            store::write_vendored_symbol(project_dir, &s)?;
            (ItemKind::Symbol, entry)
        }
        Fetched::Part(p, entry) => {
            store::write_vendored_part(project_dir, &p)?;
            (ItemKind::Part, entry)
        }
    };
    project.lock.map_mut(kind).insert(id.to_owned(), entry.clone());
    Ok(entry)
}

/// A library file must hold the item it is named after: the project stores the copy under the
/// id inside it, so another id would write over that item's copy and pin a file that is not
/// there.
fn same_id(source: &str, kind: &str, wanted: &str, inside: &str) -> Result<()> {
    if wanted == inside {
        return Ok(());
    }
    Err(Error::Source {
        name: source.to_owned(),
        message: format!("the {kind} file for {wanted:?}{DECLARES}{inside:?}"),
    })
}

/// The part of [`same_id`]'s message that marks it.
const DECLARES: &str = " declares id ";

/// Whether `take` refused a library file because it declares another id.
fn is_wrong_id(e: &Error) -> bool {
    matches!(e, Error::Source { message, .. } if message.contains(DECLARES))
}

/// What [`lock_missing`] could not pin.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Unpinned {
    /// Used items that no library source holds.
    pub missing: Vec<(ItemKind, String)>,
    /// Used items whose library file declares another id, with the reason (see [`State::WrongId`]).
    pub wrong_id: Vec<(ItemKind, String, String)>,
}

impl Unpinned {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.wrong_id.is_empty()
    }
}

/// Pin every used item that is not pinned yet. Items it cannot pin are returned.
pub fn lock_missing(project_dir: &Path, project: &mut Project, lib: &Library) -> Result<Unpinned> {
    lock_missing_then(project_dir, project, lib, || {})
}

/// [`lock_missing`], calling `after_scan` between reading the library's state and taking the
/// items, so a test can change a library file in between.
fn lock_missing_then(
    project_dir: &Path,
    project: &mut Project,
    lib: &Library,
    mut after_scan: impl FnMut(),
) -> Result<Unpinned> {
    let mut left = Unpinned::default();
    let mut todo = Vec::new();
    for s in status(project_dir, project, lib) {
        match s.state {
            State::NotLocked => todo.push((s.kind, s.id)),
            // Used but not pinned, and its library file cannot be taken.
            State::WrongId if s.locked.is_none() => left.wrong_id.push((s.kind, s.id, s.changes.join("; "))),
            _ => {}
        }
    }
    after_scan();
    for (kind, id) in todo {
        match take(project_dir, project, lib, kind, &id) {
            Ok(_) => {}
            Err(Error::NotFound { .. }) => left.missing.push((kind, id)),
            // The file changed since `status` read it: report it like the others and pin the rest.
            Err(e) if is_wrong_id(&e) => {
                let why = e.to_string();
                left.wrong_id.push((kind, id, why));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(left)
}

/// Take newer library versions. `only` limits it to the given ids; `None` updates everything
/// that changed. Returns the updated items.
pub fn update(
    project_dir: &Path,
    project: &mut Project,
    lib: &Library,
    only: Option<&[String]>,
) -> Result<Vec<(ItemKind, String)>> {
    let all = status(project_dir, project, lib);
    if let Some(bad) = wrong_named(&all, only) {
        return Err(bad);
    }
    update_items(project_dir, project, lib, &changed(&all, only))
}

/// Of `all` (from [`status`]), an item named on purpose (`only`) whose library file declares
/// another id, as the error an update stops with before anything is taken.
pub fn wrong_named(all: &[ItemStatus], only: Option<&[String]>) -> Option<Error> {
    let ids = only?;
    let bad = all.iter().find(|s| s.state == State::WrongId && ids.contains(&s.id))?;
    Some(Error::Source {
        name: bad.library_source.clone().unwrap_or_default(),
        message: bad.changes.join("; "),
    })
}

/// Of `all` (from [`status`]), the items with a newer library version (only those named in
/// `only`, when given), each with the hash of the version seen, for [`update_items`].
pub fn changed(all: &[ItemStatus], only: Option<&[String]>) -> Vec<(ItemKind, String, String)> {
    all.iter()
        .filter(|s| s.state == State::Changed && only.is_none_or(|ids| ids.contains(&s.id)))
        .filter_map(|s| Some((s.kind, s.id.clone(), s.library_hash.clone()?)))
        .collect()
}

/// Take exactly these library versions, in order: each item `(kind, id, hash)` only in the
/// version with that hash, read once and written as read. What a caller that looked at
/// [`status`] first uses, so nothing is written that it has not seen. Every item is read and
/// checked before any is written: an item whose library version is another one by now is an
/// error and nothing is taken. When a write fails, the files already written are put back as
/// they were and the lock is left as it was. Returns the items.
pub fn update_items(
    project_dir: &Path,
    project: &mut Project,
    lib: &Library,
    items: &[(ItemKind, String, String)],
) -> Result<Vec<(ItemKind, String)>> {
    let fetched = items
        .iter()
        .map(|(kind, id, hash)| fetch(lib, *kind, id, Some(hash)))
        .collect::<Result<Vec<_>>>()?;
    // Every file as it is before any is written, so a failed write can put them all back. A
    // file that cannot be read stops the update here: it could not be put back.
    let mut before = Vec::with_capacity(items.len());
    for (kind, id, _) in items {
        let path = store::vendor_path(project_dir, *kind, id)?;
        let was = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(wc_core::Error::io(&path, e).into()),
        };
        before.push((path, was));
    }
    let lock = project.lock.clone();
    for (n, ((_, id, _), item)) in items.iter().zip(fetched).enumerate() {
        if let Err(e) = store_item(project_dir, project, id, item) {
            project.lock = lock;
            let failed: Vec<String> = before[..=n]
                .iter()
                .rev()
                .filter_map(|(path, was)| {
                    let put = match was {
                        Some(text) => store::write_if_changed(path, text).map(|_| ()),
                        None => match std::fs::remove_file(path) {
                            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(wc_core::Error::io(path, e)),
                            _ => Ok(()),
                        },
                    };
                    put.err().map(|e| e.to_string())
                })
                .collect();
            if failed.is_empty() {
                return Err(e);
            }
            return Err(Error::NotPutBack {
                error: Box::new(e),
                failed: failed.join("; "),
            });
        }
    }
    Ok(items.iter().map(|(k, id, _)| (*k, id.clone())).collect())
}

/// Propose a project's vendored version of an item back to a library source. The file goes to
/// `<source>/proposed/<kind>/<id>.json` so a librarian (or a git pull request) can review it
/// before it replaces the real item.
pub fn propose(project_dir: &Path, lib: &Library, source: &str, kind: ItemKind, id: &str) -> Result<PathBuf> {
    let src = lib
        .sources
        .iter()
        .find(|s| s.name == source)
        .ok_or_else(|| Error::Source {
            name: source.into(),
            message: "no such source in this project".into(),
        })?;
    let content = match kind {
        ItemKind::Symbol => store::to_json(&store::read_vendored_symbol(project_dir, id)?)?,
        ItemKind::Part => store::to_json(&store::read_vendored_part(project_dir, id)?)?,
    };
    let path = src.root.join("proposed").join(kind.dir()).join(format!("{id}.json"));
    store::write_if_changed(&path, &content)?;
    Ok(path)
}

/// Write an item into a library folder (used by importers).
pub fn write_symbol(root: &Path, s: &Symbol) -> Result<PathBuf> {
    store::check_id(&s.id)?;
    let path = root.join("symbols").join(format!("{}.json", s.id));
    store::write_if_changed(&path, &store::to_json(s)?)?;
    Ok(path)
}

pub fn write_part(root: &Path, p: &Part) -> Result<PathBuf> {
    store::check_id(&p.id)?;
    let path = root.join("parts").join(format!("{}.json", p.id));
    store::write_if_changed(&path, &store::to_json(p)?)?;
    Ok(path)
}

#[cfg(test)]
mod tests;
