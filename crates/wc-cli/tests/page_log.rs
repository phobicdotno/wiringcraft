//! `wiringcraft vcs page-log`: one page's recorded history, run as a program.

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

/// A page the project does not have and never recorded is refused, naming the pages it has;
/// a removed page still shows the history it recorded (#1052).
#[test]
fn page_log_refuses_an_unknown_page_but_keeps_a_removed_pages_history() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p2", "--number", "2"])?;
    wiringcraft(&dir, &["vcs", "commit", "-m", "Second page"])?;

    let log = wiringcraft(&dir, &["vcs", "page-log", "2"])?;
    assert!(log.contains("Second page"), "{log}");

    let Err(err) = wiringcraft(&dir, &["vcs", "page-log", "nope"]) else {
        unreachable!("an unknown page is refused");
    };
    assert!(err.contains("no page \"nope\""), "{err}");
    assert!(err.contains("p2"), "names the pages there are: {err}");

    wiringcraft(&dir, &["page", "remove", "p2"])?;
    wiringcraft(&dir, &["vcs", "commit", "-m", "Page gone"])?;
    let log = wiringcraft(&dir, &["vcs", "page-log", "p2"])?;
    assert!(log.contains("Page gone") && log.contains("Second page"), "{log}");
    Ok(())
}

/// A commit subject or author holding ESC is printed escaped by `vcs log` and `vcs page-log`,
/// so a commit merged from someone else cannot steer the terminal (#1324).
#[test]
fn log_and_page_log_escape_control_characters_from_git() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p2", "--number", "2"])?;
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["vcs", "commit", "-m", "red\u{1b}[31mpage"])
        .env("GIT_AUTHOR_NAME", "Ev\u{1b}[2Jil")
        .env("GIT_AUTHOR_EMAIL", "tester@example.invalid")
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.invalid")
        .status()?;
    assert!(status.success());

    let log = wiringcraft(&dir, &["vcs", "log"])?;
    assert!(!log.contains('\u{1b}'), "{log:?}");
    assert!(log.contains("red\\u{1b}[31mpage"), "{log}");

    let log = wiringcraft(&dir, &["vcs", "page-log", "2"])?;
    assert!(!log.contains('\u{1b}'), "{log:?}");
    assert!(log.contains("red\\u{1b}[31mpage"), "{log}");
    assert!(log.contains("Ev\\u{1b}[2Jil"), "{log}");
    Ok(())
}
