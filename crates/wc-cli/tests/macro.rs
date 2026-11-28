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
    assert!(!copy.exists(), "the coil's copy is taken back with the failure");
    assert!(
        !wc_core::store::load(&dir)?.lock.symbols.contains_key("wc.coil"),
        "and it is not pinned"
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
fn a_part_id_that_differs_only_in_case_from_a_pinned_one_is_refused() -> TestResult {
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
    let file = lib.join(format!("parts/{lamp}.json"));
    std::fs::copy(repo.join(format!("library/parts/{lamp}.json")), &file)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let lamp_macro = |part: &str| {
        format!(
            r#"{{"name": "Lamp", "anchor": {{"x": 0.0, "y": 0.0}},
            "symbols": [{{"id": "s1", "symbol": "wc.coil", "at": {{"x": 10.0, "y": 10.0}}, "tag": "-P1"}}],
            "devices": {{"-P1": {{"part": "{part}"}}}}}}"#
        )
    };
    std::fs::create_dir_all(dir.join("macros"))?;
    std::fs::write(dir.join("macros/lamp.json"), lamp_macro(lamp))?;
    wiringcraft(&dir, &["macro", "place", "p1", "lamp", "--at", "0,0"])?;
    assert!(wc_core::store::load(&dir)?.lock.parts.contains_key(lamp));

    // The library now holds the part under an id in other case (one file, so this also works
    // on a folder that ignores case), and a second macro names it that way.
    let upper = "EXAMPLE.ex-lamp-24-rd";
    let mut part: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
    part["id"] = upper.into();
    std::fs::remove_file(&file)?;
    std::fs::write(lib.join(format!("parts/{upper}.json")), part.to_string())?;
    std::fs::write(dir.join("macros/upper.json"), lamp_macro(upper))?;
    let before = folder(&dir)?;
    let Err(err) = wiringcraft(&dir, &["macro", "place", "p1", "upper", "--at", "50,0"]) else {
        return Err("took a part whose copy would be the pinned part's file".into());
    };
    assert!(err.contains("differ only in case"), "{err}");
    assert!(err.contains(lamp) && err.contains(upper), "both ids are named: {err}");
    assert_eq!(
        changed(&before, &folder(&dir)?),
        Vec::<&Path>::new(),
        "nothing is written"
    );

    // A table row naming it is refused the same way.
    let table = tmp.path().join("rows.csv");
    std::fs::write(
        dir.join("macros/row.json"),
        lamp_macro("{{part}}").replace(r#""devices""#, r#""placeholders": [{"name": "part"}], "devices""#),
    )?;
    std::fs::write(&table, format!("part\n{lamp}\n{upper}\n"))?;
    let before = folder(&dir)?;
    let Err(err) = wiringcraft(
        &dir,
        &["macro", "generate", "row", "--table", table.to_str().ok_or("path")?],
    ) else {
        return Err("generated pages with a part whose copy would be the pinned part's file".into());
    };
    assert!(err.contains("differ only in case"), "{err}");
    assert_eq!(
        changed(&before, &folder(&dir)?),
        Vec::<&Path>::new(),
        "nothing is written"
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

    // A page column titles the pages: a change of title starts a new page, an empty cell
    // stays on the page before.
    let areas = tmp.path().join("areas.csv");
    std::fs::write(
        &areas,
        "kW;Page
1;Pumps
2;
3;Fans
4;Pumps
",
    )?;
    let out = wiringcraft(
        &dir,
        &[
            "macro",
            "generate",
            "starter",
            "--table",
            areas.to_str().ok_or("path")?,
            "--set",
            "3 kW",
        ],
    )?;
    assert_eq!(out, "p0003 p0004 p0005\n4 copies of Motor starter on 3 pages\n");
    let p = wc_core::store::load(&dir)?;
    let made: Vec<(&str, &str, usize)> = ["p0003", "p0004", "p0005"]
        .iter()
        .filter_map(|id| p.page(id))
        .map(|page| (page.number.as_str(), page.title.as_str(), page.symbols.len()))
        .collect();
    assert_eq!(made, [("4", "Pumps", 2), ("5", "Fans", 1), ("6", "Pumps", 1)]);

    // A macro column picks another macro per row (by its file name or its name); --set is for
    // the starter's rows only.
    std::fs::write(
        lib.join("macros/valve.json"),
        r#"{
  "name": "Valve",
  "symbols": [{"id": "v1", "symbol": "wc.coil", "at": {"x": 20.0, "y": 30.0}, "tag": "-Y1"}],
  "devices": {"-Y1": {"description": "Valve {{bar}} bar"}},
  "anchor": {"x": 20.0, "y": 30.0},
  "placeholders": [{"name": "bar"}]
}"#,
    )?;
    let mixed = tmp.path().join("mixed.csv");
    std::fs::write(
        &mixed,
        "macro;kW;bar
;1;
valve;;6
VALVE;;7
;2;
",
    )?;
    let generate = |table: &Path| {
        let table = table.to_str().ok_or("path")?;
        wiringcraft(
            &dir,
            &[
                "macro", "generate", "starter", "--table", table, "--set", "3 kW", "--title", "Mixed",
            ],
        )
    };
    assert_eq!(
        generate(&mixed)?,
        "p0006\n4 copies: 2 of Motor starter, 2 of Valve on 1 page\n"
    );
    let p = wc_core::store::load(&dir)?;
    let page = p.page("p0006").ok_or("page")?;
    let rows: Vec<String> = page
        .symbols
        .iter()
        .filter_map(|s| s.tag.as_ref())
        .filter_map(|t| p.devices.get(t).map(|d| format!("{t} {}", d.description)))
        .collect();
    assert_eq!(
        rows,
        [
            "-Q8 Starter 1 kW",
            "-Y1 Valve 6 bar",
            "-Y2 Valve 7 bar",
            "-Q9 Starter 2 kW"
        ]
    );
    // A value for a placeholder the row's macro lacks, and an unknown macro, name the line
    // and place nothing.
    for (table, want) in [
        (
            "macro;kW;bar\n;1;5\n",
            "line 2: macro \"Motor starter\": \"bar\" is not a placeholder",
        ),
        ("macro;kW\n;1\nnope;\n", "line 3: no macro \"nope\""),
    ] {
        std::fs::write(&mixed, table)?;
        let Err(err) = generate(&mixed) else {
            return Err(format!("generated from {table:?}").into());
        };
        assert!(err.contains(want), "{err}");
    }
    assert_eq!(wc_core::store::load(&dir)?.pages.len(), p.pages.len());

    // A rule picks the valve for the rows whose type says so (#1900); the type column is given
    // to neither macro, and a rule naming a column the table lacks places nothing.
    std::fs::write(&mixed, "type;kW;bar\nDOL;1;\n Valve ;;6\n")?;
    let table = mixed.to_str().ok_or("path")?;
    let ruled = |rule: &str| {
        wiringcraft(
            &dir,
            &[
                "macro", "generate", "starter", "--table", table, "--set", "3 kW", "--rule", rule,
            ],
        )
    };
    assert_eq!(
        ruled("type=valve:valve")?,
        "p0007\n2 copies: 1 of Motor starter, 1 of Valve on 1 page\n"
    );
    // Of two --rule options a row matches, the first given wins: the starter has no "bar".
    let valves = tmp.path().join("valves.csv");
    std::fs::write(&valves, "type;bar\nvalve;6\n")?;
    let out = wiringcraft(
        &dir,
        &[
            "macro",
            "generate",
            "starter",
            "--table",
            valves.to_str().ok_or("path")?,
            "--rule",
            "type=valve:valve",
            "--rule",
            "type=valve:starter",
        ],
    )?;
    assert!(out.starts_with("p0008\n") && out.contains("Valve"), "{out}");
    let Err(err) = ruled("kind=valve:valve") else {
        return Err("a rule on a missing column was taken".into());
    };
    assert!(err.contains("the table has no column \"kind\""), "{err}");
    let Err(err) = ruled("type:valve") else {
        return Err("a rule without a value was taken".into());
    };
    assert!(err.contains("write it as column=value:macro"), "{err}");
    assert_eq!(wc_core::store::load(&dir)?.pages.len(), p.pages.len() + 2);
    Ok(())
}

/// Rows naming the same symbol and parts take each once per run (#1864). What the run writes
/// and refuses is as when every row looked its items up again: the same pins and copies, a
/// part no library has left alone, and of two parts that cannot be taken the one of the
/// earlier row is named.
#[test]
fn a_table_whose_rows_share_parts_takes_them_as_row_by_row() -> TestResult {
    use wc_core::lock::ItemKind;
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lib = tmp.path().join("lib");
    for folder in ["symbols", "parts", "macros"] {
        std::fs::create_dir_all(lib.join(folder))?;
    }
    std::fs::copy(
        repo.join("library/symbols/wc.coil.json"),
        lib.join("symbols/wc.coil.json"),
    )?;
    let (red, green) = ("example.ex-lamp-24-rd", "example.ex-lamp-24-gn");
    for part in [red, green] {
        std::fs::copy(
            repo.join(format!("library/parts/{part}.json")),
            lib.join(format!("parts/{part}.json")),
        )?;
    }
    std::fs::write(lib.join("macros/starter.json"), STARTER)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let generate = |table: &Path| {
        let table = table.to_str().ok_or("path")?;
        wiringcraft(
            &dir,
            &["macro", "generate", "starter", "--table", table, "--set", "3 kW"],
        )
    };

    // The red lamp first, though the green one sorts before it; each named by several rows.
    let table = tmp.path().join("lamps.csv");
    std::fs::write(
        &table,
        format!("kW;part\n1;{red}\n2;nowhere.cap\n3;{green}\n4;{red}\n5;nowhere.cap\n6;{green}\n"),
    )?;
    let out = generate(&table)?;
    assert!(out.contains("\n6 copies of Motor starter on "), "{out}");
    let p = wc_core::store::load(&dir)?;
    let parts: Vec<Option<&str>> = p.devices.values().map(|d| d.part.as_deref()).collect();
    assert_eq!(
        parts,
        [red, "nowhere.cap", green, red, "nowhere.cap", green].map(Some),
        "{:?}",
        p.devices.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        p.lock.parts.keys().map(String::as_str).collect::<Vec<_>>(),
        [green, red],
        "the library's parts are pinned, the part no library has is not"
    );
    assert!(p.lock.symbols.contains_key("wc.coil"));
    for part in [red, green] {
        assert!(
            wc_core::store::vendor_path(&dir, ItemKind::Part, part)?.exists(),
            "{part}"
        );
    }
    let pins = p.lock.clone();
    let pages = p.pages.len();

    // Both copies gone and the library holding other versions: the earlier row's part is the
    // one named, and nothing is placed.
    let mut originals = Vec::new();
    for part in [red, green] {
        std::fs::remove_file(wc_core::store::vendor_path(&dir, ItemKind::Part, part)?)?;
        let file = lib.join(format!("parts/{part}.json"));
        let original = std::fs::read_to_string(&file)?;
        let mut newer: serde_json::Value = serde_json::from_str(&original)?;
        newer["description"] = "A newer lamp".into();
        std::fs::write(&file, newer.to_string())?;
        originals.push((file, original));
    }
    let Err(err) = generate(&table) else {
        return Err("generated with newer versions of pinned parts".into());
    };
    assert!(
        err.contains(&format!("part {red}: its copy in the project is missing")) && !err.contains(green),
        "{err}"
    );
    let p = wc_core::store::load(&dir)?;
    assert_eq!((p.pages.len(), &p.lock), (pages, &pins));

    // With the pinned versions back in the library the copies come back, pins as they were.
    for (file, original) in &originals {
        std::fs::write(file, original)?;
    }
    let out = generate(&table)?;
    assert!(out.contains("\n6 copies of Motor starter on "), "{out}");
    let p = wc_core::store::load(&dir)?;
    assert_eq!(p.lock, pins);
    for part in [red, green] {
        assert!(
            wc_core::store::vendor_path(&dir, ItemKind::Part, part)?.exists(),
            "{part}"
        );
    }
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
fn a_generation_refuses_a_pages_folder_that_is_a_link() -> TestResult {
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
    // The pages are kept in a folder outside the project, reached through a link (#1858): a
    // save or a rollback would write there.
    let pages = dir.join("pages");
    let kept = tmp.path().join("kept-pages");
    std::fs::rename(&pages, &kept)?;
    std::os::unix::fs::symlink(&kept, &pages)?;
    let before: Vec<_> = std::fs::read_dir(&kept)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<_, _>>()?;
    let table = tmp.path().join("motors.csv");
    std::fs::write(&table, "set\n3 kW\n")?;
    let err = wiringcraft(
        &dir,
        &["macro", "generate", "starter", "--table", table.to_str().ok_or("path")?],
    )
    .err()
    .ok_or("a linked pages folder is refused")?
    .to_string();
    assert!(err.contains("does not write through a linked folder"), "{err}");
    assert!(err.contains("pages"), "{err}");
    let after: Vec<_> = std::fs::read_dir(&kept)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<_, _>>()?;
    assert_eq!(after.len(), before.len(), "nothing is written through the link");
    assert!(
        std::fs::symlink_metadata(&pages)?.file_type().is_symlink(),
        "the link stays"
    );
    assert!(
        !wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Symbol, "wc.coil")?.exists(),
        "no symbol copy was taken before the refusal"
    );
    // An empty pages folder that is a link is refused all the same.
    for f in std::fs::read_dir(&kept)? {
        std::fs::remove_file(f?.path())?;
    }
    let err = wiringcraft(
        &dir,
        &["macro", "generate", "starter", "--table", table.to_str().ok_or("path")?],
    )
    .err()
    .ok_or("an empty linked pages folder is refused")?
    .to_string();
    assert!(err.contains("does not write through a linked folder"), "{err}");
    assert_eq!(
        std::fs::read_dir(&kept)?.count(),
        0,
        "nothing is written through the link"
    );
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

type Folder = std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>;

/// Every file under `dir` with its bytes, by path. Folders are not listed: an empty
/// `vendor/symbols` folder made for a copy that was taken back stays, as after `macro generate`.
fn folder(dir: &Path) -> Result<Folder, std::io::Error> {
    let mut found = std::collections::BTreeMap::new();
    let mut open = vec![dir.to_path_buf()];
    while let Some(at) = open.pop() {
        for entry in std::fs::read_dir(&at)? {
            let path = entry?.path();
            if path.is_dir() {
                open.push(path);
            } else {
                let bytes = std::fs::read(&path)?;
                found.insert(path, bytes);
            }
        }
    }
    Ok(found)
}

/// The paths that are in one of two [`folder`] listings only, or differ between them.
fn changed<'a>(before: &'a Folder, after: &'a Folder) -> Vec<&'a Path> {
    let mut paths: Vec<&Path> = before
        .iter()
        .filter(|(path, bytes)| after.get(*path) != Some(bytes))
        .chain(after.iter().filter(|(path, _)| !before.contains_key(*path)))
        .map(|(path, _)| path.as_path())
        .collect();
    paths.sort();
    paths
}

/// A project with a page `p1` and a library `mine` holding the coil and the macro `pair`: the
/// coil and a symbol no library has. Returns the project folder and the library's.
fn project_with_a_pair_macro(
    tmp: &Path,
    second: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), Box<dyn std::error::Error>> {
    let dir = tmp.join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
    let coil = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.coil.json");
    let lib = tmp.join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::create_dir_all(lib.join("macros"))?;
    std::fs::copy(coil, lib.join("symbols/wc.coil.json"))?;
    let pair = r#"{
  "name": "Pair",
  "symbols": [{"id": "s1", "symbol": "wc.coil", "at": {"x": 20.0, "y": 30.0}, "tag": "-K1"},
              {"id": "s2", "symbol": "SECOND", "at": {"x": 40.0, "y": 30.0}, "tag": "-K2"}],
  "anchor": {"x": 10.0, "y": 10.0}
}"#;
    std::fs::write(lib.join("macros/pair.json"), pair.replace("SECOND", second))?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    Ok((dir, lib))
}

#[test]
fn a_placement_whose_second_symbol_cannot_be_taken_leaves_the_folder_as_it_was() -> TestResult {
    // `wc.coil` is taken before `zz.nowhere`, which no library has (#1856).
    let tmp = tempfile::tempdir()?;
    let (dir, lib) = project_with_a_pair_macro(tmp.path(), "zz.nowhere")?;
    let args = ["macro", "place", "p1", "pair", "--at", "50,60"];
    let before = folder(&dir)?;
    let Err(err) = wiringcraft(&dir, &args) else {
        return Err("placed a macro with a symbol no library has".into());
    };
    assert!(err.contains("zz.nowhere"), "{err}");
    assert!(
        !dir.join("vendor/symbols/wc.coil.json").exists(),
        "the first symbol's copy is taken back"
    );
    assert_eq!(
        changed(&before, &folder(&dir)?),
        Vec::<&Path>::new(),
        "the folder is as it was"
    );
    // A copy the project has that differs from the library's, unpinned, is replaced when the
    // symbol is taken; the run that fails puts it back.
    let vendored = dir.join("vendor/symbols/wc.coil.json");
    std::fs::create_dir_all(dir.join("vendor/symbols"))?;
    let own = std::fs::read_to_string(lib.join("symbols/wc.coil.json"))?.replace("\"Relay coil\"", "\"Own coil\"");
    std::fs::write(&vendored, &own)?;
    let before = folder(&dir)?;
    assert!(wiringcraft(&dir, &args).is_err(), "the second symbol is still missing");
    assert_eq!(
        std::fs::read_to_string(&vendored)?,
        own,
        "the project's copy is put back"
    );
    assert_eq!(
        changed(&before, &folder(&dir)?),
        Vec::<&Path>::new(),
        "the folder is as it was"
    );
    Ok(())
}

#[test]
fn a_placement_whose_save_fails_leaves_the_folder_as_it_was() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (dir, _lib) = project_with_a_pair_macro(tmp.path(), "wc.coil")?;
    let args = ["macro", "place", "p1", "pair", "--at", "50,60"];
    let before = folder(&dir)?;
    // A folder where a page's staging file goes makes the save fail (the store writes
    // `<file>.tmp` and renames it into place).
    let mut blockers = Vec::new();
    for entry in std::fs::read_dir(dir.join("pages"))? {
        let mut name = entry?.path().into_os_string();
        name.push(".tmp");
        blockers.push(std::path::PathBuf::from(name));
    }
    for blocker in &blockers {
        std::fs::create_dir_all(blocker)?;
    }
    assert!(wiringcraft(&dir, &args).is_err(), "the save failed");
    for blocker in &blockers {
        std::fs::remove_dir(blocker)?;
    }
    assert!(
        !dir.join("vendor/symbols/wc.coil.json").exists(),
        "the symbol copy taken for it is taken back"
    );
    assert_eq!(
        changed(&before, &folder(&dir)?),
        Vec::<&Path>::new(),
        "the folder is as it was"
    );
    // The same run without the folders in the way places the macro.
    let ids = wiringcraft(&dir, &args)?;
    assert_eq!(ids.split_whitespace().count(), 2, "{ids}");
    assert!(dir.join("vendor/symbols/wc.coil.json").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_placement_refuses_project_files_that_are_links() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (dir, _lib) = project_with_a_pair_macro(tmp.path(), "wc.coil")?;
    // The cover page is a link to a file kept elsewhere: a save would put a plain file there.
    let cover = dir.join("pages/p0001.page.json");
    let kept = tmp.path().join("cover.page.json");
    std::fs::rename(&cover, &kept)?;
    std::os::unix::fs::symlink(&kept, &cover)?;
    let target = std::fs::read(&kept)?;
    let before = folder(&dir)?;
    let err = wiringcraft(&dir, &["macro", "place", "p1", "pair", "--at", "50,60"])
        .err()
        .ok_or("a linked page is refused")?;
    assert!(err.contains("is a link; macro place"), "{err}");
    assert!(
        std::fs::symlink_metadata(&cover)?.file_type().is_symlink(),
        "the link stays"
    );
    assert_eq!(std::fs::read(&kept)?, target, "the file it points to is as it was");
    assert!(
        !dir.join("vendor/symbols/wc.coil.json").exists(),
        "the symbol copy taken before the refusal is taken back"
    );
    assert_eq!(
        changed(&before, &folder(&dir)?),
        Vec::<&Path>::new(),
        "the folder is as it was"
    );
    Ok(())
}

/// How many times one run of `args` opened the library, as the program reports it.
fn library_opens(dir: &Path, args: &[&str]) -> Result<usize, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("WIRINGCRAFT_COUNT_LIBRARY_OPENS", "1")
        .output()
        .map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(stderr);
    }
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("library opened: "))
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| format!("no count reported: {stderr}"))
}

#[test]
fn a_macro_run_opens_the_library_once() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lib = tmp.path().join("lib");
    for folder in ["symbols", "parts", "macros"] {
        std::fs::create_dir_all(lib.join(folder))?;
    }
    std::fs::copy(
        repo.join("library/symbols/wc.coil.json"),
        lib.join("symbols/wc.coil.json"),
    )?;
    let (red, green) = ("example.ex-lamp-24-rd", "example.ex-lamp-24-gn");
    for part in [red, green] {
        std::fs::copy(
            repo.join(format!("library/parts/{part}.json")),
            lib.join(format!("parts/{part}.json")),
        )?;
    }
    std::fs::write(lib.join("macros/starter.json"), STARTER)?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;

    // Rows taking a symbol and two parts from the library, and one naming a part no library has.
    let table = tmp.path().join("lamps.csv");
    std::fs::write(&table, format!("kW;part\n1;{red}\n2;nowhere.cap\n3;{green}\n4;{red}\n"))?;
    let table = table.to_str().ok_or("path")?;
    let generate = ["macro", "generate", "starter", "--table", table, "--set", "3 kW"];
    assert_eq!(library_opens(&dir, &generate)?, 1);
    let p = wc_core::store::load(&dir)?;
    assert!(p.lock.symbols.contains_key("wc.coil"));
    assert_eq!(p.lock.parts.len(), 2);

    // Placing looks for the value set's part, which is not pinned, in the library that listed
    // the macros, not in a second one.
    let page = p.pages.first().map(|pg| pg.id.clone()).ok_or("no page")?;
    let place = [
        "macro", "place", &page, "starter", "--at", "200,150", "--set", "0.75 kW",
    ];
    assert_eq!(library_opens(&dir, &place)?, 1);
    Ok(())
}

/// `--join` runs the rails a macro draws on through a page row: a wire from each copy's right
/// edge to the next copy's left edge (#1909).
#[test]
fn joined_copies_get_a_wire_between_their_rails() -> TestResult {
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
        lib.join("macros/rail.json"),
        r#"{
  "name": "Rail",
  "wires": [{"id": "w1", "from": {"at": {"x": 0.0, "y": 10.0}}, "to": {"at": {"x": 40.0, "y": 10.0}}}],
  "anchor": {"x": 0.0, "y": 10.0}
}"#,
    )?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let table = tmp.path().join("rails.csv");
    std::fs::write(&table, "macro\nrail\nrail\nrail\n")?;
    let table = table.to_str().ok_or("path")?;
    let out = wiringcraft(&dir, &["macro", "generate", "rail", "--table", table, "--join"])?;
    assert!(
        out.ends_with("3 copies of Rail on 1 page\njoined by 2 wires\n"),
        "{out}"
    );
    let p = wc_core::store::load(&dir)?;
    let id = out.lines().next().ok_or("no page")?;
    assert_eq!(p.page(id).ok_or("page")?.wires.len(), 5);
    // Without --join no wire is added, and nothing is said of joins.
    let out = wiringcraft(&dir, &["macro", "generate", "rail", "--table", table])?;
    assert!(!out.contains("joined"), "{out}");
    let p = wc_core::store::load(&dir)?;
    let id = out.lines().next().ok_or("no page")?;
    assert_eq!(p.page(id).ok_or("page")?.wires.len(), 3);
    Ok(())
}
