//! A file the command line cannot read or write is named in the error (#554).

use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wiringcraft(cwd: &std::path::Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .current_dir(cwd)
        .env_remove(wc_core::guard::GUARD_ENV)
        .args(args)
        .output()?;
    assert!(!out.status.success());
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn a_pdf_into_a_missing_folder_names_the_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let made = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .current_dir(tmp.path())
        .args(["new", "p", "--no-git"])
        .output()?;
    assert!(made.status.success());
    let err = wiringcraft(tmp.path(), &["-C", "p", "pdf", "no/such/dir/out.pdf"])?;
    assert!(err.contains("cannot write no/such/dir/out.pdf: "), "{err}");
    Ok(())
}

#[test]
fn inspecting_a_missing_file_names_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let err = wiringcraft(tmp.path(), &["inspect", "nofile.zip"])?;
    assert!(err.contains("cannot read nofile.zip: "), "{err}");
    Ok(())
}

#[test]
fn a_pdf_with_nothing_to_print_is_not_written() -> TestResult {
    // A project without pages, exported without reports, has no sheet to print (#833).
    // `page remove` keeps the last page (#847), so the page file is deleted by hand, as a
    // project edited outside WiringCraft could be.
    let tmp = tempfile::tempdir()?;
    let made = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .current_dir(tmp.path())
        .args(["new", "p", "--no-git"])
        .output()?;
    assert!(made.status.success());
    std::fs::remove_file(tmp.path().join("p").join("pages").join("p0001.page.json"))?;
    let err = wiringcraft(tmp.path(), &["-C", "p", "pdf", "out.pdf"])?;
    assert!(err.contains("nothing to export: the project has no pages"), "{err}");
    assert!(!tmp.path().join("p").join("out.pdf").exists());
    assert!(!tmp.path().join("out.pdf").exists());
    Ok(())
}
