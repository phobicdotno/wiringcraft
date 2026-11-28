//! `wiringcraft page renumber`: the pages numbered again in their order.

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

fn numbers(dir: &Path) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let p = wc_core::store::load(dir)?;
    Ok(p.pages.iter().map(|p| (p.id.clone(), p.number.clone())).collect())
}

#[test]
fn pages_are_numbered_again_in_their_order() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let mut p = wc_core::store::load(&dir)?;
    p.pages.clear();
    wc_core::store::save(&dir, &p)?;
    for (id, n) in [("a", "1"), ("b", "4"), ("c", "4.1"), ("d", "12")] {
        wiringcraft(&dir, &["page", "add", id, "--number", n])?;
    }

    assert_eq!(wiringcraft(&dir, &["page", "renumber"])?, "3 of 4 pages renumbered\n");
    let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        v.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect()
    };
    assert_eq!(numbers(&dir)?, pairs(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]));

    // From a page on (named by its number), in tens, with a prefix.
    let out = wiringcraft(
        &dir,
        &["page", "renumber", "--from", "3", "--start", "10", "--step", "10"],
    )?;
    assert_eq!(out, "2 of 2 pages renumbered\n");
    assert_eq!(
        numbers(&dir)?,
        pairs(&[("a", "1"), ("b", "2"), ("c", "10"), ("d", "20")])
    );
    wiringcraft(&dir, &["page", "renumber", "--prefix", "=A1/"])?;
    assert_eq!(
        numbers(&dir)?,
        pairs(&[("a", "=A1/1"), ("b", "=A1/2"), ("c", "=A1/3"), ("d", "=A1/4")])
    );

    // Numbers that would repeat a kept one or change the order are refused, nothing changed.
    let before = numbers(&dir)?;
    let err = wiringcraft(&dir, &["page", "renumber", "--from", "b", "--prefix", "=A1/"])
        .err()
        .unwrap_or_default();
    assert!(err.contains("both be number \"=A1/1\""), "{err}");
    let err = wiringcraft(&dir, &["page", "renumber", "--from", "c"])
        .err()
        .unwrap_or_default();
    assert!(err.contains("page order"), "{err}");
    let err = wiringcraft(&dir, &["page", "renumber", "--prefix", "A1"])
        .err()
        .unwrap_or_default();
    assert!(err.contains("ends in a digit"), "{err}");
    let err = wiringcraft(&dir, &["page", "renumber", "--from", "nope"])
        .err()
        .unwrap_or_default();
    assert!(!err.is_empty());
    assert_eq!(numbers(&dir)?, before);
    Ok(())
}
