//! `wiringcraft page report`: a report page, which `pdf` prints its report in, in its place
//! among the pages (#1897).

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()?)
}

fn ok(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let done = run(dir, args)?;
    if !done.status.success() {
        return Err(String::from_utf8_lossy(&done.stderr).into_owned().into());
    }
    Ok(String::from_utf8_lossy(&done.stdout).into_owned())
}

fn err(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let done = run(dir, args)?;
    if done.status.success() {
        return Err(format!("{args:?} was not refused").into());
    }
    Ok(String::from_utf8_lossy(&done.stderr).into_owned())
}

/// The page count of `pdf` with `args`, as `wrote <file> (N pages)` says it.
fn pages(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = dir.join("out.pdf");
    let mut all = vec!["pdf", out.to_str().ok_or("path")?];
    all.extend_from_slice(args);
    let text = ok(dir, &all)?;
    let count = text.rsplit_once('(').map(|(_, n)| n.trim().to_owned());
    count.ok_or_else(|| format!("no page count in {text:?}").into())
}

#[test]
fn a_report_page_holds_its_report_in_the_pdf() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    let bare = pages(&dir, &[])?;
    let with_parts = pages(&dir, &["--report", "parts"])?;
    ok(
        &dir,
        &["page", "add", "bom", "--number", "50", "--title", "Bill of materials"],
    )?;
    // An unknown report is refused; the name is taken without regard to case.
    assert!(err(&dir, &["page", "report", "bom", "nonsense"])?.contains("unknown report \"nonsense\""));
    ok(&dir, &["page", "report", "bom", " Parts "])?;
    let list = ok(&dir, &["page", "list"])?;
    assert!(
        list.contains("Bill of materials (0 symbols, 0 wires), report parts"),
        "{list}"
    );
    // Nothing is drawn on a report page.
    for args in [
        &["place", "bom", "wc.coil", "--at", "50,50"][..],
        &["wire", "bom", "10,10", "20,10"][..],
    ] {
        let why = err(&dir, args)?;
        assert!(
            why.contains("is a report page") && why.contains("parts report"),
            "{why}"
        );
    }
    // The report page is the parts list's sheet: asked for again it is not printed twice.
    assert_eq!(pages(&dir, &[])?, with_parts);
    assert_eq!(pages(&dir, &["--report", "parts"])?, with_parts);
    // Normal again: one blank page more than before.
    ok(&dir, &["page", "report", "bom", "--off"])?;
    assert!(!ok(&dir, &["page", "list"])?.contains("report"));
    let more: usize = bare.split(' ').next().ok_or("count")?.parse()?;
    assert_eq!(pages(&dir, &[])?, format!("{} pages)", more + 1));

    // A report name written by hand that names no report prints the page as a normal one, with
    // a warning; a closed stderr does not stop the export.
    let file = dir.join("pages/bom.page.json");
    let text = std::fs::read_to_string(&file)?;
    let text = text.replacen("\"title\"", "\"report\": \"nonsense\",\n  \"title\"", 1);
    std::fs::write(&file, text)?;
    let out = dir.join("out.pdf");
    let done = run(&dir, &["pdf", out.to_str().ok_or("path")?])?;
    assert!(done.status.success());
    let said = String::from_utf8_lossy(&done.stderr);
    assert!(said.contains("names no report; it prints as a normal page"), "{said}");
    assert_eq!(
        String::from_utf8_lossy(&done.stdout)
            .rsplit_once('(')
            .map(|(_, n)| n.trim()),
        Some(format!("{} pages)", more + 1).as_str())
    );
    let (reader, writer) = std::io::pipe()?;
    drop(reader);
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["pdf", out.to_str().ok_or("path")?])
        .stdout(std::process::Stdio::null())
        .stderr(writer)
        .status()?;
    assert_eq!(status.code(), Some(0), "{status:?}");
    Ok(())
}
