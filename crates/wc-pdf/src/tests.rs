use super::*;
use wc_core::geom::Point;
use wc_core::symbol::{Pin, PinDir, SymbolRole};
use wc_core::{DeviceTag, Endpoint, Page, PlacedSymbol, Shape, Wire};

fn sym(id: &str, role: SymbolRole, pins: [&str; 2]) -> Symbol {
    Symbol {
        id: id.into(),
        name: id.into(),
        category: String::new(),
        letter_code: "K".into(),
        role,
        graphics: vec![Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(0.0, 15.0),
        }],
        pins: vec![
            Pin {
                name: pins[0].into(),
                at: Point::new(0.0, 0.0),
                dir: PinDir::Up,
            },
            Pin {
                name: pins[1].into(),
                at: Point::new(0.0, 15.0),
                dir: PinDir::Down,
            },
        ],
        tag_at: Point::new(-5.0, 8.0),
        description: String::new(),
    }
}

fn place(id: &str, symbol: &str, x: f64, tag: &str) -> PlacedSymbol {
    PlacedSymbol {
        id: id.into(),
        symbol: symbol.into(),
        at: Point::new(x, 60.0),
        rotation: Default::default(),
        mirror: false,
        tag: DeviceTag::parse(tag).ok(),
    }
}

#[test]
fn exports_pages_bookmarks_and_links() {
    let mut project = Project::new("Pump station Ålesund");
    let mut p1 = Page::new("p1", "1", "Control");
    p1.symbols.push(place("s1", "coil", 50.0, "-K1"));
    p1.symbols.push(place("s2", "missing", 90.0, "-X9"));
    p1.wires.push(Wire {
        id: "w1".into(),
        from: Endpoint::Pin {
            item: "s1".into(),
            pin: "A1".into(),
        },
        to: Endpoint::Point {
            at: Point::new(50.0, 30.0),
        },
        via: vec![],
        number: Some("101".into()),
        potential: Some("24V".into()),
        cable: None,
        route: Default::default(),
        spec: Default::default(),
    });
    let mut p2 = Page::new("p2", "2", "Motor");
    p2.symbols.push(place("s1", "no", 200.0, "-K1"));
    project.pages = vec![p1, p2];
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_string(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    symbols.insert("no".to_string(), sym("no", SymbolRole::Auxiliary, ["13", "14"]));

    let (pdf, report) = export(&project, &symbols, &Options::default());
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.starts_with("%PDF-1.7"));
    assert!(text.trim_end().ends_with("%%EOF"));
    assert_eq!(text.matches("/Type /Page ").count(), 2);
    assert_eq!(
        text.matches("/Subtype /Link").count(),
        2,
        "coil -> contact and contact -> coil"
    );
    assert!(text.contains("/Outlines"));
    assert_eq!(report.pages, 2);
    assert!(
        report.warnings.iter().any(|w| w.contains("missing")),
        "{:?}",
        report.warnings
    );

    // xref offsets point at the objects (work on bytes: the header has binary marker bytes).
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).rposition(|w| w == needle);
    let sx = find(&pdf, b"startxref\n").unwrap_or(0) + 10;
    let start: usize = std::str::from_utf8(&pdf[sx..])
        .ok()
        .and_then(|s| s.lines().next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert!(pdf[start..].starts_with(b"xref"));
    let table = std::str::from_utf8(&pdf[start..]).unwrap_or("");
    let first_obj = table
        .lines()
        .nth(3)
        .and_then(|l| l.split(' ').next())
        .and_then(|o| o.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(pdf[first_obj..].starts_with(b"1 0 obj"));
}

#[test]
fn selected_pages_only_and_dangling_links_dropped() {
    let mut project = Project::new("x");
    let mut p1 = Page::new("p1", "1", "A");
    p1.symbols.push(place("s1", "coil", 50.0, "-K1"));
    let mut p2 = Page::new("p2", "2", "B");
    p2.symbols.push(place("s1", "no", 50.0, "-K1"));
    project.pages = vec![p1, p2];
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_string(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    symbols.insert("no".to_string(), sym("no", SymbolRole::Auxiliary, ["13", "14"]));
    let opts = Options {
        pages: vec!["p1".into()],
        ..Default::default()
    };
    let (pdf, _) = export(&project, &symbols, &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(text.matches("/Type /Page ").count(), 1);
    assert_eq!(text.matches("/Subtype /Link").count(), 0);
}

#[test]
fn columns() {
    assert_eq!(column_of(wc_core::PageSize::A3, 0.0), 1);
    assert_eq!(column_of(wc_core::PageSize::A3, 215.0), 6);
    assert_eq!(column_of(wc_core::PageSize::A3, 1e9), 10);
}

#[test]
fn turned_text_gets_a_text_matrix() {
    let mut pen = crate::writer::Pen::new(297.0);
    pen.text_at_angle((100.0, 100.0), 2.5, 0, 0.0, "flat");
    assert!(pen.ops.contains(" Td ") && !pen.ops.contains(" Tm "));
    pen.text_at_angle((100.0, 100.0), 2.5, 0, 90.0, "up");
    // Reading upward: the matrix is a quarter turn counter-clockwise.
    assert!(pen.ops.contains("0.00000 1.00000 -1.00000 0.00000"), "{}", pen.ops);
    // Centred text moves its start back along the reading direction (down the page for 90).
    let mut c = crate::writer::Pen::new(297.0);
    c.text_at_angle((100.0, 100.0), 2.5, 1, 90.0, "centred");
    let mut l = crate::writer::Pen::new(297.0);
    l.text_at_angle((100.0, 100.0), 2.5, 0, 90.0, "centred");
    assert_ne!(c.ops, l.ops);
    let mut n = crate::writer::Pen::new(297.0);
    n.text_at_angle((1.0, 1.0), 2.5, 0, f64::NAN, "nan");
    assert!(n.ops.contains(" Td "));
}

#[test]
fn pages_export_as_svg_and_dxf() {
    let mut project = Project::new("Pump & <station>");
    let mut p1 = Page::new("p1", "1", "Control");
    p1.symbols.push(place("s1", "coil", 50.0, "-K1"));
    p1.graphics.push(Shape::Circle {
        center: Point::new(200.0, 100.0),
        radius: 5.0,
        filled: false,
    });
    p1.graphics.push(Shape::Text {
        at: Point::new(120.0, 80.0),
        text: "Ålesund".into(),
        height: 3.0,
        align: Default::default(),
        angle: 90.0,
    });
    let p2 = Page::new("p2", "2", "Motor");
    project.pages = vec![p1, p2];
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_string(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    let options = Options {
        pages: vec!["p1".into()],
        ..Default::default()
    };
    let (pages, report) = page_drawings(&project, &symbols, &options);
    assert_eq!((pages.len(), report.pages, pages[0].id.as_str()), (1, 1, "p1"));
    let (w, h) = pages[0].size_mm;

    let s = svg(&pages[0]);
    assert!(s.starts_with("<?xml") && s.trim_end().ends_with("</svg>"));
    assert!(
        s.contains(&format!("viewBox=\"0 0 {w} {h}\"")),
        "one unit per millimetre"
    );
    assert!(s.contains("<circle cx=\"200\" cy=\"100\" r=\"5\""));
    assert!(s.contains(">-K1</text>"));
    assert!(s.contains("rotate(-90 120 80)") && s.contains(">Ålesund</text>"));
    // Project name in the title block is escaped.
    assert!(s.contains("Pump &amp; &lt;station&gt;") && !s.contains("<station>"));

    let d = dxf(&pages[0]);
    let pairs: Vec<&str> = d.lines().collect();
    assert_eq!(pairs.len() % 2, 0, "code/value pairs");
    assert!(d.starts_with("0\nSECTION\n2\nHEADER\n") && d.ends_with("0\nEOF\n"));
    // y flips: the circle at 100 mm from the top sits at h - 100 from the bottom.
    let circle = d.find("0\nCIRCLE\n").unwrap_or(d.len());
    assert!(circle < d.len(), "a CIRCLE entity");
    assert!(d[circle..].contains(&format!("10\n200\n20\n{}\n40\n5\n", h - 100.0)));
    assert!(d.contains("1\n\\U+00C5lesund\n50\n90\n"));
    assert!(d.contains("1\n-K1\n"));
}

#[test]
fn arcs_keep_their_side_in_svg_and_dxf() {
    // A quarter arc from 0 to 90 degrees clockwise on the page: from the right of the centre to
    // below it. In DXF (y up) that is from 270 to 360 counter-clockwise.
    let mut pen = crate::writer::Pen::new(100.0);
    pen.arc((50.0, 50.0), 10.0, 0.0, 90.0, false);
    let page = PageDrawing {
        id: "a".into(),
        number: "1".into(),
        title: String::new(),
        size_mm: (100.0, 100.0),
        prims: pen.prims,
    };
    assert!(svg(&page).contains("d=\"M60 50 A10 10 0 0 1 50 60\""));
    assert!(dxf(&page).contains("0\nARC\n8\n0\n62\n7\n10\n50\n20\n50\n40\n10\n50\n270\n51\n0\n"));
}

#[test]
fn wire_numbers_cross_references_and_drawings_are_optional_layers() {
    let mut project = Project::new("Layers");
    let mut p1 = Page::new("p1", "1", "Control");
    p1.symbols.push(place("s1", "coil", 50.0, "-K1"));
    p1.wires.push(Wire {
        id: "w1".into(),
        from: Endpoint::Pin {
            item: "s1".into(),
            pin: "A1".into(),
        },
        to: Endpoint::Point {
            at: Point::new(50.0, 30.0),
        },
        via: vec![],
        number: Some("101".into()),
        potential: None,
        cable: None,
        route: Default::default(),
        spec: Default::default(),
    });
    p1.graphics.push(Shape::Line {
        a: Point::new(200.0, 20.0),
        b: Point::new(260.0, 20.0),
    });
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_string(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    project.pages = vec![p1];
    let (bytes, _) = export(&project, &symbols, &Options::default());
    let pdf = String::from_utf8_lossy(&bytes);
    assert!(pdf.contains("/OCProperties << /OCGs ["), "layers are declared");
    assert!(pdf.contains("/Type /OCG /Name"));
    assert!(pdf.contains("/Properties << /OC1 ") && pdf.contains("/OC2 "));
    // No cross-references on this page, so that layer is not declared.
    assert!(!pdf.contains("/OC3 "));
    // The wire number is drawn inside its layer.
    let at = pdf.find("/OC /OC2 BDC").unwrap_or(usize::MAX);
    assert!(at < pdf.len() && pdf[at..].find("Tj").unwrap_or(usize::MAX) < pdf[at..].find("EMC").unwrap_or(0));

    let (pages, _) = page_drawings(&project, &symbols, &Options::default());
    let d = dxf(&pages[0]);
    assert!(d.contains("0\nTEXT\n8\nWIRE-NUMBERS\n"));
    assert!(d.contains("0\nLINE\n8\nDRAWINGS\n"));

    // A page with nothing optional declares no layers.
    project.pages[0].wires.clear();
    project.pages[0].graphics.clear();
    let (bytes, _) = export(&project, &symbols, &Options::default());
    assert!(!String::from_utf8_lossy(&bytes).contains("/OCProperties"));
}

#[test]
fn table_of_contents_goes_first_and_its_rows_link_to_pages() {
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains"), Page::new("p2", "2", "Control")];
    let contents = wc_report::Table {
        title: "Table of contents".into(),
        columns: vec!["Page".into(), "Title".into()],
        rows: vec![vec!["1".into(), "Mains".into()], vec!["2".into(), "Control".into()]],
        row_pages: vec!["p1".into(), "p2".into()],
        front: true,
    };
    let parts = wc_report::Table {
        title: "Parts list".into(),
        columns: vec!["Part".into()],
        ..Default::default()
    };
    let opts = Options {
        reports: vec![parts, contents],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(report.pages, 4);
    // Bookmarks follow the page order: contents, the two schematic pages, then the parts list.
    let at = |title: &str| {
        text.find(&format!("/Title {}", writer::pdf_text(title)))
            .unwrap_or(usize::MAX)
    };
    assert!(at("R1 Table of contents") < at("1 Mains"));
    assert!(at("1 Mains") < at("2 Control"));
    assert!(at("2 Control") < at("R2 Parts list"));
    assert!(at("R2 Parts list") < usize::MAX);
    // The contents rows jump to the schematic pages, which are now the 2nd and 3rd page objects.
    assert!(text.contains("/Dest [9 0 R /Fit]"), "row 1 -> page 1");
    assert!(text.contains("/Dest [12 0 R /Fit]"), "row 2 -> page 2");
    assert_eq!(text.matches("/Subtype /Link").count(), 2);
}

#[test]
fn underlay_pictures_go_under_the_page_on_their_own_layer() {
    let mut png = Vec::new();
    let img =
        image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(2, 2, vec![0, 255, 255, 0]).unwrap_or_default());
    assert!(
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
    );
    let mut project = Project::new("x");
    let mut p1 = Page::new("p1", "1", "Scanned");
    p1.underlay = Some(wc_core::Underlay {
        file: "underlays/p1.png".into(),
        min: Point::new(0.0, 0.0),
        max: Point::new(420.0, 297.0),
    });
    let mut p2 = Page::new("p2", "2", "Missing picture");
    p2.underlay = Some(wc_core::Underlay {
        file: "underlays/p2.jpg".into(),
        min: Point::new(0.0, 0.0),
        max: Point::new(10.0, 10.0),
    });
    let mut p3 = Page::new("p3", "3", "Not a picture");
    p3.underlay = Some(wc_core::Underlay {
        file: "underlays/p3.jpg".into(),
        min: Point::new(0.0, 0.0),
        max: Point::new(10.0, 10.0),
    });
    project.pages = vec![p1, p2, p3];
    let mut underlays = BTreeMap::new();
    underlays.insert("underlays/p1.png".to_string(), png);
    underlays.insert("underlays/p3.jpg".to_string(), vec![0xFF, 0xD8, 0xFF]);
    let opts = Options {
        underlays,
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(text.matches("/Subtype /Image").count(), 1);
    assert!(text.contains("/Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode"));
    assert!(text.contains("/OC /OC4 BDC q 1190.551 0 0 841.890 0.000 0.000 cm /Im0 Do Q EMC"));
    assert!(text.contains(&format!("/Name {}", writer::pdf_text("Scanned sheets"))));
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    assert!(report.warnings.iter().any(|w| w.contains("p2.jpg is missing")));
    assert!(report.warnings.iter().any(|w| w.contains("damaged")));
}

#[test]
fn panel_layouts_print_after_the_schematic_pages() {
    use wc_core::panel::{Panel, PanelItem};
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    let mut panel = Panel::with_rails("main", "Cabinet CAB1", 600.0, 800.0);
    if let Ok(tag) = DeviceTag::parse("+CAB1-K1") {
        panel.items.push(PanelItem::Device {
            tag,
            at: Point::new(50.0, 100.0),
            width: 45.0,
            height: 85.0,
        });
    }
    let parts = wc_report::Table {
        title: "Parts list".into(),
        ..Default::default()
    };
    let opts = Options {
        panels: vec![panel],
        reports: vec![parts],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(report.pages, 3);
    let at = |title: &str| {
        text.find(&format!("/Title {}", writer::pdf_text(title)))
            .unwrap_or(usize::MAX)
    };
    assert!(at("1 Mains") < at("P1 Cabinet CAB1"));
    assert!(at("P1 Cabinet CAB1") < at("R1 Parts list"));
    assert!(at("R1 Parts list") < usize::MAX);
    // The caption names the plate, scale and device count; the device carries its tag.
    assert!(text.contains(&writer::pdf_text("-K1")));
    assert!(text.contains(&writer::pdf_text(
        "Cabinet CAB1  ·  plate 600 x 800 mm  ·  scale 1:5  ·  1 device(s)"
    )));
}

#[test]
fn a_contact_points_to_the_schematic_coil_not_the_overview_repeat() {
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_owned(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    let mut project = wc_core::Project::new("x");
    // The line diagram comes first in page order, as in many imported projects.
    let mut overview = Page::new("p1", "1", "Line diagram");
    overview.overview = true;
    overview.symbols.push(place("s1", "coil", 50.0, "-K1"));
    let mut schematic = Page::new("p2", "2", "Control");
    schematic.symbols.push(place("s2", "coil", 50.0, "-K1"));
    project.pages = vec![overview, schematic];
    let x = layout::CrossRefs::build(&project, &symbols);
    let tag = DeviceTag::parse("-K1").ok();
    let mains = tag
        .as_ref()
        .and_then(|t| x.mains.get(t))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let pages: Vec<&str> = mains.iter().map(|a| a.page_id.as_str()).collect();
    assert_eq!(pages, ["p2", "p1"], "the schematic first, the overview still listed");
}

#[test]
fn cross_references_give_page_column_and_row() {
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_owned(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    let mut project = wc_core::Project::new("x");
    let mut page = Page::new("p1", "12", "Control");
    // place() puts symbols at y 60: on A3 the rows are 46.17 mm high from 10 mm, so row B.
    page.symbols.push(place("s1", "coil", 100.0, "-K1"));
    project.pages = vec![page];
    let x = layout::CrossRefs::build(&project, &symbols);
    let tag = DeviceTag::parse("-K1").ok();
    let first = tag.as_ref().and_then(|t| x.mains.get(t)).and_then(|m| m.first());
    assert_eq!(first.map(|a| a.location()).as_deref(), Some("12/3.B"));
}

#[test]
fn the_frame_shows_row_letters_on_both_sides() {
    let project = wc_core::Project::new("x");
    let mut page = Page::new("p1", "1", "");
    page.size = wc_core::PageSize::A2;
    let marks = layout::frame_marks(&project, &page, 0, 1, "");
    let count = |l: &str| {
        marks
            .iter()
            .filter(|m| matches!(&m.shape, Shape::Text { text, .. } if text == l))
            .count()
    };
    assert_eq!(count("A"), 2, "left and right");
    assert_eq!(count("H"), 2, "A2 has eight rows");
    assert_eq!(count("I"), 0);
}

#[test]
fn a_filled_polyline_is_filled_not_stroked() {
    let mut pen = crate::writer::Pen::new(100.0);
    crate::layout::draw_shape(
        &mut pen,
        &wc_core::geom::Shape::Polyline {
            points: vec![
                wc_core::geom::Point::new(0.0, 0.0),
                wc_core::geom::Point::new(10.0, 0.0),
                wc_core::geom::Point::new(5.0, 5.0),
            ],
            closed: true,
            filled: true,
        },
    );
    assert!(pen.ops.contains("h f\n"), "{}", pen.ops);
    assert!(!pen.ops.contains("s\n"), "not stroked: {}", pen.ops);
    assert!(matches!(
        pen.prims.as_slice(),
        [crate::writer::Prim::Path {
            filled: true,
            closed: true,
            ..
        }]
    ));
}

#[test]
fn the_cover_and_every_title_block_show_the_same_project_fields() {
    let mut project = wc_core::Project::new("Pump station");
    let tb = &mut project.meta.title_block;
    tb.insert("customer".into(), "ACME Water".into());
    tb.insert("description".into(), "Two duty pumps".into());
    tb.insert("revision".into(), "B".into());
    tb.insert("contract".into(), "K-77".into());
    let mut cover = Page::new("p1", "1", "Cover");
    cover.cover = true;
    let schematic = Page::new("p2", "2", "Motors");
    let texts = |page: &Page, project: &wc_core::Project| -> Vec<String> {
        layout::frame_marks(project, page, 0, 2, "2026-11-17")
            .into_iter()
            .filter_map(|m| match m.shape {
                Shape::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    };
    let count = |v: &[String], t: &str| v.iter().filter(|x| *x == t).count();
    let on_cover = texts(&cover, &project);
    // Once in the cover's table, once in its title block.
    assert_eq!(count(&on_cover, "ACME Water"), 2, "{on_cover:?}");
    assert_eq!(
        count(&on_cover, "Pump station"),
        2,
        "large on the cover and in the title block"
    );
    assert_eq!(count(&on_cover, "Two duty pumps"), 1);
    assert_eq!(count(&on_cover, "K-77"), 1, "other fields are listed too");
    assert_eq!(
        count(&on_cover, "Project number"),
        1,
        "standard fields show even when empty"
    );
    assert_eq!(
        count(&on_cover, "2026-11-17"),
        2,
        "the export date when the project has none"
    );
    let on_page = texts(&schematic, &project);
    assert_eq!(
        count(&on_page, "ACME Water"),
        1,
        "a normal page has only its title block"
    );
    assert_eq!(count(&on_page, "Two duty pumps"), 0);

    // Edit the field once: the cover and every title block follow.
    project
        .meta
        .title_block
        .insert("customer".into(), "Nordic Water".into());
    assert_eq!(count(&texts(&cover, &project), "Nordic Water"), 2);
    assert_eq!(count(&texts(&schematic, &project), "Nordic Water"), 1);
    assert_eq!(count(&texts(&cover, &project), "ACME Water"), 0);
}

#[test]
fn a_blank_date_shows_the_export_date_on_the_cover_and_in_the_title_block() -> Result<(), String> {
    let mut project = wc_core::Project::new("x");
    project.meta.title_block.insert("date".into(), "  ".into());
    project
        .meta
        .title_block
        .insert("description".into(), "A very long description ".repeat(30));
    let mut cover = Page::new("p1", "1", "Cover");
    cover.cover = true;
    cover.size = wc_core::PageSize::A4;
    let marks = layout::frame_marks(&project, &cover, 0, 1, "2026-11-17");
    let dates = marks
        .iter()
        .filter(|m| matches!(&m.shape, Shape::Text { text, .. } if text == "2026-11-17"))
        .count();
    assert_eq!(dates, 2, "cover table and title block");
    // The long description is drawn smaller so it fits inside the A4 frame.
    let desc = marks
        .iter()
        .find_map(|m| match &m.shape {
            Shape::Text { text, height, .. } if text.starts_with("A very long") => Some((text.clone(), *height)),
            _ => None,
        })
        .ok_or("no description on the cover")?;
    assert!(
        crate::writer::text_width_mm(&desc.0, desc.1) <= 297.0 - 20.0,
        "{desc:?}"
    );
    Ok(())
}

#[test]
fn long_cover_table_labels_and_values_stay_in_their_columns() -> Result<(), String> {
    let mut project = wc_core::Project::new("x");
    let label = "Contractor reference for the site".repeat(3);
    let value = "W".repeat(80);
    project.meta.title_block.insert(label.clone(), value.clone());
    let mut cover = Page::new("p1", "1", "Cover");
    cover.cover = true;
    let marks = layout::frame_marks(&project, &cover, 0, 1, "2026-11-17");
    let width_of = |wanted: &str| {
        marks
            .iter()
            .find_map(|m| match &m.shape {
                Shape::Text { text, height, .. } if text == wanted => Some(crate::writer::text_width_mm(text, *height)),
                _ => None,
            })
            .ok_or(format!("{wanted:?} not on the cover"))
    };
    // The table is at most 180 mm wide: 35 % for labels, 65 % for values, less 3 mm padding.
    let (label_w, value_w) = (width_of(&label)?, width_of(&value)?);
    assert!(label_w <= 180.0 * 0.35 - 3.0 + 1e-6, "label {label_w} mm");
    assert!(value_w <= 180.0 * 0.65 - 3.0 + 1e-6, "value {value_w} mm");
    // Em dashes are 1000 units wide in Helvetica: measured on their own, the fitted row stays
    // inside the value column too.
    let dashes = "\u{2014}".repeat(120);
    project.meta.title_block.insert("Site notes".into(), dashes.clone());
    let marks = layout::frame_marks(&project, &cover, 0, 1, "2026-11-17");
    let height = marks
        .iter()
        .find_map(|m| match &m.shape {
            Shape::Text { text, height, .. } if *text == dashes => Some(*height),
            _ => None,
        })
        .ok_or("the dashes are not on the cover")?;
    let dash_w = 120.0 * height / 0.72;
    assert!(dash_w <= 180.0 * 0.65 - 3.0 + 1e-6, "dashes {dash_w} mm");
    Ok(())
}

#[test]
fn text_widths_are_helvetica_advance_widths() {
    // Adobe Helvetica AFM: W 944, O 778, H 722, e 556, l 222, o 556, i 222, space 278.
    let em_mm = |units: f64| units / 1000.0 * (10.0 / 0.72);
    // Accented: aring, eacute 556, oslash 611; emdash 1000, endash 556; `?` 556 for what
    // `pdf_text` cannot encode.
    for (text, units) in [
        ("W", 944.0),
        ("O", 778.0),
        ("Hello", 2278.0),
        ("i i", 722.0),
        ("å", 556.0),
        ("é", 556.0),
        ("ø", 611.0),
        ("\u{2014}", 1000.0),
        ("\u{2013}", 556.0),
        ("\u{4e2d}", 556.0),
    ] {
        let measured = crate::writer::text_width_mm(text, 10.0);
        assert!(
            (measured - em_mm(units)).abs() < 1e-9,
            "{text:?}: {measured} mm, want {} mm",
            em_mm(units)
        );
    }
}
