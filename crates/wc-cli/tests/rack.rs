//! `wiringcraft rack`, run as a program so the rack warnings printed after a change are read
//! from stderr on their own.

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The program's stdout and stderr, each on its own; an error when it fails.
fn run(dir: &Path, args: &[&str]) -> Result<(String, String), Box<dyn std::error::Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()?;
    let stderr = String::from_utf8(out.stderr)?;
    if !out.status.success() {
        return Err(stderr.into());
    }
    Ok((String::from_utf8(out.stdout)?, stderr))
}

fn project(at: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let dir = at.join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    Ok(dir)
}

#[test]
fn a_card_added_without_a_start_address_is_saved_with_a_warning() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let (_, stderr) = run(&dir, &["rack", "add", "-A1"])?;
    assert!(!stderr.contains("warning:"), "{stderr}");
    let (_, stderr) = run(&dir, &["rack", "card", "add", "-A1", "3"])?;
    assert!(
        stderr.contains("warning: PLC rack \"-A1\" slot 3: start address \"\" is not a PLC address"),
        "{stderr}"
    );
    // The card is kept: the warning does not refuse the edit.
    let (stdout, _) = run(&dir, &["rack", "list"])?;
    assert!(stdout.contains('3'), "{stdout}");
    Ok(())
}
