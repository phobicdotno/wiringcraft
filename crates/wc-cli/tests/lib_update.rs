//! `wiringcraft lib update`, run as a program so its exit status and messages are what is checked.

use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn lib_update_refuses_an_id_the_project_does_not_have() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let before = std::fs::read(dir.join(wc_core::store::LOCK_FILE));

    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["lib", "update", "nosuch"])
        .output()?;
    let stderr = String::from_utf8(out.stderr)?;
    let stdout = String::from_utf8(out.stdout)?;
    assert!(!out.status.success(), "stdout: {stdout}");
    assert!(stderr.contains("does not use or pin \"nosuch\""), "{stderr}");
    assert!(stderr.contains("lib status"), "{stderr}");
    assert!(!stdout.contains("updated"), "{stdout}");
    assert_eq!(
        std::fs::read(dir.join(wc_core::store::LOCK_FILE)).ok(),
        before.ok(),
        "nothing written"
    );
    Ok(())
}
