//! `wiringcraft panel step`, run as a program so the warning about guessed depths is read from
//! stderr.

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

#[test]
fn devices_without_a_depth_are_named_in_a_warning() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    // -K1 has a part with a depth; -K2 has no part.
    let lib = tmp.path().join("lib");
    let mut part = wc_core::Part {
        id: "maker.contactor".into(),
        ..wc_core::Part::default()
    };
    part.properties.insert("Depth".into(), "95 mm".into());
    wc_library::write_part(&lib, &part)?;
    run(&dir, &["lib", "add", "shop", "folder", lib.to_str().ok_or("path")?])?;
    run(&dir, &["device", "--part", "maker.contactor", "--", "-K1"])?;
    // `device <tag>` alone prints the device, so give -K2 a description to add it without a part.
    run(&dir, &["device", "--description", "Spare relay", "--", "-K2"])?;
    run(&dir, &["panel", "new", "main"])?;
    run(&dir, &["panel", "device", "main", "-K1", "--at", "50,50"])?;
    run(&dir, &["panel", "device", "main", "-K2", "--at", "150,50"])?;

    let out = tmp.path().join("main.step");
    let (stdout, stderr) = run(&dir, &["panel", "step", "main", out.to_str().ok_or("path")?])?;
    assert!(stdout.starts_with("wrote "), "{stdout}");
    assert!(
        stderr.contains("warning: no Depth on the part, drawn 75 mm deep: -K2\n"),
        "{stderr}"
    );
    assert!(out.exists());
    Ok(())
}
