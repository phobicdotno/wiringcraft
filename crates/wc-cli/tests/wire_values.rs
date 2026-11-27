//! `wiringcraft wire --number/--potential`, run as a program: the values are saved trimmed,
//! and a blank one is left out, as the app's wire panel and the assistant keep them.

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wiringcraft(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn wire_number_and_potential_are_trimmed_and_blank_ones_left_out() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
    let spaced = wiringcraft(
        &dir,
        &[
            "wire",
            "p1",
            "10,10",
            "50,10",
            "--number",
            " 101 ",
            "--potential",
            " 24V ",
        ],
    )?;
    let blank = wiringcraft(
        &dir,
        &["wire", "p1", "10,20", "50,20", "--number", "", "--potential", "  "],
    )?;

    let page: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("pages/p1.page.json"))?)?;
    let wires = page["wires"].as_array().ok_or("wires")?;
    let wire = |id: &str| {
        wires
            .iter()
            .find(|w| w["id"] == id.trim())
            .cloned()
            .ok_or(format!("no wire {id}"))
    };
    let spaced = wire(&spaced)?;
    assert_eq!(spaced["number"], "101", "{spaced}");
    assert_eq!(spaced["potential"], "24V", "{spaced}");
    let blank = wire(&blank)?;
    assert!(blank["number"].is_null(), "a blank number is left out: {blank}");
    assert!(blank["potential"].is_null(), "a blank potential is left out: {blank}");
    Ok(())
}
