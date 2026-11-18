use super::*;
use wc_core::geom::Point;
use wc_core::symbol::{Pin, PinDir, SymbolRole};
use wc_core::{DeviceTag, Endpoint, Page, PageSize, PlacedSymbol, Shape, Wire};

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
        pictures: Vec::new(),
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
    assert!(page.find("<image").unwrap_or(usize::MAX) < page.find("<g ").unwrap_or(0));
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
fn page_lines_leave_out_pictures_without_warning() {
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
    // No picture bytes are given: a DXF export does not read them, so none may be missed.
    let (pages, report) = crate::page_lines(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(pages.len(), 1);
    assert!(pages[0].pictures.is_empty());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let (_, svg_report) = page_drawings(&project, &BTreeMap::new(), &Options::default());
    assert_eq!(svg_report.warnings.len(), 2, "{:?}", svg_report.warnings);
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
