//! `wiringcraft import page-dwg` run as a program, with no DWG converter at hand: the page is
//! read by WiringCraft's own DWG reader (#600).

use std::path::Path;
use std::process::{Command, Output};

use wc_core::store;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// An AutoCAD 2010 drawing with a line, a circle and a text in model space, written by the
/// DWG reader's own test writer (`wc_import::dwg::objects::tests::model_space_dwg`).
const PAGE_DWG: &[u8] = include_bytes!("data/page.dwg");

/// Run with an empty PATH and a converter setting that always fails, so no converter can
/// have read the file.
fn wiringcraft(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("PATH", "")
        .env("WIRINGCRAFT_DWG2DXF", "no-such-converter {in} {out}")
        .output()
}

#[test]
fn a_dwg_page_imports_without_a_converter() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    store::create(&dir, "Test")?;
    let dwg = tmp.path().join("plan.dwg");
    std::fs::write(&dwg, PAGE_DWG)?;
    let page = store::load(&dir)?.pages[0].id.clone();

    let Some(file) = dwg.to_str() else {
        unreachable!("the temporary path is not text")
    };
    let out = wiringcraft(&dir, &["import", "page-dwg", file, "--page", &page])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("read with WiringCraft's own DWG reader"), "{stdout}");
    let p = store::load(&dir)?;
    let Some(pg) = p.page(&page) else {
        unreachable!("the page is gone")
    };
    assert!(pg.graphics.len() >= 3, "{:?}", pg.graphics);

    // A file the reader refuses goes to the converter, and both reasons are given.
    std::fs::write(&dwg, "not a drawing")?;
    let out = wiringcraft(&dir, &["import", "page-dwg", file, "--page", &page])?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("own DWG reader refused the file") && stderr.contains("WIRINGCRAFT_DWG2DXF"),
        "{stderr}"
    );
    Ok(())
}
