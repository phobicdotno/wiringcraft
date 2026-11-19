//! The project folder on disk.
//!
//! ```text
//! my-project/
//!   project.toml            name, title block, library sources
//!   devices.json            device tag -> part and properties
//!   library.lock            exact version of every library item in use
//!   pages/<id>.page.json    one file per page
//!   vendor/symbols/<id>.json  copies of the library items in use
//!   vendor/parts/<id>.json
//!   panels/<id>.panel.json  cabinet panel layouts
//!   comments/<page id>.comments.json  review comments on a page
//! ```
//!
//! Every file is written deterministically (sorted keys, fixed indentation, trailing newline),
//! atomically (temporary file, then rename) and only when its content changed, so version
//! control shows real changes only and a crash never leaves a half-written file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::lock::ItemKind;
use crate::project::{Device, Page, Project, ProjectMeta};
use crate::tag::DeviceTag;
use crate::{Error, Lockfile, Part, Result, Symbol};

pub const PROJECT_FILE: &str = "project.toml";
pub const DEVICES_FILE: &str = "devices.json";
pub const LOCK_FILE: &str = "library.lock";
pub const PAGES_DIR: &str = "pages";
pub const VENDOR_DIR: &str = "vendor";
pub const PAGE_SUFFIX: &str = ".page.json";
pub const PANELS_DIR: &str = "panels";
pub const PANEL_SUFFIX: &str = ".panel.json";
pub const COMMENTS_DIR: &str = "comments";
pub const COMMENTS_SUFFIX: &str = ".comments.json";

/// Ids become file names, so they are restricted to a portable set.
pub fn check_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "id {id:?} must be 1-128 of A-Z a-z 0-9 . _ - and not start with '.'"
        )))
    }
}

pub fn to_json<T: Serialize>(value: &T) -> Result<String> {
    // Through Value so object keys come out sorted whatever the struct order is.
    let v = serde_json::to_value(value).map_err(|e| Error::Json {
        path: PathBuf::new(),
        source: e,
    })?;
    let mut s = serde_json::to_string_pretty(&v).map_err(|e| Error::Json {
        path: PathBuf::new(),
        source: e,
    })?;
    s.push('\n');
    Ok(s)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_str(&text).map_err(|e| Error::Json {
        path: path.into(),
        source: e,
    })
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    toml::from_str(&text).map_err(|e| Error::Toml {
        path: path.into(),
        message: e.to_string(),
    })
}

fn to_toml<T: Serialize>(value: &T, path: &Path) -> Result<String> {
    toml::to_string_pretty(value).map_err(|e| Error::Toml {
        path: path.into(),
        message: e.to_string(),
    })
}

/// Write `content` to `path` atomically, skipping the write when the file already holds it.
/// Returns whether the file changed.
pub fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    replace_whole(path, content.as_bytes())?;
    Ok(true)
}

/// Replace the file at `path` with `bytes` as a whole: staged next to it in `<file>.tmp`, then
/// renamed over it, so a failed write leaves the file as it was rather than cut short. The
/// staging file is one this call creates itself (opened with `create_new`): a link left at that
/// path is never written through. The folder must exist.
pub fn replace_whole(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    // A staging file left by a failed write or rename is removed again (a folder in its way is
    // not ours to remove).
    let drop_tmp = || {
        if fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_file()) {
            let _ = fs::remove_file(&tmp);
        }
    };
    if let Err(e) = write_staging(&tmp, bytes) {
        drop_tmp();
        return Err(Error::io(&tmp, e));
    }
    if let Err(e) = fs::rename(&tmp, path) {
        drop_tmp();
        return Err(Error::io(path, e));
    }
    Ok(())
}

/// Write `content` to a staging file at `tmp` that this call creates itself.
///
/// The file is opened with `create_new`, which never follows a link: a symbolic link left at
/// `tmp` by something else in the project folder is removed (the link, not its target), and a
/// link that shows up again before the file is created makes the write fail instead of writing
/// through it. A regular file left by an earlier failed write is replaced the same way. Anything
/// else in the way, such as a folder, is not ours to remove and fails the write.
fn write_staging(tmp: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Ok(meta) = fs::symlink_metadata(tmp)
        && (meta.is_symlink() || meta.is_file())
    {
        fs::remove_file(tmp)?;
    }
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(tmp)?;
    file.write_all(content)
}

/// Load a whole project folder.
pub fn load(dir: &Path) -> Result<Project> {
    let meta: ProjectMeta = read_toml(&dir.join(PROJECT_FILE))?;
    if meta.format_version > ProjectMeta::FORMAT_VERSION {
        return Err(Error::Invalid(format!(
            "project format {} is newer than this WiringCraft understands ({})",
            meta.format_version,
            ProjectMeta::FORMAT_VERSION
        )));
    }
    let devices_path = dir.join(DEVICES_FILE);
    let devices: BTreeMap<DeviceTag, Device> = if devices_path.exists() {
        read_json(&devices_path)?
    } else {
        BTreeMap::new()
    };
    let lock_path = dir.join(LOCK_FILE);
    let lock: Lockfile = if lock_path.exists() {
        read_toml(&lock_path)?
    } else {
        Lockfile::default()
    };

    let mut pages = Vec::new();
    let pages_dir = dir.join(PAGES_DIR);
    if pages_dir.exists() {
        let entries = fs::read_dir(&pages_dir).map_err(|e| Error::io(&pages_dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::io(&pages_dir, e))?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(PAGE_SUFFIX) else {
                continue;
            };
            let page: Page = read_json(&path)?;
            if page.id != stem {
                return Err(Error::Invalid(format!(
                    "{} holds page id {:?}; the file name and id must match",
                    path.display(),
                    page.id
                )));
            }
            pages.push(page);
        }
    }
    let mut panels: Vec<crate::panel::Panel> = Vec::new();
    let panels_dir = dir.join(PANELS_DIR);
    if panels_dir.exists() {
        let entries = fs::read_dir(&panels_dir).map_err(|e| Error::io(&panels_dir, e))?;
        for entry in entries {
            let path = entry.map_err(|e| Error::io(&panels_dir, e))?.path();
            let Some(stem) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(PANEL_SUFFIX))
            else {
                continue;
            };
            let panel: crate::panel::Panel = read_json(&path)?;
            if panel.id != stem {
                return Err(Error::Invalid(format!(
                    "{} holds panel id {:?}; the file name and id must match",
                    path.display(),
                    panel.id
                )));
            }
            panels.push(panel);
        }
    }
    panels.sort_by(|a, b| a.id.cmp(&b.id));
    let mut comments: Vec<crate::review::Comment> = Vec::new();
    let comments_dir = dir.join(COMMENTS_DIR);
    if comments_dir.exists() {
        let entries = fs::read_dir(&comments_dir).map_err(|e| Error::io(&comments_dir, e))?;
        for entry in entries {
            let path = entry.map_err(|e| Error::io(&comments_dir, e))?.path();
            let Some(stem) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(COMMENTS_SUFFIX))
            else {
                continue;
            };
            let mut list: Vec<crate::review::Comment> = read_json(&path)?;
            // The file says which page; a comment that claims another is moved back to it.
            for c in &mut list {
                c.page = stem.to_owned();
            }
            comments.extend(list);
        }
    }
    let mut project = Project {
        meta,
        pages,
        devices,
        lock,
        panels,
        comments,
    };
    project.sort_pages();
    project.sort_comments();
    Ok(project)
}

/// Write only the project's `library.lock`; true when it changed. For a change that pins new
/// items: written before the files that use them, a failed save leaves a pin nothing uses yet
/// (harmless, and the next try finds it), never a use without its pin.
pub fn save_lock(dir: &Path, project: &Project) -> Result<bool> {
    let path = dir.join(LOCK_FILE);
    write_if_changed(&path, &to_toml(&project.lock, &path)?)
}

/// Save a project folder. Page files of deleted pages are removed. Returns the files written.
///
/// Ids that differ only in case are refused before anything is written: on a folder that ignores
/// case (macOS and Windows by default) they would be one file.
pub fn save(dir: &Path, project: &Project) -> Result<Vec<PathBuf>> {
    unique_ignoring_case("page", project.pages.iter().map(|p| p.id.as_str()))?;
    unique_ignoring_case("panel", project.panels.iter().map(|p| p.id.as_str()))?;
    let mut written = Vec::new();
    let mut put = |path: PathBuf, content: String| -> Result<()> {
        if write_if_changed(&path, &content)? {
            written.push(path);
        }
        Ok(())
    };
    put(dir.join(PROJECT_FILE), to_toml(&project.meta, &dir.join(PROJECT_FILE))?)?;
    put(dir.join(DEVICES_FILE), to_json(&project.devices)?)?;
    put(dir.join(LOCK_FILE), to_toml(&project.lock, &dir.join(LOCK_FILE))?)?;

    let pages_dir = dir.join(PAGES_DIR);
    let mut keep = std::collections::BTreeSet::new();
    for page in &project.pages {
        check_id(&page.id)?;
        let name = format!("{}{PAGE_SUFFIX}", page.id);
        put(pages_dir.join(&name), to_json(page)?)?;
        keep.insert(name);
    }
    clean_up(&pages_dir, &keep, PAGE_SUFFIX)?;
    let panels_dir = dir.join(PANELS_DIR);
    let mut keep = std::collections::BTreeSet::new();
    for panel in &project.panels {
        check_id(&panel.id)?;
        let name = format!("{}{PANEL_SUFFIX}", panel.id);
        put(panels_dir.join(&name), to_json(panel)?)?;
        keep.insert(name);
    }
    clean_up(&panels_dir, &keep, PANEL_SUFFIX)?;
    let comments_dir = dir.join(COMMENTS_DIR);
    let mut by_page: BTreeMap<&str, Vec<&crate::review::Comment>> = BTreeMap::new();
    // Comments on a page that was deleted go with it.
    for c in project.comments.iter().filter(|c| project.page(&c.page).is_some()) {
        by_page.entry(c.page.as_str()).or_default().push(c);
    }
    let mut keep = std::collections::BTreeSet::new();
    for (page, list) in &by_page {
        check_id(page)?;
        let name = format!("{page}{COMMENTS_SUFFIX}");
        put(comments_dir.join(&name), to_json(list)?)?;
        keep.insert(name);
    }
    clean_up(&comments_dir, &keep, COMMENTS_SUFFIX)?;
    remove_unused_box_symbols(dir, project)?;
    Ok(written)
}

/// Remove the files of black box symbols (`box.<page>.<id>`, the project's own symbols for the
/// boxes drawn on its pages) that no page places and the lock does not pin: left behind by a box
/// that was undone, or drawn and never saved. Other vendored symbols are left alone.
fn remove_unused_box_symbols(dir: &Path, project: &Project) -> Result<()> {
    let folder = dir.join(VENDOR_DIR).join(ItemKind::Symbol.dir());
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::io(&folder, e)),
    };
    let used: Vec<String> = project
        .lock
        .symbols
        .keys()
        .chain(project.pages.iter().flat_map(|p| p.symbols.iter().map(|s| &s.symbol)))
        .filter(|id| is_box_symbol(id))
        .map(|id| format!("{id}.json"))
        .collect();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(&folder, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // A folder that ignores case lists a used file under any case of its name.
        if !is_box_symbol(&name) || !name.ends_with(".json") || used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
            continue;
        }
        // Only plain files: never a folder or a link someone put there.
        let path = entry.path();
        let plain = fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
        if !plain {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&path, e)),
        }
    }
    Ok(())
}

/// Whether a symbol id (or its file name) is a black box's own symbol, `box.<page>.<id>`.
fn is_box_symbol(id: &str) -> bool {
    id.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("box."))
}

/// Refuse two ids that differ only in case, naming both.
fn unique_ignoring_case<'a>(kind: &str, ids: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for id in ids {
        if let Some(other) = seen.insert(id.to_ascii_lowercase(), id)
            && other != id
        {
            return Err(Error::Invalid(format!(
                "{kind} ids {other:?} and {id:?} differ only in case, so they would be one file on \
                 macOS and Windows; rename one of them"
            )));
        }
    }
    Ok(())
}

/// What to do with a file left in a folder after a save.
#[derive(Debug, PartialEq, Eq)]
enum Tidy {
    /// Nothing uses it any more.
    Remove(String),
    /// It is a file just written, listed under the old case of its name: the folder ignores case.
    Rename { from: String, to: String },
}

/// Which of the `listed` file names ending in `suffix` are not among the `keep` names written.
///
/// A name that differs from a kept one only in case is the same file on a folder that ignores
/// case, which still lists it under its old name: it is renamed to the new name, never removed.
/// When the folder lists both names, they are two files and the old one goes.
fn tidy(listed: &[String], keep: &std::collections::BTreeSet<String>, suffix: &str) -> Vec<Tidy> {
    let mut out = Vec::new();
    for name in listed {
        if !name.ends_with(suffix) || keep.contains(name) {
            continue;
        }
        match keep.iter().find(|k| k.eq_ignore_ascii_case(name)) {
            Some(k) if !listed.contains(k) => out.push(Tidy::Rename {
                from: name.clone(),
                to: k.clone(),
            }),
            _ => out.push(Tidy::Remove(name.clone())),
        }
    }
    out
}

/// Remove the files in `dir` ending in `suffix` that the save did not write (see [`tidy`]).
fn clean_up(dir: &Path, keep: &std::collections::BTreeSet<String>, suffix: &str) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
    let names = entries.map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()));
    clean_up_listed(dir, names, keep, suffix)
}

/// [`clean_up`] on the names a listing of `dir` gave.
fn clean_up_listed(
    dir: &Path,
    names: impl Iterator<Item = std::io::Result<String>>,
    keep: &std::collections::BTreeSet<String>,
    suffix: &str,
) -> Result<()> {
    // A listing cut short would look like a folder without the file just written: stop instead.
    let listed = names
        .collect::<std::io::Result<Vec<String>>>()
        .map_err(|e| Error::io(dir, e))?;
    for step in tidy(&listed, keep, suffix) {
        match step {
            Tidy::Remove(name) => {
                let path = dir.join(name);
                fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
            }
            Tidy::Rename { from, to } => {
                let to = dir.join(to);
                fs::rename(dir.join(from), &to).map_err(|e| Error::io(&to, e))?;
            }
        }
    }
    Ok(())
}

/// Create a new, empty project folder. Refuses a folder that already holds a project.
pub fn create(dir: &Path, name: &str) -> Result<Project> {
    if dir.join(PROJECT_FILE).exists() {
        return Err(Error::Invalid(format!("{} already holds a project", dir.display())));
    }
    let mut project = Project::new(name);
    let mut cover = Page::new("p0001", "1", "Cover");
    cover.cover = true;
    project.pages.push(cover);
    save(dir, &project)?;
    Ok(project)
}

/// Where a vendored item of this kind and id lives in the project folder.
pub fn vendor_path(dir: &Path, kind: ItemKind, id: &str) -> Result<PathBuf> {
    check_id(id)?;
    Ok(dir.join(VENDOR_DIR).join(kind.dir()).join(format!("{id}.json")))
}

pub fn write_vendored_symbol(dir: &Path, s: &Symbol) -> Result<bool> {
    write_if_changed(&vendor_path(dir, ItemKind::Symbol, &s.id)?, &to_json(s)?)
}

pub fn write_vendored_part(dir: &Path, p: &Part) -> Result<bool> {
    write_if_changed(&vendor_path(dir, ItemKind::Part, &p.id)?, &to_json(p)?)
}

pub fn read_vendored_symbol(dir: &Path, id: &str) -> Result<Symbol> {
    read_json(&vendor_path(dir, ItemKind::Symbol, id)?)
}

pub fn read_vendored_part(dir: &Path, id: &str) -> Result<Part> {
    read_json(&vendor_path(dir, ItemKind::Part, id)?)
}

/// All vendored symbols the project's pages use, keyed by id. Missing ones are reported.
pub fn vendored_symbols(dir: &Path, project: &Project) -> (BTreeMap<String, Symbol>, Vec<String>) {
    let mut found = BTreeMap::new();
    let mut missing = Vec::new();
    for id in project.used_symbols() {
        match read_vendored_symbol(dir, &id) {
            Ok(s) => {
                found.insert(id, s);
            }
            Err(_) => missing.push(id),
        }
    }
    (found, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::project::{Endpoint, PlacedSymbol, Wire};

    fn sample() -> Result<Project> {
        let mut p = Project::new("Demo");
        p.meta.title_block.insert("customer".into(), "ACME".into());
        let mut page = Page::new("p0002", "2", "Motor circuit");
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "wc.coil".into(),
            at: Point::new(40.0, 60.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(DeviceTag::parse("+CAB1-K1")?),
        });
        page.wires.push(Wire {
            id: "w1".into(),
            from: Endpoint::Pin {
                item: "s1".into(),
                pin: "A1".into(),
            },
            to: Endpoint::Point {
                at: Point::new(40.0, 20.0),
            },
            via: vec![],
            number: None,
            potential: Some("24V".into()),
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        p.pages.push(page);
        p.devices.insert(
            DeviceTag::parse("+CAB1-K1")?,
            Device {
                part: Some("acme.r1".into()),
                ..Default::default()
            },
        );
        let mut panel = crate::panel::Panel::with_rails("main", "Cabinet", 600.0, 800.0);
        panel.items.push(crate::panel::PanelItem::Device {
            tag: DeviceTag::parse("+CAB1-K1")?,
            at: Point::new(50.0, 100.0),
            width: 45.0,
            height: 85.0,
        });
        p.panels.push(panel);
        p.add_comment(
            "p0002",
            Some(Point::new(40.0, 60.0)),
            "Kari",
            1_700_000_000,
            "Is K1 rated for 24 V?",
        )?;
        p.reply_to_comment("c1", "Ola", 1_700_000_100, "Yes, coil is 24 V DC")?;
        p.add_comment("p0002", None, "Ola", 1_700_000_200, "Page title should say control")?;
        p.resolve_comment("c2", true)?;
        Ok(p)
    }

    #[test]
    fn round_trip_and_idempotent_save() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let p = sample()?;
        let first = save(dir.path(), &p)?;
        assert!(first.len() >= 4);
        let again = save(dir.path(), &p)?;
        assert!(again.is_empty(), "unchanged project rewrote {again:?}");
        let back = load(dir.path())?;
        assert_eq!(back, p);
        Ok(())
    }

    #[test]
    fn a_save_removes_only_box_symbols_no_page_places_and_the_lock_does_not_pin() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let dir = tmp.path();
        let mut p = sample()?;
        p.pages[0].symbols[0].symbol = "box.p0002.s1".into();
        p.lock.symbols.insert(
            "box.p0002.s7".into(),
            crate::LockEntry {
                source: "project".into(),
                hash: "h".into(),
            },
        );
        let folder = dir.join(VENDOR_DIR).join(ItemKind::Symbol.dir());
        fs::create_dir_all(folder.join("box.p0002.s9.json")).map_err(|e| Error::io(&folder, e))?;
        for name in [
            "box.p0002.s1.json",
            "box.p0002.s7.json",
            "box.p0002.s2.json",
            "BOX.P0002.S3.json",
            "box.p0002.s4.txt",
            "wc.coil.json",
        ] {
            fs::write(folder.join(name), "{}").map_err(|e| Error::io(name, e))?;
        }
        save(dir, &p)?;
        let mut left: Vec<String> = fs::read_dir(&folder)
            .map_err(|e| Error::io(&folder, e))?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "box.p0002.s1.json",
                "box.p0002.s4.txt",
                "box.p0002.s7.json",
                "box.p0002.s9.json",
                "wc.coil.json"
            ],
            "placed, pinned, not a symbol file, a folder and other symbols stay"
        );
        Ok(())
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_save_never_removes_a_file_it_wrote_under_another_case() {
        let keep = names(&["DXF-300.page.json", "p1.page.json"]).into_iter().collect();
        // A folder that ignores case lists the file just written under its old name.
        let listed = names(&["dxf-300.page.json", "p1.page.json", "gone.page.json", "notes.txt"]);
        assert_eq!(
            tidy(&listed, &keep, PAGE_SUFFIX),
            vec![
                Tidy::Rename {
                    from: "dxf-300.page.json".into(),
                    to: "DXF-300.page.json".into()
                },
                Tidy::Remove("gone.page.json".into()),
            ]
        );
        // A folder that keeps case lists both: two files, and the old one goes.
        let listed = names(&["dxf-300.page.json", "DXF-300.page.json", "p1.page.json"]);
        assert_eq!(
            tidy(&listed, &keep, PAGE_SUFFIX),
            vec![Tidy::Remove("dxf-300.page.json".into())]
        );
    }

    #[test]
    fn a_listing_cut_short_changes_nothing() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        // Names that differ by more than case, so this holds on every folder: a complete listing
        // would rename the first and remove the second.
        let files = [("dxf-300.page.json", "old"), ("gone.page.json", "stale")];
        for (name, body) in files {
            fs::write(dir.path().join(name), body).map_err(|e| Error::io(name, e))?;
        }
        let keep = names(&["DXF-300.page.json"]).into_iter().collect();
        let listing = vec![
            Ok("dxf-300.page.json".to_string()),
            Ok("gone.page.json".to_string()),
            Err(std::io::Error::other("listing cut short")),
        ];
        let out = clean_up_listed(dir.path(), listing.into_iter(), &keep, PAGE_SUFFIX);
        assert!(out.is_err_and(|e| e.to_string().contains("listing cut short")));
        let mut left = names(&[]);
        for entry in fs::read_dir(dir.path()).map_err(|e| Error::io("tmp", e))? {
            left.push(
                entry
                    .map_err(|e| Error::io("tmp", e))?
                    .file_name()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        left.sort();
        assert_eq!(left, names(&["dxf-300.page.json", "gone.page.json"]));
        for (name, body) in files {
            let path = dir.path().join(name);
            assert_eq!(fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?, body);
        }
        Ok(())
    }

    #[test]
    fn a_page_renamed_only_in_case_is_saved_and_loads() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        save(dir.path(), &p)?;
        let old = p.pages[0].id.clone();
        p.pages[0].id = old.to_ascii_uppercase();
        assert_ne!(p.pages[0].id, old);
        for c in p.comments.iter_mut().filter(|c| c.page == old) {
            c.page = p.pages[0].id.clone();
        }
        save(dir.path(), &p)?;
        let back = load(dir.path())?;
        assert_eq!(back, p);
        Ok(())
    }

    fn refused<T>(r: Result<T>) -> Result<String> {
        match r {
            Ok(_) => Err(Error::Invalid("saved ids that differ only in case".into())),
            Err(e) => Ok(e.to_string()),
        }
    }

    #[test]
    fn ids_that_differ_only_in_case_are_refused_before_anything_is_written() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        let mut twin = p.pages[0].clone();
        twin.id = twin.id.to_ascii_uppercase();
        let (a, b) = (p.pages[0].id.clone(), twin.id.clone());
        p.pages.push(twin);
        let err = refused(save(dir.path(), &p))?;
        assert!(
            err.contains(&format!("{a:?}")) && err.contains(&format!("{b:?}")),
            "{err}"
        );
        assert!(!dir.path().join(PROJECT_FILE).exists());
        p.pages.pop();
        let mut twin = p.panels[0].clone();
        twin.id = twin.id.to_ascii_uppercase();
        p.panels.push(twin);
        let err = refused(save(dir.path(), &p))?;
        assert!(err.contains("panel ids"), "{err}");
        Ok(())
    }

    #[test]
    fn deleted_pages_lose_their_file() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        save(dir.path(), &p)?;
        assert!(dir.path().join("comments/p0002.comments.json").exists());
        p.pages.clear();
        save(dir.path(), &p)?;
        assert!(!dir.path().join("pages/p0002.page.json").exists());
        assert!(dir.path().join("panels/main.panel.json").exists());
        assert!(!dir.path().join("comments/p0002.comments.json").exists());
        p.panels.clear();
        save(dir.path(), &p)?;
        assert!(!dir.path().join("panels/main.panel.json").exists());
        Ok(())
    }

    #[test]
    fn a_new_project_starts_with_its_cover_page_and_older_pages_load_as_normal_pages() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let p = create(dir.path(), "A")?;
        assert_eq!(p.cover_page(), Some(0));
        assert_eq!(load(dir.path())?.cover_page(), Some(0), "saved with the project");
        // A page written before cover pages existed has no `cover` key and is a normal page.
        let old: Page = serde_json::from_str(r#"{"id":"p0001","number":"1","title":"Cover"}"#)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        assert!(!old.cover);
        let normal = serde_json::to_string(&Page::new("p2", "2", "x")).map_err(|e| Error::Invalid(e.to_string()))?;
        assert!(
            !normal.contains("cover"),
            "normal pages keep their files as before: {normal}"
        );
        Ok(())
    }

    #[test]
    fn create_refuses_existing_project() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        create(dir.path(), "A")?;
        assert!(create(dir.path(), "B").is_err());
        Ok(())
    }

    #[test]
    fn hostile_ids_are_refused() {
        for bad in ["", "../x", ".hidden", "a/b", "a b"] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
        assert!(check_id("wc.contact-no_1").is_ok());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod staging_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_link_at_the_staging_path_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("outside.txt");
        fs::write(&target, "keep me").unwrap();
        let path = dir.path().join("devices.json");
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::os::unix::fs::symlink(&target, PathBuf::from(&tmp)).unwrap();

        assert!(write_if_changed(&path, "new content\n").unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content\n");
        assert!(fs::symlink_metadata(PathBuf::from(&tmp)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_at_the_staging_path_creates_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere").join("created.txt");
        fs::create_dir(dir.path().join("elsewhere")).unwrap();
        let path = dir.path().join("pages").join("p1.page.json");
        fs::create_dir(dir.path().join("pages")).unwrap();
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::os::unix::fs::symlink(&target, PathBuf::from(&tmp)).unwrap();

        assert!(write_if_changed(&path, "{}\n").unwrap());
        assert!(!target.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    }

    #[test]
    fn a_staging_file_left_by_a_failed_write_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.toml");
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        fs::write(PathBuf::from(&tmp), "stale and much longer than the new content").unwrap();

        assert!(write_if_changed(&path, "x\n").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "x\n");
        assert!(!PathBuf::from(&tmp).exists());
    }
}
