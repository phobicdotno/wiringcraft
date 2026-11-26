//! `wiringcraft report-layout show/set/reset`, run as a program so its output and the saved
//! project are what is checked.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(dir: &Path, args: &[&str], stdin: &str) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut input) = child.stdin.take() {
        // A command refused before it reads its input may have ended already.
        match input.write_all(stdin.as_bytes()) {
            Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => return Err(e.into()),
            _ => {}
        }
    }
    Ok(child.wait_with_output()?)
}

fn wiringcraft(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = run(dir, args, "")?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

fn new_project(root: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let dir = root.join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    Ok(dir)
}

fn show(dir: &Path, kind: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let list: serde_json::Value = serde_json::from_str(&wiringcraft(dir, &["report-layout", "show", kind])?)?;
    let [one] = list.as_array().map(Vec::as_slice).unwrap_or_default() else {
        return Err(format!("one report expected: {list}").into());
    };
    Ok(one.clone())
}

#[test]
fn a_report_layout_is_shown_set_and_reset() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = new_project(tmp.path())?;

    // Every report, each with the built-in layout.
    let all: serde_json::Value = serde_json::from_str(&wiringcraft(&dir, &["report-layout", "show"])?)?;
    let all = all.as_array().ok_or("a list")?;
    assert!(all.len() > 5, "{all:?}");
    assert!(all.iter().all(|r| r["layout"].is_null()), "{all:?}");
    let parts = show(&dir, "parts")?;
    let builtin: Vec<String> = serde_json::from_value(parts["builtin_columns"].clone())?;
    assert!(builtin.len() > 1, "{builtin:?}");

    // Set from a file: trimmed as the dialog keeps it, a title-block key with a comma whole.
    let file = tmp.path().join("layout.json");
    let first = &builtin[0];
    std::fs::write(
        &file,
        serde_json::json!({
            "title": " BOM ",
            "column": [{"from": first, "heading": " First "}],
            "sort": [format!("-{first}")],
            "header": ["project.name", " "],
        })
        .to_string(),
    )?;
    let said = wiringcraft(&dir, &["report-layout", "set", "parts", &file.to_string_lossy()])?;
    assert!(said.contains("layout is set"), "{said}");
    let layout = &show(&dir, "parts")?["layout"];
    assert_eq!(layout["title"], "BOM");
    assert_eq!(layout["column"][0]["heading"], "First");
    assert_eq!(layout["sort"][0], format!("-{first}"));
    assert_eq!(layout["header"], serde_json::json!(["project.name"]));
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    assert!(
        toml.contains("[[report]]") && toml.contains("title = \"BOM\""),
        "{toml}"
    );

    // From standard input; a column the report does not have is kept and warned about.
    let out = run(
        &dir,
        &["report-layout", "set", "parts", "-"],
        r#"{"column": [{"from": "No such column"}]}"#,
    )?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("warning:") && err.contains("No such column"), "{err}");
    let shown = show(&dir, "parts")?;
    assert_eq!(shown["layout"]["column"][0]["from"], "No such column");
    assert!(!shown["warnings"].as_array().ok_or("a list")?.is_empty(), "{shown}");

    // Reset: the table is gone.
    let said = wiringcraft(&dir, &["report-layout", "reset", "parts"])?;
    assert!(said.contains("built-in layout"), "{said}");
    assert!(show(&dir, "parts")?["layout"].is_null());
    assert!(!std::fs::read_to_string(dir.join("project.toml"))?.contains("[[report]]"));
    Ok(())
}

#[test]
fn a_bad_report_layout_is_refused_and_nothing_saved() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = new_project(tmp.path())?;
    let before = std::fs::read_to_string(dir.join("project.toml"))?;
    for (args, input, why) in [
        (
            vec!["report-layout", "set", "parts", "-"],
            r#"{"titel": "x"}"#,
            "Unknown layout key",
        ),
        (
            vec!["report-layout", "set", "parts", "-"],
            r#"{"column": [{"form": "x"}]}"#,
            "Unknown column key",
        ),
        (vec!["report-layout", "set", "parts", "-"], "[1]", "must be an object"),
        (vec!["report-layout", "set", "parts", "-"], "{", "stdin"),
        (vec!["report-layout", "set", "nope", "-"], "{}", "unknown report"),
        (vec!["report-layout", "set", "all", "-"], "{}", "name one report"),
        (vec!["report-layout", "show", "nope"], "", "unknown report"),
    ] {
        let out = run(&dir, &args, input)?;
        assert!(!out.status.success(), "{args:?} {input}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(why), "{args:?} {input}: {err}");
    }
    assert_eq!(std::fs::read_to_string(dir.join("project.toml"))?, before);
    Ok(())
}
