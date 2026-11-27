//! `wiringcraft vcs merge --abort`: backing out of a merge that stopped on conflicts (#1161).

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wiringcraft(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Tester")
        .env("GIT_AUTHOR_EMAIL", "tester@example.invalid")
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.invalid")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn a_conflicted_merge_can_be_aborted() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .status()?;
    assert!(status.success());
    let notes = dir.join("notes.txt");
    std::fs::write(&notes, "base\n")?;
    wiringcraft(&dir, &["vcs", "commit", "-m", "base"])?;
    wiringcraft(&dir, &["vcs", "branch", "other"])?;
    std::fs::write(&notes, "other\n")?;
    wiringcraft(&dir, &["vcs", "commit", "-m", "other"])?;
    wiringcraft(&dir, &["vcs", "switch", "main"])?;
    std::fs::write(&notes, "main\n")?;
    wiringcraft(&dir, &["vcs", "commit", "-m", "main"])?;

    let Err(err) = wiringcraft(&dir, &["vcs", "merge", "other"]) else {
        unreachable!("the merge stops on the conflict");
    };
    assert!(err.contains("vcs merge --abort"), "says how to back out: {err}");

    let out = wiringcraft(&dir, &["vcs", "merge", "--abort"])?;
    assert!(out.contains("merge aborted"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&notes)?,
        "main\n",
        "the file is as before the merge"
    );
    // Back to normal: switching branches works again.
    wiringcraft(&dir, &["vcs", "switch", "other"])?;

    let Err(err) = wiringcraft(&dir, &["vcs", "merge", "--abort"]) else {
        unreachable!("there is nothing to abort");
    };
    assert!(err.contains("no merge in progress"), "{err}");
    let Err(err) = wiringcraft(&dir, &["vcs", "merge"]) else {
        unreachable!("a branch or --abort is needed");
    };
    assert!(err.contains("<NAME>"), "{err}");
    Ok(())
}
