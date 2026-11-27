//! `wiringcraft macro`: listing, showing and editing macros and placing one with a chosen value set,
//! run as a program.

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
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
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

    // A graphic page takes no macro with symbols or wires.
    wiringcraft(&dir, &["page", "add", "g1", "--number", "90", "--graphic"])?;
    let Err(err) = wiringcraft(
        &dir,
        &["macro", "place", "g1", "starter", "--at", "50,60", "--set", "3 kW"],
    ) else {
        return Err("a macro with symbols was placed on a graphic page".into());
    };
    assert!(
        err.contains("graphic page") && err.contains("a macro with symbols or wires"),
        "{err}"
    );
    assert!(
        !dir.join("vendor/symbols/wc.coil.json").exists(),
        "nothing was taken for it"
    );
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
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
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

    // A pinned copy that cannot be read stops the placement, naming the file, and the page
    // stays as it was.
    let before = wc_core::store::load(&dir)?.pages;
    std::fs::write(&copy, "{ not json")?;
    let Err(err) = wiringcraft(&dir, &["macro", "place", "p1", "pair", "--at", "0,0"]) else {
        return Err("placed with a broken symbol copy".into());
    };
    assert!(err.contains("wc.coil"), "the file is named: {err}");
    assert_eq!(wc_core::store::load(&dir)?.pages, before, "nothing is placed");
    Ok(())
}

#[test]
fn placing_a_macro_copies_and_pins_the_parts_its_devices_name() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("parts"))?;
    std::fs::copy(
        repo.join("library/symbols/wc.coil.json"),
        lib.join("symbols/wc.coil.json"),
    )?;
    let lamp = "example.ex-lamp-24-rd";
    std::fs::copy(
        repo.join(format!("library/parts/{lamp}.json")),
        lib.join(format!("parts/{lamp}.json")),
    )?;
    let cap = "example.ex-lamp-24-gn";
    std::fs::copy(
        repo.join(format!("library/parts/{cap}.json")),
        lib.join(format!("parts/{cap}.json")),
    )?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    // The lamp as a device's part (with spaces around it), and as accessories a library part
    // and a part no library has.
    std::fs::create_dir_all(dir.join("macros"))?;
    std::fs::write(
        dir.join("macros/lamp.json"),
        format!(
            r#"{{"name": "Lamp", "anchor": {{"x": 0.0, "y": 0.0}},
            "symbols": [{{"id": "s1", "symbol": "wc.coil", "at": {{"x": 10.0, "y": 10.0}}, "tag": "-P1"}}],
            "devices": {{"-P1": {{"part": " {lamp} ", "accessories": [{{"part": " {cap} "}}, {{"part": "nowhere.cap"}}]}}}}}}"#
        ),
    )?;

    wiringcraft(&dir, &["macro", "place", "p1", "lamp", "--at", "0,0"])?;
    let copy = wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Part, lamp)?;
    assert!(copy.exists(), "the part is copied into the project");
    let p = wc_core::store::load(&dir)?;
    let pin = p.lock.parts.get(lamp).cloned().ok_or("the part is pinned")?;
    assert!(
        p.lock.parts.contains_key(cap),
        "the accessory is pinned: {:?}",
        p.lock.parts
    );
    assert!(
        wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Part, cap)?.exists(),
        "the accessory is copied into the project"
    );
    assert!(!p.lock.parts.contains_key("nowhere.cap"), "{:?}", p.lock.parts);
    let status = wiringcraft(&dir, &["lib", "status"])?;
    assert!(
        status.contains(&format!("part {lamp}: up to date; used by -P1")),
        "{status}"
    );

    // A pinned part whose copy is gone comes back only in its pinned version.
    std::fs::remove_file(&copy)?;
    let file = lib.join(format!("parts/{lamp}.json"));
    let original = std::fs::read_to_string(&file)?;
    let mut newer: serde_json::Value = serde_json::from_str(&original)?;
    newer["description"] = "A newer lamp".into();
    std::fs::write(&file, newer.to_string())?;
    let Err(err) = wiringcraft(&dir, &["macro", "place", "p1", "lamp", "--at", "0,0"]) else {
        return Err("placed with a newer version of a pinned part".into());
    };
    assert!(err.contains(&format!("part {lamp}")), "{err}");
    assert!(err.contains("no library has the version the project pins"), "{err}");
    std::fs::write(&file, &original)?;
    wiringcraft(&dir, &["macro", "place", "p1", "lamp", "--at", "0,0"])?;
    assert!(copy.exists(), "the pinned copy is put back");
    assert_eq!(
        wc_core::store::load(&dir)?.lock.parts.get(lamp),
        Some(&pin),
        "the pin is kept"
    );
    Ok(())
}

#[test]
fn a_project_macro_is_shown_and_edited_and_a_library_one_is_not() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("macros"))?;
    std::fs::write(
        lib.join("macros/libstarter.json"),
        STARTER.replace("Motor starter", "Lib starter"),
    )?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    std::fs::create_dir_all(dir.join("macros"))?;
    let file = dir.join("macros/starter.json");
    std::fs::write(&file, STARTER)?;

    let shown = wiringcraft(&dir, &["macro", "show", "Motor starter"])?;
    assert!(shown.starts_with("Motor starter (project)\n"), "{shown}");
    assert!(shown.contains("  section = 1.5 mm²\n"), "{shown}");
    assert!(
        shown.contains("  3 kW: kW=3, part=SIE.3RV2011-1GA10, section=2.5 mm²\n"),
        "{shown}"
    );
    assert!(shown.contains("used in texts: kW, part, section\n"), "{shown}");
    assert!(!shown.contains("problem:"), "{shown}");

    // A library macro is shown but not edited.
    assert!(wiringcraft(&dir, &["macro", "show", "libstarter"])?.starts_with("Lib starter (mine)\n"));
    let Err(err) = wiringcraft(&dir, &["macro", "edit", "libstarter", "--description", "x"]) else {
        return Err("a library macro was edited".into());
    };
    assert!(err.contains("library"), "{err}");

    // An edit that leaves a used placeholder undeclared is refused and nothing is written.
    let before = std::fs::read_to_string(&file)?;
    let Err(err) = wiringcraft(&dir, &["macro", "edit", "starter", "--remove", "part"]) else {
        return Err("removed a placeholder still in use".into());
    };
    assert!(err.contains("{{part}} is used"), "{err}");
    let Err(err) = wiringcraft(&dir, &["macro", "edit", "starter", "--set", "3 kW:volts=400"]) else {
        return Err("filled an undeclared placeholder".into());
    };
    assert!(err.contains("volts"), "{err}");
    assert_eq!(std::fs::read_to_string(&file)?, before, "nothing is written");

    let shown = wiringcraft(
        &dir,
        &[
            "macro",
            "edit",
            "starter",
            "--description",
            "Direct on line",
            "--rename",
            "kW=power",
            "--placeholder",
            "power::Motor power",
            "--placeholder",
            "voltage:400 V",
            "--set",
            "5.5 kW:power=5.5",
            "--set",
            "5.5 kW:part=SIE.3RV2021-4AA10",
            "--set",
            "3 kW:voltage=",
            "--remove-set",
            "0.75 kW",
        ],
    )?;
    assert!(shown.contains("description: Direct on line\n"), "{shown}");
    let m: wc_core::macros::Macro = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
    assert_eq!(m.description, "Direct on line");
    assert_eq!(m.used_placeholders(), ["power", "part", "section"]);
    assert_eq!(
        m.devices.values().next().map(|d| d.description.as_str()),
        Some("Starter {{power}} kW")
    );
    let names: Vec<(&str, &str, &str)> = m
        .placeholders
        .iter()
        .map(|p| (p.name.as_str(), p.default.as_str(), p.description.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("power", "", "Motor power"),
            ("part", "", ""),
            ("section", "1.5 mm²", ""),
            ("voltage", "400 V", "")
        ]
    );
    let sets: Vec<&str> = m.value_sets.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(sets, ["3 kW", "5.5 kW"]);
    let three = m.value_set("3 kW").ok_or("3 kW")?;
    assert_eq!(three.values.get("power").map(String::as_str), Some("3"));
    assert_eq!(
        three.values.get("voltage").map(String::as_str),
        Some(""),
        "stored empty"
    );
    let big = m.value_set("5.5 kW").ok_or("5.5 kW")?;
    assert_eq!(big.values.len(), 2);

    // A library source named "project" is still a library.
    let other = tmp.path().join("other");
    std::fs::create_dir_all(other.join("macros"))?;
    std::fs::write(other.join("macros/twin.json"), STARTER.replace("Motor starter", "Twin"))?;
    wiringcraft(
        &dir,
        &["lib", "add", "project", "folder", other.to_str().ok_or("path")?],
    )?;
    assert!(wiringcraft(&dir, &["macro", "show", "twin"])?.starts_with("Twin (project)\n"));
    let Err(err) = wiringcraft(&dir, &["macro", "edit", "Twin", "--description", "x"]) else {
        return Err("a library macro under a source named project was edited".into());
    };
    assert!(err.contains("library"), "{err}");

    // A file that cannot be read is named when no macro matches, and does not stop a match.
    std::fs::write(dir.join("macros/broken.json"), "{ not json")?;
    let Err(err) = wiringcraft(&dir, &["macro", "show", "Nothing"]) else {
        return Err("showed a macro that is not there".into());
    };
    assert!(err.contains("broken.json"), "{err}");
    assert!(wiringcraft(&dir, &["macro", "show", "Motor starter"]).is_ok());
    std::fs::remove_file(dir.join("macros/broken.json"))?;

    // Control characters from the file are shown escaped, one field per line.
    std::fs::write(
        dir.join("macros/odd.json"),
        STARTER
            .replace("Motor starter", r"Odd\u001b[2J")
            .replace(r#""name": "0.75 kW""#, r#""name": "0.75\nkW""#),
    )?;
    let shown = wiringcraft(&dir, &["macro", "show", "odd"])?;
    assert!(!shown.contains('\u{1b}'), "{shown:?}");
    assert!(shown.starts_with("Odd\\u{1b}[2J (project)\n"), "{shown:?}");
    assert!(shown.contains("  0.75\\nkW: "), "{shown:?}");

    // A placeholder no text uses can be removed, with its values.
    wiringcraft(&dir, &["macro", "edit", "starter", "--remove", "voltage"])?;
    let m: wc_core::macros::Macro = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
    assert!(m.placeholders.iter().all(|p| p.name != "voltage"));
    assert!(m.value_sets.iter().all(|v| !v.values.contains_key("voltage")));
    Ok(())
}

// Windows does not allow control characters in a folder name.
#[cfg(unix)]
#[test]
fn an_unreadable_macro_file_is_named_escaped() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // Linux allows control characters in a folder name.
    let dir = tmp.path().join("p\u{1b}[2J");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    std::fs::create_dir_all(dir.join("macros"))?;
    std::fs::write(dir.join("macros/broken.json"), "{ not json")?;
    let Err(err) = wiringcraft(&dir, &["macro", "show", "Nothing"]) else {
        return Err("showed a macro that is not there".into());
    };
    assert!(err.contains("broken.json"), "{err:?}");
    assert!(!err.contains('\u{1b}'), "{err:?}");
    assert!(err.contains("p\\u{1b}[2J"), "{err:?}");
    Ok(())
}

#[test]
fn a_table_generates_pages_with_one_macro_copy_per_row() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
    let coil = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("macros"))?;
    std::fs::copy(coil, lib.join("symbols/wc.coil.json"))?;
    std::fs::write(lib.join("macros/starter.json"), STARTER)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let pages_before = wc_core::store::load(&dir)?.pages.len();

    // A typo in a column name is refused before anything changes.
    let typo = tmp.path().join("typo.csv");
    std::fs::write(&typo, "kw;sectoin\n1;2\n")?;
    let Err(err) = wiringcraft(
        &dir,
        &["macro", "generate", "starter", "--table", typo.to_str().ok_or("path")?],
    ) else {
        return Err("generated from a table with an unknown column".into());
    };
    assert!(
        err.contains("\"sectoin\"") && err.contains("\"kW\", \"part\", \"section\""),
        "{err}"
    );
    // A row without a value set names its line, and nothing is placed.
    let table = tmp.path().join("motors.csv");
    std::fs::write(&table, "Set;kW;Section\n3 kW;;\n\n0.75 kW;0.8;\n;5.5;4 mm²\n")?;
    let Err(err) = wiringcraft(
        &dir,
        &["macro", "generate", "starter", "--table", table.to_str().ok_or("path")?],
    ) else {
        return Err("generated a row without a value set".into());
    };
    assert!(err.contains("line 5") && err.contains("choose a value set"), "{err}");
    assert_eq!(wc_core::store::load(&dir)?.pages.len(), pages_before);

    let out = wiringcraft(
        &dir,
        &[
            "macro",
            "generate",
            "starter",
            "--table",
            table.to_str().ok_or("path")?,
            "--set",
            "3 kW",
            "--title",
            "Motors",
            "--size",
            "A4",
        ],
    )?;
    assert_eq!(out, "p0002\n3 copies of Motor starter on 1 page\n");
    let p = wc_core::store::load(&dir)?;
    let page = p.page("p0002").ok_or("page")?;
    assert_eq!((page.number.as_str(), page.title.as_str()), ("3", "Motors"));
    assert_eq!(page.size, wc_core::PageSize::A4);
    let rows: Vec<(String, String, Option<String>)> = page
        .symbols
        .iter()
        .map(|s| {
            let tag = s.tag.clone().ok_or("tag")?;
            let device = p.devices.get(&tag).ok_or("device")?;
            Ok((tag.to_string(), device.description.clone(), device.part.clone()))
        })
        .collect::<Result<_, &str>>()?;
    assert_eq!(
        rows,
        [
            ("-Q1".into(), "Starter 3 kW".into(), Some("SIE.3RV2011-1GA10".into())),
            ("-Q2".into(), "Starter 0.8 kW".into(), Some("SIE.3RV2011-1AA10".into())),
            ("-Q3".into(), "Starter 5.5 kW".into(), Some("SIE.3RV2011-1GA10".into())),
        ]
    );
    let sections: Vec<Option<&str>> = page.wires.iter().map(|w| w.spec.section.as_deref()).collect();
    assert_eq!(sections, [Some("2.5 mm²"), Some("1.5 mm²"), Some("4 mm²")]);
    // Side by side, on the grid, in the same row.
    let xs: Vec<f64> = page.symbols.iter().map(|s| s.at.x).collect();
    let ys: Vec<f64> = page.symbols.iter().map(|s| s.at.y).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
    assert!(ys.windows(2).all(|w| w[0] == w[1]), "{ys:?}");
    assert!(xs.iter().chain(&ys).all(|v| v % 5.0 == 0.0), "{xs:?} {ys:?}");
    assert!(p.lock.symbols.contains_key("wc.coil"));
    Ok(())
}

#[test]
fn a_generation_whose_save_fails_leaves_the_project_as_it_was() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let coil = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("macros"))?;
    std::fs::copy(coil, lib.join("symbols/wc.coil.json"))?;
    std::fs::write(lib.join("macros/starter.json"), STARTER)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let table = tmp.path().join("motors.csv");
    // Thirty rows: more than one small page holds.
    std::fs::write(&table, format!("set\n{}", "3 kW\n".repeat(30)))?;
    let args = [
        "macro",
        "generate",
        "starter",
        "--table",
        table.to_str().ok_or("path")?,
        "--size",
        "100x110",
    ];
    // A folder where the second new page's file goes makes the save fail after the first.
    let before = wc_core::store::load(&dir)?;
    // (the store writes `<file>.tmp` and renames it into place).
    let blocker = dir.join("pages/p0003.page.json.tmp");
    std::fs::create_dir_all(&blocker)?;
    assert!(wiringcraft(&dir, &args).is_err(), "the save failed");
    assert!(
        !dir.join("pages/p0002.page.json").exists(),
        "the first new page is taken back"
    );
    let vendored = dir.join("vendor/symbols/wc.coil.json");
    assert!(!vendored.exists(), "the symbol copy taken for it is taken back too");
    std::fs::remove_dir(&blocker)?;
    assert_eq!(wc_core::store::load(&dir)?.pages, before.pages);
    // A copy the project has that differs from the library's, unpinned, is replaced by a run;
    // a run that fails leaves it as it was.
    std::fs::create_dir_all(dir.join("vendor/symbols"))?;
    let own = std::fs::read_to_string(lib.join("symbols/wc.coil.json"))?.replace("\"Relay coil\"", "\"Own coil\"");
    std::fs::write(&vendored, &own)?;
    std::fs::create_dir_all(&blocker)?;
    assert!(wiringcraft(&dir, &args).is_err(), "the save failed");
    assert_eq!(
        std::fs::read_to_string(&vendored)?,
        own,
        "the project's copy is put back"
    );
    std::fs::remove_dir(&blocker)?;
    std::fs::remove_file(&vendored)?;
    let out = wiringcraft(&dir, &args)?;
    assert!(out.starts_with("p0002 p0003"), "{out}");
    assert!(out.contains("30 copies"), "{out}");
    let p = wc_core::store::load(&dir)?;
    let placed: usize = p.pages.iter().map(|pg| pg.symbols.len()).sum();
    assert_eq!(placed, 30, "one copy per row");
    Ok(())
}

#[test]
fn a_generation_keeps_a_part_name_that_is_no_file_name() -> TestResult {
    // `ACME R24` is no library id (it has a space): like `macro place`, generation keeps it as
    // the device's part name instead of refusing the whole table.
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let coil = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("macros"))?;
    std::fs::copy(coil, lib.join("symbols/wc.coil.json"))?;
    std::fs::write(lib.join("macros/starter.json"), STARTER)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let table = tmp.path().join("motors.csv");
    std::fs::write(&table, "set,part\n3 kW,ACME R24\n")?;
    let out = wiringcraft(
        &dir,
        &["macro", "generate", "starter", "--table", table.to_str().ok_or("path")?],
    )?;
    assert!(out.contains("1 copy"), "{out}");
    let p = wc_core::store::load(&dir)?;
    let parts: Vec<Option<&str>> = p.devices.values().map(|d| d.part.as_deref()).collect();
    assert!(parts.contains(&Some("ACME R24")), "{parts:?}");
    assert!(
        !p.lock.parts.contains_key("ACME R24"),
        "a name no library has stays unpinned"
    );
    Ok(())
}

#[test]
fn a_generation_whose_save_fails_takes_back_a_new_black_box_symbol_copy() -> TestResult {
    // A `box.*` symbol copy is also in the save's own snapshot (the save removes unused ones):
    // after a failed save it must end as it was before the run, absent.
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let coil = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("macros"))?;
    let boxed = std::fs::read_to_string(coil)?.replace("\"wc.coil\"", "\"box.p9.s1\"");
    std::fs::write(lib.join("symbols/box.p9.s1.json"), boxed)?;
    std::fs::write(lib.join("macros/starter.json"), STARTER.replace("wc.coil", "box.p9.s1"))?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let table = tmp.path().join("motors.csv");
    std::fs::write(&table, format!("set\n{}", "3 kW\n".repeat(30)))?;
    let args = [
        "macro",
        "generate",
        "starter",
        "--table",
        table.to_str().ok_or("path")?,
        "--size",
        "100x110",
    ];
    let blocker = dir.join("pages/p0003.page.json.tmp");
    std::fs::create_dir_all(&blocker)?;
    assert!(wiringcraft(&dir, &args).is_err(), "the save failed");
    assert!(
        !dir.join("vendor/symbols/box.p9.s1.json").exists(),
        "the black box symbol copy taken for the run is taken back"
    );
    assert!(!dir.join("pages/p0002.page.json").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_generation_refuses_project_files_that_are_links() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let coil = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("macros"))?;
    std::fs::copy(coil, lib.join("symbols/wc.coil.json"))?;
    std::fs::write(lib.join("macros/starter.json"), STARTER)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    // The cover page is a link to a file kept elsewhere: a save would put a plain file there.
    let cover = dir.join("pages/p0001.page.json");
    let kept = tmp.path().join("cover.page.json");
    std::fs::rename(&cover, &kept)?;
    std::os::unix::fs::symlink(&kept, &cover)?;
    let table = tmp.path().join("motors.csv");
    std::fs::write(&table, "set\n3 kW\n")?;
    let err = wiringcraft(
        &dir,
        &["macro", "generate", "starter", "--table", table.to_str().ok_or("path")?],
    )
    .err()
    .ok_or("a linked page is refused")?
    .to_string();
    assert!(err.contains("is a link; macro generate"), "{err}");
    assert!(
        std::fs::symlink_metadata(&cover)?.file_type().is_symlink(),
        "the link stays"
    );
    assert!(!dir.join("pages/p0002.page.json").exists(), "nothing is written");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_generation_refuses_a_symbol_copy_that_is_a_dangling_link() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let coil = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("macros"))?;
    std::fs::copy(coil, lib.join("symbols/wc.coil.json"))?;
    std::fs::write(lib.join("macros/starter.json"), STARTER)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    // The coil's copy is a link to a file that is not there: a rollback would remove it.
    let copy = wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Symbol, "wc.coil")?;
    std::fs::create_dir_all(copy.parent().ok_or("parent")?)?;
    std::os::unix::fs::symlink(tmp.path().join("gone.json"), &copy)?;
    let table = tmp.path().join("motors.csv");
    std::fs::write(&table, "set\n3 kW\n")?;
    let err = wiringcraft(
        &dir,
        &["macro", "generate", "starter", "--table", table.to_str().ok_or("path")?],
    )
    .err()
    .ok_or("a linked symbol copy is refused")?
    .to_string();
    assert!(err.contains("is a link; macro generate"), "{err}");
    assert!(
        std::fs::symlink_metadata(&copy)?.file_type().is_symlink(),
        "the link stays"
    );
    assert!(!dir.join("pages/p0002.page.json").exists(), "nothing is written");
    Ok(())
}

#[test]
fn a_table_cut_short_inside_a_quoted_cell_places_nothing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    std::fs::create_dir_all(dir.join("macros"))?;
    std::fs::write(dir.join("macros/starter.json"), STARTER)?;
    let table = tmp.path().join("cut.csv");
    std::fs::write(&table, "set;kW\n3 kW;\"4\n\"\n3 kW;\"5")?;
    let Err(err) = wiringcraft(
        &dir,
        &["macro", "generate", "starter", "--table", table.to_str().ok_or("path")?],
    ) else {
        return Err("generated from a table cut short".into());
    };
    assert!(err.contains("line 4") && err.contains("not closed"), "{err}");
    Ok(())
}
