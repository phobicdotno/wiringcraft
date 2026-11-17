//! Tests on the project sample of the parent module with a cabinet layout page added.
#![allow(clippy::unwrap_used)]

use super::super::{build, read, tests::sample};
use super::*;

/// Page `10` is a cabinet layout: an 600 x 800 mm plate, a duct up the left edge, a 500 mm rail,
/// K1 with a footprint box, F1 without one, an upright rail, a rail without a size and a text.
/// Page `11` is a layout page with only a frame.
fn with_layout() -> rusqlite::Connection {
    let c = sample();
    c.execute_batch(
        "
INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (12, 3, 9, '10', 7), (13, 4, 9, '11', 7);
INSERT INTO tew_translatedtext VALUES (12, -1, 'en', 'Cabinet');
INSERT INTO tew_symbol VALUES
  (20, 12, NULL, 'Mounting plate', 0, 800, 0, 1, 1, 600, -800),
  (21, 12, NULL, 'Rail 35', 50, 600, 0, 1, 1, 500, -7.5),
  (22, 12, NULL, 'Duct 40x60', 0, 800, 0, 1, 1, 40, -800),
  (23, 12, 100, 'FP_RLY', 100, 640, 0, 1, 1, 18, -90),
  (24, 12, 101, 'FP_FUSE', 200, 640, 0, 1, 1, NULL, NULL),
  (25, 12, NULL, 'Rail 35', 580, 700, 0, 1, 1, 7.5, -500),
  (26, 12, NULL, 'Rail 35', 300, 300, 0, 1, 1, NULL, NULL),
  (27, 12, NULL, 'Text', 10, 10, 0, 1, 1, NULL, NULL),
  (28, 13, NULL, 'Frame', 0, 0, 0, 1, 1, NULL, NULL);
",
    )
    .unwrap();
    c
}

#[test]
fn a_cabinet_layout_page_becomes_a_panel() {
    let sw = read(&with_layout()).unwrap();
    let (built, _, report) = build(&sw, None);
    assert_eq!(built.panels.len(), 1);
    let panel = &built.panels[0];
    assert_eq!(panel.id, "swe-layout-10");
    assert_eq!(panel.name, "Power: Cabinet");
    assert_eq!((panel.width, panel.height), (600.0, 800.0));
    assert!(panel.items.contains(&PanelItem::Duct {
        min: Point::new(0.0, 0.0),
        max: Point::new(40.0, 800.0),
    }));
    assert!(panel.items.contains(&PanelItem::Rail {
        at: Point::new(50.0, 203.75),
        length: 500.0,
    }));
    let device = |end: &str| {
        panel
            .items
            .iter()
            .find_map(|i| match i {
                PanelItem::Device { tag, at, width, height } if tag.to_string().ends_with(end) => {
                    Some((*at, *width, *height))
                }
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(device("-K1"), (Point::new(100.0, 160.0), 18.0, 90.0));
    // No box: the default footprint at the insertion point.
    assert_eq!(device("-F1"), (Point::new(200.0, 160.0), 18.0, 90.0));
    assert_eq!(panel.items.len(), 4);
    // Every placed device is an imported device.
    for tag in panel.device_tags() {
        assert!(built.devices.contains_key(&tag), "{tag}");
    }

    let items = report.iter().find(|r| r.what == "cabinet layout items").unwrap();
    assert_eq!((items.source, items.imported), (9, 4));
    for why in [
        "1 device(s) placed with an estimated",
        "1 rail(s) or duct(s) without a length",
        "1 upright rail(s)",
        "2 plate, text",
    ] {
        assert!(items.note.contains(why), "{why}: {}", items.note);
    }
    let pages = report.iter().find(|r| r.what == "pages").unwrap();
    assert!(
        pages
            .note
            .contains("1 cabinet layout page(s), imported as panel layouts"),
        "{}",
        pages.note
    );
}

#[test]
fn layouts_without_a_plate_take_the_box_round_their_items() {
    let c = with_layout();
    c.execute_batch("DELETE FROM tew_symbol WHERE sym_id IN (20, 22);")
        .unwrap();
    let sw = read(&c).unwrap();
    let (built, _, _) = build(&sw, None);
    let panel = &built.panels[0];
    // From the rail's left end (x 50) and K1's top (y 640) to the rail's right end (x 550) and its
    // bottom edge (y 550).
    assert_eq!((panel.width, panel.height), (500.0, 90.0));
    assert!(panel.items.contains(&PanelItem::Rail {
        at: Point::new(0.0, 43.75),
        length: 500.0,
    }));
}

#[test]
fn a_project_without_layout_pages_has_no_panels_and_no_layout_line() {
    let sw = read(&sample()).unwrap();
    let (built, _, report) = build(&sw, None);
    assert!(built.panels.is_empty());
    assert!(!report.iter().any(|r| r.what == "cabinet layout items"));
}
