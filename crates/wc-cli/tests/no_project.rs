//! A command run where there is no project says so, names the folder and how to get one (#555),
//! instead of an i/o error about `project.toml`.

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn stderr_of(dir: &Path, cwd: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .current_dir(cwd)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()?;
    assert!(!out.status.success());
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn check_in_an_empty_folder_says_there_is_no_project() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let err = stderr_of(Path::new("."), tmp.path(), &["check"])?;
    assert!(err.contains("no WiringCraft project in . (no project.toml)"), "{err}");
    assert!(err.contains("wiringcraft new <folder>"), "{err}");
    assert!(!err.contains("i/o error"), "{err}");
    Ok(())
}

#[test]
fn a_missing_project_folder_is_named() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let err = stderr_of(Path::new("nope"), tmp.path(), &["report", "parts"])?;
    assert!(err.contains("no WiringCraft project in nope"), "{err}");
    Ok(())
}

#[test]
fn a_broken_project_file_is_still_reported_as_such() -> TestResult {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("project.toml"), "not = [valid")?;
    let err = stderr_of(Path::new("."), tmp.path(), &["check"])?;
    assert!(!err.contains("no WiringCraft project"), "{err}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_broken_link_as_project_file_is_not_called_a_missing_project() -> TestResult {
    let tmp = tempfile::tempdir()?;
    std::os::unix::fs::symlink(tmp.path().join("gone.toml"), tmp.path().join("project.toml"))?;
    let err = stderr_of(Path::new("."), tmp.path(), &["check"])?;
    assert!(!err.contains("no WiringCraft project"), "{err}");
    Ok(())
}
