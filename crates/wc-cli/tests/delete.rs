//! `wiringcraft delete` run as a program: an id that is not on the page is an error and
//! nothing changes, an unknown page is an error, and a deleted symbol takes the wires on its
//! pins along.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
}

fn wiringcraft(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = run(dir, args).map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A new project with page `p1` holding two free wires; returns the project dir and the ids.
fn project_with_wires(tmp: &Path) -> Result<(PathBuf, String, String), Box<dyn std::error::Error>> {
    let dir = tmp.join("p");
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .output()?;
    assert!(out.status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
    let a = wiringcraft(&dir, &["wire", "p1", "10,10", "50,10"])?.trim().to_owned();
    let b = wiringcraft(&dir, &["wire", "p1", "10,30", "50,30"])?.trim().to_owned();
    assert_ne!(a, b);
    Ok((dir, a, b))
}

/// Every file in the project with its bytes, to see that a failed delete wrote nothing.
fn snapshot(dir: &Path) -> std::io::Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut files = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in std::fs::read_dir(&d)? {
            let path = entry?.path();
            if path.is_dir() {
                todo.push(path);
            } else {
                let bytes = std::fs::read(&path)?;
                files.push((path, bytes));
            }
        }
    }
    files.sort();
    Ok(files)
}

#[test]
fn an_unknown_id_is_named_and_nothing_changes() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (dir, a, _) = project_with_wires(tmp.path())?;
    let before = snapshot(&dir)?;

    let out = run(&dir, &["delete", "p1", &a, "w99"])?;
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no symbol or wire \"w99\" on page p1"), "{err}");
    assert!(
        !err.contains(&format!("\"{a}\"")),
        "only the missing id is named: {err}"
    );
    assert!(out.stdout.is_empty(), "nothing printed as deleted");
    assert!(
        snapshot(&dir)? == before,
        "the existing wire is kept and nothing is written"
    );
    Ok(())
}

#[test]
fn every_missing_id_is_listed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (dir, _, _) = project_with_wires(tmp.path())?;
    let out = run(&dir, &["delete", "p1", "s7", "w99"])?;
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no symbol or wire \"s7\", \"w99\" on page p1"), "{err}");
    Ok(())
}

#[test]
fn an_unknown_page_is_an_error() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (dir, a, _) = project_with_wires(tmp.path())?;
    let before = snapshot(&dir)?;
    let out = run(&dir, &["delete", "nope", &a])?;
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("nope"), "the page is named: {err}");
    assert!(snapshot(&dir)? == before);
    Ok(())
}

#[test]
fn no_ids_is_a_usage_error() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (dir, _, _) = project_with_wires(tmp.path())?;
    let out = run(&dir, &["delete", "p1"])?;
    assert_eq!(out.status.code(), Some(2), "clap usage error");
    Ok(())
}

#[test]
fn a_deleted_wire_goes_and_the_other_stays() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (dir, a, b) = project_with_wires(tmp.path())?;
    let printed = wiringcraft(&dir, &["delete", "p1", &a])?;
    assert_eq!(printed.lines().collect::<Vec<_>>(), [a.as_str()]);
    // Deleting it again fails: it is gone. The other wire is still there.
    let again = run(&dir, &["delete", "p1", &a])?;
    assert!(!again.status.success());
    assert_eq!(wiringcraft(&dir, &["delete", "p1", &b])?.trim(), b);
    Ok(())
}
