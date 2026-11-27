use super::*;
use wc_core::geom::Point;
use wc_core::symbol::{Pin, PinDir, SymbolRole};
use wc_core::{DeviceTag, Endpoint, Page, PageSize, PlacedSymbol, Shape, Wire};

fn sym(id: &str, role: SymbolRole, pins: [&str; 2]) -> Symbol {
    Symbol {
        source: String::new(),
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
fn dxf_text_keeps_percent_signs_backslashes_and_wide_characters_literal() {
    // `%%c` would be read as a diameter sign and `\U+0041` as `A` (#832).
    assert_eq!(vector::dxf_text("50%%c"), "50\\U+0025\\U+0025c");
    assert_eq!(vector::dxf_text("a%%%b"), "a\\U+0025\\U+0025\\U+0025b");
    assert_eq!(vector::dxf_text("a\\U+0041"), "a\\U+005CU+0041");
    // A lone percent sign starts no code and stays readable.
    assert_eq!(vector::dxf_text("100 % on"), "100 % on");
    assert_eq!(vector::dxf_text("%\t%d"), "\\U+0025\\U+0025d");
    // Beyond U+FFFF there is no four-digit code; a five-digit one would be misread.
    assert_eq!(vector::dxf_text("ok \u{1F600}"), "ok ?");
    assert_eq!(vector::dxf_text("Ålesund\n"), "\\U+00C5lesund");
}

#[test]
fn page_files_are_named_after_the_page_number() {
    let page = |number: &str| PageDrawing {
        id: "p-7".into(),
        number: number.into(),
        title: String::new(),
        size_mm: (100.0, 100.0),
        prims: Vec::new(),
        pictures: Vec::new(),
        panel: false,
    };
    // The app and the command line name page files the same way: `1.2` keeps its dot.
    assert_eq!(crate::page_file_stem(&page("1.2")), "1.2");
    assert_eq!(crate::page_file_stem(&page(" A/1 ")), "A_1");
    // Without a number, the page's id names the file.
    assert_eq!(crate::page_file_stem(&page("  ")), "p-7");
    // Leading dots are left out, and a name of only dots falls back to `page`.
    assert_eq!(crate::page_file_stem(&page(".hidden")), "hidden");
    assert_eq!(crate::page_file_stem(&page("..")), "page");
    // A name Windows keeps for a device, in any case and also before a dot, gets a `_` in front.
    assert_eq!(crate::page_file_stem(&page(".NUL")), "_NUL");
    assert_eq!(crate::page_file_stem(&page("con")), "_con");
    assert_eq!(crate::page_file_stem(&page("com1")), "_com1");
    assert_eq!(crate::page_file_stem(&page("Aux.2")), "_Aux.2");
    assert_eq!(crate::page_file_stem(&page("LPT9")), "_LPT9");
    // Names that only start like a device stay as they are.
    assert_eq!(crate::page_file_stem(&page("CONSOLE")), "CONSOLE");
    assert_eq!(crate::page_file_stem(&page("COM10")), "COM10");
    assert_eq!(crate::page_file_stem(&page("COM0")), "COM0");
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
        pictures: Vec::new(),
        panel: false,
    };
    assert!(svg(&page).contains("d=\"M60 50 A10 10 0 0 1 50 60\""));
    assert!(dxf(&page).contains("0\nARC\n8\n0\n62\n7\n10\n50\n20\n50\n40\n10\n50\n270\n51\n0\n"));
}

#[test]
fn svg_texts_keep_their_spaces() {
    // Viewers squeeze leading and repeated spaces unless the text says to keep them.
    let mut pen = crate::writer::Pen::new(100.0);
    pen.text((10.0, 10.0), 3.0, 0, "  L1  L2");
    let page = PageDrawing {
        id: "a".into(),
        number: "1".into(),
        title: String::new(),
        size_mm: (100.0, 100.0),
        prims: pen.prims,
        pictures: Vec::new(),
        panel: false,
    };
    let s = svg(&page);
    let Some(text) = s.lines().find(|l| l.starts_with("<text")) else {
        unreachable!("no text in {s}");
    };
    assert!(text.starts_with("<text xml:space=\"preserve\" "), "{text}");
    assert!(text.ends_with(">  L1  L2</text>"), "{text}");
}

#[test]
fn svg_leaves_out_characters_xml_does_not_allow() {
    // U+FFFE and U+FFFF are not allowed anywhere in XML: one of them makes viewers refuse the
    // whole file, so they are left out like control characters.
    let mut pen = crate::writer::Pen::new(100.0);
    pen.text((10.0, 10.0), 3.0, 0, "K1\u{FFFE} \u{FFFF}24V\u{7}");
    let page = PageDrawing {
        id: "a".into(),
        number: "1\u{FFFF}".into(),
        title: "Main\u{FFFE}".into(),
        size_mm: (100.0, 100.0),
        prims: pen.prims,
        pictures: Vec::new(),
        panel: false,
    };
    let s = svg(&page);
    assert!(!s.contains(['\u{FFFE}', '\u{FFFF}']), "{s}");
    assert!(s.contains("<title>1 Main</title>"), "{s}");
    assert!(s.contains(">K1 24V</text>"), "{s}");
}

#[test]
fn dxf_fills_junction_dots_and_filled_shapes() {
    let wire = |id: &str, a: (f64, f64), b: (f64, f64)| Wire {
        id: id.into(),
        from: Endpoint::Point {
            at: Point::new(a.0, a.1),
        },
        to: Endpoint::Point {
            at: Point::new(b.0, b.1),
        },
        via: vec![],
        number: None,
        potential: None,
        cable: None,
        route: Default::default(),
        spec: Default::default(),
    };
    let mut project = Project::new("Fills");
    let mut p1 = Page::new("p1", "1", "Control");
    // A T: the second wire ends in the middle of the first, a junction at (100, 50).
    p1.wires.push(wire("w1", (50.0, 50.0), (150.0, 50.0)));
    p1.wires.push(wire("w2", (100.0, 50.0), (100.0, 100.0)));
    p1.graphics.push(Shape::Polyline {
        points: vec![
            Point::new(200.0, 100.0),
            Point::new(210.0, 100.0),
            Point::new(205.0, 90.0),
        ],
        closed: true,
        filled: true,
    });
    project.pages = vec![p1];
    let (pages, _) = page_drawings(&project, &BTreeMap::new(), &Options::default());
    let h = pages[0].size_mm.1;
    let d = dxf(&pages[0]);
    assert!(!d.contains("0\nCIRCLE\n"), "no hollow circle for the dot: {d}");
    // The dot: a closed polyline 0.6 wide of two half circles through 99.7 and 100.3.
    let dot = format!(
        "66\n1\n70\n1\n40\n0.6\n41\n0.6\n0\nVERTEX\n8\n0\n10\n99.7\n20\n{y}\n40\n0.6\n41\n0.6\n42\n1\n\
         0\nVERTEX\n8\n0\n10\n100.3\n20\n{y}\n40\n0.6\n41\n0.6\n42\n1\n0\nSEQEND\n",
        y = h - 50.0
    );
    assert!(d.contains(&dot), "{d}");
    // The triangle: one SOLID with its last corner repeated.
    let solid = format!(
        "0\nSOLID\n8\nDRAWINGS\n62\n7\n10\n200\n20\n{a}\n11\n210\n21\n{a}\n12\n205\n22\n{b}\n13\n205\n23\n{b}\n",
        a = h - 100.0,
        b = h - 90.0
    );
    assert!(d.contains(&solid), "{d}");
}

#[test]
fn filled_outlines_become_dxf_solids_when_convex() {
    let Some(quad) = crate::vector::solid_triangles(&[(0.0, 0.0), (4.0, 0.0), (4.0, 3.0), (0.0, 3.0), (0.0, 0.0)])
    else {
        unreachable!("a rectangle is convex")
    };
    assert_eq!(quad, vec![[(0.0, 0.0), (4.0, 0.0), (0.0, 3.0), (4.0, 3.0)]]);
    let Some(fan) = crate::vector::solid_triangles(&[(0.0, 0.0), (2.0, 0.0), (3.0, 1.0), (2.0, 2.0), (0.0, 2.0)])
    else {
        unreachable!("a pentagon is convex")
    };
    assert_eq!(fan.len(), 3);
    // An arrow with a notch is not convex: it stays an outline.
    assert!(crate::vector::solid_triangles(&[(0.0, 0.0), (4.0, 2.0), (0.0, 4.0), (1.0, 2.0)]).is_none());
    // A rectangle with a segment drawn back over itself turns half a turn each way at the
    // reversals: a fan would paint outside it, so it stays an outline.
    assert!(
        crate::vector::solid_triangles(&[(4.0, 4.0), (0.0, 4.0), (0.0, 0.0), (8.0, 0.0), (0.0, 0.0), (4.0, 0.0)])
            .is_none()
    );
}

#[test]
fn dxf_keeps_a_filled_path_that_is_not_convex_as_a_closed_outline() {
    // An open, filled notched arrow: PDF and SVG fill it as if closed, so the DXF outline is
    // closed too (group 70 is 1), and there is no SOLID.
    let page = PageDrawing {
        id: "a".into(),
        number: "1".into(),
        title: String::new(),
        size_mm: (100.0, 100.0),
        prims: vec![crate::writer::Prim::Path {
            points: vec![(0.0, 0.0), (4.0, 2.0), (0.0, 4.0), (1.0, 2.0)],
            closed: false,
            filled: true,
            ink: crate::writer::Stroke {
                rgb: (0.0, 0.0, 0.0),
                width: 0.25,
            },
            layer: None,
        }],
        pictures: Vec::new(),
        panel: false,
    };
    let d = dxf(&page);
    assert!(!d.contains("0\nSOLID\n"), "{d}");
    assert!(d.contains("0\nPOLYLINE\n8\n0\n62\n7\n66\n1\n70\n1\n"), "{d}");
}

#[test]
fn dxf_strokes_wider_than_the_default_pen_keep_their_width() {
    let stroke = |points: Vec<(f64, f64)>, width: f64| crate::writer::Prim::Path {
        points,
        closed: false,
        filled: false,
        ink: crate::writer::Stroke {
            rgb: (0.0, 0.0, 0.0),
            width,
        },
        layer: None,
    };
    let page = PageDrawing {
        id: "a".into(),
        number: "1".into(),
        title: String::new(),
        size_mm: (100.0, 100.0),
        prims: vec![
            stroke(vec![(0.0, 0.0), (10.0, 0.0)], 0.25),
            stroke(vec![(0.0, 10.0), (10.0, 10.0)], 0.35),
            stroke(vec![(0.0, 20.0), (10.0, 20.0), (10.0, 30.0)], 0.5),
        ],
        pictures: Vec::new(),
        panel: false,
    };
    let d = dxf(&page);
    // A thin stroke stays a plain LINE.
    assert!(
        d.contains("0\nLINE\n8\n0\n62\n7\n10\n0\n20\n100\n11\n10\n21\n100\n"),
        "{d}"
    );
    assert_eq!(d.matches("0\nLINE\n").count(), 1, "{d}");
    // A wide two-point stroke is a two-vertex polyline with its width on each part.
    assert!(
        d.contains(
            "0\nPOLYLINE\n8\n0\n62\n7\n66\n1\n70\n0\n40\n0.35\n41\n0.35\n\
             0\nVERTEX\n8\n0\n10\n0\n20\n90\n40\n0.35\n41\n0.35\n\
             0\nVERTEX\n8\n0\n10\n10\n20\n90\n40\n0.35\n41\n0.35\n0\nSEQEND\n"
        ),
        "{d}"
    );
    // A longer one carries its width too.
    assert!(d.contains("70\n0\n40\n0.5\n41\n0.5\n0\nVERTEX\n"), "{d}");
    assert_eq!(d.matches("0\nVERTEX\n").count(), 5, "{d}");
    assert!(d.contains("$ACADVER\n1\nAC1009\n") && !d.contains("LWPOLYLINE") && !d.contains("\n370\n"));
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

    // In SVG the wire number sits in its own group that Inkscape shows as a layer.
    let s = svg(&pages[0]);
    assert!(s.contains("xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\""));
    let open = "id=\"wire-numbers\" inkscape:groupmode=\"layer\" inkscape:label=\"Wire numbers\">";
    let Some(at) = s.find(open) else {
        unreachable!("no wire-numbers group: {s}")
    };
    let group = &s[at..at + s[at..].find("</g>").unwrap_or(0)];
    assert!(group.contains(">101</text>"), "{group}");
    assert!(s.contains("id=\"drawings\" inkscape:groupmode=\"layer\""));
    // Symbol labels have no layer and stay outside the layer groups.
    let Some(k1) = s.find(">-K1</text>") else {
        unreachable!("no -K1 label")
    };
    let last_open = s[..k1].rfind("<g ").unwrap_or(0);
    assert!(!s[last_open..k1].contains("inkscape:groupmode"));
    assert_well_formed(&s);

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
        ..Default::default()
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
    // The third row is the parts list (#693).
    assert_eq!(text.matches("/Subtype /Link").count(), 3);
}

#[test]
fn table_of_contents_lists_the_sheets_after_the_schematic_pages() {
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    let band = project.meta.template.band_height();
    let per_page = layout::rows_per_page(layout::report_size(project.pages[0].size, band, 1), band);
    // One row short of a full page, so the rows of the later sheets run onto a second page.
    let mut rows = vec![vec!["1".to_owned(), "Mains".to_owned()]];
    rows.resize(per_page - 1, vec![String::new(), "Note".to_owned()]);
    let contents = wc_report::Table {
        title: "Table of contents".into(),
        columns: vec!["Page".into(), "Title".into()],
        rows,
        row_pages: vec!["p1".into()],
        front: true,
        ..Default::default()
    };
    let parts = wc_report::Table {
        title: "Parts list".into(),
        columns: vec!["Part".into()],
        ..Default::default()
    };
    let opts = Options {
        reports: vec![contents, parts],
        panels: vec![wc_core::panel::Panel::with_rails("main", "Cabinet", 600.0, 800.0)],
        networks: vec![wc_core::network::Network {
            name: "PN1".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    // Two contents pages, the schematic page, the panel, the parts list and the network.
    assert_eq!(report.pages, 6);
    let at = |title: &str| {
        text.find(&format!("/Title {}", writer::pdf_text(title)))
            .unwrap_or(usize::MAX)
    };
    assert!(at("R2 Table of contents (2/2)") < usize::MAX);
    // The parts list is the third report page, after the two contents pages.
    assert!(at("R3 Parts list") < usize::MAX);
    let shows = |s: &str| text.contains(&format!("{} Tj", writer::pdf_text(s)));
    for cell in ["P1", "Cabinet", "R3", "Parts list", "N1", "Network PN1"] {
        assert!(shows(cell), "{cell}");
    }
    // The schematic page and the three later sheets are linked from the contents.
    assert_eq!(text.matches("/Subtype /Link").count(), 4);
}

/// A contents table as `wc_report::contents` builds it, with `rows` of (page, title).
fn contents_table(rows: &[(&str, &str)], row_pages: &[&str]) -> wc_report::Table {
    wc_report::Table {
        title: "Table of contents".into(),
        columns: ["Page", "Title", "Devices", "Wires"].map(String::from).to_vec(),
        rows: rows
            .iter()
            .map(|(p, t)| vec![(*p).to_owned(), (*t).to_owned(), String::new(), String::new()])
            .collect(),
        row_pages: row_pages.iter().map(|p| (*p).to_owned()).collect(),
        front: true,
        ..Default::default()
    }
}

#[test]
fn later_sheets_fill_the_columns_of_a_contents_layout() {
    use wc_core::report_layout::{ReportColumn, ReportLayout};
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    // The project's layout shows the title first and the page renamed, without the counts.
    let layout = ReportLayout {
        columns: vec![
            ReportColumn {
                from: "Title".into(),
                heading: None,
            },
            ReportColumn {
                from: "Page".into(),
                heading: Some("Sheet".into()),
            },
        ],
        ..Default::default()
    };
    let contents = contents_table(&[("1", "Mains")], &["p1"]).with_layout(&layout);
    let parts = wc_report::Table {
        title: "Parts list".into(),
        columns: vec!["Part".into()],
        ..Default::default()
    };
    let opts = Options {
        reports: vec![contents, parts],
        ..Default::default()
    };
    let (pdf, _) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    let at = |s: &str| text.find(&format!("{} Tj", writer::pdf_text(s)));
    // The row reads "Parts list | R2", like "Mains | 1" above it.
    let (Some(mains), Some(sheet), Some(title), Some(number)) = (at("Mains"), at("Sheet"), at("Parts list"), at("R2"))
    else {
        unreachable!("the contents shows both rows")
    };
    assert!(sheet < mains && mains < title && title < number);
    // Both on the contents page, not "R2" from the parts list's own sheet.
    assert!(!text[title..number].contains("endstream"));
    assert_eq!(text.matches("/Subtype /Link").count(), 2);
}

#[test]
fn the_reader_page_box_shows_each_sheet_own_number() {
    // Page labels: typing a sheet's number into the reader's page box opens that sheet (#727).
    let mut project = Project::new("x");
    project.pages = vec![
        Page::new("p1", "3", "Mains"),
        Page::new("p2", "=A1/5 (b)", "Control"),
        Page::new("p3", "", "Unnumbered"),
        Page::new("p4", "Å1", "Nordic"),
    ];
    let contents = wc_report::Table {
        title: "Table of contents".into(),
        columns: vec!["Page".into(), "Title".into()],
        front: true,
        ..Default::default()
    };
    let opts = Options {
        reports: vec![contents],
        panels: vec![wc_core::panel::Panel::with_rails("main", "Cabinet", 600.0, 800.0)],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(report.pages, 6);
    let labels = "/PageLabels << /Nums [0 << /P (R1) >> 1 << /P (3) >> 2 << /P (=A1/5 \\(b\\)) >> \
                  3 << /S /D /St 4 >> 4 << /P <FEFF00C50031> >> 5 << /P (P1) >>] >>";
    let found = text.find("/PageLabels").map_or("", |at| &text[at..]);
    assert!(text.contains(labels), "{}", found.lines().next().unwrap_or(""));
}

#[test]
fn a_contents_grouped_by_page_counts_a_heading_per_later_sheet() {
    use wc_core::report_layout::ReportLayout;
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    let band = project.meta.template.band_height();
    let per_page = layout::rows_per_page(layout::report_size(project.pages[0].size, band, 2), band);
    // Page 1 as a heading with its rows takes all but three lines. The panel and the parts
    // list then each take a heading and a row: four lines, so the parts list row runs onto
    // a second contents page and the parts list itself is R3.
    let mut rows = vec![("1", "Mains")];
    rows.resize(per_page - 4, ("1", "Note"));
    let layout = ReportLayout {
        group: Some("Page".into()),
        ..Default::default()
    };
    let contents = contents_table(&rows, &["p1"]).with_layout(&layout);
    let parts = wc_report::Table {
        title: "Parts list".into(),
        columns: vec!["Part".into()],
        ..Default::default()
    };
    let opts = Options {
        reports: vec![contents, parts],
        panels: vec![wc_core::panel::Panel::with_rails("main", "Cabinet", 600.0, 800.0)],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(report.pages, 5);
    let has = |title: &str| text.contains(&format!("/Title {}", writer::pdf_text(title)));
    assert!(has("R2 Table of contents (2/2)"));
    assert!(has("R3 Parts list"));
    // The parts list's row in the contents is headed R3: the last text before its title.
    let Some(title) = text.find(&format!("{} Tj", writer::pdf_text("Parts list"))) else {
        unreachable!("the contents lists the parts list")
    };
    let row = text[..title].rsplit("Tj").nth(1).unwrap_or_default();
    assert!(row.contains(&writer::pdf_text("Page: R3")), "{row}");
}

#[test]
fn table_of_contents_lists_only_the_exported_pages() {
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains"), Page::new("p2", "2", "Control")];
    let contents = wc_report::Table {
        title: "Table of contents".into(),
        columns: vec!["Page".into(), "Title".into()],
        rows: vec![
            vec!["1".into(), "Mains".into()],
            vec!["2".into(), "Control".into()],
            vec!["".into(), "Note".into()],
        ],
        row_pages: vec!["p1".into(), "p2".into()],
        front: true,
        ..Default::default()
    };
    let opts = Options {
        pages: vec!["p2".into()],
        reports: vec![contents],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(report.pages, 2);
    // The left-out page has no row; the exported page and the row naming no page stay.
    let shows = |s: &str| text.contains(&format!("{} Tj", writer::pdf_text(s)));
    assert!(!shows("Mains"));
    assert!(shows("Control"));
    assert!(shows("Note"));
    assert_eq!(text.matches("/Subtype /Link").count(), 1);
}

#[test]
fn a_report_in_the_project_s_own_layout_prints_that_layout() {
    use wc_core::report_layout::{ReportColumn, ReportLayout};
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    project.meta.reports.push(ReportLayout {
        kind: "parts".into(),
        title: Some("Bill of materials".into()),
        columns: vec![ReportColumn {
            from: "Order number".into(),
            heading: Some("Art. no.".into()),
        }],
        ..Default::default()
    });
    let none = BTreeMap::new();
    let inputs = wc_report::Inputs {
        project: &project,
        symbols: &none,
        parts: &BTreeMap::new(),
    };
    let opts = Options {
        reports: vec![wc_report::build(wc_report::Kind::Parts, &inputs)],
        ..Default::default()
    };
    let (pdf, _) = export(&project, &none, &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains(&format!("/Title {}", writer::pdf_text("R1 Bill of materials"))));
    assert!(text.contains(&writer::pdf_text("Art. no.")));
    assert!(!text.contains(&writer::pdf_text("Manufacturer")));
}

#[test]
fn grouped_report_pages_print_a_heading_per_group_and_the_header_values() {
    use layout::TableLine::{Group, Row};
    let row = |loc: &str, wire: &str| vec![loc.to_string(), wire.to_string()];
    let mut table = wc_report::Table {
        title: "Wires".into(),
        columns: vec!["Location".into(), "Wire".into()],
        rows: vec![row("+A", "W1"), row("+A", "W2"), row("+A", "W3"), row("+B", "W4")],
        group: Some(0),
        ..Default::default()
    };
    let g = |v: &str, continued: bool| Group {
        value: v.into(),
        continued,
    };
    // Four lines a page: +A runs onto the second page under a continued heading, and +B's
    // heading does not end a page alone.
    assert_eq!(
        layout::table_pages(&table, 4),
        vec![
            vec![g("+A", false), Row(0), Row(1), Row(2)],
            vec![g("+B", false), Row(3)],
        ]
    );
    assert_eq!(
        layout::table_pages(&table, 3),
        vec![
            vec![g("+A", false), Row(0), Row(1)],
            vec![g("+A", true), Row(2)],
            vec![g("+B", false), Row(3)],
        ]
    );
    // The line of header values takes one line of each page.
    table.header = vec!["project.name".into(), "revision".into()];
    assert_eq!(layout::table_pages(&table, 4)[0], vec![g("+A", false), Row(0), Row(1)]);
    // No page holds more lines than it has room for: without room for a heading and a row,
    // the headings are left out.
    for per_page in 1..6 {
        let pages = layout::table_pages(&table, per_page);
        // The whole budget: the header line when it is printed, and the table lines.
        let header = usize::from(layout::prints_header(&table, per_page));
        assert!(
            pages.iter().all(|p| p.len() + header <= per_page),
            "{per_page}: {pages:?}"
        );
        assert_eq!(pages.iter().flatten().filter(|l| matches!(l, Row(_))).count(), 4);
    }
    assert_eq!(layout::table_pages(&table, 2)[0], vec![Row(0)]);
    // One line a page: no room for the header values with a row, so they are not printed.
    assert!(!layout::prints_header(&table, 1));
    assert_eq!(layout::table_pages(&table, 1)[0], vec![Row(0)]);

    let mut project = Project::new("Plant 7");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    project.meta.title_block.insert("revision".into(), "C".into());
    let opts = Options {
        reports: vec![table],
        ..Default::default()
    };
    let (pdf, _) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    for shown in [
        "Location: +A",
        "Location: +B",
        "W4",
        "Project name: Plant 7    Revision: C",
    ] {
        assert!(text.contains(&writer::pdf_text(shown)), "{shown}");
    }
    // The location is in the headings, not in a column of its own.
    assert!(!text.contains(&writer::pdf_text("+A")), "no +A cell");
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
fn the_logo_shows_in_every_title_block_and_on_the_cover_stored_once() {
    let mut png = Vec::new();
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 2, image::Rgb([200, 30, 30])));
    assert!(
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
    );
    let mut project = Project::new("x");
    let mut cover = Page::new("p0", "0", "Cover");
    cover.cover = true;
    project.pages = vec![cover, Page::new("p1", "1", "Mains")];
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: Some((Point::new(100.0, 40.0), Point::new(140.0, 60.0))),
    });
    let mut underlays = BTreeMap::new();
    underlays.insert("pictures/logo.png".to_string(), png);
    let opts = Options {
        underlays,
        reports: vec![wc_report::Table {
            title: "Parts list".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(
        text.matches("/Subtype /Image").count(),
        1,
        "one image object for every page"
    );
    assert!(text.contains("/Width 4 /Height 2 /ColorSpace /DeviceRGB"));
    // Title blocks on both schematic pages and the report page, plus the cover's own place.
    assert_eq!(text.matches("cm /Pic0 Do Q").count(), 3);
    assert_eq!(text.matches("cm /Pic1 Do Q").count(), 1);
    // On the cover: 40 x 20 mm fills the 40 x 20 mm box exactly (same proportions).
    let mm = |v: f64| v * writer::PT_PER_MM;
    let cover_op = format!(
        "q {:.3} 0 0 {:.3} {:.3} {:.3} cm /Pic1 Do Q",
        mm(40.0),
        mm(20.0),
        mm(100.0),
        mm(297.0 - 60.0)
    );
    assert!(text.contains(&cover_op), "{cover_op}");
    // "WiringCraft" gives way to the logo in the title block.
    let brand = format!("{} Tj", writer::pdf_text("WiringCraft"));
    assert!(!text.contains(&brand));
    project.meta.logo = None;
    let (pdf, _) = export(&project, &BTreeMap::new(), &Options::default());
    assert!(
        String::from_utf8_lossy(&pdf).contains(&brand),
        "shown again without a logo"
    );
}

#[test]
fn a_logo_cover_box_that_is_not_a_number_takes_the_default_place() {
    let mut png = Vec::new();
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 2, image::Rgb([200, 30, 30])));
    assert!(
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
    );
    let mut project = Project::new("x");
    let mut cover = Page::new("p0", "0", "Cover");
    cover.cover = true;
    project.pages = vec![cover.clone()];
    let mut underlays = BTreeMap::new();
    underlays.insert("pictures/logo.png".to_string(), png);
    let opts = Options {
        underlays,
        ..Default::default()
    };
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: None,
    });
    let (default_pdf, _) = export(&project, &BTreeMap::new(), &opts);
    let default_box = cover_logo_box(&project, &cover);
    let cover_svg = |project: &Project| {
        let (pages, _) = page_drawings(project, &BTreeMap::new(), &opts);
        let Some(page) = pages.first() else {
            unreachable!("the cover page is drawn")
        };
        svg(page)
    };
    let default_svg = cover_svg(&project);
    // `nan` and `inf` are valid TOML numbers, so a hand-edited project.toml can hold them.
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        project.meta.logo = Some(wc_core::Logo {
            file: "pictures/logo.png".into(),
            cover: Some((Point::new(bad, 40.0), Point::new(140.0, 60.0))),
        });
        assert_eq!(cover_logo_box(&project, &cover), default_box, "{bad}");
        let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        // Drawn as if no box were set, instead of `NaN` in the page.
        assert!(pdf == default_pdf, "{bad}: the PDF differs from the default place");
        assert_eq!(cover_svg(&project), default_svg, "{bad}");
    }
}

#[test]
fn a_missing_logo_is_a_warning_and_the_export_still_works() {
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/gone.png".into(),
        cover: None,
    });
    let (pdf, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(report.pages, 1);
    assert!(pdf.starts_with(b"%PDF"));
    assert_eq!(report.warnings, ["logo picture pictures/gone.png is missing"]);
    assert!(!String::from_utf8_lossy(&pdf).contains("/Subtype /Image"));
}

#[test]
#[allow(clippy::unwrap_used)]
fn a_transparent_logo_keeps_its_transparency_through_a_soft_mask() {
    let mut png = Vec::new();
    let mut rgba = image::RgbaImage::from_pixel(2, 1, image::Rgba([0, 0, 0, 0]));
    rgba.put_pixel(1, 0, image::Rgba([10, 20, 30, 255]));
    assert!(
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
    );
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains"), Page::new("p2", "2", "More")];
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: None,
    });
    let mut underlays = BTreeMap::new();
    underlays.insert("pictures/logo.png".to_string(), png);
    let opts = Options {
        underlays,
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(
        text.matches("/SMask ").count(),
        1,
        "one mask for the one shared picture"
    );
    assert!(
        text.contains("/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length")
    );
    // The mask holds the alpha samples: transparent, then opaque.
    let mask = picture::decode(&opts.underlays["pictures/logo.png"])
        .unwrap()
        .alpha
        .unwrap();
    assert_eq!(miniz_oxide::inflate::decompress_to_vec_zlib(&mask).unwrap(), [0, 255]);
    // Every object the cross-reference table lists is in the file.
    let objs = text.matches(" 0 obj\n").count();
    assert!(text.contains(&format!("/Size {}", objs + 1)));
}

#[test]
fn a_jpeg_cut_short_after_its_header_is_a_warning_for_the_logo_and_an_underlay() {
    // A frame header and nothing else: no scan, no end marker.
    let cut = vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00,
    ];
    let mut project = Project::new("x");
    let mut p1 = Page::new("p1", "1", "Scan");
    p1.underlay = Some(wc_core::Underlay {
        file: "underlays/cut.jpg".into(),
        min: Point::new(0.0, 0.0),
        max: Point::new(10.0, 10.0),
    });
    project.pages = vec![p1];
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/cut.jpg".into(),
        cover: None,
    });
    let mut underlays = BTreeMap::new();
    underlays.insert("underlays/cut.jpg".to_string(), cut.clone());
    underlays.insert("pictures/cut.jpg".to_string(), cut);
    let opts = Options {
        underlays,
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    assert!(!String::from_utf8_lossy(&pdf).contains("/Subtype /Image"));
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    assert!(report.warnings.iter().all(|w| w.contains("damaged")));
}

/// A minimal JPEG: SOI, an optional APP14 `Adobe` segment, a baseline frame header 300 wide
/// and 200 high with `channels` components, a scan header, one scan byte and EOI.
fn tiny_jpeg(channels: u8, adobe: bool) -> Vec<u8> {
    let mut b = vec![0xFF, 0xD8];
    if adobe {
        // APP14: "Adobe", version 100, flags0, flags1, transform 0 (no colour transform).
        b.extend([0xFF, 0xEE, 0x00, 0x0E]);
        b.extend(b"Adobe");
        b.extend([0x00, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }
    // SOF0: precision 8, height 200, width 300, then one 3-byte entry per component.
    let sof_len = 8 + 3 * u16::from(channels);
    b.extend([0xFF, 0xC0]);
    b.extend(sof_len.to_be_bytes());
    b.extend([0x08, 0x00, 0xC8, 0x01, 0x2C, channels]);
    for c in 1..=channels {
        b.extend([c, 0x11, 0x00]);
    }
    // SOS with one component, then a scan byte and EOI.
    b.extend([
        0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x00, 0xFF, 0xD9,
    ]);
    b
}

#[test]
fn jpeg_pictures_get_the_colour_space_of_their_channel_count() {
    for (channels, adobe, space, extra) in [
        (1, false, "DeviceGray", ""),
        (3, false, "DeviceRGB", ""),
        (4, false, "DeviceCMYK", ""),
        (4, true, "DeviceCMYK", " /Decode [1 0 1 0 1 0 1 0]"),
        // The Adobe segment only inverts CMYK.
        (3, true, "DeviceRGB", ""),
    ] {
        let bytes = tiny_jpeg(channels, adobe);
        let Ok(img) = picture::decode(&bytes) else {
            unreachable!("{channels} channels, adobe {adobe}: not decoded");
        };
        assert_eq!((img.width, img.height), (300, 200), "{channels} channels");
        assert_eq!(img.colour_space, space, "{channels} channels, adobe {adobe}");
        assert_eq!(img.extra, extra, "{channels} channels, adobe {adobe}");
        assert_eq!(img.filter, "DCTDecode");
        assert_eq!(img.data, bytes, "a JPEG is embedded as it is");
        assert!(img.alpha.is_none());
    }
    let Err(why) = picture::decode(&tiny_jpeg(2, false)) else {
        unreachable!("a 2-channel JPEG was accepted");
    };
    assert!(why.contains("2 colour channels"), "{why}");
}

#[test]
#[allow(clippy::unwrap_used)]
fn pictures_that_cannot_be_read_say_why_and_links_out_of_the_project_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("p");
    std::fs::create_dir_all(dir.join("pictures")).unwrap();
    std::fs::write(dir.join("pictures/in.png"), b"x").unwrap();
    std::fs::write(tmp.path().join("secret.png"), b"s").unwrap();
    assert_eq!(read_picture(&dir, "pictures/in.png"), Ok(Some(b"x".to_vec())));
    assert_eq!(read_picture(&dir, "pictures/gone.png"), Ok(None));
    assert!(read_picture(&dir, "../secret.png").is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(tmp.path().join("secret.png"), dir.join("pictures/link.png")).unwrap();
        assert!(read_picture(&dir, "pictures/link.png").unwrap_err().contains("outside"));
        let mut project = Project::new("x");
        project.pages = vec![Page::new("p1", "1", "Mains")];
        project.meta.logo = Some(wc_core::Logo {
            file: "pictures/link.png".into(),
            cover: None,
        });
        let pictures = read_pictures(&dir, &project);
        assert!(pictures.files.is_empty());
        let opts = Options {
            underlays: pictures.files,
            unreadable: pictures.unreadable,
            ..Default::default()
        };
        let (_, report) = export(&project, &BTreeMap::new(), &opts);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("cannot be read") && report.warnings[0].contains("outside"));
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn a_logo_keeps_its_proportions_in_its_box() {
    let (min, max) = fit_picture((Point::new(0.0, 0.0), Point::new(30.0, 10.0)), (100, 100));
    assert_eq!((min, max), (Point::new(10.0, 0.0), Point::new(20.0, 10.0)));
    let (min, max) = fit_picture((Point::new(0.0, 0.0), Point::new(10.0, 30.0)), (200, 100));
    assert_eq!((min, max), (Point::new(0.0, 12.5), Point::new(10.0, 17.5)));
    let page = Page::new("p", "1", "x");
    let (a, b) = title_logo_box(&Project::new("x"), &page).unwrap();
    assert!(a.x < b.x && a.y < b.y && b.x < 420.0 && b.y < 297.0);
    // A logo box narrowed below its padding still has room for the logo.
    let mut project = Project::new("x");
    let logo = project.meta.template.boxes.iter_mut().find(|b| b.logo).unwrap();
    logo.left = logo.right + 1.5;
    let (a, b) = title_logo_box(&project, &page).unwrap();
    assert!(a.x < b.x && a.y < b.y, "{a:?} {b:?}");
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
            rotation: wc_core::geom::Rotation::R0,
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
fn a_panel_layout_is_drawn_for_svg_and_dxf_with_its_plate_outline() {
    use wc_core::panel::Panel;
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    let opts = Options {
        panels: vec![Panel::with_rails("main", "Cabinet CAB1", 600.0, 800.0)],
        ..Default::default()
    };
    let (pages, report) = crate::page_lines(&project, &BTreeMap::new(), &opts);
    assert_eq!(report.pages, 2);
    let [page, panel] = pages.as_slice() else {
        unreachable!("the page and the panel: {}", pages.len())
    };
    assert!(!page.panel && panel.panel);
    assert_eq!((panel.id.as_str(), panel.number.as_str()), ("panel-main", "P1"));
    assert_eq!(panel.title, "Cabinet CAB1");
    // The 600 x 800 mm plate at 1:5 is a closed 120 x 160 mm outline.
    let plate = panel.prims.iter().find_map(|p| match p {
        Prim::Path {
            points,
            closed: true,
            filled: false,
            ..
        } => {
            let xs = points.iter().map(|p| p.0);
            let ys = points.iter().map(|p| p.1);
            let (x0, x1) = (xs.clone().fold(f64::MAX, f64::min), xs.fold(f64::MIN, f64::max));
            let (y0, y1) = (ys.clone().fold(f64::MAX, f64::min), ys.fold(f64::MIN, f64::max));
            ((x1 - x0 - 120.0).abs() < 1e-6 && (y1 - y0 - 160.0).abs() < 1e-6).then_some((x0, y1))
        }
        _ => None,
    });
    let Some((x0, y1)) = plate else {
        unreachable!("no plate outline")
    };
    // In the DXF (y up) the plate's bottom left corner is a polyline vertex.
    let dxf = crate::dxf(panel);
    let corner = format!(
        "VERTEX\n8\n0\n10\n{}\n20\n{}\n",
        crate::vector::num(x0),
        crate::vector::num(panel.size_mm.1 - y1)
    );
    assert!(dxf.contains(&corner), "{corner:?} not in the DXF");
    assert!(crate::svg(panel).contains("<title>P1 Cabinet CAB1</title>"));
    // Its file is named after the panel, the same in the app and on the command line.
    assert_eq!(crate::panel_file_stem(panel), "panel-Cabinet_CAB1");
    let mut named = panel.clone();
    named.title = "Main.v1".into();
    assert_eq!(crate::panel_file_stem(&named), "panel-Main_v1");
    named.title = " ".into();
    assert_eq!(crate::panel_file_stem(&named), "panel-main");
    // Without panels in the options, only the page is drawn.
    let (pages, _) = crate::page_lines(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(pages.len(), 1);
}

#[test]
fn devices_with_the_same_product_part_are_labelled_with_their_full_tags() {
    use wc_core::panel::{Panel, PanelItem};
    let mut panel = Panel::new("main", "Cabinet", 600.0, 800.0);
    for (i, tag) in ["+CAB1-K1", "+CAB2-K1", "+CAB1-K2"].into_iter().enumerate() {
        let Ok(tag) = DeviceTag::parse(tag) else {
            unreachable!("a valid tag")
        };
        panel.items.push(PanelItem::Device {
            tag,
            at: Point::new(50.0 + 100.0 * i as f64, 100.0),
            width: 45.0,
            height: 85.0,
            rotation: wc_core::geom::Rotation::R0,
        });
    }
    let project = Project::new("x");
    let page = Page::new("panel-main", "P1", "Cabinet");
    let drawn = layout::draw_panel_page(&project, &page, 0, 1, &panel, &BTreeMap::new(), "");
    // The two `-K1` devices print their full tags, so they can be told apart.
    assert!(drawn.ops.contains(&writer::pdf_text("+CAB1-K1")));
    assert!(drawn.ops.contains(&writer::pdf_text("+CAB2-K1")));
    // A device whose product part is unique keeps the short label.
    assert!(drawn.ops.contains(&writer::pdf_text("-K2")));
    assert!(!drawn.ops.contains(&writer::pdf_text("+CAB1-K2")));
}

#[test]
fn a_device_with_a_footprint_drawing_is_drawn_with_it_on_the_layout() {
    use wc_core::panel::{Panel, PanelItem};
    let Ok(tag) = DeviceTag::parse("+CAB1-A1") else {
        unreachable!("a valid tag")
    };
    let mut project = Project::new("x");
    project.devices.insert(
        tag.clone(),
        wc_core::Device {
            part: Some("maker.io-module".into()),
            ..Default::default()
        },
    );
    let mut panel = Panel::new("main", "Cabinet", 600.0, 800.0);
    panel.items.push(PanelItem::Device {
        tag,
        at: Point::new(50.0, 100.0),
        width: 22.5,
        height: 100.0,
        rotation: wc_core::geom::Rotation::R0,
    });
    let Some(footprint) = wc_core::Footprint::from_shapes(vec![
        Shape::Rect {
            min: Point::new(0.0, 0.0),
            max: Point::new(22.5, 100.0),
        },
        Shape::Text {
            at: Point::new(2.0, 10.0),
            text: "MAKER IO".into(),
            height: 3.0,
            align: wc_core::geom::Align::Left,
            angle: 0.0,
        },
    ]) else {
        unreachable!("the outline has a size")
    };
    let part = wc_core::Part {
        id: "maker.io-module".into(),
        footprint: Some(footprint),
        ..Default::default()
    };
    let page = Page::new("panel-main", "P1", "Cabinet");
    let draw = |footprints: &BTreeMap<String, wc_core::Footprint>| {
        layout::draw_panel_page(&project, &page, 0, 1, &panel, footprints, "")
    };
    let plain = draw(&BTreeMap::new());
    let drawn = draw(&footprints([&part]));
    let label = writer::pdf_text("MAKER IO");
    assert!(!plain.ops.contains(&label), "without the drawing the device is a box");
    assert!(drawn.ops.contains(&label), "the maker's drawing is on the layout");
    assert!(drawn.ops.contains(&writer::pdf_text("-A1")), "the tag is still printed");
    // A part without a drawing, and a device without a part, stay boxes.
    let other = wc_core::Part {
        id: "other".into(),
        ..Default::default()
    };
    assert!(footprints([&other]).is_empty());
}

#[test]
fn a_turned_device_is_drawn_turned_with_its_footprint() {
    use wc_core::geom::Rotation;
    use wc_core::panel::{Panel, PanelItem};
    let Ok(tag) = DeviceTag::parse("+CAB1-G1") else {
        unreachable!("a valid tag")
    };
    let mut project = Project::new("x");
    project.devices.insert(
        tag.clone(),
        wc_core::Device {
            part: Some("maker.psu".into()),
            ..Default::default()
        },
    );
    // The maker's outline: the part's frame and a line along its top edge, 22.5 mm long.
    let Some(footprint) = wc_core::Footprint::from_shapes(vec![
        Shape::Rect {
            min: Point::new(0.0, 0.0),
            max: Point::new(22.5, 100.0),
        },
        Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(22.5, 0.0),
        },
    ]) else {
        unreachable!("the outline has a size")
    };
    let part = wc_core::Part {
        id: "maker.psu".into(),
        footprint: Some(footprint),
        ..Default::default()
    };
    let page = Page::new("panel-main", "P1", "Cabinet");
    let draw = |rotation: Rotation| {
        let mut panel = Panel::new("main", "Cabinet", 600.0, 800.0);
        panel.items.push(PanelItem::Device {
            tag: tag.clone(),
            at: Point::new(50.0, 100.0),
            width: 22.5,
            height: 100.0,
            rotation,
        });
        layout::draw_panel_page(&project, &page, 0, 1, &panel, &footprints([&part]), "")
    };
    // Open 2-point paths (the top-edge line) and closed 4-point ones (boxes), as their size on
    // the sheet in mm (width, height).
    let sizes = |d: &writer::Drawn, closed: bool| -> Vec<(f64, f64)> {
        d.prims
            .iter()
            .filter_map(|p| match p {
                writer::Prim::Path { points, closed: c, .. }
                    if *c == closed && points.len() == if closed { 4 } else { 2 } =>
                {
                    let xs = points.iter().map(|q| q.0);
                    let ys = points.iter().map(|q| q.1);
                    let w = xs.clone().fold(f64::MIN, f64::max) - xs.fold(f64::MAX, f64::min);
                    let h = ys.clone().fold(f64::MIN, f64::max) - ys.fold(f64::MAX, f64::min);
                    Some((w, h))
                }
                _ => None,
            })
            .collect()
    };
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01;
    // The plate is drawn at 1:5: 22.5 mm is 4.5 mm on the sheet, 100 mm is 20 mm.
    let upright = draw(Rotation::R0);
    assert!(
        sizes(&upright, false).iter().any(|&s| near(s, (4.5, 0.0))),
        "the top edge runs across"
    );
    assert!(
        sizes(&upright, true).iter().any(|&s| near(s, (4.5, 20.0))),
        "the box stands upright"
    );
    let turned = draw(Rotation::R90);
    assert!(
        sizes(&turned, false).iter().any(|&s| near(s, (0.0, 4.5))),
        "the top edge runs down"
    );
    assert!(!sizes(&turned, false).iter().any(|&s| near(s, (4.5, 0.0))));
    assert!(
        sizes(&turned, true).iter().any(|&s| near(s, (20.0, 4.5))),
        "the box lies on its side"
    );
    assert!(!sizes(&turned, true).iter().any(|&s| near(s, (4.5, 20.0))));
    assert!(
        turned.ops.contains(&writer::pdf_text("-G1")),
        "the tag is still printed"
    );
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
fn a_coil_does_not_list_a_contact_repeated_on_an_overview_page() {
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_owned(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    symbols.insert("no".to_owned(), sym("no", SymbolRole::Auxiliary, ["13", "14"]));
    let mut project = wc_core::Project::new("x");
    let mut overview = Page::new("p1", "1", "Line diagram");
    overview.overview = true;
    overview.symbols.push(place("s1", "no", 50.0, "-K1"));
    let mut schematic = Page::new("p2", "2", "Control");
    schematic.symbols.push(place("s2", "coil", 50.0, "-K1"));
    schematic.symbols.push(place("s3", "no", 150.0, "-K1"));
    project.pages = vec![overview, schematic];
    let x = layout::CrossRefs::build(&project, &symbols);
    let Some(page) = project.pages.get(1) else {
        unreachable!("two pages")
    };
    let mut warnings = Vec::new();
    let marks = layout::xref_marks(page, &symbols, &x, &mut warnings);
    let listed: Vec<&str> = marks
        .iter()
        .filter(|m| m.text.starts_with("13-14"))
        .map(|m| m.target_page_id.as_str())
        .collect();
    assert_eq!(listed, ["p2"], "the contact is listed once, on its schematic page");
    // The repeat on the overview still points back to its coil.
    let Some(overview) = project.pages.first() else {
        unreachable!("two pages")
    };
    let back = layout::xref_marks(overview, &symbols, &x, &mut warnings);
    assert!(back.iter().any(|m| m.target_page_id == "p2"), "{back:?}");
    assert!(warnings.is_empty(), "{warnings:?}");
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
fn a_short_cover_keeps_its_field_table_above_the_title_block() -> Result<(), String> {
    let mut project = wc_core::Project::new("Plant");
    for i in 0..12 {
        project
            .meta
            .title_block
            .insert(format!("Extra {i}"), format!("value {i}"));
    }
    let mut cover = Page::new("p1", "1", "Cover");
    cover.cover = true;
    cover.size = PageSize::custom(297, 100)?;
    let (_, h) = cover.size.mm();
    let ty = h - wc_core::project::FRAME_MARGIN - project.meta.template.band_height();
    let marks = layout::frame_marks(&project, &cover, 0, 1, "2026-11-17");
    let texts: Vec<(&str, f64, f64)> = marks
        .iter()
        .filter_map(|m| match &m.shape {
            Shape::Text { text, at, height, .. } => Some((text.as_str(), at.y, *height)),
            _ => None,
        })
        .collect();
    let row = |t: &str| texts.iter().find(|(x, ..)| *x == t).copied();
    // The rows that are shown sit above the title block, at a readable size.
    let (_, y, size) = row("value 0").ok_or(format!("the first field is not shown: {texts:?}"))?;
    assert!(y < ty && size >= 1.2, "{y} {size} {ty}");
    // The rest are counted, not drawn into the title block.
    let (_, more_y, _) = texts
        .iter()
        .find(|(t, ..)| t.ends_with(" more"))
        .copied()
        .ok_or(format!("no count of fields left out: {texts:?}"))?;
    assert!(more_y < ty, "{more_y} {ty}");
    assert!(row("value 11").is_none(), "{texts:?}");
    // Every rule of the table, picked by its ends, stays above the title block.
    let (x0, x1) = (wc_core::project::FRAME_MARGIN, 297.0 - wc_core::project::FRAME_MARGIN);
    let (tx0, tx1) = ((x0 + x1) / 2.0 - 90.0, (x0 + x1) / 2.0 + 90.0);
    let rules: Vec<f64> = marks
        .iter()
        .filter_map(|m| match &m.shape {
            Shape::Line { a, b }
                if (a.x - tx0).abs() < 1e-6 && (b.x - tx1).abs() < 1e-6 && (a.y - b.y).abs() < 1e-9 =>
            {
                Some(a.y)
            }
            _ => None,
        })
        .collect();
    assert!(!rules.is_empty(), "no table rules");
    for y in &rules {
        assert!(*y <= ty - 1.0, "rule at {y}, title block at {ty}");
    }
    // A tall sheet keeps today's layout: every field is listed.
    cover.size = PageSize::A3;
    let marks = layout::frame_marks(&project, &cover, 0, 1, "2026-11-17");
    assert!(
        marks
            .iter()
            .any(|m| matches!(&m.shape, Shape::Text { text, .. } if text == "value 11"))
    );
    Ok(())
}

#[test]
fn a_short_cover_moves_the_name_up_and_keeps_the_default_logo_above_it() -> Result<(), String> {
    let mut project = wc_core::Project::new("Plant");
    for i in 0..12 {
        project
            .meta
            .title_block
            .insert(format!("Extra {i}"), format!("value {i}"));
    }
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: None,
    });
    let mut cover = Page::new("p1", "1", "Cover");
    cover.cover = true;
    cover.size = PageSize::custom(297, 170)?;
    let (_, h) = cover.size.mm();
    let y0 = wc_core::project::FRAME_MARGIN + 6.0;
    let ty = h - wc_core::project::FRAME_MARGIN - project.meta.template.band_height();
    let marks = layout::frame_marks(&project, &cover, 0, 1, "2026-11-17");
    let (name_y, name_h) = marks
        .iter()
        .find_map(|m| match &m.shape {
            Shape::Text { text, at, height, .. } if text == "Plant" && at.y < ty => Some((at.y, *height)),
            _ => None,
        })
        .ok_or("no project name on the cover")?;
    // A fifth of the way down leaves too little room for the table here: the name moves up.
    assert!(name_y < y0 + (ty - y0) * 0.2 - 1.0, "name at {name_y}");
    // The default logo box still ends above the name's letters.
    let (a, b) = layout::cover_logo_default(&project, &cover);
    assert!(a.y >= y0 && b.y - a.y >= 5.0, "{a:?} {b:?}");
    assert!(b.y <= name_y - name_h, "logo to {}, name from {}", b.y, name_y - name_h);
    // A sheet so short that a fifth of the way down is already near the top keeps the name
    // low enough for the logo too.
    cover.size = PageSize::custom(297, 100)?;
    let marks = layout::frame_marks(&project, &cover, 0, 1, "2026-11-17");
    let ty = cover.size.mm().1 - wc_core::project::FRAME_MARGIN - project.meta.template.band_height();
    let (name_y, name_h) = marks
        .iter()
        .find_map(|m| match &m.shape {
            Shape::Text { text, at, height, .. } if text == "Plant" && at.y < ty => Some((at.y, *height)),
            _ => None,
        })
        .ok_or("no project name on the short cover")?;
    let (_, b) = layout::cover_logo_default(&project, &cover);
    assert!(b.y <= name_y - name_h, "logo to {}, name from {}", b.y, name_y - name_h);
    Ok(())
}

#[test]
fn a_cover_too_short_for_any_field_is_a_warning() -> Result<(), String> {
    let mut project = Project::new("x");
    let mut cover = Page::new("p1", "1", "Cover");
    cover.cover = true;
    cover.size = PageSize::custom(297, 100)?;
    project.meta.template.height = wc_core::template::MAX_TITLE_HEIGHT;
    project.meta.title_block.insert("customer".into(), "Acme".into());
    project.pages = vec![cover];
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(
        report.warnings,
        ["page 1: the cover page is too short to list the project fields"]
    );
    // On a sheet with room for the table, no warning.
    project.pages[0].size = PageSize::A4;
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    Ok(())
}

#[test]
fn only_a_logo_in_its_default_place_takes_room_from_the_cover_table() -> Result<(), String> {
    let mut project = Project::new("x");
    let mut cover = Page::new("p1", "1", "Cover");
    cover.cover = true;
    cover.size = PageSize::custom(297, 100)?;
    project.meta.template.height = 30.0;
    project.meta.title_block.insert("customer".into(), "Acme".into());
    project.pages = vec![cover];
    let mut logo = wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: None,
    };
    project.meta.logo = Some(logo.clone());
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert!(
        report.warnings.iter().any(|w| w.contains("too short")),
        "{:?}",
        report.warnings
    );
    // Moved off to the side, the logo leaves the name where it is without one.
    logo.cover = Some((wc_core::Point::new(200.0, 20.0), wc_core::Point::new(280.0, 40.0)));
    project.meta.logo = Some(logo);
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert!(
        !report.warnings.iter().any(|w| w.contains("too short")),
        "{:?}",
        report.warnings
    );
    let page = &project.pages[0];
    let marks = layout::frame_marks(&project, page, 0, 1, "2026-11-17");
    let ty = page.size.mm().1 - wc_core::project::FRAME_MARGIN - project.meta.template.band_height();
    assert!(
        marks
            .iter()
            .any(|m| matches!(&m.shape, Shape::Text { text, at, .. } if text == "Acme" && at.y < ty)),
        "the customer field is not on the cover"
    );
    Ok(())
}

#[test]
fn a_cover_too_short_for_its_description_leaves_it_out_of_the_title_block() -> Result<(), String> {
    let mut project = Project::new("x");
    let mut cover = Page::new("p1", "1", "Cover");
    cover.cover = true;
    cover.size = PageSize::custom(297, 100)?;
    project
        .meta
        .title_block
        .insert("description".into(), "Pump station".into());
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: None,
    });
    let cx = cover.size.mm().0 / 2.0;
    let descriptions = |project: &Project| -> Vec<f64> {
        layout::frame_marks(project, &cover, 0, 1, "2026-11-17")
            .iter()
            .filter_map(|m| match &m.shape {
                Shape::Text { text, at, .. } if text == "Pump station" && (at.x - cx).abs() < 0.01 => Some(at.y),
                _ => None,
            })
            .collect()
    };
    // Title bands in 0.1 mm steps, so one puts the baseline on the rule and others within the
    // room the letters reach below it: the description stays clear of the rule, or is left out.
    let mut left_out = false;
    for tenths in 300..=600 {
        project.meta.template.height = f64::from(tenths) / 10.0;
        let ty = cover.size.mm().1 - wc_core::project::FRAME_MARGIN - project.meta.template.band_height();
        let found = descriptions(&project);
        left_out |= found.is_empty();
        assert!(
            found.iter().all(|y| *y + 0.8 <= ty + 1e-9),
            "band {}: description at {found:?}, title block from {ty}",
            project.meta.template.height
        );
    }
    assert!(left_out, "no band left the description out");
    // Where it fits, it is there.
    project.meta.template.height = 30.0;
    assert_eq!(descriptions(&project).len(), 1);
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

#[test]
fn a_pages_own_title_block_fields_win_over_the_projects_on_that_page() {
    // Values unlike the frame's row letters.
    let mut project = wc_core::Project::new("Pump station");
    project.meta.title_block.insert("revision".into(), "Rev-A".into());
    project
        .meta
        .title_block
        .insert("checked_by".into(), "Project checker".into());
    let mut own = Page::new("p1", "1", "Motors");
    own.title_block.insert("revision".into(), "Rev-C".into());
    // A blank page value does not hide the project's.
    own.title_block.insert("checked_by".into(), " ".into());
    let plain = Page::new("p2", "2", "Supply");
    let texts = |page: &Page| -> Vec<String> {
        layout::frame_marks(&project, page, 0, 2, "2026-11-17")
            .into_iter()
            .filter_map(|m| match m.shape {
                Shape::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    };
    let on_own = texts(&own);
    assert!(on_own.contains(&"Rev-C".to_owned()), "{on_own:?}");
    assert!(!on_own.contains(&"Rev-A".to_owned()), "{on_own:?}");
    assert!(on_own.contains(&"Project checker".to_owned()), "{on_own:?}");
    let on_plain = texts(&plain);
    assert!(on_plain.contains(&"Rev-A".to_owned()), "{on_plain:?}");
    assert!(!on_plain.contains(&"Rev-C".to_owned()), "{on_plain:?}");
}

#[test]
fn long_title_block_values_stay_inside_their_boxes() {
    let mut project = wc_core::Project::new("Pump station upgrade and refurbishment ".repeat(4));
    for key in [
        "customer",
        "drawn_by",
        "checked_by",
        "project_number",
        "plant",
        "revision",
        "date",
        "location",
    ] {
        project.meta.title_block.insert(key.into(), "W".repeat(60));
    }
    let mut page = Page::new("p1", "8".repeat(30), "Motor starters and field wiring ".repeat(4));
    for page_size in [wc_core::PageSize::A4, wc_core::PageSize::A3] {
        page.size = page_size;
        let marks = layout::frame_marks(&project, &page, 0, 1, "2026-11-17");
        // The title block's dividers: every vertical line, with the band of y it covers.
        let dividers: Vec<(f64, f64, f64)> = marks
            .iter()
            .filter_map(|m| match &m.shape {
                Shape::Line { a, b } if (a.x - b.x).abs() < 1e-9 => Some((a.x, a.y.min(b.y), a.y.max(b.y))),
                _ => None,
            })
            .collect();
        let (w, h) = page.size.mm();
        let (x0, x1, y1) = (
            wc_core::project::FRAME_MARGIN,
            w - wc_core::project::FRAME_MARGIN,
            h - wc_core::project::FRAME_MARGIN,
        );
        let mut checked = 0;
        for m in &marks {
            let Shape::Text {
                at,
                text,
                height,
                align,
                ..
            } = &m.shape
            else {
                continue;
            };
            // Row letters stand in the margin outside the frame.
            if at.y < y1 - 24.0 || at.y > y1 || at.x < x0 || at.x > x1 {
                continue;
            }
            let width = crate::writer::text_width_mm(text, *height);
            let (left, right) = match align {
                wc_core::geom::Align::Left => (at.x, at.x + width),
                wc_core::geom::Align::Right => (at.x - width, at.x),
                _ => (at.x - width / 2.0, at.x + width / 2.0),
            };
            assert!(
                left >= x0 && right <= x1,
                "{text:?} runs over the frame: {left}..{right}"
            );
            // The text's middle height must not cross a divider standing at that height.
            let y = at.y - height / 2.0;
            for (x, top, bottom) in &dividers {
                if y > *top && y < *bottom {
                    assert!(
                        !(left < *x - 1e-6 && right > *x + 1e-6),
                        "{text:?} ({left}..{right}) crosses the divider at x = {x}"
                    );
                }
            }
            checked += 1;
        }
        assert!(checked >= 15, "only {checked} title-block texts");
    }
}

#[test]
fn short_title_block_values_keep_their_height() {
    let mut project = wc_core::Project::new("Pump station");
    project.meta.title_block.insert("customer".into(), "ACME".into());
    project.meta.title_block.insert("revision".into(), "Rev-B".into());
    let page = Page::new("p1", "1", "Motors");
    let marks = layout::frame_marks(&project, &page, 0, 1, "2026-11-17");
    let height_of = |wanted: &str| {
        marks.iter().find_map(|m| match &m.shape {
            Shape::Text { text, height, .. } if text == wanted => Some(*height),
            _ => None,
        })
    };
    assert_eq!(height_of("ACME"), Some(3.0));
    // Not "B": that is also a frame row letter, drawn at 2.5 mm too.
    assert_eq!(height_of("Rev-B"), Some(2.5));
    assert_eq!(height_of("Pump station"), Some(4.0));
    assert_eq!(height_of("Motors"), Some(3.0));
    assert_eq!(height_of("1 / 1"), Some(3.0));
}

#[test]
fn fractions_are_measured_at_their_helvetica_width() -> Result<(), String> {
    // Helvetica's AFM gives ¼ ½ ¾ 834 units; a 1000-unit em is the text height / 0.72.
    let mut project = wc_core::Project::new("x");
    let value = "\u{bd}".repeat(60);
    project.meta.title_block.insert("customer".into(), value.clone());
    let marks = layout::frame_marks(&project, &Page::new("p1", "1", ""), 0, 1, "2026-11-17");
    let height = marks
        .iter()
        .find_map(|m| match &m.shape {
            Shape::Text { text, height, .. } if *text == value => Some(*height),
            _ => None,
        })
        .ok_or("the customer value is not drawn")?;
    let rendered = 60.0 * 0.834 * height / 0.72;
    // The customer box is 80 mm wide, less 1.5 mm padding each side.
    assert!(rendered <= 77.0 + 1e-6, "{rendered} mm");
    Ok(())
}

#[test]
fn base64_matches_the_standard_alphabet_and_padding() {
    assert_eq!(vector::base64(b""), "");
    assert_eq!(vector::base64(b"f"), "Zg==");
    assert_eq!(vector::base64(b"fo"), "Zm8=");
    assert_eq!(vector::base64(b"foo"), "Zm9v");
    assert_eq!(vector::base64(&[0xFB, 0xFF, 0xBF]), "+/+/");
}

/// The `<image>` elements of an SVG, as (x, y, width, height, href).
fn svg_images(s: &str) -> Vec<(f64, f64, f64, f64, String)> {
    let attr = |e: &str, name: &str| -> String {
        let key = format!(" {name}=\"");
        e.find(&key)
            .map(|i| &e[i + key.len()..])
            .and_then(|r| r.split('"').next())
            .unwrap_or_default()
            .to_owned()
    };
    let f = |e: &str, name: &str| attr(e, name).parse::<f64>().unwrap_or(f64::NAN);
    s.split("<image")
        .skip(1)
        .map(|e| {
            let e = e.split("/>").next().unwrap_or_default();
            (f(e, "x"), f(e, "y"), f(e, "width"), f(e, "height"), attr(e, "href"))
        })
        .collect()
}

/// Fails unless `s` parses as XML with every element closed.
fn assert_well_formed(s: &str) {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(s);
    let mut depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => depth -= 1,
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => unreachable!("not XML at {}: {e}\n{s}", reader.buffer_position()),
        }
    }
    assert_eq!(depth, 0, "unclosed elements:\n{s}");
}

#[test]
fn svg_drawings_show_the_logo_and_the_underlay() {
    let mut png = Vec::new();
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 2, image::Rgb([200, 30, 30])));
    assert!(
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
    );
    let mut project = Project::new("x");
    let mut cover = Page::new("p0", "0", "Cover");
    cover.cover = true;
    let mut scanned = Page::new("p1", "1", "Scanned");
    scanned.underlay = Some(wc_core::Underlay {
        file: "underlays/p1.png".into(),
        min: Point::new(420.0, 297.0),
        max: Point::new(0.0, 0.0),
    });
    scanned.graphics.push(Shape::Line {
        a: Point::new(10.0, 10.0),
        b: Point::new(20.0, 10.0),
    });
    project.pages = vec![cover, scanned];
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: Some((Point::new(100.0, 40.0), Point::new(140.0, 60.0))),
    });
    let mut underlays = BTreeMap::new();
    underlays.insert("pictures/logo.png".to_string(), png.clone());
    underlays.insert("underlays/p1.png".to_string(), png.clone());
    let opts = Options {
        underlays,
        ..Default::default()
    };
    let (pages, report) = page_drawings(&project, &BTreeMap::new(), &opts);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let href = format!("data:image/png;base64,{}", vector::base64(&png));

    // The cover: the title block's logo and the cover's own, each with the picture's 2:1 shape.
    let cover = svg(&pages[0]);
    let images = svg_images(&cover);
    assert_eq!(images.len(), 2, "{cover}");
    for (_, _, w, h, link) in &images {
        assert!((w / h - 2.0).abs() < 1e-3, "{w} x {h}");
        assert_eq!(link, &href);
    }
    assert!(images.iter().any(|i| (i.0, i.1, i.2, i.3) == (100.0, 40.0, 40.0, 20.0)));
    // The logo is drawn last, on top of the drawing.
    assert!(cover.find("<image").unwrap_or(0) > cover.find("</g>").unwrap_or(usize::MAX));

    // The scanned page: the underlay over its rectangle, under the drawing, then the logo.
    let page = svg(&pages[1]);
    let images = svg_images(&page);
    assert_eq!(images.len(), 2, "{page}");
    assert_eq!(
        (images[0].0, images[0].1, images[0].2, images[0].3),
        (0.0, 0.0, 420.0, 297.0)
    );
    // The underlay sits in its own "scans" layer, before the drawing's group.
    let scans = "<g id=\"scans\" inkscape:groupmode=\"layer\" inkscape:label=\"Scanned sheets\">\n<image";
    assert!(page.contains(scans), "{page}");
    assert!(page.find(scans).unwrap_or(usize::MAX) < page.find("<g fill").unwrap_or(0));
    assert_well_formed(&page);
    assert!(page.rfind("<image").unwrap_or(0) > page.find("</g>").unwrap_or(usize::MAX));
    // DXF has no pictures and stays as it was.
    assert!(!dxf(&pages[1]).contains("base64"));
}

#[test]
fn svg_drawings_leave_out_a_missing_or_damaged_picture() {
    let mut project = Project::new("x");
    let mut p1 = Page::new("p1", "1", "Scanned");
    p1.underlay = Some(wc_core::Underlay {
        file: "underlays/p1.jpg".into(),
        min: Point::new(0.0, 0.0),
        max: Point::new(10.0, 10.0),
    });
    project.pages = vec![p1];
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/gone.png".into(),
        cover: None,
    });
    let mut underlays = BTreeMap::new();
    underlays.insert("underlays/p1.jpg".to_string(), vec![0xFF, 0xD8, 0xFF]);
    let opts = Options {
        underlays,
        ..Default::default()
    };
    let (pages, report) = page_drawings(&project, &BTreeMap::new(), &opts);
    assert_eq!(pages.len(), 1);
    assert!(!svg(&pages[0]).contains("<image"));
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    assert!(
        report
            .warnings
            .contains(&"logo picture pictures/gone.png is missing".to_string())
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.starts_with("page 1: underlay underlays/p1.jpg") && w.contains("damaged"))
    );
}

#[test]
fn page_lines_say_which_pictures_they_leave_out() {
    let mut project = Project::new("Lines");
    let mut p1 = Page::new("p1", "1", "One");
    p1.underlay = Some(wc_core::Underlay {
        file: "underlays/p1.png".into(),
        min: Point::new(0.0, 0.0),
        max: Point::new(10.0, 10.0),
    });
    project.pages = vec![p1];
    project.meta.logo = Some(wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: None,
    });
    // No picture bytes are given: a DXF export does not read them, so none is called missing,
    // but the user is told the DXF has neither the underlay nor the logo.
    let (pages, report) = crate::page_lines(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(pages.len(), 1);
    assert!(pages[0].pictures.is_empty());
    assert_eq!(
        report.warnings,
        vec![
            "page 1: the underlay underlays/p1.png is left out: DXF has no pictures (the SVG export shows it)"
                .to_string(),
            "the logo pictures/logo.png is left out: DXF has no pictures (the SVG export shows it)".to_string(),
        ]
    );
    // Without a logo or an underlay, nothing is said.
    let mut plain = project.clone();
    plain.meta.logo = None;
    plain.pages[0].underlay = None;
    let (_, quiet) = crate::page_lines(&plain, &BTreeMap::new(), &Options::default());
    assert!(quiet.warnings.is_empty(), "{:?}", quiet.warnings);
    let (_, svg_report) = page_drawings(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(svg_report.warnings.len(), 2, "{:?}", svg_report.warnings);
}

#[test]
fn page_lines_say_which_texts_lose_a_character_beyond_u_ffff() {
    let mut project = Project::new("Wide");
    let mut p1 = Page::new("p1", "1", "One");
    for text in ["Pump \u{1F600}", "Pump \u{1F600}", "Ålesund"] {
        p1.graphics.push(Shape::Text {
            at: Point::new(50.0, 50.0),
            text: text.into(),
            height: 3.0,
            align: Default::default(),
            angle: 0.0,
        });
    }
    project.pages = vec![p1];
    let (pages, report) = crate::page_lines(&project, &BTreeMap::new(), &Options::default());
    assert!(dxf(&pages[0]).contains("\nPump ?\n"));
    // Named once, however often the text is on the page; Å has a code and is not named.
    assert_eq!(
        report.warnings,
        vec![
            "page 1: the text \"Pump \u{1F600}\" is written as \"Pump ?\" in the DXF: a DXF has no code for \
             characters beyond U+FFFF, such as emoji (the SVG export keeps them)"
                .to_string()
        ]
    );
    // For a log, the same warning without the text.
    assert_eq!(
        dxf_warnings(&pages[0], false),
        vec![
            "page 1: a text is written with \"?\" in the DXF: a DXF has no code for characters beyond \
             U+FFFF, such as emoji (the SVG export keeps them)"
                .to_string()
        ]
    );
    // The SVG export keeps the character, so it says nothing.
    let (_, svg_report) = page_drawings(&project, &BTreeMap::new(), &Options::default());
    assert!(svg_report.warnings.is_empty(), "{:?}", svg_report.warnings);
}

#[test]
fn portrait_large_and_custom_sheets_export_at_their_size() {
    let mut project = Project::new("Sizes");
    let sizes = [
        PageSize::A4Portrait,
        PageSize::A1,
        PageSize::A0Portrait,
        PageSize::Custom {
            width: 500,
            height: 700,
        },
    ];
    project.pages = sizes
        .iter()
        .enumerate()
        .map(|(i, size)| {
            let mut p = Page::new(format!("p{i}"), (i + 1).to_string(), "Sheet");
            p.size = *size;
            p.symbols.push(place("s1", "coil", 50.0, "-K1"));
            p
        })
        .collect();
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_string(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    let (pdf, report) = export(&project, &symbols, &Options::default());
    assert_eq!(report.pages, sizes.len());
    let text = String::from_utf8_lossy(&pdf);
    let pt = |mm: f64| mm / 25.4 * 72.0;
    for size in sizes {
        let (w, h) = size.mm();
        let media = format!("/MediaBox [0 0 {:.2} {:.2}]", pt(w), pt(h));
        assert!(text.contains(&media), "{size:?}: {media}");
    }
    let (pages, _) = page_drawings(&project, &symbols, &Options::default());
    assert_eq!(pages.len(), sizes.len(), "every sheet has its drawing");
    for (page, size) in pages.iter().zip(sizes) {
        assert_eq!(page.size_mm, size.mm());
        let (w, h) = size.mm();
        assert!(svg(page).contains(&format!("viewBox=\"0 0 {w} {h}\"")), "{size:?}");
    }
}

#[test]
fn report_pages_move_to_a_larger_sheet_when_the_first_page_is_too_short() {
    let short = PageSize::Custom {
        width: 300,
        height: 100,
    };
    // Room for a few rows under the usual title block: the report keeps the sheet.
    assert_eq!(layout::report_size(short, 30.0, 1), short);
    // A 60 mm title block leaves no row on it: the smallest ISO sheet with room is used.
    assert_eq!(layout::report_size(short, 60.0, 1), PageSize::A4);
    assert_eq!(layout::report_size(PageSize::A3, 60.0, 1), PageSize::A3);
}

#[test]
fn an_upright_a1_page_prints_on_an_upright_a1_sheet() {
    let mut project = Project::new("x");
    let mut page = Page::new("p1", "1", "Sheet");
    page.size = PageSize::A1Portrait;
    page.symbols.push(place("s1", "coil", 50.0, "-K1"));
    project.pages = vec![page];
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_string(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    let (pdf, _) = export(&project, &symbols, &Options::default());
    let text = String::from_utf8_lossy(&pdf);
    assert!(
        text.contains("/MediaBox [0 0 1683.78 2383.94]"),
        "upright A1 is 594 x 841 mm"
    );
}

#[test]
fn a_report_after_a_short_first_page_prints_on_the_larger_sheet() {
    let mut project = Project::new("x");
    project.meta.template.height = 60.0;
    let mut page = Page::new("p1", "1", "Strip");
    page.size = PageSize::Custom {
        width: 300,
        height: 100,
    };
    project.pages = vec![page];
    let opts = Options {
        reports: vec![wc_report::Table {
            title: "Parts list".into(),
            columns: vec!["Part".into()],
            rows: vec![vec!["Relay".into()]],
            ..Default::default()
        }],
        panels: vec![wc_core::panel::Panel::with_rails("main", "Cabinet", 600.0, 800.0)],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    assert_eq!(report.pages, 3);
    let text = String::from_utf8_lossy(&pdf);
    let want = layout::report_size(project.pages[0].size, project.meta.template.band_height(), 1);
    assert_eq!(want, PageSize::A4);
    let pt = |mm: f64| mm / 25.4 * 72.0;
    let media = |(w, h): (f64, f64)| format!("[0 0 {:.2} {:.2}]", pt(w), pt(h));
    // The sheets in page order: the schematic page and the panel layout keep the page's
    // sheet; the report after them is on A4.
    let boxes: Vec<&str> = text
        .match_indices("/MediaBox ")
        .map(|(i, _)| {
            let rest = &text[i + "/MediaBox ".len()..];
            &rest[..rest.find(']').map_or(0, |e| e + 1)]
        })
        .collect();
    let strip = media(project.pages[0].size.mm());
    assert_eq!(boxes, [strip.as_str(), strip.as_str(), media(want.mm()).as_str()]);
}

#[test]
fn revision_clouds_and_the_revision_table_are_drawn() -> Result<(), String> {
    let mut project = Project::new("Revisions");
    project.pages = vec![Page::new("p1", "1", "Power")];
    project
        .add_revision("A", "2026-11-01", "First issue", "OP")
        .map_err(|e| e.to_string())?;
    project
        .add_revision("B", "2026-11-18", "Motor added", "OP")
        .map_err(|e| e.to_string())?;
    let plain = String::from_utf8_lossy(&export(&project, &BTreeMap::new(), &Options::default()).0).into_owned();
    project
        .add_cloud("p1", Point::new(50.0, 50.0), Point::new(110.0, 80.0), None)
        .map_err(|e| e.to_string())?;
    // Without the table, only the cloud's tag can print its revision letter.
    let tag = format!("{} Tj", writer::pdf_text("B"));
    let cloud_only = String::from_utf8_lossy(&export(&project, &BTreeMap::new(), &Options::default()).0).into_owned();
    assert_eq!(
        cloud_only.matches(&tag).count(),
        plain.matches(&tag).count() + 1,
        "the cloud's tag prints B"
    );
    project
        .set_revision_table("p1", Some(Point::new(200.0, 30.0)))
        .map_err(|e| e.to_string())?;
    let (bytes, report) = export(&project, &BTreeMap::new(), &Options::default());
    let pdf = String::from_utf8_lossy(&bytes);
    for text in ["Rev", "Description", "First issue", "Motor added", "2026-11-18"] {
        assert!(
            pdf.contains(&format!("{} Tj", writer::pdf_text(text))),
            "{text} is printed"
        );
    }
    // The cloud's scallops add curves.
    assert!(pdf.matches(" c\n").count() > plain.matches(" c\n").count() + 20);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    // A letter saved by hand that the PDF cannot print is reported for the cloud and the table.
    project.meta.revisions[1].letter = "Ж".into();
    project.pages[0].clouds[0].revision = "Ж".into();
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|w| w.contains("cannot print revision Ж"))
            .count(),
        2,
        "{:?}",
        report.warnings
    );
    project.meta.revisions[1].letter = "B".into();
    project.pages[0].clouds[0].revision = "B".into();

    // A cloud of a revision the project no longer lists is reported.
    project.meta.revisions.pop();
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert!(
        report.warnings.iter().any(|w| w.contains("revision B")),
        "{:?}",
        report.warnings
    );
    Ok(())
}

#[test]
fn an_unlisted_revision_is_reported_only_for_the_page_that_has_the_cloud() -> Result<(), String> {
    let mut project = Project::new("Revisions");
    // Two pages that share a display number: only the page id tells them apart.
    project.pages = vec![Page::new("p1", "1", "Clean"), Page::new("p2", "1", "Orphan")];
    project.add_revision("A", "", "", "").map_err(|e| e.to_string())?;
    project
        .add_cloud("p2", Point::new(50.0, 50.0), Point::new(80.0, 80.0), None)
        .map_err(|e| e.to_string())?;
    project.meta.revisions.clear();
    let only = |id: &str| Options {
        pages: vec![id.into()],
        ..Default::default()
    };
    let (_, clean) = export(&project, &BTreeMap::new(), &only("p1"));
    assert!(
        !clean.warnings.iter().any(|w| w.contains("revision A")),
        "{:?}",
        clean.warnings
    );
    let (_, orphan) = export(&project, &BTreeMap::new(), &only("p2"));
    assert!(
        orphan.warnings.iter().any(|w| w.contains("revision A")),
        "{:?}",
        orphan.warnings
    );
    Ok(())
}

#[test]
fn cloud_warnings_are_given_once_per_page_with_a_count() -> Result<(), String> {
    let mut project = Project::new("Revisions");
    project.pages = vec![Page::new("p1", "1", "Power")];
    project.add_revision("A", "", "", "").map_err(|e| e.to_string())?;
    project.add_revision("B", "", "", "").map_err(|e| e.to_string())?;
    for x in [20.0, 60.0, 100.0] {
        project
            .add_cloud("p1", Point::new(x, 50.0), Point::new(x + 30.0, 80.0), Some("B"))
            .map_err(|e| e.to_string())?;
    }
    project
        .add_cloud("p1", Point::new(20.0, 100.0), Point::new(50.0, 130.0), Some("A"))
        .map_err(|e| e.to_string())?;
    // Revision B is removed: its three clouds give one warning, the cloud of A none.
    project.meta.revisions.pop();
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    let unlisted: Vec<&String> = report
        .warnings
        .iter()
        .filter(|w| w.contains("which the project does not list"))
        .collect();
    assert_eq!(
        unlisted,
        ["page 1: 3 clouds carry revision B, which the project does not list"],
        "{:?}",
        report.warnings
    );
    // Two clouds without area give one warning as well.
    for c in project.pages[0].clouds.iter_mut().take(2) {
        c.max = c.min;
    }
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    let flat: Vec<&String> = report.warnings.iter().filter(|w| w.contains("no area")).collect();
    assert_eq!(
        flat,
        ["page 1: 2 revision clouds have no area"],
        "{:?}",
        report.warnings
    );
    // A letter the PDF cannot print, on two clouds, is reported once for them.
    project.meta.revisions[0].letter = "Ж".into();
    for c in project.pages[0].clouds.iter_mut().skip(2) {
        c.revision = "Ж".into();
    }
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    let unprintable: Vec<&String> = report
        .warnings
        .iter()
        .filter(|w| w.contains("cannot print revision"))
        .collect();
    assert_eq!(
        unprintable,
        ["page 1: the PDF cannot print revision Ж on 2 clouds; it can read like another letter"],
        "{:?}",
        report.warnings
    );
    Ok(())
}

#[test]
fn a_rack_of_three_cards_gives_plc_card_overview_pages() {
    use wc_core::plc_rack::{PlcCard, PlcRack};
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Mains")];
    let card = |slot: u32, tag: &str, channels: u32, start: &str| PlcCard {
        slot,
        tag: tag.into(),
        channels,
        start: start.into(),
        ..Default::default()
    };
    project.meta.racks.push(PlcRack {
        name: "-A1".into(),
        cards: vec![
            card(1, "-A1.DI1", 32, "%IX0.0"),
            card(2, "-A1.DO1", 32, "%QX4.0"),
            card(3, "-A1.AI1", 8, "%IW64"),
        ],
        ..Default::default()
    });
    let none = BTreeMap::new();
    let inputs = wc_report::Inputs {
        project: &project,
        symbols: &none,
        parts: &BTreeMap::new(),
    };
    let table = wc_report::build(wc_report::Kind::PlcCards, &inputs);
    assert_eq!(table.rows.len(), 72);
    let opts = Options {
        reports: vec![table],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &none, &opts);
    // The overview runs on over as many report sheets as its rows need, after the one
    // schematic page.
    let band = project.meta.template.band_height();
    let per_page = layout::rows_per_page(layout::report_size(project.pages[0].size, band, 1), band);
    let sheets = 72usize.div_ceil(per_page);
    assert!(sheets >= 2, "72 rows fit on one sheet of {per_page}");
    assert_eq!(report.pages, 1 + sheets);
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains(&writer::pdf_text("PLC cards")));
    for cell in ["-A1.DI1", "-A1.DO1", "-A1.AI1", "%IX3.7", "%QX7.7", "%IW78"] {
        assert!(text.contains(&writer::pdf_text(cell)), "{cell}");
    }
}

#[test]
fn a_network_gets_a_topology_page_with_its_devices_addresses_and_cables() {
    use wc_core::network::{Link as NetLink, Network, Node};
    let mut project = Project::new("x");
    let mut p1 = Page::new("p1", "1", "PLC");
    p1.symbols.push(place("s1", "coil", 50.0, "-K10"));
    project.pages.push(p1);
    let node = |d: &str, a: &str| Node {
        device: d.into(),
        address: a.into(),
        ..Node::default()
    };
    let link = |a: &str, b: &str, w: &str| NetLink {
        from: a.into(),
        to: b.into(),
        cable: w.into(),
        ..NetLink::default()
    };
    // PLC - switch - two drives, and a ring of three devices back to the first one.
    let line = Network {
        name: "PN1".into(),
        kind: "profinet".into(),
        nodes: vec![
            node("-A1", "192.168.0.1"),
            node("-K10", "192.168.0.10"),
            node("-T1", "192.168.0.21"),
            node("-T2", "192.168.0.22"),
        ],
        links: vec![
            link("-A1:X1P1", "-K10:P1", "-W1"),
            link("-K10:P2", "-T1:X1", "-W2"),
            link("-K10:P3", "-T2:X1", "-W3"),
        ],
        ..Network::default()
    };
    let ring = Network {
        name: "RING".into(),
        nodes: vec![node("-A5", ""), node("-A6", ""), node("-A7", "")],
        links: vec![
            link("-A5:P1", "-A6:P1", "-W51"),
            link("-A6:P2", "-A7:P1", "-W52"),
            link("-A7:P2", "-A5:P2", "-W53"),
        ],
        ..Network::default()
    };
    let opts = Options {
        networks: vec![line, ring],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(report.pages, 3);
    assert!(text.contains(&format!("/Title {}", writer::pdf_text("N1 Network PN1"))));
    assert!(text.contains(&format!("/Title {}", writer::pdf_text("N2 Network RING"))));
    for t in [
        "-A1",
        "-K10",
        "-T1",
        "-T2",
        "192.168.0.1",
        "192.168.0.10",
        "192.168.0.21",
        "192.168.0.22",
        "-W1",
        "-W2",
        "-W3",
        "X1P1",
        "P3",
    ] {
        assert!(text.contains(&writer::pdf_text(t)), "{t}");
    }
    // Every link of the ring is drawn once, each with its cable tag (alone, or after the
    // port names over a short lane).
    for w in ["-W51", "-W52", "-W53"] {
        let hex = writer::pdf_text(w);
        let hex = hex.trim_start_matches('<').trim_end_matches('>');
        assert_eq!(text.matches(hex).count(), 1, "{w}");
    }
    // The switch's box links to the page it is drawn on; the others are on no page.
    assert_eq!(text.matches("/Subtype /Link").count(), 1);
}

#[test]
fn a_topology_box_links_to_a_page_the_pdf_holds() {
    use wc_core::network::{Link as NetLink, Network};
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_owned(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    let mut project = Project::new("x");
    let mut p1 = Page::new("p1", "1", "Power");
    p1.symbols.push(place("s1", "coil", 50.0, "-K10"));
    let mut p2 = Page::new("p2", "2", "Control");
    p2.symbols.push(place("s2", "coil", 50.0, "-K10"));
    project.pages = vec![p1, p2];
    let net = Network {
        name: "PN1".into(),
        links: vec![NetLink {
            from: "-K10:P1".into(),
            to: "-A1:P1".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    // Only the second page is exported: the box links there, not to the first page.
    let opts = Options {
        pages: vec!["p2".into()],
        networks: vec![net],
        ..Default::default()
    };
    let (pdf, _) = export(&project, &symbols, &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(
        text.matches("/Subtype /Link").count(),
        1,
        "the -K10 box links to page 2"
    );
}

#[test]
fn a_link_between_two_ports_of_one_device_is_drawn_and_a_bad_tag_is_warned_about() {
    use wc_core::network::{Link as NetLink, Network, Node};
    let net = Network {
        name: "PN2".into(),
        nodes: vec![Node {
            device: "K 10?".into(),
            address: "192.168.0.2".into(),
            station: "drive-07".into(),
            ..Node::default()
        }],
        links: vec![
            NetLink {
                from: "-K10:P1".into(),
                to: "-K10:P2".into(),
                cable: "-W9".into(),
                ..NetLink::default()
            },
            NetLink {
                from: "-K10:P3".into(),
                to: "-K10:P4".into(),
                cable: "-W8".into(),
                ..NetLink::default()
            },
            NetLink {
                from: "-K10:P5".into(),
                to: "secret label!:P1".into(),
                cable: "-W7".into(),
                ..NetLink::default()
            },
        ],
        ..Network::default()
    };
    let opts = Options {
        networks: vec![net],
        ..Default::default()
    };
    let (pdf, report) = export(&Project::new("x"), &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains(&writer::pdf_text("P1 - P2 -W9")));
    assert!(text.contains(&writer::pdf_text("P3 - P4 -W8")));
    // Drawn as written; the warning names the node by number, not by its text.
    assert!(text.contains(&writer::pdf_text("K 10?")));
    // Its address and station are still drawn in its box.
    assert!(text.contains(&writer::pdf_text("192.168.0.2")));
    assert!(text.contains(&writer::pdf_text("drive-07")));
    assert!(
        report.warnings.iter().any(|w| w.starts_with("network PN2: node ")
            && w.ends_with(" is not a device tag: only letters, digits, '.', '_' allowed")),
        "{:?}",
        report.warnings
    );
    // A bad link end: the reason, by link number, and never the text written there.
    assert!(
        report.warnings.contains(
            &"network PN2: link 3: its to end is not a device or port: only letters, digits, '.', '_' allowed"
                .to_owned()
        ),
        "{:?}",
        report.warnings
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.contains("secret") || w.contains("K 10?"))
    );
}

#[test]
fn a_link_with_an_empty_end_is_warned_about_and_the_others_are_drawn() {
    use wc_core::network::{Link as NetLink, Network};
    let link = |from: &str, to: &str, cable: &str| NetLink {
        from: from.into(),
        to: to.into(),
        cable: cable.into(),
        ..NetLink::default()
    };
    let net = Network {
        name: "PN3".into(),
        links: vec![
            link("-A1:X1", "-K10:P1", "-W1"),
            link("-K10:P2", "", "-W2"),
            link("-K10:P3", "-T1:X1", "-W3"),
        ],
        ..Network::default()
    };
    let opts = Options {
        networks: vec![net],
        ..Default::default()
    };
    let (pdf, report) = export(&Project::new("x"), &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    // The valid links are drawn with their cables; the one with an empty end is not.
    for drawn in ["-A1", "-K10", "-T1", "-W1", "-W3"] {
        assert!(text.contains(&writer::pdf_text(drawn)), "{drawn}");
    }
    assert!(!text.contains(&writer::pdf_text("-W2")));
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w == "network PN3: link 2 is not drawn: an end names no device"),
        "{:?}",
        report.warnings
    );
    assert!(
        !report.warnings.iter().any(|w| w.contains("-K10:P2")),
        "{:?}",
        report.warnings
    );
}

#[test]
fn plc_cards_are_drawn_one_per_sheet_with_their_points_linked() {
    use wc_report::{CardChannel, CardPoint, PlcCardSheet};
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Inputs"), Page::new("p2", "2", "Outputs")];
    let point = |device: &str, page: &str| CardPoint {
        device: device.into(),
        description: format!("{device} running"),
        connected: "-X1:3".into(),
        page_number: page.trim_start_matches('p').into(),
        page_id: page.into(),
    };
    let card = |slot: u32, name: &str, prefix: &str, points: Vec<(usize, CardPoint)>| PlcCardSheet {
        rack: "-A1".into(),
        slot,
        card: name.into(),
        kind: "DI 8".into(),
        channels: (0..8)
            .map(|c| CardChannel {
                channel: c,
                address: format!("{prefix}{}.{c}", slot - 1),
                points: points
                    .iter()
                    .filter(|(at, _)| *at == c)
                    .map(|(_, p)| p.clone())
                    .collect(),
            })
            .collect(),
    };
    // Three cards: a point on the first, two points on one channel of the second, none on the third.
    let cards = vec![
        card(1, "-K1", "%IX", vec![(0, point("-S1", "p1"))]),
        card(2, "-K2", "%QX", vec![(3, point("-M1", "p2")), (3, point("-H1", "p2"))]),
        card(3, "-K3", "%IX", vec![]),
    ];
    let opts = Options {
        plc_cards: cards.clone(),
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(report.pages, 5, "two schematic pages and a sheet per card");
    for shown in [
        "PLC card -K1",
        "PLC card -K2",
        "PLC card -K3",
        "-S1",
        "-M1 running",
        "-H1",
        "%QX1.3",
        "spare",
        "-X1:3",
    ] {
        assert!(text.contains(&writer::pdf_text(shown)), "{shown} is drawn");
    }
    let at = |title: &str| {
        text.find(&format!("/Title {}", writer::pdf_text(title)))
            .unwrap_or(usize::MAX)
    };
    assert!(at("2 Outputs") < at("C1 PLC card -K1"));
    assert!(at("C1 PLC card -K1") < at("C2 PLC card -K2"));
    assert!(at("C2 PLC card -K2") < at("C3 PLC card -K3"));
    assert!(at("C3 PLC card -K3") < usize::MAX);
    // Each drawn point links to its page: -S1 to page 1, -M1 and -H1 to page 2.
    assert_eq!(text.matches("/Subtype /Link").count(), 3);
    let targets = |c: &PlcCardSheet| -> Vec<String> {
        let mut page = Page::new("c", "C1", "card");
        page.size = layout::card_size(project.pages[0].size, project.meta.template.band_height());
        let drawn = layout::draw_card_page(&project, &page, 0, 1, c, 0, (1, 1), "");
        drawn.links.into_iter().map(|l| l.target_page_id).collect()
    };
    assert_eq!(targets(&cards[0]), ["p1"], "-S1 links to page 1");
    assert_eq!(targets(&cards[1]), ["p2", "p2"], "-M1 and -H1 link to page 2");
    assert!(targets(&cards[2]).is_empty());
    let rows = |c: &PlcCardSheet| layout::card_rows_of(c).len();
    assert_eq!((rows(&cards[0]), rows(&cards[1]), rows(&cards[2])), (8, 9, 8));
}

#[test]
fn a_long_plc_card_goes_on_over_several_sheets() {
    use wc_report::{CardChannel, PlcCardSheet};
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Inputs")];
    let per = layout::card_rows_per_page(project.pages[0].size, project.meta.template.band_height());
    let card = PlcCardSheet {
        card: "-K9".into(),
        channels: (0..per + 1)
            .map(|c| CardChannel {
                channel: c,
                address: format!("%IX{}.{}", c / 8, c % 8),
                points: Vec::new(),
            })
            .collect(),
        ..Default::default()
    };
    let opts = Options {
        plc_cards: vec![card],
        ..Default::default()
    };
    let (pdf, report) = export(&project, &BTreeMap::new(), &opts);
    assert_eq!(report.pages, 3);
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains(&writer::pdf_text("PLC card -K9 (2 / 2)")));
}

#[test]
fn plc_card_sheets_move_to_a_larger_sheet_and_fit_their_fields() {
    let short = PageSize::Custom {
        width: 300,
        height: 100,
    };
    // A 60 mm title block leaves no card row on the short sheet: a larger one is used.
    assert_eq!(layout::card_size(short, 60.0), PageSize::A4);
    assert_eq!(layout::card_size(PageSize::A3, 60.0), PageSize::A3);
    assert_eq!(layout::card_size(PageSize::A4Portrait, 60.0), PageSize::A4Portrait);
    // A tall but narrow sheet has rows to spare and no room for the point columns.
    let narrow = PageSize::Custom {
        width: 100,
        height: 297,
    };
    assert_eq!(layout::card_size(narrow, 60.0), PageSize::A4);
    // A field cut for its column is measured at the height it is drawn at.
    let long = "-K100 motor protection relay for the main pump in the north basin";
    for width in [10.0, 25.0, 60.0] {
        let cut = layout::fit_at(long, width, 2.5);
        assert!(writer::text_width_mm(&cut, 2.5) <= width, "{cut:?} fits {width} mm");
    }
}

#[test]
fn a_very_long_heading_is_cut_quickly_and_keeps_its_counter() {
    let title = "Cable list for the north basin pump station ".repeat(2_300);
    let title = format!("{}\u{e9}", &title[..99_999]);
    assert_eq!(title.chars().count(), 100_000);
    let counter = " (2 / 3)";
    let heading = layout::fit_heading(&title, counter, 180.0);
    assert!(heading.ends_with(&format!("...{counter}")), "{heading:?}");
    assert!(writer::text_width_mm(&heading, 4.0) <= 180.0, "{heading:?}");
    // The cut is the longest start that fits: one more character would not.
    let Some(kept) = heading.strip_suffix(&format!("...{counter}")) else {
        unreachable!("the heading ends with the counter")
    };
    let Some(next) = title[kept.len()..].chars().next() else {
        unreachable!("the title was cut")
    };
    let longer = format!("{kept}{next}...{counter}");
    assert!(writer::text_width_mm(&longer, 4.0) > 180.0, "{longer:?}");
    // A short heading is left as it is.
    assert_eq!(layout::fit_heading("Cables", counter, 180.0), "Cables (2 / 3)");
    // No room at all for the title leaves only "..." before the counter.
    assert_eq!(layout::fit_heading(&title, counter, 1.0), format!("...{counter}"));
}

#[test]
fn a_dimension_drawn_by_hand_shows_in_pdf_svg_and_dxf() {
    let mut project = Project::new("x");
    let mut page = Page::new("p1", "1", "Cabinet");
    page.graphics.push(Shape::Dimension {
        a: wc_core::Point::new(40.0, 120.0),
        b: wc_core::Point::new(65.0, 120.0),
        offset: 10.0,
        height: 2.5,
    });
    project.pages = vec![page];
    let symbols = BTreeMap::new();
    let (bytes, _) = export(&project, &symbols, &Options::default());
    let tj = format!("{} Tj", writer::pdf_text("25"));
    assert!(
        String::from_utf8_lossy(&bytes).contains(&tj),
        "the PDF shows the length"
    );
    let (pages, _) = page_drawings(&project, &symbols, &Options::default());
    let svg = crate::vector::svg(&pages[0]);
    assert!(svg.contains(">25</text>"), "{svg}");
    let d = dxf(&pages[0]);
    assert!(d.contains("\n1\n25\n"), "the DXF has the length as text");
}

#[test]
fn a_long_page_reference_on_a_plc_card_is_cut_to_its_column() {
    use wc_report::{CardChannel, CardPoint, PlcCardSheet};
    let project = Project::new("x");
    let long_page = "=PLANT+ROOM-17/123456789";
    let card = PlcCardSheet {
        card: "-K1".into(),
        channels: vec![CardChannel {
            channel: 0,
            address: "%IX0.0".into(),
            points: vec![CardPoint {
                device: "-S1".into(),
                description: "start".into(),
                connected: "-X1:3 to the terminal strip in the field box ".repeat(6),
                page_number: long_page.into(),
                page_id: "p1".into(),
            }],
        }],
        ..Default::default()
    };
    let page = Page::new("c", "C1", "card");
    let drawn = layout::draw_card_page(&project, &page, 0, 1, &card, 0, (1, 1), "");
    let cut = layout::fit_at(long_page, layout::CARD_PAGE, 2.5);
    assert!(writer::text_width_mm(&cut, 2.5) <= layout::CARD_PAGE);
    assert_ne!(cut, long_page, "the page reference is too long for its column");
    assert!(
        drawn.ops.contains(&writer::pdf_text(&cut)),
        "the cut page reference is drawn"
    );
    assert!(!drawn.ops.contains(&writer::pdf_text(long_page)));
    // The connection text is cut too, and ends before the page column starts.
    let [_, _, conn_x, page_x] = layout::card_field_x(page.size.mm().0);
    let connected = &card.channels[0].points[0].connected;
    let shown = layout::fit_at(connected, page_x - layout::CARD_PAGE - 2.0 - conn_x, 2.5);
    assert_ne!(&shown, connected, "the connection is too long for its column");
    assert!(
        drawn.ops.contains(&writer::pdf_text(&shown)),
        "the cut connection is drawn"
    );
    assert!(!drawn.ops.contains(&writer::pdf_text(connected)));
    assert!(conn_x + writer::text_width_mm(&shown, 2.5) < page_x - layout::CARD_PAGE);
}

#[test]
fn a_declared_node_with_a_blank_tag_keeps_its_box_address_and_station() {
    use wc_core::network::{Network, Node};
    let net = Network {
        name: "PN3".into(),
        nodes: vec![
            // Text that reads like the name a blank node is shown under does not take its box.
            Node {
                device: "(node 2)".into(),
                address: "10.0.0.1".into(),
                ..Node::default()
            },
            // Nor does text holding the marker of a blank node's internal name, or text close
            // to it: each keeps a box of its own.
            Node {
                device: "\u{1}4".into(),
                address: "10.0.0.3".into(),
                ..Node::default()
            },
            Node {
                device: "\u{FFFD}4".into(),
                address: "10.0.0.4".into(),
                ..Node::default()
            },
            Node {
                device: "  ".into(),
                address: "192.168.0.2".into(),
                station: "drive-07".into(),
                ..Node::default()
            },
        ],
        links: vec![
            wc_core::network::Link {
                from: "(node 2)".into(),
                to: "-K1:P1".into(),
                ..Default::default()
            },
            wc_core::network::Link {
                from: "\u{FFFD}4".into(),
                to: "-K1:P2".into(),
                ..Default::default()
            },
        ],
        ..Network::default()
    };
    // Four nodes and -K1: five boxes, none merged, and both links drawn.
    let topo = crate::topology::layout_network(&net);
    assert_eq!(topo.boxes.len(), 5, "{:?}", topo.boxes);
    assert!(topo.unplaced.is_empty());
    let opts = Options {
        networks: vec![net],
        ..Default::default()
    };
    let (pdf, report) = export(&Project::new("x"), &BTreeMap::new(), &opts);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(text.matches(&writer::pdf_text("(node 4)")).count(), 1);
    assert!(text.contains(&writer::pdf_text("(node 2)")));
    assert!(text.contains(&writer::pdf_text("10.0.0.1")));
    assert!(text.contains(&writer::pdf_text("10.0.0.3")));
    assert!(text.contains(&writer::pdf_text("10.0.0.4")));
    assert!(text.contains(&writer::pdf_text("192.168.0.2")));
    assert!(text.contains(&writer::pdf_text("drive-07")));
    assert!(!text.contains(&writer::pdf_text("No devices on this network.")));
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.starts_with("network PN3: node 4 is not a device tag")),
        "{:?}",
        report.warnings
    );
}

#[test]
fn the_reader_title_bar_shows_the_project_name_not_the_file_name() {
    // One page, so there is a sheet to print (#833).
    let mut project = Project::new("Pump station");
    project.pages.push(Page::new("p1", "1", "Main"));
    let (pdf, _) = export(&project, &BTreeMap::new(), &Options::default());
    let text = String::from_utf8_lossy(&pdf);
    let Some(catalog) = text.split("endobj").find(|o| o.contains("/Type /Catalog")) else {
        unreachable!("no catalog in {text}");
    };
    assert!(
        catalog.contains("/ViewerPreferences << /DisplayDocTitle true >>"),
        "{catalog}"
    );
    assert!(text.contains(&format!("/Title {}", writer::pdf_text("Pump station"))));

    // The file still opens: the trailer points at the xref table, and every object it lists
    // starts where the table says, so the added catalog entry did not shift any offset.
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).rposition(|w| w == needle);
    let Some(sx) = find(&pdf, b"startxref\n") else {
        unreachable!("no startxref");
    };
    let start: usize = std::str::from_utf8(&pdf[sx + 10..])
        .ok()
        .and_then(|s| s.lines().next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    assert!(pdf.get(start..).is_some_and(|t| t.starts_with(b"xref")));
    let table = std::str::from_utf8(&pdf[start..]).unwrap_or("");
    let mut lines = table.lines().skip(1);
    let count: usize = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    assert!(count > 1, "{table}");
    for (n, line) in lines.take(count).enumerate().skip(1) {
        let offset: usize = line.split(' ').next().and_then(|o| o.parse().ok()).unwrap_or(0);
        let head = format!("{n} 0 obj");
        assert!(
            pdf.get(offset..).is_some_and(|o| o.starts_with(head.as_bytes())),
            "object {n} is not at {offset}"
        );
    }
}

#[test]
fn a_project_with_nothing_to_print_says_so() {
    // No pages and no report tables: no sheet, so the caller is told instead of being handed a
    // PDF no reader opens (#833).
    let (_, report) = export(&Project::new("x"), &BTreeMap::new(), &Options::default());
    assert_eq!(report.pages, 0);
    assert_eq!(report.warnings, vec![NOTHING_TO_EXPORT.to_string()]);

    // One page: no such warning.
    let mut project = Project::new("x");
    project.pages.push(Page::new("p1", "1", "Main"));
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(report.pages, 1);
    assert!(
        !report.warnings.iter().any(|w| w == NOTHING_TO_EXPORT),
        "{:?}",
        report.warnings
    );
}

#[test]
fn page_text_writes_the_rest_of_winansi() {
    // Euro, curly quotes and ellipsis have WinAnsi codes in the built-in font: no '?'.
    assert_eq!(writer::pdf_text("€ “x” …"), "<80209378942085>");
    assert_eq!(writer::pdf_text("„Œœ Šš™’"), "<848C9C208A9A9992>");
    // An arrow still becomes '>', and a glyph the font lacks '?'.
    assert_eq!(writer::pdf_text("→Д"), "<3E3F>");
}

#[test]
fn bookmarks_and_the_document_title_are_text_strings_not_font_codes() {
    // A reader decodes bookmark titles as PDFDocEncoding or UTF-16: the page font's WinAnsi
    // en dash (0x96) would show as "Œ" there, and Ω could not be written at all (#726).
    let mut project = Project::new("Pumpe – Ålesund");
    project.pages = vec![Page::new("p1", "1", "Motor – pump Ω"), Page::new("p2", "2", "Control")];
    let (pdf, _) = export(&project, &BTreeMap::new(), &Options::default());
    let text = String::from_utf8_lossy(&pdf);
    let utf16 = |s: &str| -> String {
        let units: String = s.encode_utf16().map(|u| format!("{u:04X}")).collect();
        format!("<FEFF{units}>")
    };
    assert_eq!(writer::pdf_text_string("1 Motor – pump Ω"), utf16("1 Motor – pump Ω"));
    assert!(text.contains(&format!("/Title {}", utf16("1 Motor – pump Ω"))));
    assert!(text.contains(&format!("/Title {} /Producer", utf16("Pumpe – Ålesund"))));
    // Plain ASCII titles stay as they were, the same bytes in every encoding.
    assert!(text.contains(&format!("/Title {}", writer::pdf_text("2 Control"))));
    // The page itself still draws the title with the font's own codes.
    assert!(text.contains(&format!("{} Tj", writer::pdf_text("Motor – pump Ω"))));
}

/// Decode a hex PDF text string written by `pdf_text_string`: UTF-16BE after a byte order mark,
/// otherwise one byte per character (printable ASCII only).
fn decode_text_string(hex: &str) -> Option<String> {
    let hex = hex.strip_prefix('<')?.strip_suffix('>')?;
    if let Some(units) = hex.strip_prefix("FEFF") {
        let units: Option<Vec<u16>> = (0..units.len())
            .step_by(4)
            .map(|i| units.get(i..i + 4).and_then(|u| u16::from_str_radix(u, 16).ok()))
            .collect();
        return String::from_utf16(&units?).ok();
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
        .map(|b| b.filter(u8::is_ascii).map(char::from))
        .collect()
}

#[test]
fn a_title_with_euro_quotes_and_other_scripts_round_trips_as_a_text_string() {
    // WinAnsi puts € at 0x80 and the curly quotes at 0x93/0x94, which PDFDocEncoding reads as
    // other characters; the document title must come out as typed (#803).
    for title in ["Pumpe 1 200 € „Nord“ “A”", "Ω → 日本"] {
        let project = Project::new(title);
        let (pdf, _) = export(&project, &BTreeMap::new(), &Options::default());
        let text = String::from_utf8_lossy(&pdf);
        let Some(at) = text.find("/Producer (WiringCraft)") else {
            unreachable!("the info dictionary is written")
        };
        let Some(start) = text[..at].rfind("/Title ") else {
            unreachable!("the info dictionary has a title")
        };
        let written = text[start + "/Title ".len()..at].trim();
        assert_eq!(written, writer::pdf_text_string(title));
        assert!(written.starts_with("<FEFF"), "{written}");
        assert_ne!(written, writer::pdf_text(title), "not the page font's codes");
        assert_eq!(decode_text_string(written).as_deref(), Some(title));
    }
    // Plain ASCII stays a simple one-byte string.
    assert_eq!(writer::pdf_text_string("Plant 7"), writer::pdf_text("Plant 7"));
    assert_eq!(
        decode_text_string(&writer::pdf_text_string("Plant 7")).as_deref(),
        Some("Plant 7")
    );
}

#[test]
fn layer_names_are_text_strings() {
    // Layer names are read as PDF text strings, like the title: each declared layer's /Name is
    // `pdf_text_string` of its title, and a non-ASCII name would come out as UTF-16 (#803).
    let mut p1 = Page::new("p1", "1", "Main");
    p1.graphics.push(Shape::Line {
        a: Point::new(20.0, 20.0),
        b: Point::new(60.0, 20.0),
    });
    let mut project = Project::new("Layers");
    project.pages = vec![p1];
    let (pdf, _) = export(&project, &BTreeMap::new(), &Options::default());
    let text = String::from_utf8_lossy(&pdf);
    let names: Vec<&str> = text
        .match_indices("/Type /OCG /Name ")
        .filter_map(|(i, m)| {
            let rest = &text[i + m.len()..];
            rest.find(" >>").map(|end| &rest[..end])
        })
        .collect();
    assert!(!names.is_empty(), "the drawing layer is declared");
    for name in &names {
        let Some(title) = decode_text_string(name) else {
            unreachable!("{name} is a text string")
        };
        assert!(writer::Layer::ALL.iter().any(|l| l.title() == title), "{title}");
        assert_eq!(*name, writer::pdf_text_string(&title));
    }
    let Some(name) = decode_text_string(&writer::pdf_text_string("Leitungen → Ω 日本")) else {
        unreachable!("non-ASCII names are written as UTF-16")
    };
    assert_eq!(name, "Leitungen → Ω 日本");
}

#[test]
fn a_missing_symbol_is_warned_about_once_per_page_with_its_placements() {
    let mut project = Project::new("x");
    let mut p1 = Page::new("p1", "1", "A");
    for (k, x) in [40.0, 80.0, 120.0].into_iter().enumerate() {
        p1.symbols.push(place(&format!("r{k}"), "relay", x, &format!("-K{k}")));
    }
    p1.symbols.push(place("t1", "lamp", 160.0, "-P1"));
    let mut p2 = Page::new("p2", "2", "B");
    p2.symbols.push(place("r9", "relay", 40.0, "-K9"));
    project.pages = vec![p1, p2];
    let (pdf, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(
        report.warnings,
        vec![
            "page 1: symbol \"lamp\" is not in the project's library copy".to_string(),
            "page 1: symbol \"relay\" is not in the project's library copy (3 placements)".to_string(),
            "page 2: symbol \"relay\" is not in the project's library copy".to_string(),
        ]
    );
    // Every placement is still drawn as a box with a question mark.
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(text.matches(&format!("{} Tj", writer::pdf_text("?"))).count(), 5);
    // The SVG and DXF exports warn the same way.
    let (_, lines) = page_lines(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(lines.warnings, report.warnings);
}

#[test]
fn text_the_pdf_cannot_print_is_warned_about_once_per_sheet() {
    // Helvetica's WinAnsi has no Ω or Cyrillic: they print as '?', so say so (Latin-1, the
    // euro and dashes print, and an arrow prints as '>' on purpose).
    let mut project = Project::new("Demo");
    project.pages = vec![
        Page::new("p1", "1", "Motor 10 kΩ Ω Жар"),
        Page::new("p2", "2", "Pumpe – Ålesund 200 € → B"),
    ];
    let (_, report) = export(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(
        report.warnings,
        vec!["page 1: the PDF cannot print Ω Ж а р in its text; they show as ?".to_string()]
    );
}

#[test]
fn a_long_report_or_plc_card_heading_is_cut_to_the_frame_and_keeps_its_counter() {
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Inputs")];
    let long = "Device list for the north basin pumping station, the second pump hall and its \
                cabinets, with every spare and every terminal named on the drawings, listed \
                again for the south basin, its pump hall, its cabinets and every spare there";
    let table = wc_report::Table {
        title: long.into(),
        columns: vec!["Tag".into()],
        ..Default::default()
    };
    let card = wc_report::PlcCardSheet {
        card: long.into(),
        ..Default::default()
    };
    let page = Page::new("r", "R1", "Report");
    let (w, _) = page.size.mm();
    let right = w - layout::MARGIN - 6.0;
    let headings = |drawn: &writer::Drawn| -> Vec<(f64, String)> {
        drawn
            .prims
            .iter()
            .filter_map(|p| match p {
                writer::Prim::Text { at, height, text, .. } if (*height - 4.0).abs() < 1e-9 => {
                    Some((at.0 + writer::text_width_mm(text, 4.0), text.clone()))
                }
                _ => None,
            })
            .collect()
    };
    let report = layout::draw_table_page(&project, &page, 0, 3, &table, &[], (2, 3), "");
    let card_sheet = layout::draw_card_page(&project, &page, 0, 3, &card, 0, (2, 3), "");
    for (what, drawn, start) in [
        ("report", &report, "Device list"),
        ("card", &card_sheet, "PLC card Device"),
    ] {
        let found: Vec<(f64, String)> = headings(drawn)
            .into_iter()
            .filter(|(_, t)| t.starts_with(start))
            .collect();
        let [(end, text)] = found.as_slice() else {
            unreachable!("one {what} heading, got {found:?}")
        };
        assert!(
            *end <= right + 1e-6,
            "{what} heading {text:?} ends at {end} mm, past {right} mm"
        );
        assert!(
            text.ends_with("... (2 / 3)"),
            "{what} heading {text:?} is cut and keeps its counter"
        );
    }
    // A short title is drawn whole.
    let short = wc_report::Table {
        title: "Devices".into(),
        ..table
    };
    let drawn = layout::draw_table_page(&project, &page, 0, 1, &short, &[], (1, 1), "");
    assert!(
        headings(&drawn).iter().any(|(_, t)| t == "Devices"),
        "{:?}",
        headings(&drawn)
    );
}

#[test]
fn a_cable_list_page_names_the_cable_its_first_cores_belong_to() {
    let mut project = Project::new("x");
    project.pages = vec![Page::new("p1", "1", "Inputs")];
    let row = |cells: [&str; 7]| cells.iter().map(|c| (*c).to_owned()).collect::<Vec<String>>();
    let table = wc_report::Table {
        title: "Cable list".into(),
        columns: ["Cable", "Type", "Description", "Core", "Page", "From", "To"]
            .iter()
            .map(|c| (*c).to_owned())
            .collect(),
        rows: vec![
            row(["-W1", "Lapp 1119304", "Motor feed", "1", "1", "-X1.1", "-M1:U"]),
            row(["", "", "", "2", "1", "-X1.2", "-M1:V"]),
            row(["", "", "", "3", "1", "-X1.3", "-M1:W"]),
        ],
        ..Default::default()
    };
    let page = Page::new("r", "R2", "Cable list");
    let texts = |lines: &[layout::TableLine]| -> Vec<String> {
        layout::draw_table_page(&project, &page, 0, 2, &table, lines, (2, 2), "")
            .prims
            .iter()
            .filter_map(|p| match p {
                writer::Prim::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    // The second page starts at core 2: it names -W1, its type and description once.
    let second = texts(&[layout::TableLine::Row(1), layout::TableLine::Row(2)]);
    for value in ["-W1", "Lapp 1119304", "Motor feed"] {
        assert_eq!(
            second.iter().filter(|t| t.as_str() == value).count(),
            1,
            "{value} on {second:?}"
        );
    }
    // The first page names it on its first row only, as before.
    let first = texts(&[layout::TableLine::Row(0), layout::TableLine::Row(1)]);
    assert_eq!(first.iter().filter(|t| t.as_str() == "-W1").count(), 1, "{first:?}");
}

#[test]
fn a_mirrored_or_upside_down_symbol_prints_its_tag_away_from_the_drawing() {
    let mut symbols = BTreeMap::new();
    symbols.insert("coil".to_string(), sym("coil", SymbolRole::Main, ["A1", "A2"]));
    symbols.insert("no".to_string(), sym("no", SymbolRole::Auxiliary, ["13", "14"]));
    let tag = "=PUMP.STATION+CABINET-K1";
    // Where each tag line starts and ends on the page, from left to right.
    let spans = |rotation: wc_core::geom::Rotation, mirror: bool| -> Vec<(f64, f64)> {
        let mut project = Project::new("x");
        let mut page = Page::new("p1", "1", "Control");
        let mut s = place("s1", "coil", 100.0, tag);
        s.rotation = rotation;
        s.mirror = mirror;
        page.symbols.push(s);
        project.pages = vec![page.clone()];
        let xref = CrossRefs::build(&project, &symbols);
        let mut warnings = Vec::new();
        let drawn = layout::draw_page(
            &project,
            &page,
            0,
            1,
            &symbols,
            &xref,
            &Options::default(),
            &mut warnings,
        );
        drawn
            .prims
            .iter()
            .filter_map(|p| match p {
                writer::Prim::Text { at, height, text, .. }
                    if (*height - 2.5).abs() < 1e-9 && ["=PUMP", "+CAB", "-K1"].iter().any(|w| text.starts_with(w)) =>
                {
                    Some((at.0, at.0 + writer::text_width_mm(text, 2.5)))
                }
                _ => None,
            })
            .collect()
    };
    // The symbol is a line at x = 100, its tag 5 mm to its left (to its right when mirrored or
    // turned half round), and each line is wider than that gap.
    for (rotation, mirror, left_of_drawing) in [
        (wc_core::geom::Rotation::R0, false, true),
        (wc_core::geom::Rotation::R0, true, false),
        (wc_core::geom::Rotation::R180, false, false),
        (wc_core::geom::Rotation::R180, true, true),
    ] {
        let lines = spans(rotation, mirror);
        assert_eq!(lines.len(), 3, "{rotation:?} {mirror}: {lines:?}");
        for (a, b) in lines {
            if left_of_drawing {
                assert!(
                    (b - 95.0).abs() < 1e-6,
                    "{rotation:?} {mirror}: ends at the tag: {a}..{b}"
                );
            } else {
                assert!(
                    (a - 105.0).abs() < 1e-6,
                    "{rotation:?} {mirror}: starts at the tag: {a}..{b}"
                );
            }
        }
    }

    // A mirrored contact's reference to its coil sits under its tag, aligned the same way.
    let mut project = Project::new("x");
    let mut page = Page::new("p1", "1", "Control");
    page.symbols.push(place("s1", "coil", 50.0, "-K1"));
    let mut contact = place("s2", "no", 100.0, "-K1");
    contact.mirror = true;
    page.symbols.push(contact);
    project.pages = vec![page.clone()];
    let xref = CrossRefs::build(&project, &symbols);
    let mut warnings = Vec::new();
    let marks = layout::xref_marks(&page, &symbols, &xref, &mut warnings);
    let Some(back) = marks.iter().find(|m| m.text.starts_with('>')) else {
        unreachable!("the contact points at its coil: {marks:?}")
    };
    assert_eq!(back.align, wc_core::geom::Align::Left);
    assert_eq!(back.at, Point::new(105.0, 60.0 + 8.0 + 1.2 + 3.0));
}
