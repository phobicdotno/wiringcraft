//! `wiringcraft revision`: revisions, clouds and the revision table, run as a program.

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
fn revisions_clouds_and_the_table_are_kept_in_the_project() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "1"])?;

    assert!(
        wiringcraft(&dir, &["revision", "cloud", "p1", "10", "10", "50", "40"]).is_err(),
        "a cloud needs a revision first"
    );
    wiringcraft(
        &dir,
        &[
            "revision",
            "add",
            "A",
            "--text",
            "First issue",
            "--date",
            "2026-11-01",
            "--by",
            "OP",
        ],
    )?;
    wiringcraft(&dir, &["revision", "add", "B", "--text", "Motor added"])?;
    assert!(
        wiringcraft(&dir, &["revision", "add", "B"]).is_err(),
        "letters are unique"
    );
    wiringcraft(&dir, &["revision", "edit", "B", "--date", "2026-11-18"])?;
    assert_eq!(
        wiringcraft(&dir, &["revision", "cloud", "p1", "50", "40", "10", "10"])?.trim(),
        "0"
    );
    wiringcraft(&dir, &["revision", "table", "p1", "200", "20"])?;

    let list = wiringcraft(&dir, &["revision", "list"])?;
    assert!(list.contains("A\t2026-11-01\tOP\tFirst issue"), "{list}");
    assert!(list.contains("B\t2026-11-18\t\tMotor added"), "{list}");
    assert!(
        list.contains("page p1 cloud 0: revision B at 10.0,10.0 to 50.0,40.0"),
        "{list}"
    );
    assert!(list.contains("page p1 table at 200.0,20.0"), "{list}");

    assert!(
        wiringcraft(&dir, &["revision", "remove", "B"]).is_err(),
        "a cloud still carries B"
    );
    wiringcraft(&dir, &["revision", "uncloud", "p1", "--rev", "B"])?;
    wiringcraft(&dir, &["revision", "remove", "B"])?;
    wiringcraft(&dir, &["revision", "table", "p1", "--off"])?;
    let list = wiringcraft(&dir, &["revision", "list"])?;
    assert!(
        !list.contains("cloud") && !list.contains("table") && !list.contains("Motor"),
        "{list}"
    );
    Ok(())
}
