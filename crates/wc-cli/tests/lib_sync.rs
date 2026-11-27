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
