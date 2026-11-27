//! `wiringcraft page graphic` and `page add --graphic`: a graphic page holds drawings only, so
//! symbols and wires are refused on it and a page that has them cannot become one.

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

fn new_project(dir: &Path) -> TestResult {
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    Ok(())
}

#[test]
fn a_graphic_page_refuses_symbols_and_wires_and_is_listed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    wiringcraft(
        &dir,
        &["page", "add", "g1", "--number", "50", "--title", "Sketch", "--graphic"],
    )?;

    let list = wiringcraft(&dir, &["page", "list"])?;
    let line = list.lines().find(|l| l.starts_with("g1 ")).ok_or("g1 not listed")?;
    assert!(line.ends_with(", graphic"), "{list:?}");

    let wire = wiringcraft(&dir, &["wire", "g1", "0,0", "10,0"])
        .err()
        .ok_or("wire was placed")?;
    assert!(wire.contains("graphic page") && wire.contains("a wire"), "{wire}");
    let boxed = wiringcraft(
        &dir,
        &["box", "g1", "--at", "20,20", "--size", "30,20", "--pin", "A:left:10"],
    )
    .err()
    .ok_or("box was placed")?;
    assert!(
        boxed.contains("graphic page") && boxed.contains("a black box"),
        "{boxed}"
    );

    // An overview and a graphic page are two different things.
    let both = wiringcraft(&dir, &["page", "overview", "g1"])
        .err()
        .ok_or("overview was set")?;
    assert!(both.contains("graphic page"), "{both}");

    // Back to a normal page: wires go on it again.
    wiringcraft(&dir, &["page", "graphic", "g1", "--off"])?;
    let list = wiringcraft(&dir, &["page", "list"])?;
    assert!(!list.contains("graphic"), "{list:?}");
    wiringcraft(&dir, &["wire", "g1", "0,0", "10,0"])?;
    Ok(())
}

#[test]
fn a_page_with_a_wire_cannot_become_a_graphic_page() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    wiringcraft(&dir, &["page", "add", "p1", "--number", "50"])?;
    wiringcraft(&dir, &["wire", "p1", "0,0", "10,0"])?;
    let refused = wiringcraft(&dir, &["page", "graphic", "p1"])
        .err()
        .ok_or("page became graphic")?;
    assert!(refused.contains("0 symbol(s) and 1 wire(s)"), "{refused}");
    let list = wiringcraft(&dir, &["page", "list"])?;
    assert!(!list.contains("graphic"), "{list:?}");

    let both = wiringcraft(
        &dir,
        &["page", "add", "p2", "--number", "51", "--graphic", "--overview"],
    );
    assert!(both.is_err(), "--graphic with --overview was taken");
    Ok(())
}
