use super::*;

fn io(e: std::io::Error) -> Error {
    Error::Core(wc_core::Error::io("tmp", e))
}

fn src(name: &str, kind: &str, location: &str, reference: Option<&str>) -> LibrarySource {
    LibrarySource {
        name: name.into(),
        kind: kind.into(),
        location: location.into(),
        reference: reference.map(str::to_owned),
    }
}

/// Runs git in `dir` with a fixed identity, so tests do not depend on the user's git config.
fn run_git(dir: &Path, args: &[&str]) -> Result<()> {
    let mut all = vec![
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "tag.gpgsign=false",
    ];
    all.extend_from_slice(args);
    git(&all, Some(dir), "test")
}

fn commit_file(repo: &Path, text: &str) -> Result<()> {
    std::fs::write(repo.join("lib.txt"), text).map_err(io)?;
    run_git(repo, &["add", "lib.txt"])?;
    run_git(repo, &["commit", "-q", "-m", text])
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(io)
}

fn source_message(r: Result<impl std::fmt::Debug>) -> (String, String) {
    match r {
        Err(Error::Source { name, message }) => (name, message),
        other => unreachable!("expected a source error, got {other:?}"),
    }
}

#[test]
fn unknown_kind_is_an_error_naming_the_kind() {
    let s = src("odd", "ftp", "somewhere", None);
    let (name, message) = source_message(Source::resolve(Path::new("."), &s));
    assert_eq!(name, "odd");
    assert!(message.contains("\"ftp\""), "{message}");
    let (name, message) = source_message(sync_one(Path::new("."), &s));
    assert_eq!(name, "odd");
    assert!(message.contains("\"ftp\""), "{message}");
}

#[test]
fn missing_folder_is_reported_as_not_reachable() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(io)?;
    let s = src("gone", "folder", "no-such-folder", None);
    let msg = sync_one(tmp.path(), &s)?;
    assert!(msg.starts_with("gone: folder "), "{msg}");
    assert!(msg.ends_with(" is not reachable"), "{msg}");
    assert!(msg.contains("no-such-folder"), "{msg}");

    std::fs::create_dir(tmp.path().join("here")).map_err(io)?;
    let msg = sync_one(tmp.path(), &src("here", "folder", "here", None))?;
    assert!(!msg.contains("not reachable"), "{msg}");
    Ok(())
}

#[test]
fn git_source_with_an_invalid_name_is_rejected() {
    for bad in ["../escape", ".hidden", "a/b", ""] {
        let s = src(bad, "git", "/nowhere", None);
        let (name, message) = source_message(Source::resolve(Path::new("."), &s));
        assert_eq!(name, bad);
        assert!(message.contains("must be"), "{message}");
        let (_, message) = source_message(sync_one(Path::new("."), &s));
        assert!(message.contains("must be"), "{message}");
    }
}

/// Environment variables are process-wide, so everything that sets them lives in this one test.
#[test]
fn cache_root_order_and_git_sync() -> Result<()> {
    let saved: Vec<_> = ["WIRINGCRAFT_CACHE", "XDG_CACHE_HOME", "HOME"]
        .into_iter()
        .map(|k| (k, std::env::var_os(k)))
        .collect();
    let result = cache_root_order().and_then(|()| git_clone_and_update());
    for (k, v) in saved {
        // SAFETY: no other test in this crate reads or writes these variables.
        unsafe {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
    result
}

fn cache_root_order() -> Result<()> {
    // SAFETY: see `cache_root_order_and_git_sync`.
    unsafe {
        std::env::set_var("WIRINGCRAFT_CACHE", "/c/wc");
        std::env::set_var("XDG_CACHE_HOME", "/c/xdg");
        std::env::set_var("HOME", "/c/home");
    }
    assert_eq!(cache_root(), Some(PathBuf::from("/c/wc")));
    unsafe { std::env::remove_var("WIRINGCRAFT_CACHE") };
    assert_eq!(cache_root(), Some(PathBuf::from("/c/xdg/wiringcraft")));
    unsafe { std::env::remove_var("XDG_CACHE_HOME") };
    assert_eq!(cache_root(), Some(PathBuf::from("/c/home/.cache/wiringcraft")));
    Ok(())
}

fn git_clone_and_update() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(io)?;
    let upstream = tmp.path().join("upstream");
    let cache = tmp.path().join("cache");
    std::fs::create_dir(&upstream).map_err(io)?;
    // SAFETY: see `cache_root_order_and_git_sync`.
    unsafe { std::env::set_var("WIRINGCRAFT_CACHE", &cache) };

    run_git(&upstream, &["init", "-q", "-b", "main"])?;
    commit_file(&upstream, "v1")?;
    run_git(&upstream, &["tag", "v1"])?;

    let loc = upstream.to_string_lossy().into_owned();
    let latest = src("latest", "git", &loc, None);
    let pinned = src("pinned", "git", &loc, Some("v1"));
    let proj = tmp.path();

    // Resolving a git source points at its folder in the cache.
    assert_eq!(Source::resolve(proj, &latest)?.root, cache.join("git").join("latest"));

    // First sync clones: without a reference it takes the default branch, with one that tag.
    assert_eq!(sync_one(proj, &latest)?, format!("latest: cloned {loc}"));
    assert_eq!(sync_one(proj, &pinned)?, format!("pinned: cloned {loc}"));
    assert_eq!(read(&cache.join("git/latest/lib.txt"))?, "v1");
    assert_eq!(read(&cache.join("git/pinned/lib.txt"))?, "v1");

    // A new upstream commit: the next sync fetches and resets; the pinned source stays on v1.
    commit_file(&upstream, "v2")?;
    assert_eq!(sync_one(proj, &latest)?, format!("latest: updated from {loc}"));
    assert_eq!(sync_one(proj, &pinned)?, format!("pinned: updated from {loc}"));
    assert_eq!(read(&cache.join("git/latest/lib.txt"))?, "v2");
    assert_eq!(read(&cache.join("git/pinned/lib.txt"))?, "v1");
    Ok(())
}
