//! SVG and DXF export of schematic pages and panel layouts. Both draw exactly what the PDF
//! export draws (frame, title block, symbols, wires, cross-references; plate, rails, ducts and
//! devices), from the primitives the same layout records.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use wc_core::{Project, Symbol};

use crate::writer::{Layer, Prim, Stroke};
use crate::{Options, Report, layout};

/// One page as drawing primitives in page millimetres (y down).
#[derive(Debug, Clone)]
pub struct PageDrawing {
    pub id: String,
    pub number: String,
    pub title: String,
    pub size_mm: (f64, f64),
    pub prims: Vec<Prim>,
    /// Pictures on the page: the underlay first (drawn under everything), then the project
    /// logo in each of its boxes (drawn on top). Only SVG shows them; DXF has no pictures.
    pub pictures: Vec<Picture>,
    /// A panel layout's sheet (from [`Options::panels`]) rather than a schematic page: `id` is
    /// `panel-<panel id>` and `title` the panel's name.
    pub panel: bool,
}

/// A picture file placed on a page, as it is embedded in an SVG.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    /// `image/png` or `image/jpeg`.
    pub mime: &'static str,
    /// The picture file as it is, shared between the pages that show it.
    pub bytes: std::sync::Arc<Vec<u8>>,
    /// Top-left and bottom-right corner on the page, in millimetres.
    pub min: (f64, f64),
    pub max: (f64, f64),
    /// Drawn over the page's drawing (the logo) rather than under it (an underlay).
    pub on_top: bool,
}

/// A picture file checked to be one the PDF export can show too, with its size in pixels.
fn checked(bytes: &[u8]) -> Result<(&'static str, (u32, u32)), String> {
    let img = crate::picture::decode(bytes)?;
    let mime = if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else {
        "image/png"
    };
    Ok((mime, (img.width, img.height)))
}

/// The bytes of picture `file` from `options`, or why it cannot be shown (`what` names it).
fn picture_bytes<'a>(options: &'a Options, file: &str, what: &str) -> Result<&'a Vec<u8>, String> {
    options
        .underlays
        .get(file)
        .ok_or_else(|| match options.unreadable.get(file) {
            Some(why) => format!("{what} picture {file} cannot be read: {why}"),
            None => format!("{what} picture {file} is missing"),
        })
}

/// The schematic pages chosen by `options.pages` (all when empty), drawn as for the PDF export,
/// followed by a sheet per panel layout in `options.panels`, drawn as its PDF sheet. Report
/// tables in `options` are not drawn; they are their own export.
pub fn page_drawings(
    project: &Project,
    symbols: &BTreeMap<String, Symbol>,
    options: &Options,
) -> (Vec<PageDrawing>, Report) {
    drawings(project, symbols, options, true)
}

/// As [`page_drawings`], but without the logo and underlays, for formats that do not show
/// pictures (DXF). Their files are not looked up; a warning names each page whose underlay is
/// left out, the logo once when a sheet has a place for it, and each text the DXF writes with
/// a `?` ([`dxf_warnings`]).
pub fn page_lines(
    project: &Project,
    symbols: &BTreeMap<String, Symbol>,
    options: &Options,
) -> (Vec<PageDrawing>, Report) {
    drawings(project, symbols, options, false)
}

fn drawings(
    project: &Project,
    symbols: &BTreeMap<String, Symbol>,
    options: &Options,
    with_pictures: bool,
) -> (Vec<PageDrawing>, Report) {
    let pages: Vec<&wc_core::Page> = project
        .pages
        .iter()
        .filter(|p| options.pages.is_empty() || options.pages.contains(&p.id))
        .collect();
    let mut report = Report {
        pages: pages.len() + options.panels.len(),
        warnings: Vec::new(),
    };
    // The logo is checked once, however many pages show it.
    let logo = project.meta.logo.as_ref().filter(|_| with_pictures).and_then(|l| {
        let found = picture_bytes(options, &l.file, "logo").and_then(|b| {
            checked(b)
                .map(|c| (c, b))
                .map_err(|why| format!("logo {}: {why}", l.file))
        });
        found
            .map_err(|w| report.warnings.push(w))
            .ok()
            .map(|((mime, size), b)| (mime, size, std::sync::Arc::new(b.clone())))
    });
    // Without pictures, whether a sheet had a place for the logo, to say it is left out.
    let mut logo_left_out = false;
    let xref = layout::CrossRefs::build(project, symbols);
    let total = pages.len() + options.panels.len();
    let mut logo_pictures = |page: &wc_core::Page| -> Vec<Picture> {
        if !with_pictures {
            logo_left_out |= !layout::logo_boxes(project, page).is_empty();
        }
        let Some((mime, size, bytes)) = &logo else {
            return Vec::new();
        };
        layout::logo_boxes(project, page)
            .into_iter()
            .map(|b| {
                let (min, max) = layout::fit_picture(b, *size);
                Picture {
                    mime,
                    bytes: bytes.clone(),
                    min: (min.x, min.y),
                    max: (max.x, max.y),
                    on_top: true,
                }
            })
            .collect()
    };
    let mut out: Vec<PageDrawing> = pages
        .iter()
        .enumerate()
        .map(|(i, page)| {
            let drawn = layout::draw_page(project, page, i, total, symbols, &xref, options, &mut report.warnings);
            let mut pictures = Vec::new();
            if let Some(u) = page.underlay.as_ref().filter(|_| !with_pictures) {
                report.warnings.push(format!(
                    "page {}: the underlay {} is left out: DXF has no pictures (the SVG export shows it)",
                    page.number, u.file
                ));
            }
            if let Some(u) = page.underlay.as_ref().filter(|_| with_pictures) {
                let found = picture_bytes(options, &u.file, "underlay")
                    .map_err(|w| format!("page {}: {w}", page.number))
                    .and_then(|b| {
                        crate::picture::underlay_rect(u)
                            .and_then(|rect| checked(b).map(|(mime, _)| (mime, b, rect)))
                            .map_err(|why| format!("page {}: underlay {}: {why}", page.number, u.file))
                    });
                match found {
                    Ok((mime, b, (min, max))) => pictures.push(Picture {
                        mime,
                        bytes: std::sync::Arc::new(b.clone()),
                        min,
                        max,
                        on_top: false,
                    }),
                    Err(w) => report.warnings.push(w),
                }
            }
            pictures.extend(logo_pictures(page));
            PageDrawing {
                id: page.id.clone(),
                number: page.number.clone(),
                title: page.title.clone(),
                size_mm: page.size.mm(),
                prims: drawn.prims,
                pictures,
                panel: false,
            }
        })
        .collect();
    // Panel layouts follow, on the first chosen page's size, as in the PDF.
    let size = pages.first().map(|p| p.size).unwrap_or_default();
    for (k, panel) in options.panels.iter().enumerate() {
        let mut page = wc_core::Page::new(format!("panel-{}", panel.id), format!("P{}", k + 1), panel.name.clone());
        page.size = size;
        let drawn = layout::draw_panel_page(
            project,
            &page,
            pages.len() + k,
            total,
            panel,
            &options.footprints,
            &options.date,
        );
        out.push(PageDrawing {
            pictures: logo_pictures(&page),
            id: page.id,
            number: page.number,
            title: page.title,
            size_mm: size.mm(),
            prims: drawn.prims,
            panel: true,
        });
    }
    if !with_pictures {
        for d in &out {
            report.warnings.extend(dxf_warnings(d, true));
        }
    }
    if let Some(l) = project.meta.logo.as_ref().filter(|_| logo_left_out) {
        report.warnings.push(format!(
            "the logo {} is left out: DXF has no pictures (the SVG export shows it)",
            l.file
        ));
    }
    (out, report)
}

/// The file name, without extension, of a panel layout's drawing: `panel-` and the panel's
/// name (its id when it has none), with every character but letters, digits and `-` written as
/// `_`, so `Main cabinet/A` becomes `panel-Main_cabinet_A`. The app and the command line both
/// name panel files this way.
pub fn panel_file_stem(d: &PageDrawing) -> String {
    let name = d.title.trim();
    let base = if name.is_empty() {
        d.id.strip_prefix("panel-").unwrap_or(&d.id)
    } else {
        name
    };
    let safe: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    format!("panel-{safe}")
}

/// The file name, without extension, of a schematic page's drawing: the page's number (its id
/// when it has none), with every character but letters, digits, `-`, `_` and `.` written as `_`
/// and leading dots left out, so `1.2` stays `1.2` and `A/1` becomes `A_1`; `page` when nothing
/// is left. A name Windows keeps for a device (`CON`, `NUL`, `COM1`, also before a dot as in
/// `nul.2`) gets a `_` in front, so `CON` becomes `_CON`. The app and the command line both
/// name page files this way.
pub fn page_file_stem(d: &PageDrawing) -> String {
    let number = d.number.trim();
    let base = if number.is_empty() { d.id.as_str() } else { number };
    let stem: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim_start_matches('.');
    if stem.is_empty() {
        "page".into()
    } else if names_a_device(stem) {
        format!("_{stem}")
    } else {
        stem.to_owned()
    }
}

/// Whether Windows takes the file name `stem` for a device such as `NUL` or `COM1`: the part
/// before the first dot is one of those names, in any case (`nul.txt` is a device as well).
fn names_a_device(stem: &str) -> bool {
    let first = stem.split('.').next().unwrap_or(stem).to_ascii_uppercase();
    match first.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => ["COM", "LPT"].iter().any(|port| {
            first
                .strip_prefix(port)
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
        }),
    }
}

/// Standard base64 with padding.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sextet = |n: u32| ABC.get(n as usize & 63).map_or('=', |c| char::from(*c));
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let (a, b, c, used) = match *chunk {
            [a, b, c] => (a, b, c, 4),
            [a, b] => (a, b, 0, 3),
            [a] => (a, 0, 0, 2),
            _ => continue,
        };
        let n = (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i < used { sextet(n >> shift) } else { '=' });
        }
    }
    out
}

/// An SVG `<image>` of a picture, stretched over its rectangle.
fn svg_image(out: &mut String, p: &Picture) {
    let _ = writeln!(
        out,
        "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" href=\"data:{};base64,{}\"/>",
        num(p.min.0),
        num(p.min.1),
        num(p.max.0 - p.min.0),
        num(p.max.1 - p.min.1),
        p.mime,
        base64(&p.bytes)
    );
}

pub(crate) fn num(v: f64) -> String {
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

/// `s` escaped for SVG text and attributes. Control characters and U+FFFE and U+FFFF, which
/// XML does not allow (a viewer refuses the whole file), are left out.
fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if c.is_control() || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => out.push(c),
        }
    }
    out
}

fn on_circle(c: (f64, f64), r: f64, deg: f64) -> (f64, f64) {
    let t = deg.to_radians();
    (c.0 + r * t.cos(), c.1 + r * t.sin())
}

/// The opening of a drawing group, without its closing `>`: the style every group shares.
const GROUP: &str = "<g fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\" \
                     font-family=\"Helvetica, Arial, sans-serif\"";

/// An SVG document of one page, sized in millimetres with one user unit per millimetre.
pub fn svg(page: &PageDrawing) -> String {
    let (w, h) = page.size_mm;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" \
         xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\" width=\"{w}mm\" \
         height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n<title>{} {}</title>\n<rect width=\"{w}\" height=\"{h}\" \
         fill=\"#ffffff\"/>\n{GROUP}>",
        xml(&page.number),
        xml(&page.title),
        w = num(w),
        h = num(h)
    );
    // Prims on an optional layer go in a group of their own that Inkscape shows as a layer. A new
    // group opens whenever the layer changes, so the drawing order stays as it is.
    let mut current: Option<Layer> = None;
    let mut opened: BTreeMap<Layer, usize> = BTreeMap::new();
    // The underlay (the scanned sheet) goes under the drawing, before the group the header
    // opened, in a layer of its own.
    let mut under = String::new();
    for p in page.pictures.iter().filter(|p| !p.on_top) {
        svg_image(&mut under, p);
    }
    if !under.is_empty() {
        opened.insert(Layer::Scans, 1);
        let at = out.rfind("<g ").unwrap_or(out.len());
        out.insert_str(
            at,
            &format!(
                "<g id=\"{}\" inkscape:groupmode=\"layer\" inkscape:label=\"{}\">\n{under}</g>\n",
                Layer::Scans.dxf_name().to_ascii_lowercase(),
                xml(Layer::Scans.title())
            ),
        );
    }
    for p in &page.prims {
        let (Prim::Path { layer, .. } | Prim::Arc { layer, .. } | Prim::Text { layer, .. }) = p;
        if *layer != current {
            out.push_str("</g>\n");
            match layer {
                Some(l) => {
                    let n = opened.entry(*l).or_insert(0);
                    *n += 1;
                    let name = l.dxf_name().to_ascii_lowercase();
                    let id = if *n == 1 { name } else { format!("{name}-{n}") };
                    let _ = writeln!(
                        out,
                        "{GROUP} id=\"{id}\" inkscape:groupmode=\"layer\" inkscape:label=\"{}\">",
                        xml(l.title())
                    );
                }
                None => {
                    let _ = writeln!(out, "{GROUP}>");
                }
            }
            current = *layer;
        }
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
                if sweep.abs() >= 359.999 {
                    let _ = writeln!(
                        out,
                        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" {paint}/>",
                        num(center.0),
                        num(center.1),
                        num(*radius)
                    );
                } else {
                    // Page angles run clockwise with y down, which is SVG's positive sweep. A
                    // negative sweep (an arc stored past a full turn) is the same path drawn
                    // from its end.
                    let (from, to) = if sweep < 0.0 { (end, start) } else { (start, end) };
                    let a = on_circle(*center, *radius, *from);
                    let b = on_circle(*center, *radius, *to);
                    let large = u8::from(sweep.abs() > 180.0);
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
                // `xml:space="preserve"` keeps leading and repeated spaces as the PDF prints them;
                // browsers, Inkscape and librsvg all honour it, unlike CSS `white-space`.
                let _ = writeln!(
                    out,
                    "<text xml:space=\"preserve\" x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" \
                     stroke=\"none\"{turn}>{}</text>",
                    num(at.0),
                    num(at.1),
                    num(height / 0.72),
                    rgb(ink),
                    xml(text)
                );
            }
        }
    }
    out.push_str("</g>\n");
    for p in page.pictures.iter().filter(|p| p.on_top) {
        svg_image(&mut out, p);
    }
    out.push_str("</svg>\n");
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
/// A backslash, and a `%` next to another `%`, are written as codes too, so readers do not take
/// them for a `\U+XXXX` or `%%` code; a character beyond U+FFFF, which has no four-digit code,
/// becomes `?` ([`dxf_warnings`] names the texts that have one).
pub(crate) fn dxf_text(s: &str) -> String {
    // Control characters are dropped first, so two `%` they stood between are seen as a pair.
    let chars: Vec<char> = s.chars().filter(|c| !c.is_control()).collect();
    let mut out = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        let percent_pair =
            ch == '%' && (i.checked_sub(1).and_then(|j| chars.get(j)) == Some(&'%') || chars.get(i + 1) == Some(&'%'));
        match ch {
            '\\' => out.push_str("\\U+005C"),
            '%' if percent_pair => out.push_str("\\U+0025"),
            // Caret notation: `^J` is read as a line feed and `^ ` as a caret. A caret at the
            // end starts no code and stays readable.
            '^' if i + 1 < chars.len() => out.push_str("\\U+005E"),
            c if c.is_ascii() => out.push(c),
            c if u32::from(c) > 0xFFFF => out.push('?'),
            c => {
                let _ = write!(out, "\\U+{:04X}", c as u32);
            }
        }
    }
    out
}

/// A warning for each text on `page` that the DXF writes with a `?` (a character beyond U+FFFF),
/// naming the page, once per text however often it is on the page. With `name_texts` it quotes
/// the text and how it is written; without, it leaves the text out (for a log).
pub fn dxf_warnings(page: &PageDrawing, name_texts: bool) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    page.prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } if text.chars().any(|c| u32::from(c) > 0xFFFF) => Some(text),
            _ => None,
        })
        .filter(|text| seen.insert(text.as_str()))
        .map(|text| {
            let what = if name_texts {
                let shown: String = text
                    .chars()
                    .filter(|c| !c.is_control())
                    .map(|c| if u32::from(c) > 0xFFFF { '?' } else { c })
                    .collect();
                format!("the text \"{text}\" is written as \"{shown}\"")
            } else {
                "a text is written with \"?\"".to_string()
            };
            format!(
                "page {}: {what} in the DXF: a DXF has no code for characters beyond U+FFFF, such as \
                 emoji (the SVG export keeps them)",
                page.number
            )
        })
        .collect()
}

/// A filled outline as DXF SOLID corners (10, 11, 12, 13 in that order; DXF draws them as
/// 1, 2, 4, 3): one SOLID for 3 or 4 points, a fan of triangles from the first point for more.
/// `None` when the outline is not convex (a fan would paint outside it): it stays an outline.
pub(crate) fn solid_triangles(points: &[(f64, f64)]) -> Option<Vec<[(f64, f64); 4]>> {
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for &p in points {
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 || pts.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return None;
    }
    // Convex: every corner turns the same way, and the turns add up to one full turn. A corner
    // that doubles straight back (a retraced segment) is a half turn either way: not convex.
    let (mut sign, mut turn) = (0.0f64, 0.0f64);
    let corners = pts
        .iter()
        .zip(pts.iter().cycle().skip(1))
        .zip(pts.iter().cycle().skip(2));
    for ((a, b), c) in corners {
        let (ux, uy, vx, vy) = (b.0 - a.0, b.1 - a.1, c.0 - b.0, c.1 - b.1);
        let (cross, dot) = (ux * vy - uy * vx, ux * vx + uy * vy);
        if cross.abs() > 1e-9 {
            if sign * cross < 0.0 {
                return None;
            }
            sign = cross.signum();
        } else if dot < 0.0 {
            return None;
        }
        turn += cross.atan2(dot);
    }
    if sign == 0.0 || turn.abs() > std::f64::consts::TAU + 1e-6 {
        return None;
    }
    Some(match pts.as_slice() {
        &[a, b, c] => vec![[a, b, c, c]],
        &[a, b, c, d] => vec![[a, b, d, c]],
        [first, rest @ ..] => rest
            .iter()
            .zip(rest.iter().skip(1))
            .map(|(&b, &c)| [*first, b, c, c])
            .collect(),
        [] => return None,
    })
}

/// Whether a stroke is drawn wider than the default 0.25 mm pen, so its DXF polyline carries
/// the width. Thinner strokes stay plain lines, as a CAD tool draws them at its default weight.
fn wide(ink: &Stroke) -> bool {
    ink.width.is_finite() && ink.width > 0.25 + 1e-9
}

/// The vertices of an arc as a DXF polyline, in page millimetres (y down), each with the bulge
/// of the segment it starts. A full turn is two half circles (a closed polyline); an arc is
/// cut into pieces of at most 90 degrees, so no bulge grows large.
fn arc_vertices(center: (f64, f64), radius: f64, start: f64, end: f64, full: bool) -> Vec<((f64, f64), f64)> {
    if full {
        let left = (center.0 - radius, center.1);
        let right = (center.0 + radius, center.1);
        return vec![(left, 1.0), (right, 1.0)];
    }
    let sweep = end - start;
    let pieces = ((sweep.abs() / 90.0).ceil() as usize).max(1);
    let step = sweep / pieces as f64;
    // Clockwise on the y-down page stays clockwise once y is flipped: a negative bulge.
    let bulge = -(step.to_radians() / 4.0).tan();
    (0..=pieces)
        .map(|k| {
            let at = on_circle(center, radius, start + step * k as f64);
            (at, if k < pieces { bulge } else { 0.0 })
        })
        .collect()
}

/// A bulge with six decimals (three would bend a long arc visibly off its circle).
fn bulge_num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.to_owned()
    }
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
                points,
                closed,
                filled,
                ink,
                ..
            } => {
                let colour = aci(ink).to_string();
                let solids = if *filled { solid_triangles(points) } else { None };
                if let Some(solids) = solids {
                    for corners in solids {
                        pair(0, "SOLID");
                        pair(8, layer);
                        pair(62, &colour);
                        for (i, (px, py)) in corners.iter().enumerate() {
                            pair(10 + i as u16, &num(*px));
                            pair(20 + i as u16, &num(y(*py)));
                        }
                    }
                } else if points.len() == 2 && !closed && !wide(ink) {
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
                    // A filled path that is not convex stays an outline, closed as its fill is.
                    pair(70, if *closed || *filled { "1" } else { "0" });
                    // R12 has no line weight: a wide stroke keeps its width as the polyline's
                    // start and end width, on the polyline and on each vertex.
                    let w = wide(ink).then(|| num(ink.width));
                    if let Some(w) = &w {
                        pair(40, w);
                        pair(41, w);
                    }
                    for (px, py) in points {
                        pair(0, "VERTEX");
                        pair(8, layer);
                        pair(10, &num(*px));
                        pair(20, &num(y(*py)));
                        if let Some(w) = &w {
                            pair(40, w);
                            pair(41, w);
                        }
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
                filled,
                ink,
                ..
            } => {
                let colour = aci(ink).to_string();
                let full = (end - start).abs() >= 359.999;
                if full && *filled {
                    // A solid dot: a closed polyline of two half circles (bulge 1) of radius
                    // r/2, drawn r wide, so it covers the whole disc.
                    let w = num(*radius);
                    pair(0, "POLYLINE");
                    pair(8, layer);
                    pair(62, &colour);
                    pair(66, "1");
                    pair(70, "1");
                    pair(40, &w);
                    pair(41, &w);
                    for dx in [-radius / 2.0, radius / 2.0] {
                        pair(0, "VERTEX");
                        pair(8, layer);
                        pair(10, &num(center.0 + dx));
                        pair(20, &num(y(center.1)));
                        pair(40, &w);
                        pair(41, &w);
                        pair(42, "1");
                    }
                    pair(0, "SEQEND");
                    pair(8, layer);
                    continue;
                }
                if !*filled && wide(ink) {
                    // R12 CIRCLE and ARC have no width: a wide one is a polyline of bulged
                    // segments along the same path, with its width as a wide line has.
                    let w = num(ink.width);
                    let vertices = arc_vertices(*center, *radius, *start, *end, full);
                    pair(0, "POLYLINE");
                    pair(8, layer);
                    pair(62, &colour);
                    pair(66, "1");
                    pair(70, if full { "1" } else { "0" });
                    pair(40, &w);
                    pair(41, &w);
                    for ((px, py), bulge) in vertices {
                        pair(0, "VERTEX");
                        pair(8, layer);
                        pair(10, &num(px));
                        pair(20, &num(y(py)));
                        pair(40, &w);
                        pair(41, &w);
                        if bulge != 0.0 {
                            pair(42, &bulge_num(bulge));
                        }
                    }
                    pair(0, "SEQEND");
                    pair(8, layer);
                    continue;
                }
                if full {
                    pair(0, "CIRCLE");
                } else {
                    pair(0, "ARC");
                }
                pair(8, layer);
                pair(62, &colour);
                pair(10, &num(center.0));
                pair(20, &num(y(center.1)));
                pair(40, &num(*radius));
                if !full {
                    // Clockwise on a y-down page is counter-clockwise with y up, mirrored: the
                    // DXF arc runs from -end to -start (from -start to -end for a negative
                    // sweep, drawn the other way round).
                    let (from, to) = if end < start { (end, start) } else { (start, end) };
                    pair(50, &num((-to).rem_euclid(360.0)));
                    pair(51, &num((-from).rem_euclid(360.0)));
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

#[cfg(test)]
mod tests {
    use super::*;

    const THIN: Stroke = Stroke {
        rgb: (0.0, 0.0, 0.0),
        width: 0.25,
    };

    fn page(prims: Vec<Prim>) -> PageDrawing {
        PageDrawing {
            id: "p-1".into(),
            number: "1".into(),
            title: String::new(),
            size_mm: (100.0, 50.0),
            prims,
            pictures: Vec::new(),
            panel: false,
        }
    }

    fn text(s: &str) -> Prim {
        Prim::Text {
            at: (1.0, 2.0),
            height: 2.5,
            angle: 0.0,
            text: s.into(),
            ink: THIN,
            layer: None,
        }
    }

    /// The DXF as `(code, value)` pairs.
    fn pairs(dxf: &str) -> Vec<(String, String)> {
        let lines: Vec<&str> = dxf.lines().collect();
        lines
            .chunks(2)
            .filter_map(|c| match c {
                [code, value] => Some(((*code).to_owned(), (*value).to_owned())),
                _ => None,
            })
            .collect()
    }

    /// The pairs of the first entity named `kind`, up to the next entity.
    fn entity(dxf: &str, kind: &str) -> Vec<(String, String)> {
        let all = pairs(dxf);
        let Some(start) = all.iter().position(|(c, v)| c == "0" && v == kind) else {
            unreachable!("no {kind} in {dxf}")
        };
        all.iter()
            .skip(start + 1)
            .take_while(|(c, _)| c != "0")
            .cloned()
            .collect()
    }

    fn value<'a>(pairs: &'a [(String, String)], code: &str) -> Option<&'a str> {
        pairs.iter().find(|(c, _)| c == code).map(|(_, v)| v.as_str())
    }

    #[test]
    fn num_rounds_to_three_decimals_and_never_writes_minus_zero_or_nan() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(0.12349), "0.123");
        assert_eq!(num(2.0004), "2");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(-3.25), "-3.25");
        assert_eq!(num(100.0), "100");
        assert_eq!(num(f64::NAN), "0");
        assert_eq!(num(f64::INFINITY), "0");
        assert_eq!(num(f64::NEG_INFINITY), "0");
    }

    #[test]
    fn bulge_num_keeps_six_decimals() {
        assert_eq!(bulge_num(1.0), "1");
        assert_eq!(bulge_num(-0.4142135), "-0.414214");
        assert_eq!(bulge_num(-0.0000001), "0");
        assert_eq!(bulge_num(0.0), "0");
    }

    #[test]
    fn xml_escapes_markup_and_drops_characters_xml_forbids() {
        assert_eq!(xml("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
        assert_eq!(xml("x\u{0}y\u{1B}z\n\t"), "xyz");
        assert_eq!(xml("\u{FFFE}ok\u{FFFF}"), "ok");
        // An apostrophe and non-ASCII text stay as they are.
        assert_eq!(xml("it's Ålesund \u{1F600}"), "it's Ålesund \u{1F600}");
    }

    #[test]
    fn colours_clamp_to_hex_and_map_to_the_nearest_aci() {
        let ink = |r, g, b| Stroke {
            rgb: (r, g, b),
            width: 0.25,
        };
        assert_eq!(rgb(&ink(0.0, 0.5, 1.0)), "#0080ff");
        assert_eq!(rgb(&ink(-1.0, 2.0, 0.2)), "#00ff33");
        // Greys: black/dark is 7, mid grey 8, light grey 9.
        assert_eq!(aci(&ink(0.0, 0.0, 0.0)), 7);
        assert_eq!(aci(&ink(0.4, 0.42, 0.38)), 8);
        assert_eq!(aci(&ink(0.8, 0.8, 0.8)), 9);
        // Colours go to red, green or blue by their largest channel.
        assert_eq!(aci(&ink(0.9, 0.1, 0.1)), 1);
        assert_eq!(aci(&ink(0.1, 0.8, 0.2)), 3);
        assert_eq!(aci(&ink(0.1, 0.2, 0.9)), 5);
    }

    #[test]
    fn arc_vertices_cut_sweeps_into_quarter_turns_with_signed_bulges() {
        let full = arc_vertices((10.0, 10.0), 2.0, 0.0, 360.0, true);
        assert_eq!(full, vec![((8.0, 10.0), 1.0), ((12.0, 10.0), 1.0)]);

        // A half turn is two quarter pieces: three vertices, the last without a bulge.
        let half = arc_vertices((0.0, 0.0), 1.0, 0.0, 180.0, false);
        assert_eq!(half.len(), 3);
        let quarter = (std::f64::consts::FRAC_PI_2 / 4.0).tan();
        let [(a, ba), (b, bb), (c, bc)] = half.as_slice() else {
            unreachable!("{half:?}")
        };
        assert!((a.0 - 1.0).abs() < 1e-9 && a.1.abs() < 1e-9, "{a:?}");
        assert!(b.0.abs() < 1e-9 && (b.1 - 1.0).abs() < 1e-9, "{b:?}");
        assert!((c.0 + 1.0).abs() < 1e-9 && c.1.abs() < 1e-9, "{c:?}");
        assert!((ba + quarter).abs() < 1e-12 && (bb + quarter).abs() < 1e-12);
        assert_eq!(*bc, 0.0);

        // A negative sweep bends the other way; a small one is one piece.
        let back = arc_vertices((0.0, 0.0), 1.0, 30.0, 0.0, false);
        assert_eq!(back.len(), 2);
        let Some((_, bulge)) = back.first() else { unreachable!() };
        assert!(*bulge > 0.0, "{back:?}");
        // A sweep just over a quarter turn needs two pieces.
        assert_eq!(arc_vertices((0.0, 0.0), 1.0, 0.0, 91.0, false).len(), 3);
    }

    #[test]
    fn wide_only_for_finite_strokes_over_the_default_pen() {
        let w = |width| Stroke {
            rgb: (0.0, 0.0, 0.0),
            width,
        };
        assert!(!wide(&w(0.25)));
        assert!(!wide(&w(0.1)));
        assert!(wide(&w(0.35)));
        assert!(!wide(&w(f64::NAN)));
        assert!(!wide(&w(f64::INFINITY)));
    }

    #[test]
    fn superscript_port_numbers_name_devices_too() {
        assert!(names_a_device("COM¹"));
        assert!(names_a_device("lpt³.txt"));
        assert!(names_a_device("prn"));
        assert!(!names_a_device("COM⁴"));
        assert!(!names_a_device("COM"));
        assert!(!names_a_device("NULL"));
    }

    #[test]
    fn dxf_flips_y_and_writes_thin_and_wide_lines_differently() {
        let line = |width| Prim::Path {
            points: vec![(10.0, 5.0), (20.0, 15.0)],
            closed: false,
            filled: false,
            ink: Stroke {
                rgb: (0.0, 0.0, 0.0),
                width,
            },
            layer: None,
        };
        let out = dxf(&page(vec![line(0.25)]));
        assert!(out.ends_with("0\nEOF\n"), "{out}");
        let l = entity(&out, "LINE");
        assert_eq!(value(&l, "8"), Some("0"));
        assert_eq!(value(&l, "10"), Some("10"));
        assert_eq!(value(&l, "20"), Some("45"));
        assert_eq!(value(&l, "21"), Some("35"));
        assert_eq!(value(&l, "62"), Some("7"));

        let out = dxf(&page(vec![line(0.5)]));
        assert!(!out.contains("\nLINE\n"), "{out}");
        let p = entity(&out, "POLYLINE");
        assert_eq!(value(&p, "70"), Some("0"));
        assert_eq!(value(&p, "40"), Some("0.5"));
        assert_eq!(value(&p, "41"), Some("0.5"));
        assert_eq!(out.matches("\nVERTEX\n").count(), 2);
        assert!(out.contains("0\nSEQEND\n"));
    }

    #[test]
    fn dxf_mirrors_arc_angles_and_keeps_text_rotation() {
        let arc = Prim::Arc {
            center: (50.0, 20.0),
            radius: 5.0,
            start: 0.0,
            end: 90.0,
            filled: false,
            ink: THIN,
            layer: None,
        };
        let turned = Prim::Text {
            at: (3.0, 4.0),
            height: 2.5,
            angle: -90.0,
            text: "50%%".into(),
            ink: THIN,
            layer: None,
        };
        let out = dxf(&page(vec![arc, turned]));
        let a = entity(&out, "ARC");
        assert_eq!(value(&a, "10"), Some("50"));
        assert_eq!(value(&a, "20"), Some("30"));
        assert_eq!(value(&a, "40"), Some("5"));
        // Clockwise 0..90 on the y-down page runs 270..0 counter-clockwise with y up.
        assert_eq!(value(&a, "50"), Some("270"));
        assert_eq!(value(&a, "51"), Some("0"));
        let t = entity(&out, "TEXT");
        assert_eq!(value(&t, "50"), Some("270"));
        assert_eq!(value(&t, "1"), Some("50\\U+0025\\U+0025"));
        assert_eq!(value(&t, "20"), Some("46"));
    }

    #[test]
    fn dxf_warnings_quote_each_text_once_with_how_it_is_written() {
        let d = page(vec![
            text("ok"),
            text("go \u{1F680}\n"),
            text("go \u{1F680}\n"),
            text("\u{1F600}"),
        ]);
        let warnings = dxf_warnings(&d, true);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let [first, second] = warnings.as_slice() else {
            unreachable!("{warnings:?}")
        };
        assert!(
            first.starts_with("page 1: the text \"go \u{1F680}\n\" is written as \"go ?\" in the DXF"),
            "{first}"
        );
        assert!(second.contains("is written as \"?\""), "{second}");
        assert!(dxf_warnings(&page(vec![text("plain Æ")]), true).is_empty());
    }
}
