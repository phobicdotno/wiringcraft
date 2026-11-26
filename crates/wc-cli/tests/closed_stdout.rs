//! Output piped into a reader that stops early (`wiringcraft ... | more`) ends the program
//! quietly on every platform, Windows too, where no SIGPIPE ends it (#567).

use std::process::{Command, Stdio};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_closed_stdout_reader_ends_the_program_quietly() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .stdout(Stdio::null())
        .status()?;
    assert!(status.success());

    // The pipe's reading end is closed before the program starts, like a reader after its
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
    assert!(!stderr.contains("error"), "{stderr}");
    // A plain success where nothing ends the program first; on Unix, a SIGPIPE that ends it
    // (#550) is as quiet.
    #[cfg(unix)]
    let by_signal = std::os::unix::process::ExitStatusExt::signal(&out.status).is_some();
    #[cfg(not(unix))]
    let by_signal = false;
    assert!(out.status.success() || by_signal, "{:?}: {stderr}", out.status);
    Ok(())
}
