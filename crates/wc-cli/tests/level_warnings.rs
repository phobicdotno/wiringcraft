//! `wiringcraft report` and `wiringcraft pdf --report`: a terminal `Level` value that is not a
//! level is warned about when the terminal plan is printed, run as a program.

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()?)
}

/// Runs a command that must succeed and gives its standard error.
fn stderr(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = run(dir, args)?;
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(err.into());
    }
    Ok(err)
}

#[test]
fn an_ignored_terminal_level_is_warned_about_in_the_terminal_plan() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let mut p = wc_core::store::load(&dir)?;
    let tag = wc_core::DeviceTag::parse("-X1.3")?;
    p.devices
        .entry(tag)
        .or_default()
        .properties
        .insert(wc_report::LEVEL_PROPERTY.into(), "two".into());
    wc_core::store::save(&dir, &p)?;

    let warning = "warning: -X1.3: Level \"two\" is ignored:";
    let err = stderr(&dir, &["report", "terminals"])?;
    assert_eq!(err.matches(warning).count(), 1, "{err}");
    let err = stderr(&dir, &["report", "all", "--format", "csv"])?;
    assert!(err.contains(warning), "{err}");
    let pdf = tmp.path().join("out.pdf");
    let pdf = pdf.to_str().ok_or("path")?;
    let err = stderr(&dir, &["pdf", pdf, "--report", "terminals"])?;
    assert!(err.contains(warning), "{err}");

    // Other reports do not show levels, so they do not warn.
    let err = stderr(&dir, &["report", "parts"])?;
    assert!(!err.contains("Level"), "{err}");
    let err = stderr(&dir, &["pdf", pdf, "--report", "parts"])?;
    assert!(!err.contains("Level"), "{err}");
    let err = stderr(&dir, &["pdf", pdf])?;
    assert!(!err.contains("Level"), "{err}");
    Ok(())
}
