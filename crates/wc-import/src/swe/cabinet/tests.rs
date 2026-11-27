//! Tests on the project sample of the parent module with a cabinet layout page added.
#![allow(clippy::unwrap_used)]

use super::super::{SweComponent, SwePage, build, read, tests::sample};
use super::*;
use std::collections::BTreeMap;

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
                PanelItem::Device {
                    tag, at, width, height, ..
                } if tag.to_string().ends_with(end) => Some((*at, *width, *height)),
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
        "1 plate(s) giving the panel size",
        "2 other plate, text",
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

#[test]
fn every_layout_item_is_accounted_for() {
    let sw = read(&with_layout()).unwrap();
    let (built, _, _) = build(&sw, None);
    let tags: BTreeMap<i64, DeviceTag> = built
        .devices
        .keys()
        .filter_map(|t| {
            let c = sw
                .components
                .values()
                .find(|c| t.to_string().ends_with(&format!("-{}", c.name)))?;
            Some((c.id, t.clone()))
        })
        .collect();
    let l = layouts(&sw, &tags, &built.devices, &built.parts);
    assert_eq!(
        l.rails + l.ducts + l.devices + l.plates + l.no_size + l.upright + l.other + l.far_off,
        l.items_in
    );
}

#[test]
fn items_far_from_the_origin_are_left_out_and_counted() {
    let c = with_layout();
    // No plate, so the panel is the box round its items; two of them sit near -1e305 and +1e305.
    c.execute_batch(
        "
DELETE FROM tew_symbol WHERE sym_id = 20;
INSERT INTO tew_symbol VALUES
  (29, 12, NULL, 'Rail 35', -1e305, 600, 0, 1, 1, 500, -7.5),
  (30, 12, NULL, 'Duct 40x60', 1e305, 800, 0, 1, 1, 40, -800);
UPDATE tew_symbol SET sym_posx = 1e305, sym_posendx = NULL, sym_posendy = NULL WHERE sym_id = 24;
",
    )
    .unwrap();
    let sw = read(&c).unwrap();
    let (built, _, report) = build(&sw, None);
    let panel = &built.panels[0];
    assert!(panel.width.is_finite() && panel.height.is_finite());
    // The duct, rail and K1 that are left span x 0 to 550 and y 0 to 800.
    assert_eq!((panel.width, panel.height), (550.0, 800.0));
    assert!(panel.items.iter().all(|i| match i {
        PanelItem::Device { at, .. } | PanelItem::Rail { at, .. } => at.x.abs() <= 1e6 && at.y.abs() <= 1e6,
        PanelItem::Duct { min, max } => min.x.abs() <= 1e6 && max.x.abs() <= 1e6,
    }));
    let items = report.iter().find(|r| r.what == "cabinet layout items").unwrap();
    assert!(
        items.note.contains("3 item(s) reaching more than 1 km"),
        "{}",
        items.note
    );
    // F1 was left out before it was counted as estimated.
    assert!(!items.note.contains("estimated"), "{}", items.note);
}

#[test]
fn tagged_plates_whole_words_and_plate_only_pages() {
    let c = sample();
    c.execute_batch(
        "
INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (103, 'MP1', '', -1), (104, 'L1', '', -1);
INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (14, 3, 9, '20', 7), (15, 4, 9, '21', 7);
INSERT INTO tew_symbol VALUES
  (30, 14, 103, 'Mounting plate', 0, 500, 0, 1, 1, 400, -500),
  (31, 14, 104, 'INDUCTOR_FP', 100, 400, 0, 1, 1, 30, -40),
  (32, 14, NULL, 'Kabelkanal 40', 0, 500, 0, 1, 1, 400, -40),
  (33, 15, NULL, 'Montageplatte', 0, 300, 0, 1, 1, 200, -300);
",
    )
    .unwrap();
    let sw = read(&c).unwrap();
    let (built, _, _) = build(&sw, None);
    let ids: Vec<&str> = built.panels.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["swe-layout-20", "swe-layout-21"]);
    let p = &built.panels[0];
    // The tagged plate gives the size and is not placed as a device.
    assert_eq!((p.width, p.height), (400.0, 500.0));
    // `INDUCTOR` holds `duct` but is a device; `Kabelkanal` is a duct.
    let tags: Vec<String> = p.device_tags().iter().map(ToString::to_string).collect();
    assert_eq!(tags, ["-L1"]);
    assert!(p.items.iter().any(|i| matches!(i, PanelItem::Duct { .. })));
    // A page with only a plate is a bare plate.
    let bare = &built.panels[1];
    assert_eq!((bare.width, bare.height, bare.items.len()), (200.0, 300.0, 0));
}

#[test]
fn words_match_whole_tokens() {
    for (name, want) in [
        ("Rail 35", true),
        ("RAIL35x7.5", true),
        ("din_rails", true),
        ("TRAILER_CONTROL", false),
        ("Tragschiene", true),
    ] {
        assert_eq!(has_word(name, RAIL), want, "{name}");
    }
    for (name, want) in [
        ("Duct 40x60", true),
        ("INDUCTOR", false),
        ("Kabelkanal", true),
        ("conductor", false),
    ] {
        assert_eq!(has_word(name, DUCT), want, "{name}");
    }
}

#[test]
fn a_footprint_without_a_box_takes_its_catalog_part_size() {
    // F1 (no box) names part FUSE-1 by Maker F, which the catalog has with a size; K1 has a box
    // and keeps it although its part has a size too; the test's third device has neither.
    let c = with_layout();
    c.execute_batch(
        "
UPDATE tew_buildofmaterial SET bom_reference = 'FUSE-1', bom_manufacturer = 'Maker F' WHERE bom_id = 2;
UPDATE tew_buildofmaterial SET bom_objectid = 101 WHERE bom_id = 2;
INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (105, 'S1', '', -1);
INSERT INTO tew_symbol VALUES (40, 12, 105, 'FP_SWITCH', 300, 640, 0, 1, 1, NULL, NULL);
",
    )
    .unwrap();
    let mut sw = read(&c).unwrap();
    let part = |maker: &str, number: &str, w: &str, h: &str| Part {
        id: Part::make_id(maker, number),
        manufacturer: maker.into(),
        order_number: number.into(),
        properties: BTreeMap::from([("Width".to_owned(), w.to_owned()), ("Height".to_owned(), h.to_owned())]),
        ..Default::default()
    };
    sw.catalog = vec![
        part("Maker F", "FUSE-1", "9 mm", "60 mm"),
        part("Maker A", "RLY-24", "50", "50"),
    ];
    let (built, _, report) = build(&sw, None);
    let panel = &built.panels[0];
    let device = |end: &str| {
        panel
            .items
            .iter()
            .find_map(|i| match i {
                PanelItem::Device {
                    tag, at, width, height, ..
                } if tag.to_string().ends_with(end) => Some((*at, *width, *height)),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(device("-F1"), (Point::new(200.0, 160.0), 9.0, 60.0));
    assert_eq!(device("-K1"), (Point::new(100.0, 160.0), 18.0, 90.0), "the box wins");
    assert_eq!(
        device("-S1"),
        (Point::new(300.0, 160.0), 18.0, 90.0),
        "neither: default"
    );
    // The parts came along with the devices that use them.
    assert!(built.parts.iter().any(|p| p.id == "maker-f.fuse-1"));
    let items = report.iter().find(|r| r.what == "cabinet layout items").unwrap();
    assert!(
        items
            .note
            .contains("1 device(s) placed with their catalog part's width and height"),
        "{}",
        items.note
    );
    assert!(
        items.note.contains("1 device(s) placed with an estimated"),
        "{}",
        items.note
    );
}

#[test]
fn a_footprint_without_a_box_takes_the_size_of_its_block_drawing() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    // F1 (no box) has a 45 x 80 mm block drawing below and right of its insertion point and a
    // catalog part with a size too: the drawing wins. S1 (no box) is drawn at half its width.
    // K1 has a box and a drawing: the box wins.
    let c = with_layout();
    c.execute_batch(
        "
UPDATE tew_buildofmaterial SET bom_reference = 'FUSE-1', bom_manufacturer = 'Maker F' WHERE bom_id = 2;
UPDATE tew_buildofmaterial SET bom_objectid = 101 WHERE bom_id = 2;
INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (105, 'S1', '', -1);
INSERT INTO tew_symbol VALUES (40, 12, 105, 'FP_SWITCH', 300, 640, 0, 0.5, 1, NULL, NULL);
",
    )
    .unwrap();
    let mut sw = read(&c).unwrap();
    sw.catalog = vec![Part {
        id: Part::make_id("Maker F", "FUSE-1"),
        manufacturer: "Maker F".into(),
        order_number: "FUSE-1".into(),
        properties: BTreeMap::from([
            ("Width".to_owned(), "9".to_owned()),
            ("Height".to_owned(), "60".to_owned()),
        ]),
        ..Default::default()
    }];
    let outline = |w: f64, h: f64| {
        model_space_dwg(&[
            E::Line((0.0, 0.0), (w, 0.0)),
            E::Line((w, 0.0), (w, -h)),
            E::Line((w, -h), (0.0, -h)),
            E::Line((0.0, -h), (0.0, 0.0)),
        ])
    };
    sw.drawings.add_block("FP_FUSE", &outline(45.0, 80.0)).unwrap();
    // Drawn left of and above its insertion point: the footprint sits where the drawing is.
    sw.drawings
        .add_block(
            "fp_switch",
            &model_space_dwg(&[E::Circle((-10.0, 10.0), 5.0), E::Line((-30.0, 0.0), (10.0, 40.0))]),
        )
        .unwrap();
    sw.drawings.add_block("FP_RLY", &outline(100.0, 100.0)).unwrap();
    let (built, _, report) = build(&sw, None);
    let panel = &built.panels[0];
    let device = |end: &str| {
        panel
            .items
            .iter()
            .find_map(|i| match i {
                PanelItem::Device {
                    tag, at, width, height, ..
                } if tag.to_string().ends_with(end) => Some((*at, *width, *height)),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(device("-F1"), (Point::new(200.0, 160.0), 45.0, 80.0));
    // x from -30 to 10 at half width: -15 to 5; y from 0 to 40 above the insertion point.
    assert_eq!(device("-S1"), (Point::new(285.0, 120.0), 20.0, 40.0));
    assert_eq!(device("-K1"), (Point::new(100.0, 160.0), 18.0, 90.0), "the box wins");
    let items = report.iter().find(|r| r.what == "cabinet layout items").unwrap();
    assert!(
        items
            .note
            .contains("2 device(s) placed with the size of their block's drawing"),
        "{}",
        items.note
    );
    assert!(!items.note.contains("catalog part"), "{}", items.note);
    assert!(!items.note.contains("estimated"), "{}", items.note);
}

#[test]
fn a_footprint_drawing_sizes_an_arc_by_its_sweep_and_a_cut_off_drawing_sizes_nothing() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    let c = with_layout();
    c.execute_batch(
        "
UPDATE tew_buildofmaterial SET bom_reference = 'FUSE-1', bom_manufacturer = 'Maker F' WHERE bom_id = 2;
UPDATE tew_buildofmaterial SET bom_objectid = 101 WHERE bom_id = 2;
",
    )
    .unwrap();
    let mut sw = read(&c).unwrap();
    sw.catalog = vec![Part {
        id: Part::make_id("Maker F", "FUSE-1"),
        manufacturer: "Maker F".into(),
        order_number: "FUSE-1".into(),
        properties: BTreeMap::from([
            ("Width".to_owned(), "9".to_owned()),
            ("Height".to_owned(), "60".to_owned()),
        ]),
        ..Default::default()
    }];
    let size = |sw: &SweProject| {
        let (built, _, _) = build(sw, None);
        built.panels[0]
            .items
            .iter()
            .find_map(|i| match i {
                PanelItem::Device { tag, width, height, .. } if tag.to_string().ends_with("-F1") => {
                    Some((*width, *height))
                }
                _ => None,
            })
            .unwrap()
    };
    // A quarter arc of radius 20 spans 20 x 20, not its whole circle.
    let mut arc = sw.clone();
    arc.drawings
        .add_block("FP_FUSE", &model_space_dwg(&[E::Arc((0.0, 0.0), 20.0, 0.0, 90.0)]))
        .unwrap();
    assert_eq!(size(&arc), (20.0, 20.0));
    // A drawing cut off by the limit would give a box round part of it: the part's size is used.
    sw.drawings
        .add_block(
            "FP_FUSE",
            &model_space_dwg(&[
                E::Line((0.0, 0.0), (10.0, 0.0)),
                E::Line((10.0, 0.0), (10.0, -10.0)),
                E::Line((0.0, 0.0), (45.0, -80.0)),
            ]),
        )
        .unwrap();
    sw.drawings.set_draw_limit(2);
    assert_eq!(size(&sw), (9.0, 60.0));
    // With just enough for the whole drawing, it gives the size.
    sw.drawings.set_draw_limit(3);
    assert_eq!(size(&sw), (45.0, 80.0));
}

/// A symbol on `page` with insertion point `at` and an optional box corner, unturned.
fn sym(
    id: i64,
    page: i64,
    component: Option<i64>,
    block: &str,
    at: (f64, f64),
    corner: Option<(f64, f64)>,
) -> SweSymbol {
    SweSymbol {
        id,
        page,
        component,
        block: block.into(),
        at: Point::new(at.0, at.1),
        corner: corner.map(|(x, y)| Point::new(x, y)),
        pins: Vec::new(),
        angle: 0.0,
        scale: (1.0, 1.0),
    }
}

fn layout_page(id: i64, label: &str, title: &str, folder: &str) -> SwePage {
    SwePage {
        id,
        kind: KIND_CABINET,
        label: label.into(),
        title: title.into(),
        folder: folder.into(),
        ..Default::default()
    }
}

fn no_devices() -> (BTreeMap<i64, DeviceTag>, BTreeMap<DeviceTag, Device>) {
    (BTreeMap::new(), BTreeMap::new())
}

#[test]
fn panel_ids_stay_unique_and_names_fall_back_to_title_or_label() {
    let plate = |id, page| sym(id, page, None, "Mounting plate", (0.0, 100.0), Some((50.0, 0.0)));
    let mut other_kind = layout_page(5, "X", "Schematic", "");
    other_kind.kind = 1;
    let p = SweProject {
        pages: vec![
            layout_page(1, "", "Door", ""),
            layout_page(2, "", "Door", "Main"),
            layout_page(3, "7", "", "Main"),
            layout_page(4, "Door", "", ""),
            other_kind,
        ],
        symbols: (1..=5).map(|page| plate(page, page)).collect(),
        ..Default::default()
    };
    let (tags, devices) = no_devices();
    let l = layouts(&p, &tags, &devices, &[]);
    let got: Vec<(&str, &str)> = l.panels.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
    // An empty label takes the title for the id; a taken id gets a number; the other page kind
    // makes no panel.
    assert_eq!(
        got,
        [
            ("swe-layout-door", "Door"),
            ("swe-layout-door-2", "Main: Door"),
            ("swe-layout-7", "Cabinet layout 7"),
            ("swe-layout-door-3", "Cabinet layout Door"),
        ]
    );
    assert_eq!((l.items_in, l.plates), (4, 4));
}

#[test]
fn a_second_plate_is_other_and_a_far_off_plate_does_not_size_the_panel() {
    let p = SweProject {
        pages: vec![layout_page(1, "1", "A", ""), layout_page(2, "2", "B", "")],
        symbols: vec![
            sym(1, 1, None, "Backplate", (0.0, 2e6), Some((100.0, 0.0))),
            sym(2, 1, None, "Mounting plate", (0.0, 300.0), Some((200.0, 0.0))),
            sym(3, 1, None, "Platine", (0.0, 400.0), Some((400.0, 0.0))),
            sym(4, 2, None, "Frame", (0.0, 0.0), None),
            sym(5, 2, None, "Text", (0.0, 0.0), None),
        ],
        ..Default::default()
    };
    let (tags, devices) = no_devices();
    let l = layouts(&p, &tags, &devices, &[]);
    assert_eq!(l.panels.len(), 1);
    // The far-off plate is left out, the next one gives the size, the third is other.
    let panel = &l.panels[0];
    assert_eq!((panel.width, panel.height), (200.0, 300.0));
    assert_eq!(
        (l.far_off, l.plates, l.other, l.empty_pages, l.items_in),
        (1, 1, 3, 1, 5)
    );
}

#[test]
fn a_device_box_without_width_falls_back_and_untagged_items_are_other() {
    let k1 = DeviceTag::parse("-K1").unwrap();
    let k2 = DeviceTag::parse("-K2").unwrap();
    let tags = BTreeMap::from([(1, k1.clone()), (2, k2.clone()), (3, DeviceTag::parse("-K3").unwrap())]);
    // K3 has a tag but did not become a device.
    let devices = BTreeMap::from([
        (k1.clone(), Device::default()),
        (
            k2.clone(),
            Device {
                part: Some("p-1".into()),
                ..Default::default()
            },
        ),
    ]);
    let parts = [Part {
        id: "p-1".into(),
        properties: BTreeMap::from([
            ("Width".to_owned(), "22.5".to_owned()),
            ("Height".to_owned(), "45".to_owned()),
        ]),
        ..Default::default()
    }];
    let p = SweProject {
        pages: vec![layout_page(1, "1", "A", "")],
        symbols: vec![
            sym(1, 1, None, "Mounting plate", (0.0, 500.0), Some((300.0, 0.0))),
            // A box with no width: the default footprint at the insertion point.
            sym(2, 1, Some(1), "FP", (100.0, 400.0), Some((100.0, 300.0))),
            // A box with no height: the part's size.
            sym(3, 1, Some(2), "FP", (200.0, 400.0), Some((240.0, 400.0))),
            sym(4, 1, Some(3), "FP", (10.0, 10.0), Some((20.0, 0.0))),
        ],
        ..Default::default()
    };
    let l = layouts(&p, &tags, &devices, &parts);
    let panel = &l.panels[0];
    assert!(panel.items.contains(&PanelItem::Device {
        tag: k1,
        at: Point::new(100.0, 100.0),
        width: 18.0,
        height: 90.0,
        rotation: wc_core::geom::Rotation::R0,
    }));
    assert!(panel.items.contains(&PanelItem::Device {
        tag: k2,
        at: Point::new(200.0, 100.0),
        width: 22.5,
        height: 45.0,
        rotation: wc_core::geom::Rotation::R0,
    }));
    assert_eq!(panel.items.len(), 2);
    assert_eq!((l.devices, l.estimated, l.from_part, l.other), (2, 1, 1, 1));
}

#[test]
fn untagged_components_are_sorted_by_their_description_and_part_number() {
    let component = |id: i64, description: &str, part_number: &str| SweComponent {
        id,
        description: description.into(),
        part_number: part_number.into(),
        ..Default::default()
    };
    let p = SweProject {
        pages: vec![layout_page(1, "1", "A", "")],
        components: BTreeMap::from([
            (1, component(1, "DIN rail 35 mm", "")),
            (2, component(2, "", "WIREWAY-40x60")),
            (3, component(3, "Mounting plate", "")),
            (4, component(4, "Inductor", "")),
        ]),
        symbols: vec![
            sym(1, 1, Some(1), "BLK1", (0.0, 100.0), Some((200.0, 92.5))),
            sym(2, 1, Some(2), "BLK2", (0.0, 300.0), Some((40.0, 0.0))),
            sym(3, 1, Some(3), "BLK3", (0.0, 300.0), Some((250.0, 0.0))),
            sym(4, 1, Some(4), "BLK4", (100.0, 200.0), Some((130.0, 160.0))),
        ],
        ..Default::default()
    };
    let (tags, devices) = no_devices();
    let l = layouts(&p, &tags, &devices, &[]);
    assert_eq!((l.rails, l.ducts, l.plates, l.other), (1, 1, 1, 1));
    let panel = &l.panels[0];
    assert_eq!((panel.width, panel.height), (250.0, 300.0));
    assert!(panel.items.contains(&PanelItem::Rail {
        at: Point::new(0.0, 203.75),
        length: 200.0,
    }));
}

#[test]
fn boxes_drawn_from_any_corner_are_normalised_and_a_square_rail_lies_flat() {
    let p = SweProject {
        pages: vec![layout_page(1, "1", "A", "")],
        symbols: vec![
            // Plate and duct with the corner given below-left of the insertion point.
            sym(1, 1, None, "Mounting plate", (400.0, 600.0), Some((0.0, 0.0))),
            sym(2, 1, None, "Duct 40x60", (440.0, 600.0), Some((400.0, 0.0))),
            // As wide as high: not upright.
            sym(3, 1, None, "Rail 35", (100.0, 300.0), Some((110.0, 290.0))),
        ],
        ..Default::default()
    };
    let (tags, devices) = no_devices();
    let l = layouts(&p, &tags, &devices, &[]);
    let panel = &l.panels[0];
    assert_eq!((panel.width, panel.height), (400.0, 600.0));
    // The duct reaches past the plate's right edge, still placed from the plate's corner.
    assert!(panel.items.contains(&PanelItem::Duct {
        min: Point::new(400.0, 0.0),
        max: Point::new(440.0, 600.0),
    }));
    assert!(panel.items.contains(&PanelItem::Rail {
        at: Point::new(100.0, 305.0),
        length: 10.0,
    }));
    assert_eq!((l.rails, l.upright, l.ducts), (1, 0, 1));
}

#[test]
fn plate_words_match_whole_tokens_and_german_endings() {
    for (name, want) in [
        ("Mounting plate", true),
        ("BACKPLATE", true),
        ("plates", true),
        ("Platine 600x800", true),
        ("Montageplatte", true),
        ("plateau", false),
        ("Template", false),
    ] {
        assert_eq!(has_word(name, PLATE), want, "{name}");
    }
    assert!(has_word("Verdrahtungskanäle", DUCT));
    assert!(!has_word("", RAIL));
}
