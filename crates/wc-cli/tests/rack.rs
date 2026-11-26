//! `wiringcraft rack`, run as a program so the rack warnings printed after a change are read
//! from stderr on their own.

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The program's stdout and stderr, each on its own; an error when it fails.
fn run(dir: &Path, args: &[&str]) -> Result<(String, String), Box<dyn std::error::Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()?;
    let stderr = String::from_utf8(out.stderr)?;
    if !out.status.success() {
        return Err(stderr.into());
    }
    Ok((String::from_utf8(out.stdout)?, stderr))
}

fn project(at: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let dir = at.join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    Ok(dir)
}

#[test]
fn a_card_added_without_a_start_address_is_saved_with_a_warning() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let (_, stderr) = run(&dir, &["rack", "add", "-A1"])?;
    assert!(!stderr.contains("warning:"), "{stderr}");
    let (_, stderr) = run(&dir, &["rack", "card", "add", "-A1", "3"])?;
    assert!(
        stderr.contains("warning: PLC rack \"-A1\" slot 3: start address \"\" is not a PLC address"),
        "{stderr}"
    );
    // The card is kept: the warning does not refuse the edit.
    let (stdout, _) = run(&dir, &["rack", "list"])?;
    assert!(stdout.contains('3'), "{stdout}");
    Ok(())
}

#[test]
fn an_unreadable_part_is_named_with_its_reason_and_the_rack_edit_still_goes_ahead() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let lib = tmp.path().join("lib");
    let part = wc_core::Part {
        id: "maker.contactor".into(),
        ..wc_core::Part::default()
    };
    wc_library::write_part(&lib, &part)?;
    run(&dir, &["lib", "add", "shop", "folder", lib.to_str().ok_or("path")?])?;
    run(&dir, &["device", "--part", "maker.contactor", "--", "-K1"])?;
    let vendored = wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Part, "maker.contactor")?;
    assert!(vendored.exists(), "{}", vendored.display());
    std::fs::write(&vendored, "not a part")?;

    run(&dir, &["rack", "add", "-A1"])?;
    let (_, stderr) = run(&dir, &["rack", "card", "add", "-A1", "3"])?;
    let Some(line) = stderr
        .lines()
        .find(|l| l.starts_with("warning: cannot read part maker.contactor: "))
    else {
        unreachable!("no unreadable part warning: {stderr}");
    };
    // The reason follows the part id.
    assert!(
        line.len() > "warning: cannot read part maker.contactor: ".len(),
        "{stderr}"
    );
    // The rack warnings still print, and the card is saved.
    assert!(
        stderr.contains("warning: PLC rack \"-A1\" slot 3: start address \"\" is not a PLC address"),
        "{stderr}"
    );
    let (stdout, _) = run(&dir, &["rack", "list"])?;
    assert!(stdout.contains('3'), "{stdout}");
    Ok(())
}
