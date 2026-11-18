//! `wiringcraft template show`, run as a program so its printed output is what is checked.

use std::path::Path;
use std::process::Command;

use wc_core::template::PageTemplate;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wiringcraft(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

#[test]
fn template_show_prints_the_template_in_use_as_toml() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .args(["new"])
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());

    // The built-in layout, when the project has none.
    let shown: PageTemplate = toml::from_str(&wiringcraft(&dir, &["template", "show"])?)?;
    assert_eq!(shown, PageTemplate::default());

    // A changed one, as the project has it.
    wiringcraft(&dir, &["template", "columns", "14"])?;
    let shown: PageTemplate = toml::from_str(&wiringcraft(&dir, &["template", "show"])?)?;
    assert_eq!(shown.columns, 14);
    assert_eq!(shown.boxes, PageTemplate::default().boxes);
    Ok(())
}
