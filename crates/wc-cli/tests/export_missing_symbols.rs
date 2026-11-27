//! `wiringcraft aml`, `plc-vars` and `nets`: a symbol the project has no vendored copy of is
//! warned about once, as `wiringcraft pdf` does, since the output is built without it, run as
//! a program.

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

const WARNING: &str = "warning: symbol wc.plc.input has no vendored copy (run `wiringcraft lib lock`)";

/// A new project with a PLC input drawn with a symbol whose copy is not in its vendor folder.
fn project(tmp: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let dir = tmp.join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
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
    Ok(dir)
}

#[test]
fn aml_warns_about_a_symbol_without_a_vendored_copy() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let out = tmp.path().join("p.aml");
    let err = stderr(&dir, &["aml", out.to_str().ok_or("path")?])?;
    assert_eq!(err.matches(WARNING).count(), 1, "{err}");
    Ok(())
}

#[test]
fn plc_vars_warns_about_a_symbol_without_a_vendored_copy() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let out = tmp.path().join("vars.st");
    let err = stderr(&dir, &["plc-vars", out.to_str().ok_or("path")?])?;
    assert_eq!(err.matches(WARNING).count(), 1, "{err}");
    Ok(())
}

#[test]
fn nets_warns_about_a_symbol_without_a_vendored_copy() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let err = stderr(&dir, &["nets"])?;
    assert_eq!(err.matches(WARNING).count(), 1, "{err}");
    Ok(())
}
