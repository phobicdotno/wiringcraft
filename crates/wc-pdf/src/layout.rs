//! Page layout: frame, column grid, title block, symbols, wires and cross-references.

use std::collections::BTreeMap;

use wc_core::geom::{Align, Point, Shape};
use wc_core::symbol::SymbolRole;
use wc_core::{DeviceTag, Endpoint, Page, PageSize, PlacedSymbol, Project, Symbol};

use crate::Options;
use crate::writer::{Drawn, Link, Pen, text_width_mm};

/// Number of reference columns across the drawing area.
pub const FRAME_COLUMNS: usize = 10;
const MARGIN: f64 = 10.0;
const HEADER: f64 = 6.0;
const TITLE_H: f64 = 24.0;

/// The column (1-based) a point falls into.
pub fn column_of(size: PageSize, x: f64) -> usize {
    let (w, _) = size.mm();
    let col_w = (w - 2.0 * MARGIN) / FRAME_COLUMNS as f64;
    (((x - MARGIN) / col_w).floor() as i64 + 1).clamp(1, FRAME_COLUMNS as i64) as usize
}

/// A pin's position on the page.
pub fn pin_position(placed: &PlacedSymbol, symbol: &Symbol, pin: &str) -> Option<Point> {
    symbol
        .pin(pin)
        .map(|p| p.at.transform(placed.rotation, placed.mirror) + placed.at)
}

/// One appearance of a tagged symbol.
#[derive(Debug, Clone)]
pub struct Appearance {
    pub page_id: String,
    pub page_number: String,
    pub column: usize,
    /// The pins, for contact listings such as `13-14`.
    pub pins: Vec<String>,
}

impl Appearance {
    pub fn location(&self) -> String {
        format!("{}.{}", self.page_number, self.column)
    }
}

#[derive(Debug, Default)]
pub struct CrossRefs {
    pub mains: BTreeMap<DeviceTag, Vec<Appearance>>,
    pub auxes: BTreeMap<DeviceTag, Vec<Appearance>>,
}

impl CrossRefs {
    pub fn build(project: &Project, symbols: &BTreeMap<String, Symbol>) -> Self {
        let mut x = CrossRefs::default();
        for page in &project.pages {
            for s in &page.symbols {
                let (Some(tag), Some(sym)) = (&s.tag, symbols.get(&s.symbol)) else {
                    continue;
                };
                let a = Appearance {
                    page_id: page.id.clone(),
                    page_number: page.number.clone(),
                    column: column_of(page.size, s.at.x),
                    pins: sym.pins.iter().map(|p| p.name.clone()).collect(),
                };
                match sym.role {
                    SymbolRole::Main => x.mains.entry(tag.clone()).or_default().push(a),
                    SymbolRole::Auxiliary => x.auxes.entry(tag.clone()).or_default().push(a),
                    _ => {}
                }
            }
        }
        x
    }
}

fn tuple(p: Point) -> (f64, f64) {
    (p.x, p.y)
}

fn draw_shape(pen: &mut Pen, s: &Shape) {
    match s {
        Shape::Line { a, b } => pen.line(tuple(*a), tuple(*b)),
        Shape::Polyline { points, closed } => {
            let pts: Vec<(f64, f64)> = points.iter().map(|p| tuple(*p)).collect();
            pen.polyline(&pts, *closed);
        }
        Shape::Rect { min, max } => pen.rect(tuple(*min), tuple(*max)),
        Shape::Circle { center, radius, filled } => pen.circle(tuple(*center), *radius, *filled),
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => pen.arc(tuple(*center), *radius, *start, *end, false),
        Shape::Text {
            at,
            text,
            height,
            align,
        } => {
            let a = match align {
                Align::Left => 0,
                Align::Center => 1,
                Align::Right => 2,
            };
            pen.text(tuple(*at), *height, a, text);
        }
    }
}

/// How a frame mark is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    /// Heavy outline.
    Thick,
    /// Normal lines and values.
    Thin,
    /// Small grey field labels.
    Label,
}

/// One line or text of the page frame and title block.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameMark {
    pub shape: Shape,
    pub ink: Ink,
}

/// The page frame: border, numbered column strip and title block, filled from the project's
/// title-block fields. The editor and the PDF both draw this list, so they always match.
/// `date` is shown when the project has no `date` field of its own.
pub fn frame_marks(project: &Project, page: &Page, index: usize, total: usize, date: &str) -> Vec<FrameMark> {
    let (w, h) = page.size.mm();
    let (x0, y0, x1, y1) = (MARGIN, MARGIN, w - MARGIN, h - MARGIN);
    let mut out = Vec::new();
    let line = |a: (f64, f64), b: (f64, f64), ink: Ink, out: &mut Vec<FrameMark>| {
        out.push(FrameMark {
            shape: Shape::Line {
                a: Point::new(a.0, a.1),
                b: Point::new(b.0, b.1),
            },
            ink,
        });
    };
    let text = |at: (f64, f64), height: f64, align: Align, t: &str, ink: Ink, out: &mut Vec<FrameMark>| {
        if !t.is_empty() {
            out.push(FrameMark {
                shape: Shape::Text {
                    at: Point::new(at.0, at.1),
                    text: t.to_owned(),
                    height,
                    align,
                },
                ink,
            });
        }
    };
    out.push(FrameMark {
        shape: Shape::Rect {
            min: Point::new(x0, y0),
            max: Point::new(x1, y1),
        },
        ink: Ink::Thick,
    });
    // Column strip along the top.
    line((x0, y0 + HEADER), (x1, y0 + HEADER), Ink::Thin, &mut out);
    let col_w = (x1 - x0) / FRAME_COLUMNS as f64;
    for c in 0..FRAME_COLUMNS {
        let cx = x0 + col_w * c as f64;
        if c > 0 {
            line((cx, y0), (cx, y0 + HEADER), Ink::Thin, &mut out);
        }
        text(
            (cx + col_w / 2.0, y0 + HEADER - 1.8),
            2.5,
            Align::Center,
            &(c + 1).to_string(),
            Ink::Thin,
            &mut out,
        );
    }
    // Title block: a full-width band at the bottom, divided into fields.
    let ty = y1 - TITLE_H;
    let mid = ty + TITLE_H / 2.0;
    line((x0, ty), (x1, ty), Ink::Thick, &mut out);
    let fx = [x0, x0 + 80.0, x1 - 150.0, x1 - 60.0, x1 - 30.0];
    for x in fx.iter().skip(1) {
        line((*x, ty), (*x, y1), Ink::Thin, &mut out);
    }
    line((fx[0], mid), (fx[1], mid), Ink::Thin, &mut out);
    line((fx[0] + 40.0, mid), (fx[0] + 40.0, y1), Ink::Thin, &mut out);
    line((fx[2], mid), (x1, mid), Ink::Thin, &mut out);
    let tb = &project.meta.title_block;
    let get = |k: &str| tb.get(k).map(String::as_str).unwrap_or("");
    let field = |x: f64, y: f64, label: &str, value: &str, height: f64, out: &mut Vec<FrameMark>| {
        text((x + 1.5, y + 3.0), 1.8, Align::Left, label, Ink::Label, out);
        text((x + 1.5, y + 9.5), height, Align::Left, value, Ink::Thin, out);
    };
    field(fx[0], ty, "Customer", get("customer"), 3.0, &mut out);
    field(fx[0], mid, "Drawn by", get("drawn_by"), 2.5, &mut out);
    field(fx[0] + 40.0, mid, "Checked by", get("checked_by"), 2.5, &mut out);
    text(
        (fx[1] + 1.5, ty + 3.0),
        1.8,
        Align::Left,
        "Project",
        Ink::Label,
        &mut out,
    );
    text(
        (fx[1] + 3.0, ty + 11.0),
        4.0,
        Align::Left,
        &project.meta.name,
        Ink::Thin,
        &mut out,
    );
    text(
        (fx[1] + 3.0, ty + 19.5),
        3.0,
        Align::Left,
        &page.title,
        Ink::Thin,
        &mut out,
    );
    let numbers: Vec<&str> = [get("project_number"), get("plant")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    text(
        (fx[2] - 2.0, ty + 22.0),
        2.2,
        Align::Right,
        &numbers.join("  \u{b7}  "),
        Ink::Thin,
        &mut out,
    );
    field(fx[2], ty, "Revision", get("revision"), 2.5, &mut out);
    let d = if get("date").is_empty() { date } else { get("date") };
    field(fx[2], mid, "Date", d, 2.5, &mut out);
    text((fx[3] + 1.5, ty + 3.0), 1.8, Align::Left, "Page", Ink::Label, &mut out);
    text(
        (fx[3] + 15.0, ty + 10.5),
        4.0,
        Align::Center,
        &page.number,
        Ink::Thin,
        &mut out,
    );
    text(
        (fx[3] + 1.5, mid + 3.0),
        1.8,
        Align::Left,
        "Location",
        Ink::Label,
        &mut out,
    );
    text(
        (fx[3] + 15.0, mid + 9.5),
        2.5,
        Align::Center,
        get("location"),
        Ink::Thin,
        &mut out,
    );
    text((fx[4] + 1.5, ty + 3.0), 1.8, Align::Left, "Sheet", Ink::Label, &mut out);
    text(
        (fx[4] + 15.0, ty + 10.0),
        3.0,
        Align::Center,
        &format!("{} / {}", index + 1, total),
        Ink::Thin,
        &mut out,
    );
    text(
        (fx[4] + 15.0, mid + 9.5),
        2.0,
        Align::Center,
        "WiringCraft",
        Ink::Thin,
        &mut out,
    );
    out
}

fn frame(pen: &mut Pen, project: &Project, page: &Page, index: usize, total: usize, date: &str) {
    for m in frame_marks(project, page, index, total, date) {
        match m.ink {
            Ink::Thick => {
                pen.colour(0.0, 0.0, 0.0);
                pen.width(0.5);
            }
            Ink::Thin => {
                pen.colour(0.0, 0.0, 0.0);
                pen.width(0.25);
            }
            Ink::Label => pen.colour(0.45, 0.45, 0.45),
        }
        draw_shape(pen, &m.shape);
    }
    pen.colour(0.0, 0.0, 0.0);
}

#[allow(clippy::too_many_arguments)]
pub fn draw_page(
    project: &Project,
    page: &Page,
    index: usize,
    total: usize,
    symbols: &BTreeMap<String, Symbol>,
    xref: &CrossRefs,
    options: &Options,
    warnings: &mut Vec<String>,
) -> Drawn {
    let (_, h) = page.size.mm();
    let mut pen = Pen::new(h);
    let mut links = Vec::new();
    pen.ops.push_str("1 J 1 j\n");
    pen.colour(0.0, 0.0, 0.0);
    frame(&mut pen, project, page, index, total, &options.date);

    // Free graphics: texts, lines and rectangles drawn on the sheet, and imported drawings.
    pen.colour(0.0, 0.0, 0.0);
    pen.width(0.25);
    for g in &page.graphics {
        draw_shape(&mut pen, g);
    }

    // Wires under symbols.
    pen.colour(0.0, 0.0, 0.0);
    pen.width(0.35);
    let mut polylines: Vec<Vec<(f64, f64)>> = Vec::new();
    for w in &page.wires {
        let resolve = |e: &Endpoint| -> Option<Point> {
            match e {
                Endpoint::Point { at } => Some(*at),
                Endpoint::Pin { item, pin } => {
                    let placed = page.symbol(item)?;
                    pin_position(placed, symbols.get(&placed.symbol)?, pin)
                }
            }
        };
        let (Some(a), Some(b)) = (resolve(&w.from), resolve(&w.to)) else {
            warnings.push(format!(
                "page {}: wire {} has an end on a missing symbol or pin",
                page.number, w.id
            ));
            continue;
        };
        let mut pts = vec![tuple(a)];
        pts.extend(w.via.iter().map(|p| tuple(*p)));
        pts.push(tuple(b));
        pen.polyline(&pts, false);
        if options.wire_labels {
            let label: Vec<&str> = [w.number.as_deref(), w.potential.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            if !label.is_empty() {
                // Left of a vertical first segment, above a horizontal one; pin numbers sit on the right.
                let (p, q) = (pts[0], pts[1]);
                let mid = ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
                if (p.0 - q.0).abs() < (p.1 - q.1).abs() {
                    pen.text((mid.0 - 1.0, mid.1 + 0.8), 2.0, 2, &label.join(" "));
                } else {
                    pen.text((mid.0, mid.1 - 1.0), 2.0, 1, &label.join(" "));
                }
            }
        }
        polylines.push(pts);
    }
    for (x, y) in junctions(&polylines) {
        pen.circle((x, y), 0.6, true);
    }

    // Symbols.
    pen.width(0.3);
    for s in &page.symbols {
        let Some(sym) = symbols.get(&s.symbol) else {
            warnings.push(format!(
                "page {}: symbol {:?} is not in the project's library copy",
                page.number, s.symbol
            ));
            pen.colour(0.85, 0.1, 0.1);
            pen.rect((s.at.x - 4.0, s.at.y - 4.0), (s.at.x + 4.0, s.at.y + 4.0));
            pen.text((s.at.x, s.at.y + 1.0), 2.5, 1, "?");
            pen.colour(0.0, 0.0, 0.0);
            continue;
        };
        for g in &sym.graphics {
            draw_shape(&mut pen, &g.placed(s.at, s.rotation, s.mirror));
        }
        let Some(tag) = &s.tag else { continue };
        let ta = sym.tag_at.transform(s.rotation, s.mirror) + s.at;
        // One aspect per line, right-aligned, centred on the tag position.
        let mut lines = Vec::new();
        if !tag.function.is_empty() {
            lines.push(format!("={}", tag.function.join(".")));
        }
        if !tag.location.is_empty() {
            lines.push(format!("+{}", tag.location.join(".")));
        }
        lines.push(format!("-{}", tag.product.join(".")));
        let top = ta.y - (lines.len() as f64 - 1.0) * 1.5 + 1.2;
        for (i, l) in lines.iter().enumerate() {
            pen.text((ta.x, top + 3.0 * i as f64), 2.5, 2, l);
        }

        // Cross-references: a main symbol lists its contacts; a contact points back to its main.
        let line_h = 2.6;
        match sym.role {
            SymbolRole::Main => {
                let auxes = xref.auxes.get(tag).map(Vec::as_slice).unwrap_or(&[]);
                let below = sym
                    .graphics
                    .iter()
                    .map(|g| g.placed(s.at, s.rotation, s.mirror).bounds().1.y)
                    .fold(s.at.y, f64::max);
                for (i, a) in auxes.iter().enumerate() {
                    let text = format!("{} {}", a.pins.join("-"), a.location());
                    let at = (s.at.x + 5.0, below + 3.5 + line_h * i as f64);
                    pen.colour(0.1, 0.3, 0.7);
                    pen.text(at, 1.8, 0, &text);
                    pen.colour(0.0, 0.0, 0.0);
                    let wdt = text_width_mm(&text, 1.8);
                    links.push(Link {
                        min: (at.0, at.1 - 2.0),
                        max: (at.0 + wdt, at.1 + 0.5),
                        target_page_id: a.page_id.clone(),
                    });
                }
            }
            SymbolRole::Auxiliary => {
                if let Some(main) = xref.mains.get(tag).and_then(|m| m.first()) {
                    let text = format!("> {}", main.location());
                    let at = (ta.x - text_width_mm(&text, 1.8), top + 3.0 * lines.len() as f64);
                    pen.colour(0.1, 0.3, 0.7);
                    pen.text(at, 1.8, 0, &text);
                    pen.colour(0.0, 0.0, 0.0);
                    let wdt = text_width_mm(&text, 1.8);
                    links.push(Link {
                        min: (at.0, at.1 - 2.0),
                        max: (at.0 + wdt, at.1 + 0.5),
                        target_page_id: main.page_id.clone(),
                    });
                } else {
                    warnings.push(format!("page {}: contact {tag} has no coil", page.number));
                }
            }
            _ => {}
        }
    }
    Drawn { ops: pen.ops, links }
}

/// Points where three or more wire branches meet: wire ends and bends count once per wire
/// touching them, and a point in the middle of another wire's segment counts twice (a T).
fn junctions(polylines: &[Vec<(f64, f64)>]) -> Vec<(f64, f64)> {
    const EPS: f64 = 0.01;
    let on_segment = |p: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        if len2 < EPS * EPS {
            return false;
        }
        let t = ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2;
        let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
        t > 0.0 && t < 1.0 && (cx - p.0).hypot(cy - p.1) < EPS
    };
    let same = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < EPS && (a.1 - b.1).abs() < EPS;
    let mut out: Vec<(f64, f64)> = Vec::new();
    for pl in polylines {
        for v in pl {
            if out.iter().any(|o| same(*o, *v)) {
                continue;
            }
            let mut degree = 0;
            for other in polylines {
                let n = other.len();
                for (i, q) in other.iter().enumerate() {
                    if same(*q, *v) {
                        degree += if i == 0 || i + 1 == n { 1 } else { 2 };
                    }
                }
                for seg in other.windows(2) {
                    if on_segment(*v, seg[0], seg[1]) {
                        degree += 2;
                    }
                }
            }
            if degree >= 3 {
                out.push(*v);
            }
        }
    }
    out
}

const TABLE_TEXT: f64 = 2.2;
const ROW_H: f64 = 4.6;
const CELL_PAD: f64 = 1.5;

/// How many table rows fit on one report page of this size (below the heading and header row).
pub fn rows_per_page(size: PageSize) -> usize {
    let (_, h) = size.mm();
    let usable = h - 2.0 * MARGIN - HEADER - TITLE_H - 14.0 - ROW_H;
    ((usable / ROW_H).floor() as usize).max(1)
}

/// Column widths for a table that must fit `avail` mm: natural widths, shrunk proportionally
/// when they do not fit.
fn column_widths(t: &wc_report::Table, avail: f64) -> Vec<f64> {
    let mut w: Vec<f64> = t
        .columns
        .iter()
        .map(|c| text_width_mm(c, TABLE_TEXT) + 2.0 * CELL_PAD)
        .collect();
    for r in &t.rows {
        for (i, c) in r.iter().enumerate() {
            if let Some(slot) = w.get_mut(i) {
                *slot = slot.max((text_width_mm(c, TABLE_TEXT) + 2.0 * CELL_PAD).min(avail * 0.6));
            }
        }
    }
    let total: f64 = w.iter().sum();
    if total > avail && total > 0.0 {
        w.iter_mut().for_each(|x| *x *= avail / total);
    } else if total > 0.0 {
        // Spread the spare room over the last column so the table spans the frame.
        if let Some(last) = w.last_mut() {
            *last += avail - total;
        }
    }
    w
}

/// Cut text to fit a width, marking the cut with "...".
fn fit(s: &str, width: f64) -> String {
    if text_width_mm(s, TABLE_TEXT) <= width {
        return s.to_owned();
    }
    let mut out: String = s.to_owned();
    while !out.is_empty() && text_width_mm(&format!("{out}..."), TABLE_TEXT) > width {
        out.pop();
    }
    format!("{out}...")
}

/// One report page: frame and title block like a schematic page, the report title, and a slice
/// of the table's rows starting at `first`.
#[allow(clippy::too_many_arguments)]
pub fn draw_table_page(
    project: &Project,
    page: &Page,
    index: usize,
    total: usize,
    table: &wc_report::Table,
    first: usize,
    part: (usize, usize),
    date: &str,
) -> Drawn {
    let (w, h) = page.size.mm();
    let mut pen = Pen::new(h);
    pen.ops.push_str("1 J 1 j\n");
    pen.colour(0.0, 0.0, 0.0);
    frame(&mut pen, project, page, index, total, date);
    let (x0, x1) = (MARGIN + 6.0, w - MARGIN - 6.0);
    let mut y = MARGIN + HEADER + 10.0;
    let heading = if part.1 > 1 {
        format!("{} ({} / {})", table.title, part.0, part.1)
    } else {
        table.title.clone()
    };
    pen.text((x0, y), 4.0, 0, &heading);
    y += 4.0;
    let widths = column_widths(table, x1 - x0);
    let rows = table.rows.iter().skip(first).take(rows_per_page(page.size));
    // Header row on a grey band.
    pen.colour(0.88, 0.89, 0.91);
    pen.fill_rect((x0, y), (x1, y + ROW_H));
    pen.colour(0.0, 0.0, 0.0);
    let mut cx = x0;
    for (c, wdt) in table.columns.iter().zip(&widths) {
        pen.text(
            (cx + CELL_PAD, y + ROW_H - 1.4),
            TABLE_TEXT,
            0,
            &fit(c, wdt - 2.0 * CELL_PAD),
        );
        cx += wdt;
    }
    pen.width(0.35);
    pen.line((x0, y + ROW_H), (x1, y + ROW_H));
    y += ROW_H;
    pen.width(0.12);
    for r in rows {
        let mut cx = x0;
        for (c, wdt) in r.iter().zip(&widths) {
            pen.text(
                (cx + CELL_PAD, y + ROW_H - 1.4),
                TABLE_TEXT,
                0,
                &fit(c, wdt - 2.0 * CELL_PAD),
            );
            cx += wdt;
        }
        y += ROW_H;
        pen.colour(0.7, 0.7, 0.7);
        pen.line((x0, y), (x1, y));
        pen.colour(0.0, 0.0, 0.0);
    }
    if table.rows.is_empty() {
        pen.colour(0.45, 0.45, 0.45);
        pen.text((x0 + CELL_PAD, y + ROW_H - 1.4), TABLE_TEXT, 0, "Nothing to list.");
    }
    Drawn {
        ops: pen.ops,
        links: Vec::new(),
    }
}
