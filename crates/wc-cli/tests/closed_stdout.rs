//! Output piped into a reader that stops early (`wiringcraft ... | more`, `| head -1`) ends the
//! program quietly on every platform (#550, #567), and the command still finishes: what it
//! changes is saved.

use std::path::Path;
use std::process::{Command, Output, Stdio};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Run the program on the project in `dir` with a stdout whose reader is already closed, like
/// a reader after its lines, so its first write always finds no reader.
fn with_closed_stdout(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    let (reader, writer) = std::io::pipe()?;
    drop(reader);
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
}

#[test]
fn a_closed_stdout_reader_ends_the_program_quietly_after_its_save() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .stdout(Stdio::null())
        .status()?;
    assert!(status.success());

    // `panel new` prints before it saves.
    let out = with_closed_stdout(&dir, &["panel", "new", "P1"])?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(out.status.code(), Some(0), "{:?}: {stderr}", out.status);

    let listed = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["panel", "list"])
        .output()?;
    assert!(listed.status.success());
    assert!(String::from_utf8_lossy(&listed.stdout).contains("P1"));

    // A command with more output than one write.
    let out = with_closed_stdout(&dir, &["panel", "show", "P1"])?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(out.status.code(), Some(0), "{:?}: {stderr}", out.status);
    Ok(())
}
