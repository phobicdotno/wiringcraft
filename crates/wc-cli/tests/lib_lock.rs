//! `wiringcraft lib lock`, run as a program so what it prints, and where, is what is checked.

use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn lib_lock_warns_on_stderr_about_a_library_file_declaring_another_id() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());

    // A library whose `bad.json` holds the starter coil declaring `other`.
    let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
    let mut coil: wc_core::Symbol = wc_core::store::read_json(&starter.join("symbols").join("wc.coil.json"))?;
    coil.id = "other".into();
    let lib = tmp.path().join("lib");
    let written = wc_library::write_symbol(&lib, &coil)?;
    std::fs::rename(&written, lib.join("symbols").join("bad.json"))?;

    let mut p = wc_core::store::load(&dir)?;
    p.meta.libraries = vec![wc_core::project::LibrarySource {
        name: "lib".into(),
        kind: "folder".into(),
        location: lib.to_string_lossy().into_owned(),
        reference: None,
    }];
    p.pages[0].symbols.push(wc_core::PlacedSymbol {
        id: "s1".into(),
        symbol: "bad".into(),
        at: wc_core::Point::new(10.0, 10.0),
        rotation: Default::default(),
        mirror: false,
        tag: None,
    });
    wc_core::store::save(&dir, &p)?;

    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["lib", "lock"])
        .output()?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8(out.stderr)?;
    let stdout = String::from_utf8(out.stdout)?;
    assert!(
        stderr.contains("warning: symbol bad is not pinned: "),
        "stderr: {stderr}\nstdout: {stdout}"
    );
    assert!(stderr.contains("declares id \"other\""), "{stderr}");
    assert!(!stderr.contains("no library source"), "{stderr}");
    assert!(!stdout.contains("bad"), "warnings go to stderr: {stdout}");
    assert!(!wc_core::store::load(&dir)?.lock.symbols.contains_key("bad"));
    Ok(())
}

#[test]
fn lib_lock_restores_a_deleted_vendored_copy_of_a_pinned_symbol() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
    let mut p = wc_core::store::load(&dir)?;
    p.meta.libraries = vec![wc_core::project::LibrarySource {
        name: "starter".into(),
        kind: "folder".into(),
        location: starter.to_string_lossy().into_owned(),
        reference: None,
    }];
    p.pages[0].symbols.push(wc_core::PlacedSymbol {
        id: "s1".into(),
        symbol: "wc.coil".into(),
        at: wc_core::Point::new(10.0, 10.0),
        rotation: Default::default(),
        mirror: false,
        tag: None,
    });
    wc_core::store::save(&dir, &p)?;
    let lock = || {
        Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
            .arg("-C")
            .arg(&dir)
            .args(["lib", "lock"])
            .output()
    };
    assert!(lock()?.status.success());
    let copy = wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Symbol, "wc.coil")?;
    std::fs::remove_file(&copy)?;

    let out = lock()?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout)?;
    assert!(
        stdout.contains("restored the vendored copy of symbol wc.coil"),
        "{stdout}"
    );
    assert!(wc_core::store::read_vendored_symbol(&dir, "wc.coil").is_ok());
    Ok(())
}
