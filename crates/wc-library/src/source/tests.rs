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

/// One source synced through [`sync_all`], as the project's sync does it.
fn sync_one(project_dir: &Path, s: &LibrarySource) -> Result<String> {
    let mut outcomes = sync_all(project_dir, std::slice::from_ref(s))?;
    assert_eq!(outcomes.len(), 1);
    outcomes.pop().unwrap_or_else(|| unreachable!())
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

#[test]
fn cache_root_order() {
    let os = |s: &str| Some(OsString::from(s));
    assert_eq!(
        cache_root_from(os("/c/wc"), os("/c/xdg"), os("/c/home")),
        Some(PathBuf::from("/c/wc"))
    );
    assert_eq!(
        cache_root_from(None, os("/c/xdg"), os("/c/home")),
        Some(PathBuf::from("/c/xdg/wiringcraft"))
    );
    assert_eq!(
        cache_root_from(None, None, os("/c/home")),
        Some(PathBuf::from("/c/home/.cache/wiringcraft"))
    );
    assert_eq!(cache_root_from(None, None, None), None);
}

#[test]
fn git_source_clones_then_updates() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(io)?;
    let upstream = tmp.path().join("upstream");
    let cache = tmp.path().join("cache");
    std::fs::create_dir(&upstream).map_err(io)?;

    // main is one commit ahead of the v1 tag, so a pinned clone differs from a default one.
    run_git(&upstream, &["init", "-q", "-b", "main"])?;
    commit_file(&upstream, "v1")?;
    run_git(&upstream, &["tag", "v1"])?;
    commit_file(&upstream, "v2")?;

    let loc = upstream.to_string_lossy().into_owned();
    let latest = src("latest", "git", &loc, None);
    let pinned = src("pinned", "git", &loc, Some("v1"));
    let proj = tmp.path();
    let sync = |s: &LibrarySource| sync_one_in(proj, s, Some(&cache));

    // Resolving a git source points at its folder in the cache.
    assert_eq!(
        Source::resolve_in(proj, &latest, Some(&cache))?.root,
        cache.join("git").join("latest")
    );

    // First sync clones: without a reference it takes the default branch, with one that tag.
    assert_eq!(sync(&latest)?, format!("latest: cloned {loc}"));
    assert_eq!(sync(&pinned)?, format!("pinned: cloned {loc}"));
    assert_eq!(read(&cache.join("git/latest/lib.txt"))?, "v2");
    assert_eq!(read(&cache.join("git/pinned/lib.txt"))?, "v1");

    // A new upstream commit: the next sync fetches and resets; the pinned source stays on v1.
    commit_file(&upstream, "v3")?;
    assert_eq!(sync(&latest)?, format!("latest: updated from {loc}"));
    assert_eq!(sync(&pinned)?, format!("pinned: updated from {loc}"));
    assert_eq!(read(&cache.join("git/latest/lib.txt"))?, "v3");
    assert_eq!(read(&cache.join("git/pinned/lib.txt"))?, "v1");
    Ok(())
}

#[test]
fn git_source_follows_a_changed_location() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(io)?;
    let cache = tmp.path().join("cache");
    let (old, new) = (tmp.path().join("old"), tmp.path().join("new"));
    for (repo, text) in [(&old, "old"), (&new, "new")] {
        std::fs::create_dir(repo).map_err(io)?;
        run_git(repo, &["init", "-q", "-b", "main"])?;
        commit_file(repo, text)?;
    }
    let sync = |loc: &Path| {
        sync_one_in(
            tmp.path(),
            &src("lib", "git", &loc.to_string_lossy(), None),
            Some(&cache),
        )
    };

    sync(&old)?;
    assert_eq!(read(&cache.join("git/lib/lib.txt"))?, "old");
    // The user points the source at another repository: the next sync takes that one.
    assert_eq!(sync(&new)?, format!("lib: updated from {}", new.display()));
    assert_eq!(read(&cache.join("git/lib/lib.txt"))?, "new");
    Ok(())
}

#[test]
fn a_failing_source_does_not_stop_the_ones_after_it() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(io)?;
    let cache = tmp.path().join("cache");
    let upstream = tmp.path().join("upstream");
    std::fs::create_dir(&upstream).map_err(io)?;
    run_git(&upstream, &["init", "-q", "-b", "main"])?;
    commit_file(&upstream, "v1")?;
    let loc = upstream.to_string_lossy().into_owned();
    let missing = tmp.path().join("no-such-repo").to_string_lossy().into_owned();
    let sources = [
        src("gone", "git", &missing, None),
        src("odd", "ftp", "somewhere", None),
        src("good", "git", &loc, None),
    ];

    let outcomes = sync_all_in(tmp.path(), &sources, Some(&cache))?;
    assert_eq!(outcomes.len(), 3);
    let mut outcomes = outcomes.into_iter();
    let (name, _) = source_message(outcomes.next().unwrap_or_else(|| unreachable!()));
    assert_eq!(name, "gone");
    let (name, message) = source_message(outcomes.next().unwrap_or_else(|| unreachable!()));
    assert_eq!(name, "odd");
    assert!(message.contains("\"ftp\""), "{message}");
    // The source after the failing ones is still cloned.
    assert_eq!(
        outcomes.next().unwrap_or_else(|| unreachable!())?,
        format!("good: cloned {loc}")
    );
    assert_eq!(read(&cache.join("git/good/lib.txt"))?, "v1");
    Ok(())
}

#[test]
fn two_sources_with_one_name_are_refused_before_anything_is_synced() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(io)?;
    let cache = tmp.path().join("cache");
    let upstream = tmp.path().join("upstream");
    std::fs::create_dir(&upstream).map_err(io)?;
    run_git(&upstream, &["init", "-q", "-b", "main"])?;
    commit_file(&upstream, "v1")?;
    let loc = upstream.to_string_lossy().into_owned();
    // The names differ only in case: on Windows and macOS they would share one cache folder.
    let sources = [src("lib", "git", &loc, None), src("Lib", "git", &loc, None)];

    let (name, message) = source_message(sync_all_in(tmp.path(), &sources, Some(&cache)));
    assert_eq!(name, "Lib");
    assert!(message.contains("two library sources"), "{message}");
    assert!(!cache.join("git").exists(), "nothing was synced");
    Ok(())
}

#[test]
fn redact_location_hides_the_user_and_password_of_a_url() {
    assert_eq!(
        redact_location("https://user:token@example.com/lib.git"),
        "https://***@example.com/lib.git"
    );
    assert_eq!(redact_location("https://token@example.com"), "https://***@example.com");
    // An @ after the host is part of the path, not a user name.
    assert_eq!(
        redact_location("https://example.com/a@b/lib.git"),
        "https://example.com/a@b/lib.git"
    );
    assert_eq!(redact_location("git@example.com:lib.git"), "git@example.com:lib.git");
    assert_eq!(redact_location("../lib"), "../lib");
}

#[test]
fn redact_location_hides_the_query_and_fragment_of_a_url() {
    assert_eq!(
        redact_location("https://u:p@host/path?token=x#f"),
        "https://***@host/path?***"
    );
    assert_eq!(redact_location("https://host/path?token=x"), "https://host/path?***");
    assert_eq!(redact_location("https://host/path#f"), "https://host/path#***");
    assert_eq!(redact_location("https://u@host?token=x"), "https://***@host?***");
}

#[test]
fn git_error_text_is_redacted_in_every_form_of_the_url() {
    let location = "https://u:p@host/lib.git?token=x";
    let shown = "https://***@host/lib.git?***";
    for printed in [location, "https://host/lib.git?token=x", "host/lib.git?token=x"] {
        let text = redact_text(&format!("fatal: unable to access '{printed}': refused"), location);
        assert_eq!(text, format!("fatal: unable to access '{shown}': refused"));
    }
}

#[test]
fn git_credential_prompts_do_not_show_the_user_name_or_password() {
    let location = "https://user:pw@host/lib.git";
    for printed in ["https://user:pw@host", "https://user@host", "user:pw@host"] {
        let text = redact_text(
            &format!("fatal: could not read Password for '{printed}': terminal prompts disabled"),
            location,
        );
        assert!(!text.contains("user") && !text.contains("pw"), "{text}");
        assert!(text.contains("***@host"), "{text}");
    }
}

// A file URL with a host needs a Unix path after it.
#[cfg(unix)]
#[test]
fn sync_messages_do_not_show_a_password_in_the_location() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(io)?;
    let cache = tmp.path().join("cache");
    let upstream = tmp.path().join("upstream");
    std::fs::create_dir(&upstream).map_err(io)?;
    run_git(&upstream, &["init", "-q", "-b", "main"])?;
    commit_file(&upstream, "v1")?;
    let path = upstream.to_string_lossy();
    let url = format!("file://user:secret@localhost{path}");
    let shown = format!("file://***@localhost{path}");
    let missing = format!("file://user:secret@localhost{path}-gone");

    let sync = |s: &LibrarySource| sync_one_in(tmp.path(), s, Some(&cache));
    assert_eq!(sync(&src("lib", "git", &url, None))?, format!("lib: cloned {shown}"));
    assert_eq!(
        sync(&src("lib", "git", &url, None))?,
        format!("lib: updated from {shown}")
    );
    let (_, message) = source_message(sync(&src("gone", "git", &missing, None)));
    assert!(!message.contains("secret"), "{message}");
    Ok(())
}
