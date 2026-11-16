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
