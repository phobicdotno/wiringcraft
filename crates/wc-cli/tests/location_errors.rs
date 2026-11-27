//! `location set` with a bad location names it as typed and gives the reason (#1119), not the
//! made-up tag the check parses it in.

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
fn a_bad_location_is_named_as_typed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let Some(dir_s) = dir.to_str() else {
        unreachable!("temp path is not UTF-8")
    };
    assert!(wc(tmp.path(), &["new", dir_s, "--no-git"])?.status.success());
    for (typed, want) in [
        (
            "+A..B",
            "\"+A..B\" is not a location: the location aspect (+) has an empty level",
        ),
        ("+A-K1", "\"+A-K1\" is not a location: give only the + part of a tag"),
    ] {
        let out = wc(&dir, &["location", "set", typed, "x"])?;
        assert!(!out.status.success(), "{typed}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(want), "{typed}: {err}");
        assert!(!err.contains("-X\""), "no made-up tag: {err}");
    }
    Ok(())
}
