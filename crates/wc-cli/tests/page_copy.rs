//! `wiringcraft page copy`: a page copied with new tags and no wire numbers, as the app's
//! Duplicate page does.

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

#[test]
fn a_copied_page_gets_new_tags_and_no_wire_numbers() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "7", "--title", "Pump"])?;
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    let symbols = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols");
    std::fs::copy(symbols.join("wc.terminal.json"), lib.join("symbols/wc.terminal.json"))?;
    wiringcraft(&dir, &["lib", "add", "mine", "folder", lib.to_str().ok_or("path")?])?;
    let a = wiringcraft(&dir, &["place", "p1", "wc.terminal", "--at", "40,40", "--tag=-X1.1"])?;
    let b = wiringcraft(&dir, &["place", "p1", "wc.terminal", "--at", "80,40", "--tag=-X1.2"])?;
    wiringcraft(
        &dir,
        &["wire", "p1", &format!("{}:2", a.trim()), &format!("{}:1", b.trim())],
    )?;
    wiringcraft(&dir, &["number-wires"])?;
    let pages = wc_core::store::load(&dir)?.pages.len();

    let id = wiringcraft(&dir, &["page", "copy", "7"])?;
    let p = wc_core::store::load(&dir)?;
    assert_eq!(p.pages.len(), pages + 1);
    let copy = p.page(id.trim()).ok_or("the new page")?;
    assert_eq!((copy.number.as_str(), copy.title.as_str()), ("8", "Pump (copy)"));
    let tags: Vec<String> = copy
        .symbols
        .iter()
        .filter_map(|s| s.tag.as_ref().map(|t| t.to_string()))
        .collect();
    assert_eq!(tags.len(), 2);
    assert!(
        !tags.contains(&"-X1.1".to_owned()) && !tags.contains(&"-X1.2".to_owned()),
        "{tags:?}"
    );
    assert!(copy.wires.iter().all(|w| w.number.is_none()));
    let original = p.page("p1").ok_or("p1")?;
    assert!(
        original.wires.iter().all(|w| w.number.is_some()),
        "the original keeps its numbers"
    );

    // An id, number and title of its own; a taken id or number is refused.
    let out = wiringcraft(
        &dir,
        &[
            "page", "copy", "p1", "--id", "spare", "--number", "7.1", "--title", "Spare",
        ],
    )?;
    assert_eq!(out, "spare\n");
    let p = wc_core::store::load(&dir)?;
    let spare = p.page("spare").ok_or("spare")?;
    assert_eq!((spare.number.as_str(), spare.title.as_str()), ("7.1", "Spare"));
    let before = p.pages.len();
    let err = wiringcraft(&dir, &["page", "copy", "p1", "--id", "SPARE"])
        .err()
        .unwrap_or_default();
    assert!(err.contains("exists"), "{err}");
    let err = wiringcraft(&dir, &["page", "copy", "p1", "--number", "7"])
        .err()
        .unwrap_or_default();
    assert!(!err.is_empty());
    let err = wiringcraft(&dir, &["page", "copy", "nope"]).err().unwrap_or_default();
    assert!(err.contains("no page"), "{err}");
    assert_eq!(wc_core::store::load(&dir)?.pages.len(), before);
    Ok(())
}
