//! New projects from a base project: a company's template folder with its settings, title
//! block, page template, cover page, standard pages, library sources, logo and vendored
//! library items. The new project gets everything the base project holds except its review
//! comments and history, under its own name and date.
//!
//! In the base project's title block and page titles, `{name}` becomes the new project's name
//! and `{date}` its date. The project's `date` field is set to the new date, and a page's own
//! `date` is dropped, so every page shows the new project's date.
//!
//! Library sources keep pointing at the same libraries: a folder source inside the base
//! project is copied along and stays relative; a relative folder outside it (for example
//! `../library`) is written as the absolute folder it named from the base; absolute folders
//! and git sources are kept as they are.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::store::{self, COMMENTS_DIR, PROJECT_FILE};
use crate::{Error, Project, Result};

/// Folders of a base project that do not go into a new project: its history and its review
/// comments.
const SKIPPED: &[&str] = &[".git", COMMENTS_DIR];

/// Create a project in `dir`, named `name` and dated `date` (as the title block shows it, for
/// example `2026-11-18`), from the base project in `base`. `dir` must not exist yet or be an
/// empty folder, and must lie outside the base project. The project is put together in a
/// staging folder next to `dir` and moved into place only once it is complete, so a failure
/// leaves nothing behind and can simply be tried again.
pub fn create_from(base: &Path, dir: &Path, name: &str, date: &str) -> Result<Project> {
    if !base.join(PROJECT_FILE).is_file() {
        return Err(Error::Invalid(format!(
            "{} holds no project to start from",
            base.display()
        )));
    }
    let base_full = fs::canonicalize(base).map_err(|e| Error::io(base, e))?;
    let target = resolved(dir)?;
    if target.starts_with(&base_full) {
        return Err(Error::Invalid(format!(
            "{} is inside the base project {}; pick a folder outside it",
            dir.display(),
            base.display()
        )));
    }
    if target.exists() {
        let mut entries = fs::read_dir(&target).map_err(|e| Error::io(&target, e))?;
        if entries.next().is_some() {
            return Err(Error::Invalid(format!(
                "{} is not empty; pick a new or empty folder",
                dir.display()
            )));
        }
    }
    let mut project = store::load(base)?;
    fill(&mut project, name, date);
    keep_libraries(&mut project, &base_full)?;

    let parent = target
        .parent()
        .ok_or_else(|| Error::Invalid(format!("{} has no parent folder", dir.display())))?;
    fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    let staging = staging_dir(&target)?;
    if let Ok(meta) = fs::metadata(&target) {
        // An empty folder that is replaced keeps its permissions.
        if let Err(e) = fs::set_permissions(&staging, meta.permissions()) {
            let _ = fs::remove_dir(&staging);
            return Err(Error::io(&staging, e));
        }
    }
    let built = copy_tree(base, &staging, true)
        .and_then(|()| fail_for_test())
        .and_then(|()| store::save(&staging, &project).map(|_| ()));
    let placed = built.and_then(|()| {
        if target.exists() {
            fs::remove_dir(&target).map_err(|e| Error::io(&target, e))?;
        }
        fs::rename(&staging, &target).map_err(|e| Error::io(&target, e))
    });
    if let Err(e) = placed {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    Ok(project)
}

#[cfg(test)]
thread_local! {
    /// Tests: fail once the files are copied, before the project is saved.
    static FAIL_AFTER_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn fail_for_test() -> Result<()> {
    if FAIL_AFTER_COPY.with(|f| f.replace(false)) {
        return Err(Error::Invalid("test failure after copying".into()));
    }
    Ok(())
}

#[cfg(not(test))]
fn fail_for_test() -> Result<()> {
    Ok(())
}

/// Give a project loaded from a base project its own name and date and drop its comments.
pub fn fill(project: &mut Project, name: &str, date: &str) {
    let put = |s: &str| s.replace("{name}", name).replace("{date}", date);
    project.meta.name = name.to_owned();
    for v in project.meta.title_block.values_mut() {
        *v = put(v);
    }
    project.meta.title_block.insert("date".into(), date.to_owned());
    for page in &mut project.pages {
        page.title = put(&page.title);
        page.title_block.remove("date");
        for v in page.title_block.values_mut() {
            *v = put(v);
        }
    }
    project.comments.clear();
}

/// Relative folder sources that lead out of the base project become the absolute folder they
/// named from it; everything else already points at the same library from the new project.
fn keep_libraries(project: &mut Project, base: &Path) -> Result<()> {
    for lib in &mut project.meta.libraries {
        if lib.kind != "folder" || Path::new(&lib.location).is_absolute() {
            continue;
        }
        // Where the folder really is, with links followed before `..` as the library loader
        // does when it opens the path.
        let parts = || base.components().chain(written_parts(&lib.location));
        let named = walk(parts()).map_err(|e| {
            Error::Invalid(format!(
                "the library \"{}\" points at {}, which cannot be reached: {e}",
                lib.name, lib.location
            ))
        })?;
        match named.strip_prefix(base) {
            Err(_) => lib.location = named.to_string_lossy().into_owned(),
            // Inside the base, but reached through a link: links are not copied, so point
            // at the folder the link leads to, which is.
            Ok(rel) if named != lexical(parts()) => {
                let parts: Vec<_> = rel.iter().map(|c| c.to_string_lossy()).collect();
                lib.location = parts.join("/");
            }
            Ok(_) => {}
        }
    }
    Ok(())
}

/// The relative folder sources of a project that lead out of its folder (such as
/// `../library`): a base project fetched on its own (from a repository) cannot reach them.
pub fn outside_libraries(project: &Project) -> Vec<String> {
    let root = Path::new("/base");
    project
        .meta
        .libraries
        .iter()
        .filter(|l| l.kind == "folder" && !Path::new(&l.location).is_absolute())
        .filter(|l| !lexical(root.components().chain(written_parts(&l.location))).starts_with(root))
        .map(|l| l.location.clone())
        .collect()
}

/// The parts with `.` and `..` worked out by name, without looking at the disk.
fn lexical<'a>(parts: impl IntoIterator<Item = Component<'a>>) -> PathBuf {
    let mut out = PathBuf::new();
    for c in parts {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Where `dir` really is. Each existing folder on the way is looked up on disk with its
/// links resolved before a `..` after it is applied, so `link/../new` goes up from where the
/// link points. Past the first folder that does not exist yet, `..` is worked out by name (a
/// folder that does not exist holds no links).
fn resolved(dir: &Path) -> Result<PathBuf> {
    // Not `std::path::absolute`: on Windows it works out `..` by name before anything is
    // checked on disk.
    let abs = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir().map_err(|e| Error::io(dir, e))?.join(dir)
    };
    walk(abs.components())
}

/// The parts of a relative path written in a project file, split where the system splits
/// a path (`/`, and on Windows `\\` too), so they name what the library loader opens.
/// Kept apart from the folder they are relative to: joining them onto a Windows `\\?\` path
/// would work out `..` by name before the disk is asked.
fn written_parts(rel: &str) -> impl Iterator<Item = Component<'_>> {
    rel.split(std::path::is_separator)
        .filter(|p| !p.is_empty())
        .map(|p| match p {
            "." => Component::CurDir,
            ".." => Component::ParentDir,
            other => Component::Normal(std::ffi::OsStr::new(other)),
        })
}

/// Follow `parts` from the root as [`resolved`] describes.
fn walk<'a>(parts: impl Iterator<Item = Component<'a>>) -> Result<PathBuf> {
    let mut real = PathBuf::new();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    for c in parts {
        match c {
            Component::Prefix(_) | Component::RootDir => real.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                if missing.pop().is_none() {
                    // `file/..` is no path: only a folder has a folder above it.
                    if !real.is_dir() {
                        return Err(Error::Invalid(format!("{} is not a folder", real.display())));
                    }
                    real.pop();
                }
            }
            Component::Normal(n) if missing.is_empty() => {
                let next = real.join(n);
                match fs::canonicalize(&next) {
                    Ok(p) => real = p,
                    // Only a folder can hold what is missing; Windows reports a file used
                    // as a folder as "not found" too.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound && real.is_dir() => {
                        missing.push(n.to_owned());
                    }
                    // A file where a folder should be, or no access: the path cannot be used.
                    Err(e) => return Err(Error::io(&next, e)),
                }
            }
            Component::Normal(n) => missing.push(n.to_owned()),
        }
    }
    Ok(missing.iter().fold(real, |p, c| p.join(c)))
}

/// A new, empty folder next to `target` to put the project together in.
fn staging_dir(target: &Path) -> Result<PathBuf> {
    let parent = target.parent().unwrap_or(Path::new("."));
    let stem = target
        .file_name()
        .map_or_else(|| "project".into(), |n| n.to_string_lossy());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for n in 0..100u32 {
        let p = parent.join(format!(".{stem}.new-{}-{nanos}-{n}", std::process::id()));
        match fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io(&p, e)),
        }
    }
    Err(Error::Invalid(format!(
        "no staging folder could be made in {}",
        parent.display()
    )))
}

/// Copy the files and folders of `from` into `to`. Links are not followed or copied, so
/// nothing from outside the base project comes along.
fn copy_tree(from: &Path, to: &Path, top: bool) -> Result<()> {
    fs::create_dir_all(to).map_err(|e| Error::io(to, e))?;
    let entries = fs::read_dir(from).map_err(|e| Error::io(from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(from, e))?;
        let name = entry.file_name();
        if top && SKIPPED.iter().any(|s| name == *s) {
            continue;
        }
        let kind = entry.file_type().map_err(|e| Error::io(entry.path(), e))?;
        let target = to.join(&name);
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, false)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target).map_err(|e| Error::io(&target, e))?;
        }
    }
    Ok(())
}

/// Today's date (UTC) as `YYYY-MM-DD`.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_date((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The calendar date (year, month, day) `days` days after 1970-01-01 (Howard Hinnant's
/// civil-from-days).
pub fn civil_date(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::review::Comment;
    use crate::{Logo, Page};

    fn base(dir: &Path) {
        let mut p = Project::new("ACME base");
        p.meta.title_block.insert("customer".into(), "ACME".into());
        p.meta
            .title_block
            .insert("project_title".into(), "{name} ({date})".into());
        p.meta.title_block.insert("date".into(), "2020-01-01".into());
        p.meta.logo = Some(Logo {
            file: "pictures/logo.png".into(),
            cover: None,
        });
        let mut cover = Page::new("p0001", "1", "Cover");
        cover.cover = true;
        let mut legend = Page::new("p0002", "2", "Legend for {name}");
        legend.title_block.insert("date".into(), "2020-02-02".into());
        legend.title_block.insert("drawn_by".into(), "base team".into());
        p.pages = vec![cover, legend];
        p.comments.push(Comment {
            id: "c1".into(),
            page: "p0002".into(),
            at: None,
            author: "Kari".into(),
            time: 0,
            text: "Check the legend".into(),
            resolved: false,
            replies: Vec::new(),
        });
        store::save(dir, &p).unwrap();
        fs::create_dir_all(dir.join("pictures")).unwrap();
        fs::write(dir.join("pictures/logo.png"), b"\x89PNG\r\n\x1a\nlogo").unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    #[test]
    fn a_new_project_takes_the_base_projects_pages_and_settings_with_its_own_name_and_date() {
        let tmp = tempfile::tempdir().unwrap();
        let (b, new) = (tmp.path().join("base"), tmp.path().join("Pump station"));
        base(&b);
        let made = create_from(&b, &new, "Pump station", "2026-11-18").unwrap();
        let read = store::load(&new).unwrap();
        assert_eq!(made, read);
        assert_eq!(read.meta.name, "Pump station");
        assert_eq!(read.meta.title_block["customer"], "ACME");
        assert_eq!(read.meta.title_block["project_title"], "Pump station (2026-11-18)");
        assert_eq!(read.meta.title_block["date"], "2026-11-18");
        assert_eq!(read.pages.len(), 2);
        assert!(read.pages[0].cover);
        assert_eq!(read.pages[1].title, "Legend for Pump station");
        assert_eq!(read.pages[1].title_field(&read.meta, "date"), "2026-11-18");
        assert_eq!(read.pages[1].title_block["drawn_by"], "base team");
        assert!(read.comments.is_empty(), "review comments stay in the base project");
        assert!(!new.join(COMMENTS_DIR).exists());
        assert!(!new.join(".git").exists(), "the history stays in the base project");
        assert_eq!(
            fs::read(new.join("pictures/logo.png")).unwrap(),
            b"\x89PNG\r\n\x1a\nlogo"
        );
        // The base project is untouched.
        let still = store::load(&b).unwrap();
        assert_eq!(still.meta.name, "ACME base");
        assert_eq!(still.comments.len(), 1);
    }

    #[test]
    fn a_folder_with_a_project_or_inside_the_base_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().join("base");
        base(&b);
        assert!(create_from(&b, &b.join("sub").join("new"), "X", "2026-11-18").is_err());
        assert!(!b.join("sub").exists());
        let taken = tmp.path().join("taken");
        store::create(&taken, "Taken").unwrap();
        assert!(create_from(&b, &taken, "X", "2026-11-18").is_err());
        assert_eq!(store::load(&taken).unwrap().meta.name, "Taken");
        let empty = tmp.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(create_from(&empty, &tmp.path().join("y"), "Y", "2026-11-18").is_err());
        // A detour through a folder that does not exist still lands inside the base.
        let detour = tmp.path().join("missing").join("..").join("base").join("new");
        assert!(create_from(&b, &detour, "X", "2026-11-18").is_err());
        assert!(!b.join("new").exists() && !tmp.path().join("missing").exists());
        // An empty folder is fine.
        create_from(&b, &empty, "Into empty", "2026-11-18").unwrap();
        assert_eq!(store::load(&empty).unwrap().meta.name, "Into empty");
    }

    #[test]
    fn a_folder_with_other_files_is_left_as_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().join("base");
        base(&b);
        let busy = tmp.path().join("busy");
        fs::create_dir_all(busy.join("pictures")).unwrap();
        fs::write(busy.join("pictures/logo.png"), "mine").unwrap();
        assert!(create_from(&b, &busy, "X", "2026-11-18").is_err());
        assert_eq!(fs::read_to_string(busy.join("pictures/logo.png")).unwrap(), "mine");
        assert!(!busy.join(PROJECT_FILE).exists());
    }

    #[test]
    fn a_failed_copy_leaves_nothing_and_can_be_tried_again() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().join("base");
        base(&b);
        let new = tmp.path().join("new");
        FAIL_AFTER_COPY.with(|f| f.set(true));
        assert!(create_from(&b, &new, "New", "2026-11-18").is_err());
        assert!(!new.exists());
        let left: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            left,
            vec![std::ffi::OsString::from("base")],
            "no staging folder is left"
        );
        create_from(&b, &new, "New", "2026-11-18").unwrap();
        assert_eq!(store::load(&new).unwrap().meta.name, "New");
    }

    #[test]
    fn library_folders_keep_pointing_at_the_same_library() {
        use crate::project::LibrarySource;
        let tmp = tempfile::tempdir().unwrap();
        let company = tmp.path().join("company");
        let b = company.join("base");
        base(&b);
        fs::create_dir_all(company.join("library")).unwrap();
        fs::create_dir_all(b.join("own-library")).unwrap();
        let source = |name: &str, kind: &str, location: &str| LibrarySource {
            name: name.into(),
            kind: kind.into(),
            location: location.into(),
            reference: None,
        };
        let shared = tmp.path().join("shared").to_string_lossy().into_owned();
        let mut p = store::load(&b).unwrap();
        p.meta.libraries = vec![
            source("company", "folder", "../library"),
            source("own", "folder", "own-library"),
            source("shared", "folder", &shared),
            source("remote", "git", "https://example.com/lib.git"),
        ];
        store::save(&b, &p).unwrap();
        assert_eq!(outside_libraries(&p), vec!["../library".to_owned()]);

        let new = tmp.path().join("jobs").join("pump");
        let made = create_from(&b, &new, "Pump", "2026-11-18").unwrap();
        let libs = &made.meta.libraries;
        let company_lib = fs::canonicalize(company.join("library")).unwrap();
        assert_eq!(
            Path::new(&libs[0].location),
            company_lib,
            "the same folder as from the base"
        );
        assert_eq!(libs[1].location, "own-library", "copied along, still relative");
        assert!(new.join("own-library").is_dir());
        assert_eq!(libs[2].location, shared, "an absolute folder is kept");
        assert_eq!(libs[3].location, "https://example.com/lib.git");
        assert_eq!(store::load(&new).unwrap().meta.libraries, *libs);
    }

    #[cfg(unix)]
    #[test]
    fn links_in_the_base_project_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().join("base");
        base(&b);
        fs::write(tmp.path().join("secret.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("secret.txt"), b.join("pictures/secret.txt")).unwrap();
        let new = tmp.path().join("new");
        create_from(&b, &new, "New", "2026-11-18").unwrap();
        assert!(!new.join("pictures/secret.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_library_path_through_a_link_keeps_pointing_where_the_link_leads() {
        use crate::project::LibrarySource;
        let tmp = tempfile::tempdir().unwrap();
        let company = tmp.path().join("company");
        let b = company.join("base");
        base(&b);
        // `company/link` points at `other/deep`, so `../link/../library` is `other/library`.
        fs::create_dir_all(tmp.path().join("other/deep")).unwrap();
        fs::create_dir_all(tmp.path().join("other/library")).unwrap();
        fs::create_dir_all(company.join("library")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("other/deep"), company.join("link")).unwrap();
        let mut p = store::load(&b).unwrap();
        p.meta.libraries = vec![LibrarySource {
            name: "through".into(),
            kind: "folder".into(),
            location: "../link/../library".into(),
            reference: None,
        }];
        store::save(&b, &p).unwrap();
        let made = create_from(&b, &tmp.path().join("new"), "New", "2026-11-18").unwrap();
        assert_eq!(
            Path::new(&made.meta.libraries[0].location),
            fs::canonicalize(tmp.path().join("other/library")).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_library_link_inside_the_base_points_at_the_copied_folder() {
        use crate::project::LibrarySource;
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().join("base");
        base(&b);
        fs::create_dir_all(b.join("own-library/parts")).unwrap();
        std::os::unix::fs::symlink(b.join("own-library"), b.join("lib-link")).unwrap();
        let mut p = store::load(&b).unwrap();
        p.meta.libraries = vec![LibrarySource {
            name: "linked".into(),
            kind: "folder".into(),
            location: "lib-link/parts".into(),
            reference: None,
        }];
        store::save(&b, &p).unwrap();
        let new = tmp.path().join("new");
        let made = create_from(&b, &new, "New", "2026-11-18").unwrap();
        assert_eq!(made.meta.libraries[0].location, "own-library/parts");
        assert!(new.join(&made.meta.libraries[0].location).is_dir());
    }

    #[test]
    fn a_library_path_through_a_file_is_refused() {
        use crate::project::LibrarySource;
        let tmp = tempfile::tempdir().unwrap();
        let company = tmp.path().join("company");
        let b = company.join("base");
        base(&b);
        fs::write(company.join("file"), "not a folder").unwrap();
        fs::create_dir_all(company.join("library")).unwrap();
        let mut p = store::load(&b).unwrap();
        p.meta.libraries = vec![LibrarySource {
            name: "broken".into(),
            kind: "folder".into(),
            location: "../file/../library".into(),
            reference: None,
        }];
        store::save(&b, &p).unwrap();
        let new = tmp.path().join("new");
        let err = create_from(&b, &new, "New", "2026-11-18").unwrap_err();
        assert!(err.to_string().contains("broken"), "{err}");
        assert!(!new.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_backslash_in_a_library_name_is_part_of_the_name_on_unix() {
        use crate::project::LibrarySource;
        let tmp = tempfile::tempdir().unwrap();
        let company = tmp.path().join("company");
        let b = company.join("base");
        base(&b);
        fs::create_dir_all(b.join("..\\library")).unwrap();
        fs::create_dir_all(company.join("library")).unwrap();
        let mut p = store::load(&b).unwrap();
        p.meta.libraries = vec![LibrarySource {
            name: "odd".into(),
            kind: "folder".into(),
            location: "..\\library".into(),
            reference: None,
        }];
        store::save(&b, &p).unwrap();
        assert!(outside_libraries(&p).is_empty());
        let made = create_from(&b, &tmp.path().join("new"), "New", "2026-11-18").unwrap();
        assert_eq!(made.meta.libraries[0].location, "..\\library");
        assert!(tmp.path().join("new").join("..\\library").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_new_folder_path_is_followed_before_going_up() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().join("work").join("base");
        base(&b);
        // `work/link` points into the base project, so `work/link/../new` is `base/new`.
        std::os::unix::fs::symlink(b.join("pictures"), tmp.path().join("work/link")).unwrap();
        let through = tmp.path().join("work").join("link").join("..").join("new");
        assert!(create_from(&b, &through, "X", "2026-11-18").is_err());
        assert!(!b.join("new").exists() && !tmp.path().join("work/new").exists());
        // A link that leads out of the base is fine and lands where the link points.
        let out = tmp.path().join("elsewhere").join("deep");
        fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(&out, tmp.path().join("work/out")).unwrap();
        let via = tmp.path().join("work").join("out").join("..").join("made");
        create_from(&b, &via, "Made", "2026-11-18").unwrap();
        assert!(tmp.path().join("elsewhere/made").join(PROJECT_FILE).is_file());
    }

    #[test]
    fn a_file_in_the_new_folder_path_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().join("base");
        base(&b);
        fs::write(tmp.path().join("file"), "not a folder").unwrap();
        let through = tmp.path().join("file").join("child").join("..").join("..").join("new");
        assert!(create_from(&b, &through, "X", "2026-11-18").is_err());
        let up = tmp.path().join("file").join("..").join("new");
        assert!(create_from(&b, &up, "X", "2026-11-18").is_err());
        assert!(!tmp.path().join("new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_folder_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().join("base");
        base(&b);
        let private = tmp.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        create_from(&b, &private, "Private", "2026-11-18").unwrap();
        let mode = fs::metadata(&private).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert_eq!(store::load(&private).unwrap().meta.name, "Private");
    }

    #[test]
    fn days_count_into_calendar_dates() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(59), (1970, 3, 1));
        assert_eq!(civil_date(11_016), (2000, 2, 29));
        assert_eq!(civil_date(20_775), (2026, 11, 18));
        assert_eq!(civil_date(-1), (1969, 12, 31));
        assert_eq!(today().len(), 10);
    }
}
