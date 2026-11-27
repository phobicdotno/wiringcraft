//! `wiringcraft import symbol-svg`, `symbol-dxf`, `symbol-kicad` and `symbol-qet` run as a
//! program on tiny drawings written here (#691): each writes its symbol into `--lib`, with the
//! category and letter code given, says what it wrote, and fails with a message, not a panic,
//! on a missing or broken file.

use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The program run in `cwd` (no project needed: symbol imports only write into `--lib`).
fn run(cwd: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .current_dir(cwd)
        .args(args)
        .output()?)
}

/// Its stdout and stderr; an error with its stderr when it fails.
fn run_ok(cwd: &Path, args: &[&str]) -> Result<(String, String), Box<dyn std::error::Error>> {
    let out = run(cwd, args)?;
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(format!("{}: {err}", out.status).into());
    }
    Ok((String::from_utf8(out.stdout)?, err))
}

/// Its stderr, checking that it failed with a message and did not panic.
fn run_failing(cwd: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = run(cwd, args)?;
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(err.starts_with("error:"), "{err}");
    Ok(err)
}

/// The symbol written to `lib` under `id`, read back.
fn symbol(lib: &Path, id: &str) -> Result<wc_core::Symbol, Box<dyn std::error::Error>> {
    let path = lib.join("symbols").join(format!("{id}.json"));
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// The pins' names, in order.
fn pin_names(s: &wc_core::Symbol) -> Vec<&str> {
    s.pins.iter().map(|p| p.name.as_str()).collect()
}

/// A coil: a straight body line between two named pins (straight lines only).
const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 20">
  <line x1="5" y1="0" x2="5" y2="20" stroke="black"/>
  <polyline points="2,7 8,7 8,13 2,13 2,7" fill="none" stroke="black"/>
  <circle id="pin:A1" cx="5" cy="0" r="0.5"/>
  <circle data-pin="A2" cx="5" cy="20" r="0.5"/>
</svg>"#;

#[test]
fn symbol_svg_writes_the_symbol_with_its_category_and_letter_code() -> TestResult {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("coil.svg"), SVG)?;
    let (out, _) = run_ok(
        tmp.path(),
        &[
            "import",
            "symbol-svg",
            "coil.svg",
            "--id",
            "my.coil",
            "--name",
            "Coil",
            "--lib",
            "lib",
            "--category",
            "relays",
            "--letter-code",
            "K",
        ],
    )?;
    let s = symbol(&tmp.path().join("lib"), "my.coil")?;
    assert_eq!(s.id, "my.coil");
    assert_eq!(s.name, "Coil");
    assert_eq!(s.category, "relays");
    assert_eq!(s.letter_code, "K");
    assert_eq!(pin_names(&s), ["A1", "A2"]);
    assert!(!s.graphics.is_empty());
    assert!(out.starts_with("wrote "), "{out}");
    assert!(out.contains("my.coil.json"), "{out}");
    assert!(out.contains(&format!("(2 pins, {} shapes)", s.graphics.len())), "{out}");
    Ok(())
}

#[test]
fn symbol_svg_fails_with_a_message_on_a_missing_or_broken_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let args = |f: &'static str| ["import", "symbol-svg", f, "--id", "my.x", "--name", "X", "--lib", "lib"];
    let err = run_failing(tmp.path(), &args("missing.svg"))?;
    assert!(err.contains("missing.svg"), "{err}");
    std::fs::write(tmp.path().join("broken.svg"), "<html></html>")?;
    run_failing(tmp.path(), &args("broken.svg"))?;
    assert!(!tmp.path().join("lib").exists());
    Ok(())
}

#[test]
fn symbol_svg_warns_about_a_path_that_stops_at_bad_data() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // The second path's arc is cut short: its line is drawn, the arc is not.
    let svg = SVG.replace("</svg>", r#"<path id="arm" d="M 0 0 L 1 0 A 5 5 0 0 1 10"/></svg>"#);
    std::fs::write(tmp.path().join("arm.svg"), svg)?;
    let (out, err) = run_ok(
        tmp.path(),
        &[
            "import",
            "symbol-svg",
            "arm.svg",
            "--id",
            "my.arm",
            "--name",
            "Arm",
            "--lib",
            "lib",
        ],
    )?;
    assert!(out.starts_with("wrote "), "{out}");
    assert_eq!(pin_names(&symbol(&tmp.path().join("lib"), "my.arm")?), ["A1", "A2"]);
    let want = "warning: path 1 (id \"arm\") is drawn only in part: stopped at command 3 (A): an arc is cut short \
                after argument 6 (an arc takes 7)";
    assert!(err.lines().any(|l| l == want), "{err}");
    Ok(())
}

/// One DXF entity: its type and group code / value pairs.
fn entity(kind: &str, codes: &[(u32, &str)]) -> String {
    let mut s = format!("0\n{kind}\n8\n0\n");
    for (code, value) in codes {
        s.push_str(&format!("{code}\n{value}\n"));
    }
    s
}

/// A contact: a line between two pins (POINT entities), and a 3D face the importer does not read.
fn dxf() -> String {
    let entities = [
        entity("LINE", &[(10, "0"), (20, "0"), (11, "0"), (21, "-10")]),
        entity("POINT", &[(10, "0"), (20, "0")]),
        entity("POINT", &[(10, "0"), (20, "-10")]),
        entity(
            "3DFACE",
            &[
                (10, "0"),
                (20, "0"),
                (30, "0"),
                (11, "1"),
                (21, "0"),
                (31, "0"),
                (12, "1"),
                (22, "1"),
                (32, "0"),
                (13, "0"),
                (23, "1"),
                (33, "0"),
            ],
        ),
    ]
    .concat();
    format!("0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n")
}

#[test]
fn symbol_dxf_writes_the_symbol_and_warns_about_what_it_did_not_read() -> TestResult {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("contact.dxf"), dxf())?;
    let (out, err) = run_ok(
        tmp.path(),
        &[
            "import",
            "symbol-dxf",
            "contact.dxf",
            "--id",
            "my.contact",
            "--name",
            "Contact",
            "--lib",
            "lib",
            "--category",
            "contacts",
            "--letter-code",
            "S",
        ],
    )?;
    let s = symbol(&tmp.path().join("lib"), "my.contact")?;
    assert_eq!(s.name, "Contact");
    assert_eq!(s.category, "contacts");
    assert_eq!(s.letter_code, "S");
    assert_eq!(s.pins.len(), 2);
    assert_eq!(s.graphics.len(), 1, "{:?}", s.graphics);
    assert!(out.starts_with("wrote "), "{out}");
    assert!(out.contains("my.contact.json (2 pins, 1 shapes)"), "{out}");
    assert!(err.contains("warning: 1 3DFACE entity not read"), "{err}");
    Ok(())
}

#[test]
fn symbol_dxf_fails_with_a_message_on_a_missing_or_broken_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let args = |f: &'static str| ["import", "symbol-dxf", f, "--id", "my.x", "--name", "X", "--lib", "lib"];
    let err = run_failing(tmp.path(), &args("missing.dxf"))?;
    assert!(err.contains("missing.dxf"), "{err}");
    std::fs::write(tmp.path().join("broken.dxf"), "not a drawing")?;
    run_failing(tmp.path(), &args("broken.dxf"))?;
    assert!(!tmp.path().join("lib").exists());
    Ok(())
}

#[test]
fn symbol_svg_and_dxf_refuse_an_empty_name() -> TestResult {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("coil.svg"), SVG)?;
    std::fs::write(tmp.path().join("contact.dxf"), dxf())?;
    for (cmd, file) in [("symbol-svg", "coil.svg"), ("symbol-dxf", "contact.dxf")] {
        for name in ["", "   "] {
            let args = ["import", cmd, file, "--id", "my.x", "--name", name, "--lib", "lib"];
            let err = run_failing(tmp.path(), &args)?;
            assert!(err.contains("the symbol name cannot be empty"), "{cmd} {name:?}: {err}");
        }
    }
    assert!(!tmp.path().join("lib").exists());
    Ok(())
}

/// A one-unit lamp with a body line and two pins.
const KICAD: &str = r#"(kicad_symbol_lib (version 20231120) (generator "test")
  (symbol "Lamp" (in_bom yes) (on_board yes)
    (property "Reference" "H" (at 0 0 0))
    (property "Value" "Lamp" (at 0 0 0))
    (symbol "Lamp_0_1"
      (polyline (pts (xy 0 1.27) (xy 0 -1.27)) (stroke (width 0)))
    )
    (symbol "Lamp_1_1"
      (pin passive line (at 0 3.81 270) (length 2.54) (name "~") (number "X1"))
      (pin passive line (at 0 -3.81 90) (length 2.54) (name "~") (number "X2"))
    )
  )
)"#;

#[test]
fn symbol_kicad_writes_each_symbol_with_its_category() -> TestResult {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("lamps.kicad_sym"), KICAD)?;
    let (out, _) = run_ok(
        tmp.path(),
        &[
            "import",
            "symbol-kicad",
            "lamps.kicad_sym",
            "--lib",
            "lib",
            "--category",
            "lamps",
        ],
    )?;
    let s = symbol(&tmp.path().join("lib"), "kicad.lamps.lamp")?;
    assert_eq!(s.name, "Lamp");
    assert_eq!(s.category, "lamps");
    assert_eq!(s.letter_code, "H");
    assert_eq!(pin_names(&s), ["X1", "X2"]);
    assert!(!s.graphics.is_empty());
    assert!(out.contains("kicad.lamps.lamp.json"), "{out}");
    Ok(())
}

#[test]
fn symbol_kicad_fails_with_a_message_on_a_missing_or_broken_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let err = run_failing(
        tmp.path(),
        &["import", "symbol-kicad", "missing.kicad_sym", "--lib", "lib"],
    )?;
    assert!(err.contains("missing.kicad_sym"), "{err}");
    std::fs::write(tmp.path().join("broken.kicad_sym"), "(not_a_library)")?;
    let err = run_failing(
        tmp.path(),
        &["import", "symbol-kicad", "broken.kicad_sym", "--lib", "lib"],
    )?;
    assert!(err.contains("not a KiCad symbol library"), "{err}");
    assert!(!tmp.path().join("lib").exists());
    Ok(())
}

#[test]
fn symbol_kicad_fails_when_no_symbol_could_be_imported() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // The warnings naming each skipped symbol come first, then the error.
    let fails = |args: &[&str]| -> Result<String, Box<dyn std::error::Error>> {
        let out = run(tmp.path(), args)?;
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(1), "{err}");
        assert!(
            err.contains("error: no symbol could be imported from the file"),
            "{err}"
        );
        Ok(err)
    };
    // No symbols at all, and one whose only symbol draws nothing.
    std::fs::write(
        tmp.path().join("empty.kicad_sym"),
        "(kicad_symbol_lib (version 20231120))",
    )?;
    std::fs::write(
        tmp.path().join("blank.kicad_sym"),
        r#"(kicad_symbol_lib (symbol "Blank" (property "Reference" "X" (at 0 0 0))))"#,
    )?;
    for file in ["empty.kicad_sym", "blank.kicad_sym"] {
        fails(&["import", "symbol-kicad", file, "--lib", "lib"])?;
    }
    // An id prefix that is not a valid id skips every symbol.
    std::fs::write(tmp.path().join("lamps.kicad_sym"), KICAD)?;
    let err = fails(&[
        "import",
        "symbol-kicad",
        "lamps.kicad_sym",
        "--lib",
        "lib",
        "--prefix",
        "bad prefix!",
    ])?;
    assert!(err.contains("Lamp:"), "{err}");
    assert!(!tmp.path().join("lib").join("symbols").exists());
    Ok(())
}

/// A normally open contact: a line between two terminals.
const QET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<definition type="element" width="20" height="40" hotspot_x="10" hotspot_y="20" version="0.80">
  <names><name lang="en">NO contact</name></names>
  <description>
    <line x1="0" y1="-20" x2="0" y2="20" style="line-style:normal"/>
    <terminal x="0" y="-20" orientation="n" name="13"/>
    <terminal x="0" y="20" orientation="s" name="14"/>
  </description>
</definition>"#;

#[test]
fn symbol_qet_writes_the_symbol_with_its_category_and_letter_code() -> TestResult {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("no_contact.elmt"), QET)?;
    let (out, _) = run_ok(
        tmp.path(),
        &[
            "import",
            "symbol-qet",
            "no_contact.elmt",
            "--lib",
            "lib",
            "--category",
            "contacts",
            "--letter-code",
            "K",
        ],
    )?;
    let s = symbol(&tmp.path().join("lib"), "qet.no_contact")?;
    assert_eq!(s.name, "NO contact");
    assert_eq!(s.category, "contacts");
    assert_eq!(s.letter_code, "K");
    assert_eq!(pin_names(&s), ["13", "14"]);
    assert_eq!(s.graphics.len(), 1, "{:?}", s.graphics);
    assert!(out.starts_with("NO contact -> "), "{out}");
    assert!(out.contains("qet.no_contact.json (2 pins, 1 shapes)"), "{out}");
    Ok(())
}

#[test]
fn symbol_qet_keeps_the_letter_code_from_the_label_without_the_option() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let labelled = QET.replace(
        "<names>",
        r#"<elementInformations><elementInformation name="label" show="1">-K1</elementInformation></elementInformations><names>"#,
    );
    std::fs::write(tmp.path().join("no_contact.elmt"), labelled)?;
    run_ok(tmp.path(), &["import", "symbol-qet", "no_contact.elmt", "--lib", "lib"])?;
    let s = symbol(&tmp.path().join("lib"), "qet.no_contact")?;
    assert_eq!(s.letter_code, "K");
    Ok(())
}

#[test]
fn symbol_qet_writes_the_good_files_and_fails_naming_the_missing_or_broken_ones() -> TestResult {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("good.elmt"), QET)?;
    std::fs::write(tmp.path().join("broken.elmt"), "<html></html>")?;
    let err = run_failing(
        tmp.path(),
        &[
            "import",
            "symbol-qet",
            "missing.elmt",
            "broken.elmt",
            "good.elmt",
            "--lib",
            "lib",
        ],
    )?;
    assert!(err.contains("missing.elmt"), "{err}");
    assert!(err.contains("broken.elmt"), "{err}");
    assert!(err.contains("2 of 3 element(s) could not be imported"), "{err}");
    symbol(&tmp.path().join("lib"), "qet.good")?;
    Ok(())
}

#[test]
fn symbol_kicad_reads_an_old_lib_library() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let lib = "EESchema-LIBRARY Version 2.4\n#encoding utf-8\n#\n# LAMP\n#\nDEF LAMP H 0 40 N N 1 F N\nF0 \"H\" 100 0 50 H V L CNN\nF1 \"LAMP\" 100 -100 50 H V L CNN\nDRAW\nC 0 0 70 0 1 10 N\nP 2 0 1 0 -50 50 50 -50 N\nX X1 X1 0 200 130 D 50 50 1 1 P\nX X2 X2 0 -200 130 U 50 50 1 1 P\nENDDRAW\nENDDEF\n#\n#End Library\n";
    std::fs::write(tmp.path().join("old-lamps.lib"), lib)?;
    let (out, _) = run_ok(tmp.path(), &["import", "symbol-kicad", "old-lamps.lib", "--lib", "lib"])?;
    let s = symbol(&tmp.path().join("lib"), "kicad.old-lamps.lamp")?;
    assert_eq!(s.letter_code, "H");
    assert_eq!(pin_names(&s), ["X1", "X2"]);
    // 400 mil between the pins: 10.16 mm, the second one below the first.
    let ys: Vec<f64> = s.pins.iter().map(|p| p.at.y).collect();
    assert_eq!(ys, [0.0, 10.16]);
    assert!(s.graphics.iter().any(|g| matches!(g, wc_core::Shape::Circle { .. })));
    assert!(out.contains("kicad.old-lamps.lamp.json"), "{out}");
    Ok(())
}
