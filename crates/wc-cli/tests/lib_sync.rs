//! `wiringcraft lib sync`, run as a program so its output, messages and exit status are what is
//! checked.

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wiringcraft(dir: &Path, cache: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("WIRINGCRAFT_CACHE", cache)
        .output()
}

#[test]
fn lib_sync_goes_on_past_failing_sources_and_reports_them() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let cache = tmp.path().join("cache");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    std::fs::create_dir(dir.join("lib"))?;
    let gone = tmp.path().join("no-such-repo").to_string_lossy().into_owned();
    let lost = tmp.path().join("no-other-repo").to_string_lossy().into_owned();

    // The failing sources come first, so a sync that stops at a failure never reaches "local".
    for add in [
        ["lib", "add", "gone", "git", gone.as_str()],
        ["lib", "add", "lost", "git", lost.as_str()],
        ["lib", "add", "local", "folder", "lib"],
    ] {
        let out = wiringcraft(&dir, &cache, &add)?;
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    let out = wiringcraft(&dir, &cache, &["lib", "sync"])?;
    let stdout = String::from_utf8(out.stdout)?;
    let stderr = String::from_utf8(out.stderr)?;
    assert!(!out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    // The folder's path is shown as the command resolved it, so only its shape is checked.
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("local: folder ") && l.ends_with("lib")),
        "{stdout}"
    );
    assert!(!stdout.contains("gone") && !stdout.contains("lost"), "{stdout}");
    assert!(stderr.contains("library source \"gone\""), "{stderr}");
    assert!(stderr.contains("library source \"lost\""), "{stderr}");
    assert!(
        stderr.contains("error: 2 of 3 library sources could not be synced"),
        "{stderr}"
    );
    Ok(())
}

/// `lib add` puts a source after the existing ones, and the help of `lib add` and of
/// `import swe-library` says so instead of telling the reader to add it "ahead of" others
/// (#1827).
#[test]
fn lib_add_appends_the_source_and_the_help_says_so() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let cache = tmp.path().join("cache");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    for add in [
        ["lib", "add", "old", "folder", "a"],
        ["lib", "add", "new", "folder", "b"],
    ] {
        let out = wiringcraft(&dir, &cache, &add)?;
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    let project = wc_core::store::load(&dir)?;
    let names: Vec<&str> = project.meta.libraries.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names.last().copied(), Some("new"), "{names:?}");
    let old = names.iter().position(|n| *n == "old");
    assert!(
        old.is_some() && old < names.iter().position(|n| *n == "new"),
        "{names:?}"
    );

    for args in [["import", "swe-library", "--help"], ["lib", "add", "--help"]] {
        let out = wiringcraft(&dir, &cache, &args)?;
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        // Wrapped lines are joined before the text is checked.
        let help = String::from_utf8(out.stdout)?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!help.contains("ahead of"), "{args:?}: {help}");
        assert!(
            help.contains("arlier sources win") && help.contains("`[[library]]`") && help.contains("`project.toml`"),
            "{args:?}: {help}"
        );
    }
    Ok(())
}

#[test]
fn lib_add_refuses_a_name_that_differs_only_in_case() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let cache = tmp.path().join("cache");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());

    let out = wiringcraft(&dir, &cache, &["lib", "add", "lib", "folder", "a"])?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // `lib sync` refuses two names that match ignoring case, so `lib add` must not save one.
    let out = wiringcraft(&dir, &cache, &["lib", "add", "Lib", "folder", "b"])?;
    let stderr = String::from_utf8(out.stderr)?;
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("source \"Lib\" exists"), "{stderr}");
    let out = wiringcraft(&dir, &cache, &["lib", "sync"])?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}
