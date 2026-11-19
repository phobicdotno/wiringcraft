//! Output files while someone else holds a page in a shared session (#458): the app names the
//! project in `WIRINGCRAFT_GUARD_PROJECT`, and the command line refuses to write an output
//! file that lands on a project file, also through a link made after the app looked.

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wiringcraft(project: &Path, guard: Option<&Path>, args: &[&Path]) -> std::io::Result<Output> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wiringcraft"));
    cmd.arg("-C").arg(project).arg("plc-vars").args(args);
    match guard {
        Some(g) => cmd.env(wc_core::guard::GUARD_ENV, g),
        None => cmd.env_remove(wc_core::guard::GUARD_ENV),
    };
    cmd.output()
}

/// A new project at `<tmp>/p`.
fn new_project() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(tmp.path().join("p"))
        .arg("--no-git")
        .output()?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Ok(tmp)
}

#[cfg(unix)]
#[test]
fn a_link_to_a_project_file_swapped_in_is_not_written_through() -> TestResult {
    let tmp = new_project()?;
    let project = tmp.path().join("p");
    let target = project.join("project.toml");
    let before = std::fs::read(&target)?;
    let out_dir = tempfile::tempdir()?;
    let file = out_dir.path().join("vars.st");
    // The app looked at `file` and found nothing there; the link comes after that look.
    std::os::unix::fs::symlink(&target, &file)?;
    let out = wiringcraft(&project, Some(&project), &[&file])?;
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("shared session"), "{err}");
    assert_eq!(std::fs::read(&target)?, before);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_hard_link_to_a_project_file_is_not_written() -> TestResult {
    let tmp = new_project()?;
    let project = tmp.path().join("p");
    let target = project.join("project.toml");
    let before = std::fs::read(&target)?;
    // Next to the project, so on the same file system.
    let file = tmp.path().join("vars.st");
    std::fs::hard_link(&target, &file)?;
    let out = wiringcraft(&project, Some(&project), &[&file])?;
    assert!(!out.status.success());
    assert_eq!(std::fs::read(&target)?, before);
    Ok(())
}

#[test]
fn an_output_file_outside_the_project_is_written() -> TestResult {
    let tmp = new_project()?;
    let project = tmp.path().join("p");
    let out_dir = tempfile::tempdir()?;
    let file = out_dir.path().join("vars.st");
    std::fs::write(&file, "an older and much longer list than the new one will be")?;
    let out = wiringcraft(&project, Some(&project), &[&file])?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let guarded = std::fs::read_to_string(&file)?;
    // The same as without the guard: the old content is gone, not partly kept.
    let plain = out_dir.path().join("plain.st");
    let out = wiringcraft(&project, None, &[&plain])?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(guarded, std::fs::read_to_string(&plain)?);
    Ok(())
}

#[test]
fn an_output_path_into_the_project_is_refused() -> TestResult {
    let tmp = new_project()?;
    let project = tmp.path().join("p");
    let file = project.join("vars.st");
    let out = wiringcraft(&project, Some(&project), &[&file])?;
    assert!(!out.status.success());
    assert!(!file.exists());
    // Without someone holding a page, writing there is the user's own choice.
    let out = wiringcraft(&project, None, &[&file])?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}
