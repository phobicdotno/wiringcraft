//! Output piped into a reader that stops early (`wiringcraft ... | head -1`) ends the program
//! quietly instead of a panic (#550). Unix only: that is where the program restores the
//! default SIGPIPE action.
#![cfg(unix)]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_closed_stdout_reader_does_not_make_the_program_panic() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .args(["new"])
        .arg(&dir)
        .arg("--no-git")
        .stdout(Stdio::null())
        .status()?;
    assert!(status.success());

    // The pipe's reading end is closed before the program starts, like `head` after its
    // lines, so its first write always finds no reader.
    let (reader, writer) = std::io::pipe()?;
    drop(reader);
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["translations", "--lang", "nb"])
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!stderr.contains("stopped unexpectedly"), "{stderr}");
    // Ended by the signal, as `cat` would be.
    assert_eq!(out.status.signal(), Some(libc::SIGPIPE), "{:?}: {stderr}", out.status);
    Ok(())
}
