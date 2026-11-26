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

/// The reader's refusal can quote the file (here its version tag, with an ESC in it): it is
/// escaped both when the converter then fails and when it works.
#[cfg(unix)]
#[test]
fn a_refusal_quoting_the_file_is_escaped_in_both_messages() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    store::create(&dir, "Test")?;
    let page = store::load(&dir)?.pages[0].id.clone();
    let dwg = tmp.path().join("plan.dwg");
    std::fs::write(&dwg, b"AC\x1b[31 not a drawing")?;
    let Some(file) = dwg.to_str() else {
        unreachable!("the temporary path is not text")
    };
    let escaped = "drawing version AC\\u{1b}[31 is not supported";

    // No converter: the error names the refusal, escaped.
    let out = wiringcraft(&dir, &["import", "page-dwg", file, "--page", &page])?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
    assert!(stderr.contains(escaped), "{stderr}");

    // A converter (shell built-ins only: PATH is empty) that writes a DXF with one line.
    // Under /tmp: the converter setting is split on spaces.
    let bin = tempfile::tempdir_in("/tmp")?;
    let script = bin.path().join("convert.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '0\\nSECTION\\n2\\nENTITIES\\n0\\nLINE\\n8\\n0\\n10\\n0\\n20\\n0\\n11\\n10\\n21\\n10\\n0\\nENDSEC\\n0\\nEOF\\n' > \"$2\"\n",
    )?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["import", "page-dwg", file, "--page", &page])
        .env("PATH", "")
        .env("WIRINGCRAFT_DWG2DXF", format!("{} {{in}} {{out}}", script.display()))
        .output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!stdout.contains('\u{1b}'), "{stdout:?}");
    assert!(
        stdout.contains("converted to DXF with") && stdout.contains(escaped),
        "{stdout}"
    );
    Ok(())
}
