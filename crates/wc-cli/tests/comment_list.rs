//! `comment list` escapes control characters read from a comment file (#1343): comments come
//! from other people through git, and an ESC or a tab must not reach the terminal as it is.

use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wc(dir: &std::path::Path, args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
}

#[test]
fn control_characters_in_a_comment_file_are_escaped() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let Some(dir_s) = dir.to_str() else {
        unreachable!("temp path is not UTF-8")
    };
    assert!(wc(tmp.path(), &["new", dir_s, "--no-git"])?.status.success());
    let comments = dir.join("comments");
    std::fs::create_dir_all(&comments)?;
    std::fs::write(
        comments.join("p0001.comments.json"),
        r#"[{"id":"c1","page":"p0001","author":"Ann\tB","time":0,
            "text":"first\u001b[2J\r\nsecond",
            "replies":[{"author":"Bob\nX","time":0,"text":"ok\u001b[31m\nthen"}]}]"#,
    )?;
    let out = wc(&dir, &["comment", "list"])?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout)?;
    assert!(
        !text.contains('\u{1b}') && !text.contains('\t') && !text.contains('\r'),
        "{text:?}"
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "c1 page 1  Ann\\tB  1970-01-01",
            "    first\\u{1b}[2J",
            "    second",
            "    > Bob\\nX (1970-01-01): ok\\u{1b}[31m then",
            "1 open, 0 resolved",
        ],
        "{text}"
    );
    Ok(())
}

/// A CR at the very end of a comment or reply is not part of a CRLF line break, so it is
/// printed escaped instead of being dropped.
#[test]
fn a_trailing_cr_is_escaped_not_dropped() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let Some(dir_s) = dir.to_str() else {
        unreachable!("temp path is not UTF-8")
    };
    assert!(wc(tmp.path(), &["new", dir_s, "--no-git"])?.status.success());
    let comments = dir.join("comments");
    std::fs::create_dir_all(&comments)?;
    std::fs::write(
        comments.join("p0001.comments.json"),
        r#"[{"id":"c1","page":"p0001","author":"Ann","time":0,
            "text":"one\r\nend\r",
            "replies":[{"author":"Bob","time":0,"text":"a\r\nreply\r"}]}]"#,
    )?;
    let out = wc(&dir, &["comment", "list"])?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout)?;
    assert!(!text.contains('\r'), "{text:?}");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "c1 page 1  Ann  1970-01-01",
            "    one",
            "    end\\r",
            "    > Bob (1970-01-01): a reply\\r",
            "1 open, 0 resolved",
        ],
        "{text}"
    );
    Ok(())
}
