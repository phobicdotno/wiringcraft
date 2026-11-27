//! `wiringcraft page list` and `check`: page numbers are printed escaped, run as a program.

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wiringcraft(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn page_list_and_check_escape_control_characters_in_a_page_number() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    // The number is only trimmed, so a tab, a line break and ESC are kept in it.
    wiringcraft(&dir, &["page", "add", "p1", "--number", "3\u{1b}[2J\tx\ny"])?;

    let list = wiringcraft(&dir, &["page", "list"])?;
    assert!(!list.contains('\u{1b}') && !list.contains('\t'), "{list:?}");
    assert_eq!(list.lines().count(), 2, "one line per page: {list:?}");
    assert!(list.contains("p1         3\\u{1b}[2J\\tx\\ny "), "{list:?}");

    // A wire with both ends in the air gives findings on the page.
    wiringcraft(&dir, &["wire", "p1", "0,0", "10,0"])?;
    let check = wiringcraft(&dir, &["check"])?;
    assert!(!check.contains('\u{1b}') && !check.contains('\t'), "{check:?}");
    assert!(
        check.contains("page 3\\u{1b}[2J\\tx\\ny: wire ends in the air"),
        "{check:?}"
    );
    assert_eq!(check.lines().count(), 3, "two findings and the count: {check:?}");
    Ok(())
}
