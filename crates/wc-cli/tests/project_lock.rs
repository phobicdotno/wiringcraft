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
    // More than a pipe holds: the writer only gets through when the command reads its input
    // while it is still waiting for the lock.
    let mut rules = String::from("[[rule]]\nkind = \"max-wires\"\nmax = 2\n");
    for _ in 0..4096 {
        rules.push_str("# padding to fill the pipe ................................\n");
    }
    assert!(rules.len() > 200_000);
    let writer = std::thread::spawn(move || input.write_all(rules.as_bytes()));
    let start = Instant::now();
    while !writer.is_finished() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let read_first = writer.is_finished();
    let waits = !ends_within(&mut child, Duration::from_millis(300))?;
    lock.unlock()?;
    let ended = ends_within(&mut child, Duration::from_secs(20))?;
    if !ended {
        child.kill()?;
    }
    assert!(read_first, "the input is read before the lock is taken");
    assert!(waits, "it waits for the lock");
    assert!(ended, "it runs once the lock is free");
    let Ok(Ok(())) = writer.join() else {
        unreachable!("the input was written");
    };
    let out = child.wait_with_output()?;
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("waiting for another wiringcraft command"), "{err}");
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    assert!(toml.contains("max-wires"), "{toml}");
    Ok(())
}

/// A git hook that runs wiringcraft on the project while `vcs commit` holds the project's lock
/// does not wait for that lock (it would wait for good: the commit waits for the hook).
#[cfg(unix)]
#[test]
fn a_git_hook_running_wiringcraft_on_the_project_does_not_hang_a_commit() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .status()?;
    assert!(status.success());
    let hook = dir.join(".git").join("hooks").join("pre-commit");
    std::fs::create_dir_all(hook.parent().ok_or("a folder")?)?;
    let marker = tmp.path().join("hook-ran");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n\"{}\" -C . page list > /dev/null && touch \"{}\"\n",
            env!("CARGO_BIN_EXE_wiringcraft"),
            marker.display()
        ),
    )?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    let added = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["page", "add", "p2", "--number", "2"])
        .status()?;
    assert!(added.success());
    let mut commit = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["vcs", "commit", "-m", "A page"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let ended = ends_within(&mut commit, Duration::from_secs(30))?;
    if !ended {
        commit.kill()?;
    }
    let out = commit.wait_with_output()?;
    assert!(ended, "the commit ends");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(marker.exists(), "the hook ran wiringcraft");
    Ok(())
}
