//! `wiringcraft macro`: listing macros and placing one with a chosen value set, run as a program.

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

const STARTER: &str = r#"{
  "name": "Motor starter",
  "symbols": [{"id": "s1", "symbol": "wc.coil", "at": {"x": 20.0, "y": 30.0}, "tag": "-Q1"}],
  "wires": [{"id": "w1", "from": {"item": "s1", "pin": "A1"}, "to": {"at": {"x": 20.0, "y": 10.0}},
             "section": "{{section}}"}],
  "devices": {"-Q1": {"part": "{{part}}", "description": "Starter {{kW}} kW"}},
  "anchor": {"x": 10.0, "y": 10.0},
  "placeholders": [{"name": "kW"}, {"name": "part"}, {"name": "section", "default": "1.5 mm²"}],
  "value_sets": [
    {"name": "0.75 kW", "values": {"kW": "0.75", "part": "SIE.3RV2011-1AA10"}},
    {"name": "3 kW", "values": {"kW": "3", "part": "SIE.3RV2011-1GA10", "section": "2.5 mm²"}}
  ]
}"#;

#[test]
fn a_macro_is_placed_with_the_chosen_value_set() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "1"])?;
    // A library source with the coil and the macro.
    let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("macros"))?;
    std::fs::copy(starter, lib.join("symbols/wc.coil.json"))?;
    std::fs::write(lib.join("macros/starter.json"), STARTER)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    // A project macro filling a placeholder it does not declare is listed with the problem.
    std::fs::create_dir_all(dir.join("macros"))?;
    std::fs::write(
        dir.join("macros/broken.json"),
        STARTER
            .replace(r#""part": "SIE.3RV2011-1AA10""#, r#""voltage": "400""#)
            .replace("Motor starter", "Broken starter"),
    )?;

    let list = wiringcraft(&dir, &["macro", "list"])?;
    assert!(
        list.contains("mine\tstarter\tMotor starter  [0.75 kW, 3 kW]\n"),
        "{list}"
    );
    assert!(list.contains("project\tbroken\t") && list.contains("voltage"), "{list}");

    let Err(err) = wiringcraft(&dir, &["macro", "place", "p1", "starter", "--at", "50,60"]) else {
        return Err("placed without a value set".into());
    };
    assert!(err.contains("choose a value set"), "{err}");
    let Err(err) = wiringcraft(
        &dir,
        &["macro", "place", "p1", "starter", "--at", "50,60", "--set", "5 kW"],
    ) else {
        return Err("placed with an unknown value set".into());
    };
    assert!(err.contains("no value set"), "{err}");

    let ids = wiringcraft(
        &dir,
        &[
            "macro",
            "place",
            "p1",
            "Motor starter",
            "--at",
            "50,60",
            "--set",
            "3 kW",
        ],
    )?;
    assert_eq!(ids.trim(), "s1 w1");
    let ids = wiringcraft(
        &dir,
        &[
            "macro", "place", "p1", "starter", "--at", "90,60", "--set", "0.75 kW", "--value", "kW=0.8",
        ],
    )?;
    assert_eq!(ids.trim(), "s2 w2");

    // The file name works too, and a pinned symbol whose copy is gone comes back only in its
    // pinned version.
    let pin = wc_core::store::load(&dir)?
        .lock
        .symbols
        .get("wc.coil")
        .cloned()
        .ok_or("pinned")?;
    let copy = wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Symbol, "wc.coil")?;
    std::fs::remove_file(&copy)?;
    let original = std::fs::read_to_string(lib.join("symbols/wc.coil.json"))?;
    let mut newer: serde_json::Value = serde_json::from_str(&original)?;
    newer["description"] = "A newer coil".into();
    std::fs::write(lib.join("symbols/wc.coil.json"), newer.to_string())?;
    let Err(err) = wiringcraft(
        &dir,
        &["macro", "place", "p1", "starter.json", "--at", "0,0", "--set", "3 kW"],
    ) else {
        return Err("placed with a newer version of a pinned symbol".into());
    };
    assert!(err.contains("no library has the version the project pins"), "{err}");
    std::fs::write(lib.join("symbols/wc.coil.json"), &original)?;
    let ids = wiringcraft(
        &dir,
        &["macro", "place", "p1", "starter.json", "--at", "0,0", "--set", "3 kW"],
    )?;
    assert_eq!(ids.trim(), "s3 w3");
    assert!(copy.exists(), "the pinned copy is put back");
    let p = wc_core::store::load(&dir)?;
    assert_eq!(p.lock.symbols.get("wc.coil"), Some(&pin), "the pin is kept");
    let page = p.page("p1").ok_or("page")?;
    let s3 = page.symbol("s3").ok_or("s3")?;
    assert_eq!((s3.at.x, s3.at.y), (10.0, 20.0));

    let page = p.page("p1").ok_or("page")?;
    let device = |id: &str| -> Result<wc_core::Device, String> {
        let tag = page.symbol(id).and_then(|s| s.tag.clone()).ok_or("tag")?;
        p.devices.get(&tag).cloned().ok_or_else(|| format!("no device {tag}"))
    };
    let (big, small) = (device("s1")?, device("s2")?);
    assert_eq!(big.description, "Starter 3 kW");
    assert_eq!(big.part.as_deref(), Some("SIE.3RV2011-1GA10"));
    assert_eq!(small.description, "Starter 0.8 kW", "the typed value wins");
    assert_eq!(small.part.as_deref(), Some("SIE.3RV2011-1AA10"));
    let sections: Vec<Option<&str>> = page.wires.iter().map(|w| w.spec.section.as_deref()).collect();
    assert_eq!(sections, [Some("2.5 mm²"), Some("1.5 mm²"), Some("2.5 mm²")]);
    let s2 = page.symbol("s2").ok_or("s2")?;
    assert_eq!((s2.at.x, s2.at.y), (100.0, 80.0));

    // With the library gone, a project macro whose symbols all have their pinned copies is
    // still placed.
    std::fs::create_dir_all(dir.join("macros"))?;
    std::fs::copy(lib.join("macros/starter.json"), dir.join("macros/starter.json"))?;
    std::fs::remove_dir_all(&lib)?;
    // A source of a kind it does not know makes the library fail to open.
    let mut p = wc_core::store::load(&dir)?;
    p.meta.libraries[0].kind = "zip".into();
    wc_core::store::save(&dir, &p)?;
    assert!(
        wiringcraft(&dir, &["lib", "status"]).is_err(),
        "the library cannot be opened"
    );
    let ids = wiringcraft(
        &dir,
        &["macro", "place", "p1", "starter", "--at", "0,0", "--set", "3 kW"],
    )?;
    assert_eq!(ids.trim(), "s4 w4");
    Ok(())
}

#[test]
fn a_retry_after_a_failed_placement_pins_every_symbol() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "1"])?;
    let coil = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::copy(&coil, lib.join("symbols/wc.coil.json"))?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    // Two symbols: the coil, then one the library does not have yet.
    std::fs::create_dir_all(dir.join("macros"))?;
    std::fs::write(
        dir.join("macros/pair.json"),
        r#"{"name": "Pair", "anchor": {"x": 0.0, "y": 0.0}, "symbols": [
            {"id": "s1", "symbol": "wc.coil", "at": {"x": 10.0, "y": 10.0}, "tag": "-K1"},
            {"id": "s2", "symbol": "wc.zcoil", "at": {"x": 30.0, "y": 10.0}, "tag": "-K2"}]}"#,
    )?;
    assert!(wiringcraft(&dir, &["macro", "place", "p1", "pair", "--at", "0,0"]).is_err());
    let copy = wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Symbol, "wc.coil")?;
    assert!(copy.exists(), "the coil was copied before the failure");
    assert!(
        !wc_core::store::load(&dir)?.lock.symbols.contains_key("wc.coil"),
        "but not pinned"
    );

    let mut second: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&coil)?)?;
    second["id"] = "wc.zcoil".into();
    std::fs::write(lib.join("symbols/wc.zcoil.json"), second.to_string())?;
    wiringcraft(&dir, &["macro", "place", "p1", "pair", "--at", "0,0"])?;
    let lock = wc_core::store::load(&dir)?.lock.symbols;
    assert!(
        lock.contains_key("wc.coil") && lock.contains_key("wc.zcoil"),
        "both are pinned: {lock:?}"
    );
    Ok(())
}
