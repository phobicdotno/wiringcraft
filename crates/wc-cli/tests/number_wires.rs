//! `wiringcraft number-wires --rule`, run as a program.

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
}

#[test]
fn number_wires_reads_the_rule_as_the_assistant_writes_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    assert!(run(&dir, &["page", "add", "p1", "--number", "1"])?.status.success());
    for rule in ["page_column", "page-column", "Sequential", " potential "] {
        let out = run(&dir, &["number-wires", "--rule", rule])?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{rule:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains("numbered 0 connection(s)"), "{rule:?}: {stdout}");
    }
    let out = run(&dir, &["number-wires", "--rule", "potentials"])?;
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown rule \"potentials\""), "{stderr}");
    Ok(())
}
