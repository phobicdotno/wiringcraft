//! New projects from a base project kept in a folder or in a git repository (see
//! [`wc_core::base`]). A repository is fetched (latest version only) into a private folder
//! that is removed afterwards; its history does not come along.

use std::path::Path;

use wc_core::Project;

use crate::{Git, Result};

/// Whether `source` names a git repository rather than a folder.
pub fn is_url(source: &str) -> bool {
    ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|p| source.starts_with(p))
        || (source.starts_with("git@") && source.contains(':'))
}

/// Create a project in `dir`, named `name` and dated `date`, from the base project in the
/// folder or git repository `source`.
pub fn create_from_base(source: &str, dir: &Path, name: &str, date: &str) -> Result<Project> {
    if !is_url(source) {
        return Ok(wc_core::base::create_from(Path::new(source), dir, name, date)?);
    }
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io(std::env::temp_dir(), e))?;
    let fetched = tmp.path().join("base");
    let target = fetched.to_string_lossy().into_owned();
    Git::new(tmp.path()).run(&["clone", "-q", "--depth", "1", "--", source, &target])?;
    Ok(wc_core::base::create_from(&fetched, dir, name, date)?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use wc_core::store;

    /// A `file://` URL for a local folder, also with a Windows drive letter.
    fn file_url(p: &std::path::Path) -> String {
        let path = p.to_string_lossy().replace('\\', "/");
        if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        }
    }

    #[test]
    fn urls_and_folders_are_told_apart() {
        for url in [
            "https://github.com/acme/base.git",
            "ssh://git@host/base",
            "git@github.com:acme/base.git",
            "file:///srv/base",
        ] {
            assert!(is_url(url), "{url}");
        }
        for folder in ["base", "/srv/base", "C:\\projects\\base", "git@folder"] {
            assert!(!is_url(folder), "{folder}");
        }
    }

    #[test]
    fn a_base_project_comes_from_a_git_repository_without_its_history() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let mut p = store::create(&base, "ACME base").unwrap();
        p.meta.title_block.insert("customer".into(), "ACME".into());
        store::save(&base, &p).unwrap();
        Git::new(&base).init().unwrap();
        let url = file_url(&base);
        let new = tmp.path().join("new");
        let made = create_from_base(&url, &new, "Pump station", "2026-11-18").unwrap();
        assert_eq!(made.meta.name, "Pump station");
        assert_eq!(store::load(&new).unwrap().meta.title_block["customer"], "ACME");
        assert!(!new.join(".git").exists());
        // A folder works the same way.
        let other = tmp.path().join("other");
        create_from_base(&base.to_string_lossy(), &other, "Other", "2026-11-18").unwrap();
        assert_eq!(store::load(&other).unwrap().meta.name, "Other");
        // A repository that cannot be fetched is an error, and nothing is made.
        let missing = file_url(&tmp.path().join("nowhere"));
        assert!(create_from_base(&missing, &tmp.path().join("x"), "X", "2026-11-18").is_err());
        assert!(!tmp.path().join("x").exists());
    }
}
