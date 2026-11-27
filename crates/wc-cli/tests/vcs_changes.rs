//! `wiringcraft vcs changes`: what changed between versions, run as a program.

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wiringcraft(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Tester")
        .env("GIT_AUTHOR_EMAIL", "tester@example.invalid")
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.invalid")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A page number or title holding ESC is listed escaped, so a page merged in from someone
/// else cannot steer the terminal (#1527).
#[test]
fn changes_escape_control_characters_in_page_numbers_and_titles() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["vcs", "init"])?;
    wiringcraft(
        &dir,
        &[
            "page",
            "add",
            "p2",
            "--number",
            "2\u{1b}[31m",
            "--title",
            "Ev\u{1b}[2Jil",
        ],
    )?;

    let out = wiringcraft(&dir, &["vcs", "changes"])?;
    assert!(!out.contains('\u{1b}'), "{out:?}");
    assert!(out.contains("page 2\\u{1b}[31m added (Ev\\u{1b}[2Jil)"), "{out}");
    Ok(())
}
