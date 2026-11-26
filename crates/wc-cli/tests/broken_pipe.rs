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

    let mut child = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["translations", "--lang", "nb"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Close the reading end before the program prints anything, like `head` after its lines.
    drop(child.stdout.take());
    let out = child.wait_with_output()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!stderr.contains("stopped unexpectedly"), "{stderr}");
    assert_ne!(out.status.code(), Some(101), "{stderr}");
    // Ended by the signal, as `cat` would be, or done before the pipe closed.
    assert!(
        out.status.signal() == Some(libc::SIGPIPE) || out.status.success(),
        "{:?}",
        out.status
    );
    Ok(())
}
