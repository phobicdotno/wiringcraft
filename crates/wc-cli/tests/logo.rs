//! `logo show`, `logo set` and `logo remove` end to end: a missing or wrong file is refused and
//! leaves the project as it was, a picture is copied once, and its file outlives `remove`.
//! The picture path is read from the working folder, not the `-C` project folder.

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really a picture";

fn run(cwd: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .current_dir(cwd)
        .env_remove(wc_core::guard::GUARD_ENV)
        .args(args)
        .output()?)
}

fn new_project(tmp: &Path) -> TestResult {
    let made = run(tmp, &["new", "p", "--no-git"])?;
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    Ok(())
}

fn ok_stdout(tmp: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let mut all = vec!["-C", "p"];
    all.extend_from_slice(args);
    let out = run(tmp, &all)?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn err_stderr(tmp: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let mut all = vec!["-C", "p"];
    all.extend_from_slice(args);
    let out = run(tmp, &all)?;
    assert!(!out.status.success());
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn a_new_project_has_no_logo_and_remove_is_a_no_op() -> TestResult {
    let tmp = tempfile::tempdir()?;
    new_project(tmp.path())?;
    assert_eq!(ok_stdout(tmp.path(), &["logo", "show"])?, "no logo\n");
    let before = std::fs::read(tmp.path().join("p").join("project.toml"))?;
    ok_stdout(tmp.path(), &["logo", "remove"])?;
    assert_eq!(std::fs::read(tmp.path().join("p").join("project.toml"))?, before);
    assert_eq!(ok_stdout(tmp.path(), &["logo", "show"])?, "no logo\n");
    Ok(())
}

#[test]
fn setting_a_missing_file_names_it_and_sets_nothing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    new_project(tmp.path())?;
    let err = err_stderr(tmp.path(), &["logo", "set", "nope.png"])?;
    assert!(err.contains("Cannot read nope.png: "), "{err}");
    assert_eq!(ok_stdout(tmp.path(), &["logo", "show"])?, "no logo\n");
    Ok(())
}

#[test]
fn a_file_that_is_not_png_or_jpeg_is_refused_and_not_copied() -> TestResult {
    let tmp = tempfile::tempdir()?;
    new_project(tmp.path())?;
    std::fs::write(tmp.path().join("logo.gif"), b"GIF89a....")?;
    let err = err_stderr(tmp.path(), &["logo", "set", "logo.gif"])?;
    assert!(err.contains("The logo must be a PNG or JPEG picture."), "{err}");
    let pictures = tmp.path().join("p").join("pictures");
    assert!(!pictures.exists() || std::fs::read_dir(&pictures)?.next().is_none());
    assert_eq!(ok_stdout(tmp.path(), &["logo", "show"])?, "no logo\n");
    Ok(())
}

#[test]
fn the_same_picture_is_copied_once_and_another_gets_a_new_name() -> TestResult {
    let tmp = tempfile::tempdir()?;
    new_project(tmp.path())?;
    let p = tmp.path().join("p");
    std::fs::write(tmp.path().join("my logo.png"), PNG)?;
    assert_eq!(
        ok_stdout(tmp.path(), &["logo", "set", "my logo.png"])?,
        "pictures/my_logo.png\n"
    );
    assert_eq!(
        ok_stdout(tmp.path(), &["logo", "set", "my logo.png"])?,
        "pictures/my_logo.png\n"
    );
    assert_eq!(std::fs::read_dir(p.join("pictures"))?.count(), 1);
    assert_eq!(ok_stdout(tmp.path(), &["logo", "show"])?, "pictures/my_logo.png\n");

    // A JPEG under a .png name is copied as .jpg; other bytes under a name in use keep the
    // first file and take the next free name.
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
    std::fs::write(tmp.path().join("my logo.png"), jpeg)?;
    assert_eq!(
        ok_stdout(tmp.path(), &["logo", "set", "my logo.png"])?,
        "pictures/my_logo.jpg\n"
    );
    std::fs::write(p.join("pictures").join("other.png"), b"\x89PNG\r\n\x1a\nother")?;
    std::fs::write(tmp.path().join("other.png"), PNG)?;
    assert_eq!(
        ok_stdout(tmp.path(), &["logo", "set", "other.png"])?,
        "pictures/other-2.png\n"
    );
    assert_eq!(std::fs::read(p.join("pictures").join("other-2.png"))?, PNG);
    Ok(())
}

#[test]
fn remove_takes_the_logo_off_but_keeps_its_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    new_project(tmp.path())?;
    let p = tmp.path().join("p");
    std::fs::write(tmp.path().join("logo.png"), PNG)?;
    ok_stdout(tmp.path(), &["logo", "set", "logo.png"])?;
    ok_stdout(tmp.path(), &["logo", "remove"])?;
    assert_eq!(ok_stdout(tmp.path(), &["logo", "show"])?, "no logo\n");
    assert_eq!(std::fs::read(p.join("pictures").join("logo.png"))?, PNG);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_pictures_folder_linked_out_of_the_project_is_refused() -> TestResult {
    let tmp = tempfile::tempdir()?;
    new_project(tmp.path())?;
    let p = tmp.path().join("p");
    let outside = tmp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::os::unix::fs::symlink(&outside, p.join("pictures"))?;
    std::fs::write(tmp.path().join("logo.png"), PNG)?;
    let err = err_stderr(tmp.path(), &["logo", "set", "logo.png"])?;
    assert!(err.contains("leads out of the project folder"), "{err}");
    assert!(std::fs::read_dir(&outside)?.next().is_none());
    assert_eq!(ok_stdout(tmp.path(), &["logo", "show"])?, "no logo\n");
    Ok(())
}
