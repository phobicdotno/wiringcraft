//! `wiringcraft drawing` writes a file per panel layout (#927). A part that cannot be read is
//! named in a warning with the reason, and the readable parts still draw their footprints. The
//! PDF export warns about it the same way (#1521).

use std::process::Command;

use wc_core::lock::ItemKind;
use wc_core::panel::{Panel, PanelItem};
use wc_core::store;
use wc_core::{DeviceTag, Footprint, Part, Point, Shape};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The file names in `dir`, sorted.
fn files(dir: &std::path::Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// `--page` writes the named pages only: no panel layout files (#1796).
#[test]
fn picking_pages_leaves_the_panel_layouts_out() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
            .arg("-C")
            .arg(&dir)
            .args(args)
            .output()
    };
    let made = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .output()?;
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let added = run(&["page", "add", "p2", "--number", "2"])?;
    assert!(added.status.success(), "{}", String::from_utf8_lossy(&added.stderr));
    let mut project = store::load(&dir)?;
    project.panels.push(Panel::new("main", "Main", 600.0, 800.0));
    project.panels.push(Panel::new("door", "Door", 600.0, 800.0));
    store::save(&dir, &project)?;

    for ext in ["svg", "dxf"] {
        let picked = tmp.path().join(format!("picked-{ext}"));
        let out = run(&[
            "drawing",
            picked.to_str().ok_or("path")?,
            "--format",
            ext,
            "--page",
            "p2",
        ])?;
        assert!(out.status.success(), "{ext}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(files(&picked)?, [format!("2.{ext}")], "{ext}");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(&format!("wrote 1 {ext} file(s)")),
            "{ext}"
        );

        // Without --page every page and every panel layout is written.
        let all = tmp.path().join(format!("all-{ext}"));
        let out = run(&["drawing", all.to_str().ok_or("path")?, "--format", ext])?;
        assert!(out.status.success(), "{ext}: {}", String::from_utf8_lossy(&out.stderr));
        let written = files(&all)?;
        for name in ["2", "panel-Main", "panel-Door"] {
            assert!(written.contains(&format!("{name}.{ext}")), "{ext}: {written:?}");
        }
    }
    Ok(())
}

#[test]
fn an_unreadable_part_is_warned_about_and_the_readable_footprint_is_still_drawn() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let made = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .output()?;
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));

    // Two devices on one plate: one whose part has a footprint (a circle), one whose part
    // file is damaged.
    let mut project = store::load(&dir)?;
    let mut panel = Panel::new("main", "Main", 600.0, 800.0);
    for (i, (tag, part)) in [("+CAB1-A1", "maker.good"), ("+CAB1-A2", "maker.bad")]
        .into_iter()
        .enumerate()
    {
        let tag = DeviceTag::parse(tag)?;
        project.devices.insert(
            tag.clone(),
            wc_core::Device {
                part: Some(part.into()),
                ..Default::default()
            },
        );
        panel.items.push(PanelItem::Device {
            tag,
            at: Point::new(100.0 + 200.0 * i as f64, 100.0),
            width: 50.0,
            height: 50.0,
            rotation: Default::default(),
        });
    }
    project.panels.push(panel);
    store::save(&dir, &project)?;
    let good = Part {
        id: "maker.good".into(),
        footprint: Some(Footprint {
            width: 50.0,
            height: 50.0,
            shapes: vec![Shape::Circle {
                center: Point::new(25.0, 25.0),
                radius: 20.0,
                filled: false,
            }],
        }),
        ..Part::default()
    };
    store::write_vendored_part(&dir, &good)?;
    let bad = store::vendor_path(&dir, ItemKind::Part, "maker.bad")?;
    std::fs::write(&bad, "{ not json")?;

    for ext in ["dxf", "svg"] {
        let out_dir = tmp.path().join(ext);
        let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
            .arg("-C")
            .arg(&dir)
            .arg("drawing")
            .arg(&out_dir)
            .args(["--format", ext])
            .output()?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{ext}: {stderr}");
        // The warning names the part and says why it cannot be read.
        let Some(line) = stderr.lines().find(|l| l.contains("cannot read part maker.bad")) else {
            unreachable!("{ext}: no warning about maker.bad in {stderr}")
        };
        assert!(line.starts_with("warning: ") && line.len() > "warning: cannot read part maker.bad: ".len());
        assert!(!stderr.contains("maker.good"), "{ext}: {stderr}");
        // The readable part's footprint circle (radius 20 mm at 1:5) is drawn on the panel.
        let drawn = std::fs::read_to_string(out_dir.join(format!("panel-Main.{ext}")))?;
        let circle = if ext == "dxf" { "CIRCLE" } else { "<circle " };
        assert_eq!(drawn.matches(circle).count(), 1, "{ext}");
        assert!(
            drawn.contains(if ext == "dxf" { "40\n4\n" } else { "r=\"4\"" }),
            "{ext}"
        );
    }

    // `pdf` names the damaged part once, with the reason, and still writes the PDF.
    let pdf = tmp.path().join("out.pdf");
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .arg("pdf")
        .arg(&pdf)
        .output()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "pdf: {stderr}");
    let warned: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("cannot read part maker.bad"))
        .collect();
    assert_eq!(warned.len(), 1, "pdf: {stderr}");
    assert!(warned[0].starts_with("warning: ") && warned[0].len() > "warning: cannot read part maker.bad: ".len());
    assert!(!stderr.contains("maker.good"), "pdf: {stderr}");
    assert!(std::fs::read(&pdf)?.starts_with(b"%PDF"));
    Ok(())
}
