//! `wiringcraft jumper` run as a program: refused adds and removes leave the project as it
//! was, and jumpers that share a terminal become one.

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

/// The error text of a run that must fail.
fn refused(dir: &Path, args: &[&str]) -> Result<String, String> {
    match wiringcraft(dir, args) {
        Ok(out) => Err(format!("{args:?} should fail, printed {out}")),
        Err(err) => Ok(err),
    }
}

/// A project with terminals -X1.1, -X1.2 and -X1.3 drawn on one page.
fn project(tmp: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = tmp.join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
    let lib = tmp.join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    let symbols = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols");
    std::fs::copy(symbols.join("wc.terminal.json"), lib.join("symbols/wc.terminal.json"))?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    for (n, x) in [("1", "40"), ("2", "60"), ("3", "80")] {
        wiringcraft(
            &dir,
            &[
                "place",
                "p1",
                "wc.terminal",
                "--at",
                &format!("{x},40"),
                &format!("--tag=-X1.{n}"),
            ],
        )?;
    }
    Ok(dir)
}

#[test]
fn adding_a_terminal_the_strip_does_not_have_is_refused_and_saves_nothing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let err = refused(&dir, &["jumper", "add", "-X1", "1", "9"])?;
    assert!(err.contains("-X1 has no terminal 9"), "{err}");
    let err = refused(&dir, &["jumper", "add", "-X5", "1", "2"])?;
    assert!(err.contains("no terminal strip -X5"), "{err}");
    assert_eq!(wiringcraft(&dir, &["jumper", "list"])?, "");
    Ok(())
}

#[test]
fn a_jumper_needs_two_different_terminals() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    for args in [
        &["jumper", "add", "-X1", "1"][..],
        &["jumper", "add", "-X1", "2", " 2 "][..],
    ] {
        let err = refused(&dir, args)?;
        assert!(
            err.contains("a jumper needs at least two different terminals"),
            "{args:?}: {err}"
        );
    }
    assert_eq!(wiringcraft(&dir, &["jumper", "list"])?, "");
    Ok(())
}

#[test]
fn jumpers_sharing_a_terminal_become_one_and_removing_splits_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    wiringcraft(&dir, &["jumper", "add", "-X1", "3", "2"])?;
    assert_eq!(wiringcraft(&dir, &["jumper", "list"])?, "-X1 2 3\n");
    wiringcraft(&dir, &["jumper", "add", "-X1", "1", "2"])?;
    assert_eq!(wiringcraft(&dir, &["jumper", "list"])?, "-X1 1 2 3\n");
    wiringcraft(&dir, &["jumper", "remove", "-X1", " 2 "])?;
    assert_eq!(wiringcraft(&dir, &["jumper", "list"])?, "-X1 1 3\n");
    Ok(())
}

#[test]
fn removing_or_giving_a_part_to_a_terminal_in_no_jumper_is_refused() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    wiringcraft(&dir, &["jumper", "add", "-X1", "1", "2"])?;
    let err = refused(&dir, &["jumper", "remove", "-X1", "3"])?;
    assert!(err.contains("no jumper of -X1 has terminal 3"), "{err}");
    let err = refused(&dir, &["jumper", "part", "-X1", "3", "wago.2002-402"])?;
    assert!(err.contains("no jumper of -X1 has terminal 3"), "{err}");
    // Clearing a part the jumper does not have changes nothing and is no error.
    wiringcraft(&dir, &["jumper", "part", "-X1", "1"])?;
    assert_eq!(wiringcraft(&dir, &["jumper", "list"])?, "-X1 1 2\n");
    Ok(())
}
