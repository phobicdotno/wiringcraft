//! A minimal PDF writer: pages with content streams, link annotations and outlines.

use std::collections::BTreeMap;
use std::fmt::Write as _;

pub const PT_PER_MM: f64 = 72.0 / 25.4;

/// A clickable rectangle (in page millimetres, y down) that jumps to another page.
#[derive(Debug, Clone)]
pub struct Link {
    pub min: (f64, f64),
    pub max: (f64, f64),
    pub target_page_id: String,
}

/// One page's drawing, already as PDF operators in points (y up).
#[derive(Debug, Clone, Default)]
pub struct Drawn {
    pub ops: String,
    pub links: Vec<Link>,
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
/// (covers æ ø å ä ö ü é ...); anything else becomes '?'.
pub fn pdf_text(s: &str) -> String {
    let mut out = String::from("<");
    for c in s.chars() {
        let b = match c as u32 {
            0x20..=0x7e | 0xa0..=0xff => c as u32 as u8,
            0x2013 => 0x96,
            0x2014 => 0x97,
            0x2022 => 0x95,
            0x2192 => b'>',
            _ => b'?',
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
        let mut objs: Vec<(usize, Vec<u8>)> = Vec::new();
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", page_obj(i))).collect();
        objs.push((
            1,
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R /PageMode /UseOutlines >>".to_vec(),
        ));
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
            objs.push((
                page_obj(i),
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w:.2} {h:.2}] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R /Annots [{}] >>",
                    page_obj(i) + 1,
                    annots.join(" ")
                )
                .into_bytes(),
            ));
            let stream = p.drawn.ops.as_bytes();
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

/// Builds a content stream in page millimetres (y down), converting to PDF points (y up).
pub struct Pen {
    pub ops: String,
    page_h: f64,
}

impl Pen {
    pub fn new(page_h_mm: f64) -> Self {
        Self {
            ops: String::new(),
            page_h: page_h_mm,
        }
    }

    fn p(&self, x: f64, y: f64) -> (f64, f64) {
        (x * PT_PER_MM, (self.page_h - y) * PT_PER_MM)
    }

    pub fn width(&mut self, mm: f64) {
        let _ = writeln!(self.ops, "{:.3} w", mm * PT_PER_MM);
    }

    pub fn colour(&mut self, r: f64, g: f64, b: f64) {
        let _ = writeln!(self.ops, "{r:.3} {g:.3} {b:.3} RG {r:.3} {g:.3} {b:.3} rg");
    }

    pub fn polyline(&mut self, pts: &[(f64, f64)], closed: bool) {
        let Some(first) = pts.first() else { return };
        let (x, y) = self.p(first.0, first.1);
        let _ = writeln!(self.ops, "{x:.2} {y:.2} m");
        for q in &pts[1..] {
            let (x, y) = self.p(q.0, q.1);
            let _ = writeln!(self.ops, "{x:.2} {y:.2} l");
        }
        self.ops.push_str(if closed { "s\n" } else { "S\n" });
    }

    pub fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.polyline(&[a, b], false);
    }

    pub fn rect(&mut self, min: (f64, f64), max: (f64, f64)) {
        let (x, y) = self.p(min.0, max.1);
        let _ = writeln!(
            self.ops,
            "{x:.2} {y:.2} {:.2} {:.2} re S",
            (max.0 - min.0) * PT_PER_MM,
            (max.1 - min.1) * PT_PER_MM
        );
    }

    pub fn fill_rect(&mut self, min: (f64, f64), max: (f64, f64)) {
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
        let segs = ((sweep / 90.0).ceil() as usize).max(1);
        let step = sweep / segs as f64;
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
        if s.is_empty() {
            return;
        }
        let size = height_mm * PT_PER_MM / 0.72; // cap height to font size
        let w = text_width_mm(s, height_mm);
        let x = match align {
            1 => at.0 - w / 2.0,
            2 => at.0 - w,
            _ => at.0,
        };
        let (px, py) = self.p(x, at.1);
        let _ = writeln!(self.ops, "BT /F1 {size:.2} Tf {px:.2} {py:.2} Td {} Tj ET", pdf_text(s));
    }
}

/// Approximate Helvetica advance widths (per 1000 em), enough for alignment.
pub fn text_width_mm(s: &str, height_mm: f64) -> f64 {
    let em = height_mm / 0.72;
    let units: f64 = s
        .chars()
        .map(|c| match c {
            'i' | 'j' | 'l' | '.' | ',' | ':' | ';' | '\'' | '!' | '|' => 250.0,
            'f' | 't' | 'r' | 'I' | ' ' | '-' | '(' | ')' | '[' | ']' | '/' => 320.0,
            'm' | 'w' | 'M' | 'W' => 850.0,
            c if c.is_ascii_uppercase() => 680.0,
            c if c.is_ascii_digit() => 556.0,
            _ => 540.0,
        })
        .sum();
    units / 1000.0 * em
}
