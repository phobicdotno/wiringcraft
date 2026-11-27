//! `wiringcraft check` with the project's own `[[rule]]` tables in `project.toml`, run as a
//! program.

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
}

fn wiringcraft(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = run(dir, args).map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

const RULES: &str = r#"
# Every motor has a cable.
[[rule]]
kind = "cable"
devices = "-M*"

# Terminals are tagged like -X1.3.
[[rule]]
kind = "tag"
role = "terminal"
pattern = "-X#.#"
severity = "error"

# At most one wire on a terminal pin.
[[rule]]
kind = "max-wires"
max = 1
"#;

#[test]
fn check_runs_the_rules_in_project_toml() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "2"])?;
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    let symbols = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols");
    for s in ["wc.motor.1ph", "wc.terminal"] {
        std::fs::copy(symbols.join(format!("{s}.json")), lib.join(format!("symbols/{s}.json")))?;
    }
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let motor = wiringcraft(&dir, &["place", "p1", "wc.motor.1ph", "--at", "100,100", "--tag=-M1"])?;
    let good = wiringcraft(&dir, &["place", "p1", "wc.terminal", "--at", "40,40", "--tag=-X1.1"])?;
    let bad = wiringcraft(&dir, &["place", "p1", "wc.terminal", "--at", "60,40", "--tag=-X2"])?;
    let (motor, good, bad) = (motor.trim(), good.trim(), bad.trim());
    wiringcraft(&dir, &["wire", "p1", &format!("{good}:2"), &format!("{motor}:L")])?;
    wiringcraft(&dir, &["wire", "p1", &format!("{good}:2"), &format!("{bad}:1")])?;

    // No rules yet: nothing from them.
    let before = wiringcraft(&dir, &["check"])?;
    assert!(!before.contains("check rule"), "{before}");

    let toml = dir.join("project.toml");
    let mut text = std::fs::read_to_string(&toml)?;
    text.push_str(RULES);
    std::fs::write(&toml, text)?;

    let out = run(&dir, &["check"])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "a rule with severity error fails the check: {stdout}"
    );
    assert!(
        stdout.contains("warning page 2: -M1 has no cable (check rule 1)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("error   page 2: -X2 does not match -X#.# (check rule 2)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("warning page 2: -X1.1:2 has 2 wires, at most 1 allowed (check rule 3)"),
        "{stdout}"
    );
    assert!(!stdout.contains("-X1.1 does not match"), "{stdout}");

    // A cable on the motor's wire satisfies rule 1.
    wiringcraft(
        &dir,
        &[
            "wire",
            "p1",
            &format!("{motor}:N"),
            "100,140",
            "--cable=-W1",
            "--core",
            "1",
        ],
    )?;
    let after = String::from_utf8_lossy(&run(&dir, &["check"])?.stdout).into_owned();
    assert!(!after.contains("has no cable"), "{after}");
    // Saving the project on that edit kept the rules.
    assert!(after.contains("-X2 does not match -X#.# (check rule 2)"), "{after}");

    // The library source's house rules run too, named by their source.
    std::fs::write(
        lib.join("rules.toml"),
        "[[rule]]\nname = \"short-tags\"\nkind = \"tag\"\npattern = \"-?#\"\n\n[[rule]]\nkind = \"max-wires\"\nmax = 1\nseverity = \"note\"\n",
    )?;
    let house = String::from_utf8_lossy(&run(&dir, &["check", "--all"])?.stdout).into_owned();
    assert!(
        house.contains("warning page 2: -X1.1 does not match -?# (check rule 1 \"short-tags\" of library mine)"),
        "{house}"
    );
    assert!(
        house.contains("note    page 2: -X1.1:2 has 2 wires, at most 1 allowed (check rule 2 of library mine)"),
        "{house}"
    );
    // The project turns one off by name and one by number.
    let mut text = std::fs::read_to_string(&toml)?;
    text = format!("rules_off = [\"mine:short-tags\", \"mine:2\"]\n{text}");
    std::fs::write(&toml, text)?;
    let off = String::from_utf8_lossy(&run(&dir, &["check", "--all"])?.stdout).into_owned();
    assert!(!off.contains("of library mine"), "{off}");
    assert!(
        off.contains("(check rule 2)"),
        "the project's own rules still run: {off}"
    );
    Ok(())
}

#[test]
fn rules_lists_sets_and_turns_library_rules_off_and_on() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    std::fs::write(
        lib.join("rules.toml"),
        "[[rule]]\nname = \"short-tags\"\nkind = \"tag\"\npattern = \"-?#\"\n\n[[rule]]\nkind = \"max-wires\"\nmax = 1\n\n[[rule]]\nname = \"2\"\nkind = \"cable\"\n",
    )?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;

    // Nothing of the project's own yet; the library's rules all run.
    let list = wiringcraft(&dir, &["rules", "list"])?;
    assert_eq!(
        list,
        "mine:1\ton\ttag name=short-tags pattern=-?#\nmine:2\ton\tmax-wires max=1\nmine:3\ton\tcable name=2\n"
    );

    // Set the project's own rules from TOML; the dialog's checks normalise them.
    let file = tmp.path().join("rules.toml");
    std::fs::write(&file, RULES.replace("kind = \"cable\"", "kind = \"Cable\""))?;
    assert_eq!(
        wiringcraft(&dir, &["rules", "set", file.to_str().ok_or("path")?])?,
        "3 check rules\n"
    );
    let list = wiringcraft(&dir, &["rules", "list"])?;
    assert!(
        list.starts_with(
            "1\ton\tcable devices=-M*\n2\ton\ttag role=terminal pattern=-X#.# severity=error\n3\ton\tmax-wires max=1\n"
        ),
        "{list}"
    );
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    assert!(toml.contains("kind = \"cable\""), "{toml}");

    // A rule the check could not run is refused, and nothing changes.
    let bad = tmp.path().join("bad.json");
    for (text, why) in [
        (r#"[{"kind": "tag"}]"#, "Rule 1: a tag rule needs the pattern"),
        (
            r#"[{"kind": "cable"}, {"kind": "cable", "severty": "error"}]"#,
            "Rule 2: no such key severty",
        ),
        (
            r#"{"rules": [{"kind": "max-wires", "role": "boss", "max": 2}]}"#,
            "Rule 1: pick a role",
        ),
        (r#"{"kind": "cable"}"#, "not a list of rules"),
    ] {
        std::fs::write(&bad, text)?;
        let Err(err) = wiringcraft(&dir, &["rules", "set", bad.to_str().ok_or("path")?]) else {
            unreachable!("{text} is refused");
        };
        assert!(err.contains(why), "{text}: {err}");
    }
    // A misspelt table name in TOML is refused too, rather than read as no rules.
    let misspelt = tmp.path().join("misspelt.toml");
    std::fs::write(&misspelt, "[[rules]]\nkind = \"cable\"\n")?;
    let Err(err) = wiringcraft(&dir, &["rules", "set", misspelt.to_str().ok_or("path")?]) else {
        unreachable!("[[rules]] is refused");
    };
    assert!(err.contains("\"rules\" is not a rule table"), "{err}");
    assert_eq!(std::fs::read_to_string(dir.join("project.toml"))?, toml);

    // JSON, as `rules list --json` prints them.
    std::fs::write(&bad, r#"{"rules": [{"kind": "max-wires", "max": 2, "name": "two"}]}"#)?;
    assert_eq!(
        wiringcraft(&dir, &["rules", "set", bad.to_str().ok_or("path")?])?,
        "1 check rules\n"
    );
    let json: serde_json::Value = serde_json::from_str(&wiringcraft(&dir, &["rules", "list", "--json"])?)?;
    assert_eq!(
        json["rules"],
        serde_json::json!([{"kind": "max-wires", "max": 2, "name": "two"}])
    );
    assert_eq!(json["library"][0]["rules"][1]["rule"], "mine:2");

    // Library rules off by name and by number, then on again.
    assert_eq!(
        wiringcraft(&dir, &["rules", "off", "mine:short-tags"])?,
        "mine:short-tags is off\n"
    );
    assert_eq!(
        wiringcraft(&dir, &["rules", "off", "mine:1"])?,
        "mine:1 is off already\n"
    );
    assert_eq!(
        wiringcraft(&dir, &["rules", "off", "mine:max-wires"])
            .err()
            .map(|e| e.contains("No library check rule \"mine:max-wires\"")),
        Some(true)
    );
    // "mine:2" is rule 2 by number and rule 3 by name: refused rather than guessed.
    let Err(err) = wiringcraft(&dir, &["rules", "off", "mine:2"]) else {
        unreachable!("an ambiguous selector is refused");
    };
    assert!(err.contains("could be rule mine:2 or mine:3"), "{err}");
    let list = wiringcraft(&dir, &["rules", "list"])?;
    assert!(list.contains("mine:1\toff\t"), "{list}");
    assert!(list.contains("mine:2\ton\t"), "{list}");
    let check = String::from_utf8_lossy(&run(&dir, &["check", "--all"])?.stdout).into_owned();
    assert!(!check.contains("short-tags"), "{check}");
    assert_eq!(wiringcraft(&dir, &["rules", "on", "mine:1"])?, "mine:1 is on\n");
    let list = wiringcraft(&dir, &["rules", "list"])?;
    assert!(list.contains("mine:1\ton\t"), "{list}");
    assert_eq!(wiringcraft(&dir, &["rules", "on", "mine:1"])?, "mine:1 is on already\n");
    // A misspelt rule is refused rather than reported on already (#1041).
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    let Err(err) = wiringcraft(&dir, &["rules", "on", "mine:short-tag"]) else {
        unreachable!("a selector naming no rule is refused");
    };
    assert!(err.contains("No library check rule \"mine:short-tag\""), "{err}");
    assert_eq!(std::fs::read_to_string(dir.join("project.toml"))?, toml);

    // An entry naming no rule is listed and can be dropped.
    let text = std::fs::read_to_string(dir.join("project.toml"))?;
    std::fs::write(dir.join("project.toml"), format!("rules_off = [\"gone:4\"]\n{text}"))?;
    assert!(wiringcraft(&dir, &["rules", "list"])?.ends_with("gone:4\toff\tnames no library rule\n"));
    assert_eq!(wiringcraft(&dir, &["rules", "on", "gone:4"])?, "gone:4 is on\n");
    assert!(!std::fs::read_to_string(dir.join("project.toml"))?.contains("gone:4"));
    Ok(())
}

#[test]
fn check_names_an_unreadable_part_copy_with_its_reason_not_as_missing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let lib = tmp.path().join("lib");
    for id in ["maker.contactor", "maker.relay"] {
        let part = wc_core::Part {
            id: id.into(),
            ..wc_core::Part::default()
        };
        wc_library::write_part(&lib, &part)?;
    }
    wiringcraft(&dir, &["lib", "add", "shop", "folder", lib.to_str().ok_or("path")?])?;
    wiringcraft(&dir, &["device", "--part", "maker.contactor", "--", "-K1"])?;
    wiringcraft(&dir, &["device", "--part", "maker.relay", "--", "-K2"])?;
    // -K1's copy is damaged, -K2's is gone.
    let copy = |id: &str| wc_core::store::vendor_path(&dir, wc_core::lock::ItemKind::Part, id);
    std::fs::write(copy("maker.contactor")?, "not a part")?;
    std::fs::remove_file(copy("maker.relay")?)?;

    let out = wiringcraft(&dir, &["check"])?;
    let Some(line) = out
        .lines()
        .find(|l| l.contains("-K1: part maker.contactor in the project's library copy cannot be read: "))
    else {
        unreachable!("no unreadable part warning: {out}");
    };
    assert!(line.starts_with("warning"), "{out}");
    assert!(!line.ends_with("cannot be read: "), "the reason follows: {out}");
    assert!(!out.contains("part maker.contactor is missing"), "{out}");
    assert!(
        out.contains("-K2: part maker.relay is missing from the project's library copy"),
        "{out}"
    );
    Ok(())
}
