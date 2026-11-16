//! SVG and DXF export of schematic pages. Both draw exactly what the PDF export draws (frame,
//! title block, symbols, wires, cross-references), from the primitives the same layout records.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use wc_core::{Project, Symbol};

use crate::writer::{Prim, Stroke};
use crate::{Options, Report, layout};

/// One page as drawing primitives in page millimetres (y down).
#[derive(Debug, Clone)]
pub struct PageDrawing {
    pub id: String,
    pub number: String,
    pub title: String,
    pub size_mm: (f64, f64),
    pub prims: Vec<Prim>,
}

/// The schematic pages chosen by `options.pages` (all when empty), drawn as for the PDF export.
/// Report tables in `options` are not drawn; they are their own export.
pub fn page_drawings(
    project: &Project,
    symbols: &BTreeMap<String, Symbol>,
    options: &Options,
) -> (Vec<PageDrawing>, Report) {
    let pages: Vec<&wc_core::Page> = project
        .pages
        .iter()
        .filter(|p| options.pages.is_empty() || options.pages.contains(&p.id))
        .collect();
    let mut report = Report {
        pages: pages.len(),
        warnings: Vec::new(),
    };
    let xref = layout::CrossRefs::build(project, symbols);
    let total = pages.len();
    let out = pages
        .iter()
        .enumerate()
        .map(|(i, page)| {
            let drawn = layout::draw_page(project, page, i, total, symbols, &xref, options, &mut report.warnings);
            PageDrawing {
                id: page.id.clone(),
                number: page.number.clone(),
                title: page.title.clone(),
                size_mm: page.size.mm(),
                prims: drawn.prims,
            }
        })
        .collect();
    (out, report)
}

fn num(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.to_owned()
    }
}

fn rgb(ink: &Stroke) -> String {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", c(ink.rgb.0), c(ink.rgb.1), c(ink.rgb.2))
}

fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn on_circle(c: (f64, f64), r: f64, deg: f64) -> (f64, f64) {
    let t = deg.to_radians();
    (c.0 + r * t.cos(), c.1 + r * t.sin())
}

/// An SVG document of one page, sized in millimetres with one user unit per millimetre.
pub fn svg(page: &PageDrawing) -> String {
    let (w, h) = page.size_mm;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" \
         height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n<title>{} {}</title>\n<rect width=\"{w}\" height=\"{h}\" \
         fill=\"#ffffff\"/>\n<g fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\" \
         font-family=\"Helvetica, Arial, sans-serif\">",
        xml(&page.number),
        xml(&page.title),
        w = num(w),
        h = num(h)
    );
    for p in &page.prims {
        match p {
            Prim::Path {
                points,
                closed,
                filled,
                ink,
                ..
            } => {
                if points.len() < 2 {
                    continue;
                }
                let pts: Vec<String> = points.iter().map(|(x, y)| format!("{},{}", num(*x), num(*y))).collect();
                let tag = if *closed || *filled { "polygon" } else { "polyline" };
                let paint = if *filled {
                    format!("fill=\"{}\" stroke=\"none\"", rgb(ink))
                } else {
                    format!("stroke=\"{}\" stroke-width=\"{}\"", rgb(ink), num(ink.width))
                };
                let _ = writeln!(out, "<{tag} points=\"{}\" {paint}/>", pts.join(" "));
            }
            Prim::Arc {
                center,
                radius,
                start,
                end,
                filled,
                ink,
                ..
            } => {
                let paint = if *filled {
                    format!("fill=\"{}\" stroke=\"none\"", rgb(ink))
                } else {
                    format!("stroke=\"{}\" stroke-width=\"{}\"", rgb(ink), num(ink.width))
                };
                let sweep = end - start;
                if sweep >= 359.999 {
                    let _ = writeln!(
                        out,
                        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" {paint}/>",
                        num(center.0),
                        num(center.1),
                        num(*radius)
                    );
                } else {
                    // Page angles run clockwise with y down, which is SVG's positive sweep.
                    let a = on_circle(*center, *radius, *start);
                    let b = on_circle(*center, *radius, *end);
                    let large = u8::from(sweep > 180.0);
                    let _ = writeln!(
                        out,
                        "<path d=\"M{} {} A{r} {r} 0 {large} 1 {} {}\" {paint}/>",
                        num(a.0),
                        num(a.1),
                        num(b.0),
                        num(b.1),
                        r = num(*radius)
                    );
                }
            }
            Prim::Text {
                at,
                height,
                angle,
                text,
                ink,
                ..
            } => {
                let turn = if *angle == 0.0 {
                    String::new()
                } else {
                    // Counter-clockwise on paper is a negative SVG rotation (y runs down).
                    format!(" transform=\"rotate({} {} {})\"", num(-angle), num(at.0), num(at.1))
                };
                let _ = writeln!(
                    out,
                    "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" stroke=\"none\"{turn}>{}</text>",
                    num(at.0),
                    num(at.1),
                    num(height / 0.72),
                    rgb(ink),
                    xml(text)
                );
            }
        }
    }
    out.push_str("</g>\n</svg>\n");
    out
}

/// AutoCAD colour index nearest to an ink: black prints as 7 (white on a dark screen).
fn aci(ink: &Stroke) -> u8 {
    let (r, g, b) = ink.rgb;
    let grey = (r - g).abs() < 0.1 && (g - b).abs() < 0.1;
    if grey {
        return if r < 0.2 {
            7
        } else if r < 0.6 {
            8
        } else {
            9
        };
    }
    if r >= g && r >= b {
        1
    } else if g >= b {
        3
    } else {
        5
    }
}

/// DXF text: ASCII as is, anything else as `\U+XXXX`, which DXF readers turn back into text.
fn dxf_text(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            c if c.is_ascii() && !c.is_control() => out.push(c),
            c if c.is_control() => {}
            c => {
                let _ = write!(out, "\\U+{:04X}", c as u32);
            }
        }
    }
    out
}

/// A DXF (R12, ASCII) drawing of one page in millimetres, y up with the origin at the page's
/// bottom left corner, as CAD tools expect.
pub fn dxf(page: &PageDrawing) -> String {
    let h = page.size_mm.1;
    let y = |v: f64| h - v;
    let mut out = String::new();
    let mut pair = |code: u16, value: &str| {
        let _ = write!(out, "{code}\n{value}\n");
    };
    pair(0, "SECTION");
    pair(2, "HEADER");
    pair(9, "$ACADVER");
    pair(1, "AC1009");
    pair(9, "$EXTMIN");
    pair(10, "0");
    pair(20, "0");
    pair(9, "$EXTMAX");
    pair(10, &num(page.size_mm.0));
    pair(20, &num(h));
    pair(0, "ENDSEC");
    pair(0, "SECTION");
    pair(2, "ENTITIES");
    for p in &page.prims {
        let layer = match p {
            Prim::Path { layer, .. } | Prim::Arc { layer, .. } | Prim::Text { layer, .. } => {
                layer.map_or("0", |l| l.dxf_name())
            }
        };
        match p {
            Prim::Path {
                points, closed, ink, ..
            } => {
                let colour = aci(ink).to_string();
                if points.len() == 2 && !closed {
                    pair(0, "LINE");
                    pair(8, layer);
                    pair(62, &colour);
                    pair(10, &num(points[0].0));
                    pair(20, &num(y(points[0].1)));
                    pair(11, &num(points[1].0));
                    pair(21, &num(y(points[1].1)));
                } else if points.len() >= 2 {
                    pair(0, "POLYLINE");
                    pair(8, layer);
                    pair(62, &colour);
                    pair(66, "1");
                    pair(70, if *closed { "1" } else { "0" });
                    for (px, py) in points {
                        pair(0, "VERTEX");
                        pair(8, layer);
                        pair(10, &num(*px));
                        pair(20, &num(y(*py)));
                    }
                    pair(0, "SEQEND");
                    pair(8, layer);
                }
            }
            Prim::Arc {
                center,
                radius,
                start,
                end,
                ink,
                ..
            } => {
                let colour = aci(ink).to_string();
                if end - start >= 359.999 {
                    pair(0, "CIRCLE");
                } else {
                    pair(0, "ARC");
                }
                pair(8, layer);
                pair(62, &colour);
                pair(10, &num(center.0));
                pair(20, &num(y(center.1)));
                pair(40, &num(*radius));
                if end - start < 359.999 {
                    // Clockwise on a y-down page is counter-clockwise with y up, mirrored: the
                    // DXF arc runs from -end to -start.
                    pair(50, &num((-end).rem_euclid(360.0)));
                    pair(51, &num((-start).rem_euclid(360.0)));
                }
            }
            Prim::Text {
                at,
                height,
                angle,
                text,
                ink,
                ..
            } => {
                pair(0, "TEXT");
                pair(8, layer);
                pair(62, &aci(ink).to_string());
                pair(10, &num(at.0));
                pair(20, &num(y(at.1)));
                pair(40, &num(*height));
                pair(1, &dxf_text(text));
                if *angle != 0.0 {
                    pair(50, &num(angle.rem_euclid(360.0)));
                }
            }
        }
    }
    pair(0, "ENDSEC");
    pair(0, "EOF");
    out
}
