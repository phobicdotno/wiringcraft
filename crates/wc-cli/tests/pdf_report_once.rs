//! `wiringcraft pdf --report`: a report named twice, or named besides `all`, prints once.

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The page count `pdf` prints for `reports`, as `wrote <file> (N pages)` says it.
fn pages(dir: &Path, out: &Path, reports: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiringcraft"));
    cmd.arg("-C").arg(dir).arg("pdf").arg(out);
    for r in reports {
        cmd.args(["--report", r]);
    }
    let done = cmd.output()?;
    let text = String::from_utf8_lossy(&done.stdout).into_owned();
    if !done.status.success() {
        return Err(String::from_utf8_lossy(&done.stderr).into_owned().into());
    }
    let count = text.rsplit_once('(').map(|(_, n)| n.trim().to_owned());
    count.ok_or_else(|| format!("no page count in {text:?}").into())
}

#[test]
fn a_report_named_twice_prints_once() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let out = tmp.path().join("out.pdf");

    let once = pages(&dir, &out, &["parts"])?;
    assert_eq!(pages(&dir, &out, &["parts", "parts"])?, once);
    let all = pages(&dir, &out, &["all"])?;
    assert_ne!(all, once);
    assert_eq!(pages(&dir, &out, &["all", "parts"])?, all);
    assert_eq!(pages(&dir, &out, &["parts", "all", "all"])?, all);
    Ok(())
}
