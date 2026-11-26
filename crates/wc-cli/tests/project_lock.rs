//! Two wiringcraft processes on one project take turns (#610), whatever their environment, and
//! a command reading its input from a pipe reads it before it waits.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn new_project(dir: &Path) -> TestResult {
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    Ok(())
}

/// Wait up to `limit` for the child to end: `true` when it did.
fn ends_within(child: &mut Child, limit: Duration) -> Result<bool, Box<dyn std::error::Error>> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(false)
}

#[test]
fn a_command_waits_for_the_lock_another_process_holds() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    // Held as another wiringcraft process would hold it.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".wiringcraft.lock"))?;
    lock.lock()?;
    let own_tmp = tmp.path().join("elsewhere");
    std::fs::create_dir(&own_tmp)?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["rules", "set", "-"])
        // Another temporary folder changes nothing: the lock is in the project.
        .env("TMPDIR", &own_tmp)
        .env("TMP", &own_tmp)
        .env("TEMP", &own_tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(mut input) = child.stdin.take() else {
        unreachable!("stdin is piped");
    };
    input.write_all(b"[[rule]]\nkind = \"max-wires\"\nmax = 2\n")?;
    drop(input);
    assert!(
        !ends_within(&mut child, Duration::from_millis(600))?,
        "it waits for the lock"
    );
    lock.unlock()?;
    assert!(
        ends_within(&mut child, Duration::from_secs(20))?,
        "it runs once the lock is free"
    );
    let out = child.wait_with_output()?;
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("waiting for another wiringcraft command"), "{err}");
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    assert!(toml.contains("max-wires"), "{toml}");
    Ok(())
}
