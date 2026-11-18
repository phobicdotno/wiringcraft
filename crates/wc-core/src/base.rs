//! New projects from a base project: a company's template folder with its settings, title
//! block, page template, cover page, standard pages, library sources, logo and vendored
//! library items. The new project gets everything the base project holds except its review
//! comments and history, under its own name and date.
//!
//! In the base project's title block and page titles, `{name}` becomes the new project's name
//! and `{date}` its date. The project's `date` field is set to the new date, and a page's own
//! `date` is dropped, so every page shows the new project's date.

use std::fs;
use std::path::Path;

use crate::store::{self, COMMENTS_DIR, PROJECT_FILE};
use crate::{Error, Project, Result};

/// Folders of a base project that do not go into a new project: its history and its review
/// comments.
const SKIPPED: &[&str] = &[".git", COMMENTS_DIR];

/// Create a project in `dir`, named `name` and dated `date` (as the title block shows it, for
/// example `2026-11-18`), from the base project in `base`. Refuses a `dir` that already holds
/// a project, and one inside the base project.
pub fn create_from(base: &Path, dir: &Path, name: &str, date: &str) -> Result<Project> {
    if dir.join(PROJECT_FILE).exists() {
        return Err(Error::Invalid(format!("{} already holds a project", dir.display())));
    }
    if !base.join(PROJECT_FILE).is_file() {
        return Err(Error::Invalid(format!(
            "{} holds no project to start from",
            base.display()
        )));
    }
    let base_full = fs::canonicalize(base).map_err(|e| Error::io(base, e))?;
    if let Some(inside) = existing_ancestor(dir)
        && inside.starts_with(&base_full)
    {
        return Err(Error::Invalid(format!(
            "{} is inside the base project {}; pick a folder outside it",
            dir.display(),
            base.display()
        )));
    }
    let mut project = store::load(base)?;
    copy_tree(base, dir, true)?;
    fill(&mut project, name, date);
    store::save(dir, &project)?;
    Ok(project)
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

/// The nearest folder of `dir` (itself or a parent) that exists, made absolute.
fn existing_ancestor(dir: &Path) -> Option<std::path::PathBuf> {
    let abs = std::path::absolute(dir).ok()?;
    abs.ancestors().find_map(|a| fs::canonicalize(a).ok())
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
    date_of_day((secs / 86_400) as i64)
}

/// The calendar date `days` days after 1970-01-01, as `YYYY-MM-DD`.
fn date_of_day(days: i64) -> String {
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
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

    #[test]
    fn days_count_into_calendar_dates() {
        assert_eq!(date_of_day(0), "1970-01-01");
        assert_eq!(date_of_day(59), "1970-03-01");
        assert_eq!(date_of_day(11_016), "2000-02-29");
        assert_eq!(date_of_day(20_775), "2026-11-18");
        assert_eq!(today().len(), 10);
    }
}
