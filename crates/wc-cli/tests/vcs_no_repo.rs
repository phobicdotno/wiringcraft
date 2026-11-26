//! A `vcs` command in a project that is not under version control says so and how to start
//! (#1067), instead of git's own "not a git repository" or its diff usage text.

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Runs wiringcraft in `dir`, with git kept from looking above `dir`'s parent for a repository
/// (the temporary folder could be inside one).
fn run(dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    let ceiling = dir.parent().unwrap_or(dir);
    Ok(Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .current_dir(dir)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .args(args)
        .output()?)
}

#[test]
fn vcs_commands_without_a_repository_say_to_run_vcs_init() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let Some(dir_arg) = dir.to_str() else {
        unreachable!("temporary path is not UTF-8")
    };
    assert!(run(tmp.path(), &["new", "--no-git", dir_arg])?.status.success());
    for args in [
        &["vcs", "commit", "-m", "x"][..],
        &["vcs", "log"],
        &["vcs", "branch"],
        &["vcs", "changes"],
        &["vcs", "page-log", "1"],
    ] {
        let out = run(&dir, args)?;
        assert!(!out.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("is not under version control"), "{args:?}: {err}");
        assert!(err.contains("wiringcraft vcs init"), "{args:?}: {err}");
        assert!(!err.contains("fatal:") && !err.contains("usage:"), "{args:?}: {err}");
        assert!(!dir.join(".git").exists(), "{args:?}");
    }
    // `vcs init` itself still makes the repository, and the commands work after it.
    assert!(run(&dir, &["vcs", "init"])?.status.success());
    let out = run(&dir, &["vcs", "log"])?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}
