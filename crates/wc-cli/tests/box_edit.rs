//! `wiringcraft box-edit`, run as a program so what it prints, and where, is what is checked.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SYMBOL: &str = "box.p0001.s1";

fn run(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
}

/// A new project whose first page holds box `s1` with point `A` and wire `w1` on it.
fn project_with_box(tmp: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = tmp.join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    for args in [
        &["box", "p0001", "--at", "20,20", "--size", "30,20", "--pin", "A:left:10"][..],
        &["wire", "p0001", "s1:A", "5,30"][..],
    ] {
        let out = run(&dir, args)?;
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(dir)
}

/// Every file under `dir` with its bytes.
fn files(dir: &Path) -> std::io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in std::fs::read_dir(&d)? {
            let path = entry?.path();
            if path.is_dir() {
                todo.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path)?);
            }
        }
    }
    Ok(out)
}

#[test]
fn box_edit_escapes_control_characters_in_its_warnings() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project_with_box(tmp.path())?;
    let mut p = wc_core::store::load(&dir)?;
    p.pages[0].number = "1\u{1b}[31m".into();
    wc_core::store::save(&dir, &p)?;

    // Taking the point off leaves `w1` with a free end, which is warned about with the page number.
    let out = run(&dir, &["box-edit", "p0001", "s1", "--clear-pins"])?;
    let stderr = String::from_utf8(out.stderr)?;
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("warning: wire w1 on page 1\\u{1b}[31m ended on connection point A of s1"),
        "{stderr}"
    );
    assert!(!stderr.chars().any(|c| c.is_control() && c != '\n'), "{stderr:?}");
    Ok(())
}

#[test]
fn box_edit_refuses_a_box_the_project_does_not_pin_and_changes_nothing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project_with_box(tmp.path())?;
    let mut p = wc_core::store::load(&dir)?;
    assert!(p.lock.symbols.remove(SYMBOL).is_some());
    wc_core::store::save(&dir, &p)?;
    let before = files(&dir)?;

    let out = run(&dir, &["box-edit", "p0001", "s1", "--size", "40,30"])?;
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr)?;
    assert!(
        stderr.contains(&format!("the project does not pin symbol {SYMBOL}")),
        "{stderr}"
    );
    // `lib lock` cannot pin a box no library has: the lock entry comes back from history.
    assert!(
        stderr.contains("library.lock back from the project's history"),
        "{stderr}"
    );
    assert!(!stderr.contains("not the version the project pins"), "{stderr}");
    assert_eq!(files(&dir)?, before, "a refused edit leaves the files as they were");
    Ok(())
}

#[test]
fn box_edit_refuses_a_box_file_changed_outside_and_changes_nothing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project_with_box(tmp.path())?;
    let mut symbol = wc_core::store::read_vendored_symbol(&dir, SYMBOL)?;
    symbol.description = "changed by hand".into();
    wc_core::store::write_vendored_symbol(&dir, &symbol)?;
    let before = files(&dir)?;

    let out = run(&dir, &["box-edit", "p0001", "s1", "--size", "40,30"])?;
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr)?;
    assert!(
        stderr.contains(&format!(
            "the file of symbol {SYMBOL} is not the version the project pins"
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("does not pin"), "{stderr}");
    assert_eq!(files(&dir)?, before, "a refused edit leaves the files as they were");
    Ok(())
}
