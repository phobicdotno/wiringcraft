//! New projects from a base project kept in a folder or in a git repository (see
//! [`wc_core::base`]). A repository is fetched (latest version only) into a private folder
//! that is removed afterwards; its history does not come along.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

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
    create_from_base_until(source, dir, name, date, &AtomicBool::new(false))
}

/// [`create_from_base`] that stops once `cancel` is set, while the repository is fetched or
/// read, up to the point where the project is made (that then finishes), and then returns [`Error::Cancelled`](crate::Error::Cancelled)
/// with nothing made in `dir`. Meant to run off the window thread.
pub fn create_from_base_until(
    source: &str,
    dir: &Path,
    name: &str,
    date: &str,
    cancel: &AtomicBool,
) -> Result<Project> {
    if !is_url(source) {
        return Ok(wc_core::base::create_from(Path::new(source), dir, name, date)?);
    }
    let tmp = private_temp_dir().map_err(|e| wc_core::Error::io(std::env::temp_dir(), e))?;
    let fetched = tmp.path().join("base");
    let target = fetched.to_string_lossy().into_owned();
    Git::new(tmp.path()).run_until(&["clone", "-q", "--depth", "1", "--", source, &target], cancel)?;
    #[cfg(test)]
    if tests::CANCEL_AFTER_CLONE.get() {
        cancel.store(true, Ordering::Relaxed);
    }
    // A library folder next to the repository was not fetched with it.
    let outside = wc_core::base::outside_libraries(&wc_core::store::load(&fetched)?);
    if !outside.is_empty() {
        return Err(wc_core::Error::Invalid(format!(
            "the base project at {source} uses library folders outside its repository ({}); \
             start from a local copy of it instead, or point those sources at absolute folders or git",
            outside.join(", ")
        ))
        .into());
    }
    // The last point to stop: once the project is being made, it is made in full.
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::Error::Cancelled);
    }
    Ok(wc_core::base::create_from(&fetched, dir, name, date)?)
}

/// A temporary folder only this user can read: a private base project is cloned into it.
#[cfg(unix)]
fn private_temp_dir() -> std::io::Result<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
}

/// A temporary folder in the user's own temporary folder.
#[cfg(not(unix))]
fn private_temp_dir() -> std::io::Result<tempfile::TempDir> {
    tempfile::tempdir()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use wc_core::store;

    thread_local! {
        /// Set: Cancel is pressed right after the clone, while the fetched project is read.
        pub(super) static CANCEL_AFTER_CLONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

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

        // A repository whose project uses a library folder beside it is refused with why.
        let mut p = store::load(&base).unwrap();
        p.meta.libraries.push(wc_core::project::LibrarySource {
            name: "company".into(),
            kind: "folder".into(),
            location: "../library".into(),
            reference: None,
        });
        store::save(&base, &p).unwrap();
        Git::new(&base).commit("library beside it").unwrap();
        let e = create_from_base(&url, &tmp.path().join("z"), "Z", "2026-11-18").unwrap_err();
        assert!(e.to_string().contains("../library"), "{e}");
        assert!(!tmp.path().join("z").exists());
    }

    #[test]
    fn a_cancelled_fetch_makes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        store::create(&base, "ACME base").unwrap();
        Git::new(&base).init().unwrap();
        let new = tmp.path().join("new");
        let cancel = AtomicBool::new(true);
        let e = create_from_base_until(&file_url(&base), &new, "New", "2026-11-19", &cancel).unwrap_err();
        assert!(matches!(e, crate::Error::Cancelled), "{e}");
        assert!(!new.exists());
        // Cancelled once the clone is in, before the project is made: still nothing.
        CANCEL_AFTER_CLONE.set(true);
        let cancel = AtomicBool::new(false);
        let e = create_from_base_until(&file_url(&base), &new, "New", "2026-11-19", &cancel).unwrap_err();
        CANCEL_AFTER_CLONE.set(false);
        assert!(matches!(e, crate::Error::Cancelled), "{e}");
        assert!(!new.exists());
        // Not cancelled, the same call makes the project.
        let cancel = AtomicBool::new(false);
        create_from_base_until(&file_url(&base), &new, "New", "2026-11-19", &cancel).unwrap();
        assert_eq!(store::load(&new).unwrap().meta.name, "New");
    }

    #[cfg(unix)]
    #[test]
    fn a_git_command_is_stopped_while_it_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            stop.store(true, Ordering::Relaxed);
        });
        // A git command that takes 30 seconds, like a fetch from a server that does not answer.
        let started = std::time::Instant::now();
        let r = Git::new(tmp.path()).run_until(&["-c", "alias.wait=!sleep 30", "wait"], &cancel);
        setter.join().unwrap();
        assert!(matches!(r, Err(crate::Error::Cancelled)), "{r:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_git_command_says_why_also_in_bytes_that_are_not_utf8() {
        let tmp = tempfile::tempdir().unwrap();
        let r = Git::new(tmp.path()).run_until(
            &["-c", "alias.bad=!printf 'no \\377 such server' >&2; exit 1", "bad"],
            &AtomicBool::new(false),
        );
        let Err(crate::Error::Git { message, .. }) = r else {
            unreachable!("{r:?}")
        };
        assert!(message.contains("no \u{fffd} such server"), "{message:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_clone_folder_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        const CHILD: &str = "WC_PRIVATE_TEMP_DIR_CHILD";
        if std::env::var_os(CHILD).is_some() {
            // Run under umask 022, where a plain temporary folder would be 0755.
            let tmp = private_temp_dir().unwrap();
            let mode = std::fs::metadata(tmp.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "mode {mode:o}");
            let plain = tempfile::tempdir().unwrap();
            let open = std::fs::metadata(plain.path()).unwrap().permissions().mode();
            assert_eq!(open & 0o777, 0o755, "the umask is in force: mode {open:o}");
            return;
        }
        // The umask belongs to the whole process, so set it in a child of its own.
        let exe = std::env::current_exe().unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("umask 022 && exec \"$0\" --exact --test-threads 1 base::tests::the_clone_folder_is_private_to_its_owner")
            .arg(&exe)
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
