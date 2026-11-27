//! An export named after one of the project's own files is refused, and the project stays as
//! it was; an export elsewhere in the project folder is still written (#1252).

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Files of the project by their path in it, with their bytes.
type Files = Vec<(String, Vec<u8>)>;

fn wiringcraft(cwd: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .current_dir(cwd)
        .env_remove(wc_core::guard::GUARD_ENV)
        .args(args)
        .output()
}

/// A new project `p` in `tmp`, and the text of its files that the exports below aim at.
fn project(tmp: &Path) -> Result<Files, Box<dyn std::error::Error>> {
    assert!(wiringcraft(tmp, &["new", "p", "--no-git"])?.status.success());
    let files = ["project.toml", "devices.json", "pages/p0001.page.json"];
    files
        .iter()
        .map(|f| Ok((f.to_string(), std::fs::read(tmp.join("p").join(f))?)))
        .collect()
}

#[test]
fn an_export_onto_a_project_file_is_refused() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let before = project(tmp.path())?;
    let dir = tmp.path().join("p");
    for args in [
        &["pdf", "project.toml"][..],
        &["report", "parts", "--format", "csv", "--out", "devices.json"],
        &["aml", "pages/p0001.page.json"],
        &["plc-vars", "./project.toml"],
    ] {
        let out = wiringcraft(&dir, args)?;
        assert!(!out.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("it is the project's"), "{args:?}: {err}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
    // From outside the project too, with `-C`.
    let out = wiringcraft(tmp.path(), &["-C", "p", "pdf", "p/project.toml"])?;
    assert!(!out.status.success());
    for (f, bytes) in &before {
        assert_eq!(&std::fs::read(dir.join(f))?, bytes, "{f} changed");
    }
    assert!(wiringcraft(&dir, &["check"])?.status.success());
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_export_through_a_link_to_a_project_file_is_refused() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let before = project(tmp.path())?;
    std::os::unix::fs::symlink(tmp.path().join("p/project.toml"), tmp.path().join("out.pdf"))?;
    let out = wiringcraft(tmp.path(), &["-C", "p", "pdf", "out.pdf"])?;
    assert!(!out.status.success());
    assert_eq!(std::fs::read(tmp.path().join("p/project.toml"))?, before[0].1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_export_through_a_link_to_a_missing_project_file_is_refused() -> TestResult {
    let tmp = tempfile::tempdir()?;
    project(tmp.path())?;
    let target = tmp.path().join("p/pages/new.page.json");
    std::os::unix::fs::symlink(&target, tmp.path().join("out.pdf"))?;
    let out = wiringcraft(tmp.path(), &["-C", "p", "pdf", "out.pdf"])?;
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("it is the project's"));
    assert!(!target.exists(), "an export was written into the pages folder");
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_export_through_a_long_chain_of_links_to_a_missing_project_file_is_refused() -> TestResult {
    let tmp = tempfile::tempdir()?;
    project(tmp.path())?;
    let target = tmp.path().join("p/pages/new.page.json");
    // As many links as one look-up follows: out0 -> out1 -> ... -> out39 -> the missing page.
    for i in 0..40 {
        let next = if i == 39 {
            target.clone()
        } else {
            tmp.path().join(format!("out{}", i + 1))
        };
        std::os::unix::fs::symlink(next, tmp.path().join(format!("out{i}")))?;
    }
    let out = wiringcraft(tmp.path(), &["-C", "p", "pdf", "out0"])?;
    assert!(!out.status.success());
    assert!(!target.exists(), "an export was written into the pages folder");
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_export_onto_a_project_file_that_links_out_of_its_folder_is_refused() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let before = project(tmp.path())?;
    let page = tmp.path().join("p/pages/p0001.page.json");
    let outside = tmp.path().join("page-elsewhere.json");
    std::fs::rename(&page, &outside)?;
    std::os::unix::fs::symlink(&outside, &page)?;
    let out = wiringcraft(&tmp.path().join("p"), &["aml", "pages/p0001.page.json"])?;
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("it is the project's"));
    assert_eq!(std::fs::read(&outside)?, before[2].1);
    Ok(())
}

#[test]
fn an_export_beside_the_project_files_is_written() -> TestResult {
    let tmp = tempfile::tempdir()?;
    project(tmp.path())?;
    let dir = tmp.path().join("p");
    // A name that only starts like a project folder is not one.
    for name in ["out.pdf", "pages.pdf"] {
        let out = wiringcraft(&dir, &["pdf", name])?;
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(std::fs::read(dir.join(name))?.starts_with(b"%PDF"));
        // Again, over the export written before.
        assert!(wiringcraft(&dir, &["pdf", name])?.status.success());
    }
    Ok(())
}
