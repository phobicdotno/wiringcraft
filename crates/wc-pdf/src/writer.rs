//! A minimal PDF writer: pages with content streams, link annotations and outlines.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub const PT_PER_MM: f64 = 72.0 / 25.4;

/// A clickable rectangle (in page millimetres, y down) that jumps to another page.
#[derive(Debug, Clone)]
pub struct Link {
    pub min: (f64, f64),
    pub max: (f64, f64),
    pub target_page_id: String,
}

/// One page's drawing, already as PDF operators in points (y up), plus the same drawing as
/// primitives in page millimetres (y down) for the SVG and DXF exports.
#[derive(Debug, Clone, Default)]
pub struct Drawn {
    pub ops: String,
    pub links: Vec<Link>,
    pub prims: Vec<Prim>,
    /// Optional content layers the page draws on.
    pub layers: BTreeSet<Layer>,
    /// A picture drawn under everything else, on the `Scans` layer.
    pub underlay: Option<PageImage>,
    /// Pictures drawn over the page (the project logo). One image used on several pages is
    /// stored in the file once.
    pub pictures: Vec<Placed>,
}

/// A shared picture placed in a rectangle of a page, in millimetres (y down).
#[derive(Debug, Clone)]
pub struct Placed {
    pub image: std::sync::Arc<PageImage>,
    pub min: (f64, f64),
    pub max: (f64, f64),
}

/// A picture ready to embed: JPEG data as stored, or Flate-compressed samples.
#[derive(Debug, Clone, PartialEq)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    /// `DeviceGray`, `DeviceRGB` or `DeviceCMYK`.
    pub colour_space: &'static str,
    /// `DCTDecode` or `FlateDecode`.
    pub filter: &'static str,
    /// Extra dictionary entries, such as an inverted `/Decode` for Adobe CMYK JPEGs.
    pub extra: &'static str,
    pub data: Vec<u8>,
    /// Transparency: one Flate-compressed 8-bit alpha sample per pixel, written as the image's
    /// soft mask. `None` for an opaque picture.
    pub alpha: Option<Vec<u8>>,
    /// Where it goes on the page, in millimetres (y down).
    pub min: (f64, f64),
    pub max: (f64, f64),
}

/// Optional content a reader can hide: PDF layers (optional content groups) and DXF layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// Free drawings and texts on the sheet: notes, construction lines, imported graphics.
    Drawings,
    /// Wire numbers, potential names and cable cores next to wires.
    WireLabels,
    /// Cross-references between coils and contacts and to other pages.
    CrossReferences,
    /// Scanned sheets under the drawing.
    Scans,
}

impl Layer {
    pub const ALL: [Layer; 4] = [Layer::Drawings, Layer::WireLabels, Layer::CrossReferences, Layer::Scans];

    /// The name a reader shows.
    pub fn title(self) -> &'static str {
        match self {
            Layer::Drawings => "Drawings and notes",
            Layer::WireLabels => "Wire numbers",
            Layer::CrossReferences => "Cross-references",
            Layer::Scans => "Scanned sheets",
        }
    }

    /// The layer name in DXF files.
    pub fn dxf_name(self) -> &'static str {
        match self {
            Layer::Drawings => "DRAWINGS",
            Layer::WireLabels => "WIRE-NUMBERS",
            Layer::CrossReferences => "CROSS-REFERENCES",
            Layer::Scans => "SCANS",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Layer::Drawings => "OC1",
            Layer::WireLabels => "OC2",
            Layer::CrossReferences => "OC3",
            Layer::Scans => "OC4",
        }
    }
}

/// One drawn primitive in page millimetres (y down), with the pen state it was drawn with.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Path {
        points: Vec<(f64, f64)>,
        closed: bool,
        filled: bool,
        ink: Stroke,
        layer: Option<Layer>,
    },
    /// Arc from `start` to `end` degrees, clockwise on the page from +x; a full turn is a circle.
    Arc {
        center: (f64, f64),
        radius: f64,
        start: f64,
        end: f64,
        filled: bool,
        ink: Stroke,
        layer: Option<Layer>,
    },
    /// Left-aligned text: `at` is the start of the baseline, already shifted for alignment.
    Text {
        at: (f64, f64),
        height: f64,
        angle: f64,
        text: String,
        ink: Stroke,
        layer: Option<Layer>,
    },
}

/// Pen colour (0..1 RGB) and line width in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub rgb: (f64, f64, f64),
    pub width: f64,
}

struct PageData {
    size_mm: (f64, f64),
    title: String,
    drawn: Drawn,
}

pub struct Doc {
    title: String,
    pages: Vec<PageData>,
}

/// Encode text for the WinAnsi Helvetica font as a hex string. Latin-1 characters map directly
/// (covers æ ø å ä ö ü é ...), the euro, curly quotes, ellipsis, dashes and the rest of WinAnsi
/// to their codes; an arrow becomes '>' and anything else '?'.
pub fn pdf_text(s: &str) -> String {
    let mut out = String::from("<");
    for c in s.chars() {
        let b = match c {
            '\u{2192}' => b'>',
            _ => wc_core::font::winansi_code(c).unwrap_or(b'?'),
        };
        let _ = write!(out, "{b:02X}");
    }
    out.push('>');
    out
}

impl Doc {
    pub fn new(title: &str) -> Self {
        Self {
            title: title.to_owned(),
            pages: Vec::new(),
        }
    }

    pub fn add_page(&mut self, size_mm: (f64, f64), title: &str, drawn: Drawn) {
        self.pages.push(PageData {
            size_mm,
            title: title.to_owned(),
            drawn,
        });
    }

    /// `index` maps page ids to their position in this document, for links.
    pub fn finish(self, index: &BTreeMap<String, usize>) -> Vec<u8> {
        let n = self.pages.len();
        // Object numbers: 1 catalog, 2 pages, 3 font, 4 outlines, 5 info,
        // then per page i: page = 6+3i, content = 7+3i, outline item = 8+3i.
        let page_obj = |i: usize| 6 + 3 * i;
        // Optional content groups (layers) follow the pages, one object each.
        let used: Vec<Layer> = Layer::ALL
            .into_iter()
            .filter(|l| {
                self.pages
                    .iter()
                    .any(|p| p.drawn.layers.contains(l) || (*l == Layer::Scans && p.drawn.underlay.is_some()))
            })
            .collect();
        let ocg_obj = |k: usize| 6 + 3 * n + k;
        let ocg_refs: Vec<String> = (0..used.len()).map(|k| format!("{} 0 R", ocg_obj(k))).collect();
        // Underlay pictures follow the layers, one object per page that has one.
        let mut image_obj: BTreeMap<usize, usize> = BTreeMap::new();
        for (i, p) in self.pages.iter().enumerate() {
            if p.drawn.underlay.is_some() {
                image_obj.insert(i, 6 + 3 * n + used.len() + image_obj.len());
            }
        }
        // Placed pictures follow, one object per distinct image however many pages show it.
        let mut shared: Vec<(*const PageImage, usize)> = Vec::new();
        let mut objs: Vec<(usize, Vec<u8>)> = Vec::new();
        for p in &self.pages {
            for placed in &p.drawn.pictures {
                let key = std::sync::Arc::as_ptr(&placed.image);
                if !shared.iter().any(|(k, _)| *k == key) {
                    let obj = 6 + 3 * n + used.len() + image_obj.len() + shared.len();
                    shared.push((key, obj));
                }
            }
        }
        // Soft masks of transparent pictures come last, numbered as they are needed.
        let mut next_free = 6 + 3 * n + used.len() + image_obj.len() + shared.len();
        for p in &self.pages {
            for placed in &p.drawn.pictures {
                let key = std::sync::Arc::as_ptr(&placed.image);
                if let Some((_, obj)) = shared.iter().find(|(k, _)| *k == key)
                    && !objs.iter().any(|(o, _)| o == obj)
                {
                    objs.extend(image_objects(&placed.image, *obj, &mut next_free));
                }
            }
        }
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", page_obj(i))).collect();
        let oc = if used.is_empty() {
            String::new()
        } else {
            let refs = ocg_refs.join(" ");
            format!(" /OCProperties << /OCGs [{refs}] /D << /Order [{refs}] /ON [{refs}] >> >>")
        };
        objs.push((
            1,
            format!("<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R /PageMode /UseOutlines{oc} >>").into_bytes(),
        ));
        for (k, l) in used.iter().enumerate() {
            objs.push((
                ocg_obj(k),
                format!("<< /Type /OCG /Name {} >>", pdf_text(l.title())).into_bytes(),
            ));
        }
        let properties = if used.is_empty() {
            String::new()
        } else {
            let entries: Vec<String> = used
                .iter()
                .enumerate()
                .map(|(k, l)| format!("/{} {} 0 R", l.tag(), ocg_obj(k)))
                .collect();
            format!(" /Properties << {} >>", entries.join(" "))
        };
        objs.push((
            2,
            format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes(),
        ));
        objs.push((
            3,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
        ));
        if n > 0 {
            objs.push((
                4,
                format!(
                    "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {n} >>",
                    8,
                    8 + 3 * (n - 1)
                )
                .into_bytes(),
            ));
        } else {
            objs.push((4, b"<< /Type /Outlines /Count 0 >>".to_vec()));
        }
        objs.push((
            5,
            format!("<< /Title {} /Producer (WiringCraft) >>", pdf_text(&self.title)).into_bytes(),
        ));
        for (i, p) in self.pages.iter().enumerate() {
            let (w, h) = (p.size_mm.0 * PT_PER_MM, p.size_mm.1 * PT_PER_MM);
            let annots: Vec<String> = p
                .drawn
                .links
                .iter()
                .filter_map(|l| {
                    let target = index.get(&l.target_page_id)?;
                    let x0 = l.min.0 * PT_PER_MM;
                    let x1 = l.max.0 * PT_PER_MM;
                    let y0 = h - l.max.1 * PT_PER_MM;
                    let y1 = h - l.min.1 * PT_PER_MM;
                    Some(format!(
                        "<< /Type /Annot /Subtype /Link /Rect [{x0:.2} {y0:.2} {x1:.2} {y1:.2}] /Border [0 0 0] /Dest [{} 0 R /Fit] >>",
                        page_obj(*target)
                    ))
                })
                .collect();
            let mut ops = String::new();
            let mut xobjects = String::new();
            if let (Some(img), Some(obj)) = (&p.drawn.underlay, image_obj.get(&i)) {
                let (x0, x1) = (img.min.0 * PT_PER_MM, img.max.0 * PT_PER_MM);
                let (y0, y1) = (h - img.max.1 * PT_PER_MM, h - img.min.1 * PT_PER_MM);
                let _ = writeln!(
                    ops,
                    "/OC /{} BDC q {:.3} 0 0 {:.3} {x0:.3} {y0:.3} cm /Im0 Do Q EMC",
                    Layer::Scans.tag(),
                    x1 - x0,
                    y1 - y0
                );
                xobjects = format!("/Im0 {obj} 0 R");
                objs.extend(image_objects(img, *obj, &mut next_free));
            }
            ops.push_str(&p.drawn.ops);
            for (k, placed) in p.drawn.pictures.iter().enumerate() {
                let key = std::sync::Arc::as_ptr(&placed.image);
                let Some((_, obj)) = shared.iter().find(|(s, _)| *s == key) else {
                    continue;
                };
                let (x0, x1) = (placed.min.0 * PT_PER_MM, placed.max.0 * PT_PER_MM);
                let (y0, y1) = (h - placed.max.1 * PT_PER_MM, h - placed.min.1 * PT_PER_MM);
                let _ = writeln!(
                    ops,
                    "q {:.3} 0 0 {:.3} {x0:.3} {y0:.3} cm /Pic{k} Do Q",
                    x1 - x0,
                    y1 - y0
                );
                let _ = write!(xobjects, " /Pic{k} {obj} 0 R");
            }
            let xobjects = if xobjects.is_empty() {
                String::new()
            } else {
                format!(" /XObject << {} >>", xobjects.trim_start())
            };
            objs.push((
                page_obj(i),
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w:.2} {h:.2}] /Resources << /Font << /F1 3 0 R >>{properties}{xobjects} >> /Contents {} 0 R /Annots [{}] >>",
                    page_obj(i) + 1,
                    annots.join(" ")
                )
                .into_bytes(),
            ));
            let stream = ops.as_bytes();
            let mut content = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
            content.extend_from_slice(stream);
            content.extend_from_slice(b"\nendstream");
            objs.push((page_obj(i) + 1, content));
            let mut item = format!(
                "<< /Title {} /Parent 4 0 R /Dest [{} 0 R /Fit]",
                pdf_text(&p.title),
                page_obj(i)
            );
            if i > 0 {
                let _ = write!(item, " /Prev {} 0 R", page_obj(i - 1) + 2);
            }
            if i + 1 < n {
                let _ = write!(item, " /Next {} 0 R", page_obj(i + 1) + 2);
            }
            item.push_str(" >>");
            objs.push((page_obj(i) + 2, item.into_bytes()));
        }
        objs.sort_by_key(|o| o.0);

        let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = vec![0usize; objs.len() + 1];
        for (num, body) in &objs {
            if let Some(slot) = offsets.get_mut(*num) {
                *slot = out.len();
            }
            out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_at = out.len();
        let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
        for off in offsets.iter().skip(1) {
            let _ = writeln!(xref, "{off:010} 00000 n ");
        }
        let _ = write!(
            xref,
            "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objs.len() + 1
        );
        out.extend_from_slice(xref.as_bytes());
        out
    }
}

/// The image XObject holding `img`'s samples as object `obj`, and its soft mask (numbered from
/// `next_free`, which moves past it) when the picture is transparent.
fn image_objects(img: &PageImage, obj: usize, next_free: &mut usize) -> Vec<(usize, Vec<u8>)> {
    let stream = |dict: String, data: &[u8]| {
        let mut body = format!("{dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        body
    };
    let mut out = Vec::new();
    let mut smask = String::new();
    if let Some(alpha) = &img.alpha {
        let m = *next_free;
        *next_free += 1;
        smask = format!(" /SMask {m} 0 R");
        out.push((
            m,
            stream(
                format!(
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",
                    img.width, img.height
                ),
                alpha,
            ),
        ));
    }
    out.push((
        obj,
        stream(
            format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /{} /BitsPerComponent 8 /Filter /{}{}{smask}",
                img.width, img.height, img.colour_space, img.filter, img.extra
            ),
            &img.data,
        ),
    ));
    out
}

/// Builds a content stream in page millimetres (y down), converting to PDF points (y up).
pub struct Pen {
    pub ops: String,
    pub prims: Vec<Prim>,
    pub layers: BTreeSet<Layer>,
    layer: Option<Layer>,
    ink: Stroke,
    page_h: f64,
}

impl Pen {
    pub fn new(page_h_mm: f64) -> Self {
        Self {
            ops: String::new(),
            prims: Vec::new(),
            layers: BTreeSet::new(),
            layer: None,
            ink: Stroke {
                rgb: (0.0, 0.0, 0.0),
                width: 0.25,
            },
            page_h: page_h_mm,
        }
    }

    fn p(&self, x: f64, y: f64) -> (f64, f64) {
        (x * PT_PER_MM, (self.page_h - y) * PT_PER_MM)
    }

    pub fn width(&mut self, mm: f64) {
        let _ = writeln!(self.ops, "{:.3} w", mm * PT_PER_MM);
        self.ink.width = mm;
    }

    pub fn colour(&mut self, r: f64, g: f64, b: f64) {
        let _ = writeln!(self.ops, "{r:.3} {g:.3} {b:.3} RG {r:.3} {g:.3} {b:.3} rg");
        self.ink.rgb = (r, g, b);
    }

    pub fn polyline(&mut self, pts: &[(f64, f64)], closed: bool) {
        let Some(first) = pts.first() else { return };
        self.path(pts.to_vec(), closed, false);
        let (x, y) = self.p(first.0, first.1);
        let _ = writeln!(self.ops, "{x:.2} {y:.2} m");
        for q in &pts[1..] {
            let (x, y) = self.p(q.0, q.1);
            let _ = writeln!(self.ops, "{x:.2} {y:.2} l");
        }
        self.ops.push_str(if closed { "s\n" } else { "S\n" });
    }

    /// A closed outline filled with the pen colour.
    pub fn polygon_filled(&mut self, pts: &[(f64, f64)]) {
        let Some(first) = pts.first() else { return };
        self.path(pts.to_vec(), true, true);
        let (x, y) = self.p(first.0, first.1);
        let _ = writeln!(self.ops, "{x:.2} {y:.2} m");
        for q in &pts[1..] {
            let (x, y) = self.p(q.0, q.1);
            let _ = writeln!(self.ops, "{x:.2} {y:.2} l");
        }
        self.ops.push_str("h f\n");
    }

    pub fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.polyline(&[a, b], false);
    }

    fn path(&mut self, points: Vec<(f64, f64)>, closed: bool, filled: bool) {
        self.prims.push(Prim::Path {
            points,
            closed,
            filled,
            ink: self.ink,
            layer: self.layer,
        });
    }

    /// Draw what follows on an optional layer, until [`Pen::end_layer`]. Layers do not nest.
    pub fn begin_layer(&mut self, layer: Layer) {
        if self.layer.is_some() {
            self.end_layer();
        }
        let _ = writeln!(self.ops, "/OC /{} BDC", layer.tag());
        self.layers.insert(layer);
        self.layer = Some(layer);
    }

    pub fn end_layer(&mut self) {
        if self.layer.take().is_some() {
            self.ops.push_str("EMC\n");
        }
    }

    fn corners(min: (f64, f64), max: (f64, f64)) -> Vec<(f64, f64)> {
        vec![min, (max.0, min.1), max, (min.0, max.1)]
    }

    pub fn rect(&mut self, min: (f64, f64), max: (f64, f64)) {
        self.path(Self::corners(min, max), true, false);
        let (x, y) = self.p(min.0, max.1);
        let _ = writeln!(
            self.ops,
            "{x:.2} {y:.2} {:.2} {:.2} re S",
            (max.0 - min.0) * PT_PER_MM,
            (max.1 - min.1) * PT_PER_MM
        );
    }

    pub fn fill_rect(&mut self, min: (f64, f64), max: (f64, f64)) {
        self.path(Self::corners(min, max), true, true);
        let (x, y) = self.p(min.0, max.1);
        let _ = writeln!(
            self.ops,
            "{x:.2} {y:.2} {:.2} {:.2} re f",
            (max.0 - min.0) * PT_PER_MM,
            (max.1 - min.1) * PT_PER_MM
        );
    }

    /// Arc from `start` to `end` degrees, angles measured clockwise on the page from +x.
    pub fn arc(&mut self, c: (f64, f64), r: f64, start: f64, end: f64, filled: bool) {
        let mut sweep = end - start;
        if sweep <= 0.0 {
            sweep += 360.0;
        }
        self.prims.push(Prim::Arc {
            center: c,
            radius: r,
            start,
            end: start + sweep,
            filled,
            ink: self.ink,
            layer: self.layer,
        });
        // A negative sweep (an arc stored past a full turn) is drawn backwards, as long as it is.
        // A sweep of a full turn or more looks the same as one turn, which is all that is drawn:
        // angles stored far out (1e9° to 0°) cost no more than a circle.
        let drawn = sweep.clamp(-360.0, 360.0);
        let segs = ((drawn.abs() / 90.0).ceil() as usize).max(1);
        let step = drawn / segs as f64;
        let k = 4.0 / 3.0 * (step.to_radians() / 4.0).tan();
        let at = |deg: f64| {
            let t = deg.to_radians();
            (c.0 + r * t.cos(), c.1 + r * t.sin())
        };
        let d = |deg: f64| {
            let t = deg.to_radians();
            (-r * t.sin(), r * t.cos())
        };
        let s0 = at(start);
        let (x, y) = self.p(s0.0, s0.1);
        let _ = writeln!(self.ops, "{x:.2} {y:.2} m");
        for i in 0..segs {
            let a0 = start + step * i as f64;
            let a1 = a0 + step;
            let (p0, p3) = (at(a0), at(a1));
            let (d0, d1) = (d(a0), d(a1));
            let c1 = self.p(p0.0 + k * d0.0, p0.1 + k * d0.1);
            let c2 = self.p(p3.0 - k * d1.0, p3.1 - k * d1.1);
            let e = self.p(p3.0, p3.1);
            let _ = writeln!(
                self.ops,
                "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c",
                c1.0, c1.1, c2.0, c2.1, e.0, e.1
            );
        }
        self.ops.push_str(if filled { "f\n" } else { "S\n" });
    }

    pub fn circle(&mut self, c: (f64, f64), r: f64, filled: bool) {
        self.arc(c, r, 0.0, 360.0, filled);
    }

    /// Text with its baseline at `at`. `align`: 0 left, 1 centre, 2 right.
    pub fn text(&mut self, at: (f64, f64), height_mm: f64, align: u8, s: &str) {
        self.text_at_angle(at, height_mm, align, 0.0, s);
    }

    /// Text turned by `angle` degrees counter-clockwise around its anchor.
    pub fn text_at_angle(&mut self, at: (f64, f64), height_mm: f64, align: u8, angle: f64, s: &str) {
        if s.is_empty() {
            return;
        }
        let size = height_mm * PT_PER_MM / 0.72; // cap height to font size
        let w = text_width_mm(s, height_mm);
        let shift = match align {
            1 => -w / 2.0,
            2 => -w,
            _ => 0.0,
        };
        let angle = if angle.is_finite() { angle } else { 0.0 };
        let (sin, cos) = angle.to_radians().sin_cos();
        self.prims.push(Prim::Text {
            at: (at.0 + shift * cos, at.1 - shift * sin),
            height: height_mm,
            angle,
            text: s.to_owned(),
            ink: self.ink,
            layer: self.layer,
        });
        if angle == 0.0 {
            let (px, py) = self.p(at.0 + shift, at.1);
            let _ = writeln!(self.ops, "BT /F1 {size:.2} Tf {px:.2} {py:.2} Td {} Tj ET", pdf_text(s));
            return;
        }
        // The page's y runs down and the PDF's up, so a counter-clockwise turn on paper is a
        // counter-clockwise turn in PDF space too; the alignment shift runs along the text.
        let (px, py) = self.p(at.0 + shift * cos, at.1 - shift * sin);
        let _ = writeln!(
            self.ops,
            "BT /F1 {size:.2} Tf {cos:.5} {sin:.5} {:.5} {cos:.5} {px:.2} {py:.2} Tm {} Tj ET",
            -sin,
            pdf_text(s)
        );
    }
}

pub use wc_core::font::text_width_mm;
