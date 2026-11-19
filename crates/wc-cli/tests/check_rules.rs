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
    wiringcraft(&dir, &["page", "add", "p1", "--number", "1"])?;
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
        stdout.contains("warning page 1: -M1 has no cable (check rule 1)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("error   page 1: -X2 does not match -X#.# (check rule 2)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("warning page 1: -X1.1:2 has 2 wires, at most 1 allowed (check rule 3)"),
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
        house.contains("warning page 1: -X1.1 does not match -?# (check rule 1 \"short-tags\" of library mine)"),
        "{house}"
    );
    assert!(
        house.contains("note    page 1: -X1.1:2 has 2 wires, at most 1 allowed (check rule 2 of library mine)"),
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
