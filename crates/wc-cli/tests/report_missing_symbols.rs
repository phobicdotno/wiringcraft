//! `wiringcraft report`: a symbol the project has no vendored copy of is warned about, as
//! `wiringcraft pdf` does, since the reports are built without it (the PLC I/O list leaves its
//! points out), run as a program.

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
fn a_symbol_without_a_vendored_copy_is_warned_about_in_reports() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    // A PLC input drawn with a symbol whose copy is not in the project's vendor folder.
    let mut p = wc_core::store::load(&dir)?;
    let mut page = wc_core::Page::new("p2", "2", "PLC");
    page.symbols.push(wc_core::PlacedSymbol {
        id: "s1".into(),
        symbol: "wc.plc.input".into(),
        at: wc_core::geom::Point::new(50.0, 50.0),
        rotation: Default::default(),
        mirror: false,
        tag: Some(wc_core::DeviceTag::parse("-A1")?),
    });
    p.pages.push(page);
    wc_core::store::save(&dir, &p)?;

    let warning = "warning: symbol wc.plc.input has no vendored copy (run `wiringcraft lib lock`)";
    let err = stderr(&dir, &["report", "plc"])?;
    assert_eq!(err.matches(warning).count(), 1, "{err}");
    // Once per run, also for every report at once and in every format.
    let err = stderr(&dir, &["report", "all", "--format", "csv"])?;
    assert_eq!(err.matches(warning).count(), 1, "{err}");
    let xlsx = tmp.path().join("r.xlsx");
    let xlsx = xlsx.to_str().ok_or("path")?;
    let err = stderr(&dir, &["report", "all", "--format", "xlsx", "--out", xlsx])?;
    assert_eq!(err.matches(warning).count(), 1, "{err}");
    Ok(())
}
