//! `wiringcraft revision`: revisions, clouds and the revision table, run as a program.

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
fn revisions_clouds_and_the_table_are_kept_in_the_project() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "1"])?;

    assert!(
        wiringcraft(&dir, &["revision", "cloud", "p1", "10", "10", "50", "40"]).is_err(),
        "a cloud needs a revision first"
    );
    wiringcraft(
        &dir,
        &[
            "revision",
            "add",
            "A",
            "--text",
            "First issue",
            "--date",
            "2026-11-01",
            "--by",
            "OP",
        ],
    )?;
    wiringcraft(&dir, &["revision", "add", "B", "--text", "Motor added"])?;
    assert!(
        wiringcraft(&dir, &["revision", "add", "B"]).is_err(),
        "letters are unique"
    );
    wiringcraft(&dir, &["revision", "edit", "B", "--date", "2026-11-18"])?;
    assert_eq!(
        wiringcraft(&dir, &["revision", "cloud", "p1", "50", "40", "10", "10"])?.trim(),
        "0"
    );
    wiringcraft(&dir, &["revision", "table", "p1", "200", "20"])?;

    let list = wiringcraft(&dir, &["revision", "list"])?;
    assert!(list.contains("A\t2026-11-01\tOP\tFirst issue"), "{list}");
    assert!(list.contains("B\t2026-11-18\t\tMotor added"), "{list}");
    assert!(
        list.contains("page p1 cloud 0: revision B at 10.0,10.0 to 50.0,40.0"),
        "{list}"
    );
    assert!(list.contains("page p1 table at 200.0,20.0"), "{list}");

    assert!(
        wiringcraft(&dir, &["revision", "remove", "B"]).is_err(),
        "a cloud still carries B"
    );
    wiringcraft(&dir, &["revision", "uncloud", "p1", "--rev", "B"])?;
    wiringcraft(&dir, &["revision", "remove", "B"])?;
    wiringcraft(&dir, &["revision", "table", "p1", "--off"])?;
    let list = wiringcraft(&dir, &["revision", "list"])?;
    assert!(
        !list.contains("cloud") && !list.contains("table") && !list.contains("Motor"),
        "{list}"
    );
    Ok(())
}

#[test]
fn revision_list_escapes_control_characters_from_the_project_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    // A hand-edited file can hold what the CLI would never write.
    let file = dir.join("project.toml");
    let mut text = std::fs::read_to_string(&file)?;
    text.push_str("\n[[revision]]\nletter = \"X\\u001b[2J\"\ntext = \"two\\nlines\\tand a tab\"\nauthor = \"OP\"\n");
    std::fs::write(&file, text)?;

    // A cloud whose revision a hand edit filled with control characters.
    wiringcraft(&dir, &["page", "add", "p1", "--number", "1"])?;
    wiringcraft(&dir, &["revision", "cloud", "p1", "10", "10", "50", "40"])?;
    let page = dir.join("pages").join("p1.page.json");
    let json = std::fs::read_to_string(&page)?;
    let edited = json.replacen(
        "\"revision\": \"X\\u001b[2J\"",
        "\"revision\": \"Y\\u001b]0;\\u0007\"",
        1,
    );
    assert_ne!(json, edited, "the cloud's revision is in the page file: {json}");
    std::fs::write(&page, edited)?;

    let list = wiringcraft(&dir, &["revision", "list"])?;
    assert!(
        !list.contains('\u{1b}') && !list.contains('\u{7}'),
        "no raw control characters: {list:?}"
    );
    assert!(
        list.contains("page p1 cloud 0: revision Y\\u{1b}]0;\\u{7} at"),
        "{list:?}"
    );
    let line = list.lines().next().ok_or("no revision line")?;
    assert_eq!(line.split('\t').count(), 4, "{line:?}");
    assert!(line.starts_with("X\\u{1b}[2J\t"), "{line:?}");
    assert!(line.ends_with("two\\nlines\\tand a tab"), "{line:?}");
    Ok(())
}

/// `store::load` takes a page's id from its file stem, so on Unix a hand-made file name can
/// carry control characters into the page id the cloud and table lines print.
#[cfg(unix)]
#[test]
fn revision_list_escapes_a_page_id_from_a_hand_made_file_name() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "1"])?;
    wiringcraft(&dir, &["revision", "add", "A"])?;
    wiringcraft(&dir, &["revision", "cloud", "p1", "10", "10", "50", "40"])?;
    wiringcraft(&dir, &["revision", "table", "p1", "200", "20"])?;
    let pages = dir.join("pages");
    let json = std::fs::read_to_string(pages.join("p1.page.json"))?;
    let odd = "p\u{1b}[2J";
    std::fs::write(
        pages.join(format!("{odd}.page.json")),
        json.replacen("\"id\": \"p1\"", "\"id\": \"p\\u001b[2J\"", 1),
    )?;
    std::fs::remove_file(pages.join("p1.page.json"))?;

    let list = wiringcraft(&dir, &["revision", "list"])?;
    assert!(!list.contains('\u{1b}'), "no raw ESC: {list:?}");
    assert!(list.contains("page p\\u{1b}[2J cloud 0: revision A"), "{list:?}");
    assert!(list.contains("page p\\u{1b}[2J table at 200.0,20.0"), "{list:?}");
    Ok(())
}

#[test]
fn uncloud_says_when_no_cloud_has_the_revision_and_takes_the_letter_as_cloud_does() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    wiringcraft(&dir, &["page", "add", "p1", "--number", "1"])?;
    wiringcraft(&dir, &["revision", "add", "A 1"])?;
    // Typed with a no-break space, as `revision cloud` and `revision remove` take it.
    wiringcraft(
        &dir,
        &["revision", "cloud", "p1", "10", "10", "50", "40", "--rev", "A\u{a0}1"],
    )?;

    let Err(err) = wiringcraft(&dir, &["revision", "uncloud", "p1", "--rev", "Z"]) else {
        unreachable!("no cloud carries Z");
    };
    assert!(err.contains("page \"p1\" has no cloud of revision Z"), "{err}");
    let Err(err) = wiringcraft(&dir, &["revision", "uncloud", "p1"]) else {
        unreachable!("neither an index nor --rev");
    };
    assert!(
        err.contains("give a cloud's index or --rev") && !err.contains("not both"),
        "{err}"
    );
    let Err(err) = wiringcraft(&dir, &["revision", "uncloud", "p1", "0", "--rev", "A 1"]) else {
        unreachable!("both an index and --rev");
    };
    assert!(err.contains("not both"), "{err}");
    assert!(wiringcraft(&dir, &["revision", "list"])?.contains("cloud 0"));

    wiringcraft(&dir, &["revision", "uncloud", "p1", "--rev", "A\u{a0}1"])?;
    let list = wiringcraft(&dir, &["revision", "list"])?;
    assert!(!list.contains("cloud"), "{list}");
    Ok(())
}
