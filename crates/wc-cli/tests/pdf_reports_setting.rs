//! `wiringcraft pdf-reports`: the project's choice of report pages, which `pdf` prints when no
//! `--report` is given (#1882).

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

fn ok(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let done = run(dir, args)?;
    if !done.status.success() {
        return Err(String::from_utf8_lossy(&done.stderr).into_owned().into());
    }
    Ok(String::from_utf8_lossy(&done.stdout).into_owned())
}

/// The page count of `pdf` with `args`, as `wrote <file> (N pages)` says it.
fn pages(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = dir.join("out.pdf");
    let mut all = vec!["pdf", out.to_str().ok_or("path")?];
    all.extend_from_slice(args);
    let text = ok(dir, &all)?;
    let count = text.rsplit_once('(').map(|(_, n)| n.trim().to_owned());
    count.ok_or_else(|| format!("no page count in {text:?}").into())
}

#[test]
fn the_project_s_report_choice_is_printed_when_none_is_asked_for() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let bare = pages(&dir, &[])?;
    let parts = pages(&dir, &["--report", "parts"])?;
    let all = pages(&dir, &["--report", "all"])?;
    assert_ne!(bare, parts);

    // Not chosen yet: said so, and `pdf` prints only what it is asked for.
    assert!(ok(&dir, &["pdf-reports"])?.starts_with("no reports chosen"));
    // Names are checked and kept by their own name, each once.
    assert_eq!(ok(&dir, &["pdf-reports", "parts", "parts"])?.trim(), "parts");
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    assert!(toml.contains("pdf_reports = [\"parts\"]"), "{toml}");
    assert_eq!(pages(&dir, &[])?, parts);
    // `--report` still wins, and `none` prints no report.
    assert_eq!(pages(&dir, &["--report", "all"])?, all);
    assert_eq!(pages(&dir, &["--report", "none"])?, bare);
    assert_eq!(ok(&dir, &["pdf-reports", "all"])?.trim(), "all");
    assert_eq!(pages(&dir, &[])?, all);
    assert_eq!(ok(&dir, &["pdf-reports", "none"])?.trim(), "none");
    assert_eq!(pages(&dir, &[])?, bare);

    // A wrong name changes nothing.
    let done = run(&dir, &["pdf-reports", "parts", "wirez"])?;
    assert!(!done.status.success());
    assert!(String::from_utf8_lossy(&done.stderr).contains("\"wirez\""));
    let done = run(&dir, &["pdf-reports", "none", "parts"])?;
    assert!(!done.status.success());
    assert_eq!(ok(&dir, &["pdf-reports"])?.trim(), "none");

    // A name the project file holds that is no report is named with where it comes from.
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    std::fs::write(
        dir.join("project.toml"),
        toml.replace("pdf_reports = []", "pdf_reports = [\"wirez\"]"),
    )?;
    let out = dir.join("out.pdf");
    let done = run(&dir, &["pdf", out.to_str().ok_or("path")?])?;
    assert!(!done.status.success());
    let err = String::from_utf8_lossy(&done.stderr);
    assert!(err.contains("pdf_reports"), "{err}");
    // Listed, a control character in a stored name is shown escaped.
    std::fs::write(
        dir.join("project.toml"),
        toml.replace("pdf_reports = []", "pdf_reports = [\"bad\\u001b[2J\"]"),
    )?;
    let listed = ok(&dir, &["pdf-reports"])?;
    assert!(!listed.contains('\u{1b}') && listed.contains("bad"), "{listed:?}");

    assert!(ok(&dir, &["pdf-reports", "--clear"])?.starts_with("no reports chosen"));
    assert!(!std::fs::read_to_string(dir.join("project.toml"))?.contains("pdf_reports"));
    assert_eq!(pages(&dir, &[])?, bare);
    Ok(())
}
