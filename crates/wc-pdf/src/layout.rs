//! Page layout: frame, column grid, title block, symbols, wires and cross-references.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::geom::{Align, Point, Shape};
use wc_core::symbol::SymbolRole;
use wc_core::{DeviceTag, Endpoint, Page, PageSize, PlacedSymbol, Project, Symbol};

use crate::Options;
use crate::writer::{Drawn, Layer, Link, Pen, text_width_mm};

pub use wc_core::project::FRAME_COLUMNS;
pub(crate) const MARGIN: f64 = wc_core::project::FRAME_MARGIN;
pub(crate) const HEADER: f64 = 6.0;
#[cfg(test)]
const TITLE_H: f64 = wc_core::template::TITLE_HEIGHT;

/// The column (1-based) a point falls into with the built-in column count; a project's own
/// count is [`Project::column_of`].
pub fn column_of(size: PageSize, x: f64) -> usize {
    size.column_of(x)
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
    /// The reference row, 1 for `A`.
    pub row: usize,
    /// The pins, for contact listings such as `13-14`.
    pub pins: Vec<String>,
    /// The placed symbol's id on its page.
    pub item: String,
}

impl Appearance {
    pub fn location(&self) -> String {
        format!(
            "{}/{}.{}",
            self.page_number,
            self.column,
            wc_core::project::row_letter(self.row)
        )
    }
}

#[derive(Debug, Default)]
pub struct CrossRefs {
    pub mains: BTreeMap<DeviceTag, Vec<Appearance>>,
    pub auxes: BTreeMap<DeviceTag, Vec<Appearance>>,
    /// Interruption points by tag.
    pub links: BTreeMap<DeviceTag, Vec<Appearance>>,
}

/// A cross-reference text next to a symbol, pointing at another page.
#[derive(Debug, Clone, PartialEq)]
pub struct XrefMark {
    /// Baseline anchor in page mm.
    pub at: Point,
    pub align: Align,
    pub text: String,
    /// The page the reference points to (the first one when it lists several).
    pub target_page_id: String,
}

pub const XREF_HEIGHT: f64 = 1.8;

/// Cross-reference texts for a page: a coil lists its contacts with their pins and locations,
/// a contact points back to its coil, an interruption point lists where the wire continues.
/// Problems (a contact without a coil, an interruption point without a partner) go to `warnings`.
pub fn xref_marks(
    page: &Page,
    symbols: &BTreeMap<String, Symbol>,
    xref: &CrossRefs,
    warnings: &mut Vec<String>,
) -> Vec<XrefMark> {
    let mut out = Vec::new();
    let line_h = 2.6;
    for s in &page.symbols {
        let (Some(tag), Some(sym)) = (&s.tag, symbols.get(&s.symbol)) else {
            continue;
        };
        let placed: Vec<Shape> = sym
            .graphics
            .iter()
            .map(|g| g.placed(s.at, s.rotation, s.mirror))
            .collect();
        let below = placed.iter().map(|g| g.bounds().1.y).fold(s.at.y, f64::max);
        let right = placed.iter().map(|g| g.bounds().1.x).fold(s.at.x, f64::max);
        let ta = sym.tag_at.transform(s.rotation, s.mirror) + s.at;
        let tag_lines = 1 + usize::from(!tag.function.is_empty()) + usize::from(!tag.location.is_empty());
        let tag_top = ta.y - (tag_lines as f64 - 1.0) * 1.5 + 1.2;
        match sym.role {
            SymbolRole::Main => {
                for (i, a) in xref.auxes.get(tag).map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
                    out.push(XrefMark {
                        at: Point::new(s.at.x + 5.0, below + 3.5 + line_h * i as f64),
                        align: Align::Left,
                        text: format!("{} {}", a.pins.join("-"), a.location()),
                        target_page_id: a.page_id.clone(),
                    });
                }
            }
            SymbolRole::Auxiliary => match xref.mains.get(tag).and_then(|m| m.first()) {
                Some(main) => out.push(XrefMark {
                    at: Point::new(ta.x, tag_top + 3.0 * tag_lines as f64),
                    align: Align::Right,
                    text: format!("> {}", main.location()),
                    target_page_id: main.page_id.clone(),
                }),
                None => warnings.push(format!("page {}: contact {tag} has no coil", page.number)),
            },
            SymbolRole::Interruption => {
                let others: Vec<&Appearance> = xref
                    .links
                    .get(tag)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|a| !(a.page_id == page.id && a.item == s.id))
                    .collect();
                match others.first() {
                    Some(first) => out.push(XrefMark {
                        at: Point::new(right + 1.5, s.at.y + 0.7 + (below - s.at.y) / 2.0),
                        align: Align::Left,
                        text: others.iter().map(|a| a.location()).collect::<Vec<_>>().join(" "),
                        target_page_id: first.page_id.clone(),
                    }),
                    None => warnings.push(format!("page {}: interruption point {tag} has no partner", page.number)),
                }
            }
            _ => {}
        }
    }
    out
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
                    column: project.column_of(page, s.at.x),
                    row: page.size.row_of(s.at.y),
                    pins: sym.pins.iter().map(|p| p.name.clone()).collect(),
                    item: s.id.clone(),
                };
                match sym.role {
                    SymbolRole::Main => x.mains.entry(tag.clone()).or_default().push(a),
                    SymbolRole::Auxiliary => x.auxes.entry(tag.clone()).or_default().push(a),
                    SymbolRole::Interruption => x.links.entry(tag.clone()).or_default().push(a),
                    _ => {}
                }
            }
        }
        // A contact points to its coil's first drawing: the detailed one on a schematic page,
        // not a repeat on an overview page (line diagram) that may come first in page order.
        let overview: BTreeSet<&str> = project
            .pages
            .iter()
            .filter(|p| p.overview)
            .map(|p| p.id.as_str())
            .collect();
        for list in x.mains.values_mut() {
            list.sort_by_key(|a| overview.contains(a.page_id.as_str()));
        }
        x
    }
}

fn tuple(p: Point) -> (f64, f64) {
    (p.x, p.y)
}

pub(crate) fn draw_shape(pen: &mut Pen, s: &Shape) {
    match s {
        Shape::Line { a, b } => pen.line(tuple(*a), tuple(*b)),
        Shape::Polyline { points, closed, filled } => {
            let pts: Vec<(f64, f64)> = points.iter().map(|p| tuple(*p)).collect();
            if *filled {
                pen.polygon_filled(&pts);
            } else {
                pen.polyline(&pts, *closed);
            }
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
            angle,
        } => {
            let a = match align {
                Align::Left => 0,
                Align::Center => 1,
                Align::Right => 2,
            };
            pen.text_at_angle(tuple(*at), *height, a, *angle, text);
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
/// The text height for `t` so it is at most `room` millimetres wide: `height` when it fits,
/// smaller when it does not. Long texts are drawn smaller so they stay inside their box; text is
/// never wrapped.
fn fit_height(t: &str, height: f64, room: f64) -> f64 {
    let w = text_width_mm(t, height);
    if w > room && room > 0.0 {
        height * room / w
    } else {
        height
    }
}

use wc_core::template::{self, PAD};

/// Collinear segments `(at, from, to)` with overlapping or touching pieces joined, sorted by
/// `at` then `from`.
fn merge_segments(mut segs: Vec<(f64, f64, f64)>) -> Vec<(f64, f64, f64)> {
    const EPS: f64 = 1e-6;
    for s in &mut segs {
        if s.1 > s.2 {
            (s.1, s.2) = (s.2, s.1);
        }
    }
    segs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out: Vec<(f64, f64, f64)> = Vec::new();
    for s in segs {
        if let Some(last) = out.last_mut()
            && (last.0 - s.0).abs() < EPS
            && s.1 <= last.2 + EPS
        {
            last.2 = last.2.max(s.2);
        } else {
            out.push(s);
        }
    }
    out
}

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
                    angle: 0.0,
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
    let template = &project.meta.template;
    let columns = template.column_count();
    let col_w = (x1 - x0) / columns as f64;
    for c in 0..columns {
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
    // Row letters down both sides, in the margin outside the frame, so nothing inside moves.
    let rows = page.size.frame_rows();
    let row_h = (y1 - y0) / rows as f64;
    for r in 0..rows {
        let ry = y0 + row_h * r as f64;
        if r > 0 {
            line((x0 - 3.0, ry), (x0, ry), Ink::Thin, &mut out);
            line((x1, ry), (x1 + 3.0, ry), Ink::Thin, &mut out);
        }
        let letter = wc_core::project::row_letter(r + 1).to_string();
        let cy = ry + row_h / 2.0 + 0.9;
        text(
            (x0 - MARGIN / 2.0, cy),
            2.5,
            Align::Center,
            &letter,
            Ink::Thin,
            &mut out,
        );
        text(
            (x1 + MARGIN / 2.0, cy),
            2.5,
            Align::Center,
            &letter,
            Ink::Thin,
            &mut out,
        );
    }
    // Title block: a full-width band at the bottom, divided into the template's boxes.
    let band = template.band_height();
    let ty = y1 - band;
    line((x0, ty), (x1, ty), Ink::Thick, &mut out);
    let boxes = template.boxes_on(x0, x1, band);
    // Box sides, less those on the band's outline, with overlapping pieces drawn once.
    let mut across: Vec<(f64, f64, f64)> = Vec::new();
    let mut down: Vec<(f64, f64, f64)> = Vec::new();
    for (_, (l, t, r, b)) in &boxes {
        for y in [*t, *b] {
            if y > 1e-6 && y < band - 1e-6 {
                across.push((ty + y, *l, *r));
            }
        }
        for x in [*l, *r] {
            if x > x0 + 1e-6 && x < x1 - 1e-6 {
                down.push((x, ty + t, ty + b));
            }
        }
    }
    for (x, a, b) in merge_segments(down) {
        line((x, a), (x, b), Ink::Thin, &mut out);
    }
    for (y, a, b) in merge_segments(across) {
        line((a, y), (b, y), Ink::Thin, &mut out);
    }
    // The page's own fields (its revision, date, ...) win over the project's.
    let sheet = format!("{} / {}", index + 1, total);
    let value = |key: &str| -> String {
        match key {
            template::PROJECT_NAME => project.meta.name.clone(),
            template::PAGE_TITLE => page.title.clone(),
            template::PAGE_NUMBER => page.number.clone(),
            template::SHEET => sheet.clone(),
            "date" if page.title_field(&project.meta, "date").trim().is_empty() => date.to_owned(),
            k => page.title_field(&project.meta, k).to_owned(),
        }
    };
    for (b, (l, t, r, bottom)) in &boxes {
        let top = ty + t;
        let bottom = ty + bottom;
        // A box shorter than its layout (a low band) has the whole layout squeezed into it:
        // every baseline and text height scaled alike, so texts keep apart as they are at full
        // height instead of piling up on the box's foot.
        let deepest = b
            .texts
            .iter()
            .map(|bt| bt.y)
            .chain(b.logo.then_some(LOGO_BASELINE))
            .fold(LABEL_BASELINE, f64::max);
        let squeeze = ((bottom - top - BELOW_BASELINE) / deepest).clamp(0.0, 1.0);
        // Labels and values never reach past their box: they shrink to fit, and a box too narrow
        // for any text shows none.
        let label_room = r - l - 2.0 * PAD;
        if label_room > 0.0
            && let Some((y, h)) = fit_in_box(
                top,
                bottom,
                top + LABEL_BASELINE * squeeze,
                fit_height(&b.label, 1.8, label_room) * squeeze,
            )
        {
            text((l + PAD, y), h, Align::Left, &b.label, Ink::Label, &mut out);
        }
        // Each value is shrunk to the width of its box (less padding), so a long one never runs
        // into the next box or over the frame.
        for bt in &b.texts {
            let shown: Vec<String> = bt.values.iter().map(|k| value(k)).filter(|v| !v.is_empty()).collect();
            let shown = shown.join("  \u{b7}  ");
            let (x, room) = match bt.align {
                Align::Left => (l + bt.inset, r - l - bt.inset - PAD),
                Align::Right => (r - bt.inset, r - l - bt.inset - PAD),
                Align::Center => ((l + r) / 2.0, r - l - 2.0 * PAD),
            };
            if room <= 0.0 {
                continue;
            }
            let Some((y, h)) = fit_in_box(
                top,
                bottom,
                top + bt.y * squeeze,
                fit_height(&shown, bt.size, room) * squeeze,
            ) else {
                continue;
            };
            text((x, y), h, bt.align, &shown, Ink::Thin, &mut out);
        }
        // The project's logo takes a logo box when it has one (see `title_logo_box`).
        if b.logo
            && project.meta.logo.is_none()
            && let Some((y, h)) = fit_in_box(
                top,
                bottom,
                top + LOGO_BASELINE * squeeze,
                fit_height("WiringCraft", 2.0, r - l - 2.0 * PAD) * squeeze,
            )
        {
            text(((l + r) / 2.0, y), h, Align::Center, "WiringCraft", Ink::Thin, &mut out);
        }
    }
    if page.cover {
        cover_marks(project, (x0, y0 + HEADER, x1, ty), date, &mut out);
    }
    out
}

/// Room kept between a title-block text's baseline and the bottom of its box, for descenders.
const BELOW_BASELINE: f64 = 0.8;
/// A title-block box's label baseline, below the box's top.
const LABEL_BASELINE: f64 = 3.0;
/// The baseline of "WiringCraft" in a logo box of a project without a logo, below its top.
const LOGO_BASELINE: f64 = 9.5;

/// Where text of height `h` with its baseline at `y` goes in a title-block box from `top` to
/// `bottom`: moved up to stay inside it, and shrunk when the box is too short for it.
/// `None` when no text fits.
fn fit_in_box(top: f64, bottom: f64, y: f64, h: f64) -> Option<(f64, f64)> {
    let y = y.min(bottom - BELOW_BASELINE);
    let h = h.min(y - top);
    (h > 0.5).then_some((y, h))
}

/// The title block's logo box on `page` (top-left and bottom-right, in millimetres): the
/// page template's first logo box, less 1 mm all round, where pages without a logo show
/// "WiringCraft". `None` when the template has no logo box.
pub fn title_logo_box(project: &Project, page: &Page) -> Option<(Point, Point)> {
    title_logo_boxes(project, page).into_iter().next()
}

/// Every logo box of the page template on `page`, as [`title_logo_box`], in template order.
pub fn title_logo_boxes(project: &Project, page: &Page) -> Vec<(Point, Point)> {
    let (w, h) = page.size.mm();
    let (x0, x1, y1) = (MARGIN, w - MARGIN, h - MARGIN);
    let band = project.meta.template.band_height();
    let ty = y1 - band;
    project
        .meta
        .template
        .boxes_on(x0, x1, band)
        .into_iter()
        .filter(|(b, _)| b.logo)
        .map(|(_, (l, t, r, b))| {
            // 1 mm of padding, less in a box narrowed below 4 mm, so the logo keeps an area.
            let (ix, iy) = (((r - l) / 4.0).min(1.0), ((b - t) / 4.0).min(1.0));
            (Point::new(l + ix, ty + t + iy), Point::new(r - ix, ty + b - iy))
        })
        .filter(|(a, b)| a.x < b.x && a.y < b.y)
        .collect()
}

/// Where the cover page shows the logo until the user moves it: centred above the project's
/// name.
pub fn cover_logo_default(project: &Project, page: &Page) -> (Point, Point) {
    let (w, h) = page.size.mm();
    let band = project.meta.template.band_height();
    let (x0, y0, x1, ty) = (MARGIN, MARGIN + HEADER, w - MARGIN, h - MARGIN - band);
    let cx = (x0 + x1) / 2.0;
    let name_y = cover_name_y(cover_rows(project, "").len(), default_cover_logo(project), y0, ty);
    let top = y0 + COVER_LOGO_TOP;
    let bottom = (name_y - COVER_LOGO_TO_NAME).max(top + COVER_LOGO_MIN);
    let half = 45.0_f64.min((x1 - x0) / 2.0 - 5.0);
    (Point::new(cx - half, top), Point::new(cx + half, bottom))
}

/// The logo's box on the cover page: where the user put it, or its default place. `None`
/// when `page` is not the cover or the project has no logo.
pub fn cover_logo_box(project: &Project, page: &Page) -> Option<(Point, Point)> {
    let logo = project.meta.logo.as_ref()?;
    if !page.cover {
        return None;
    }
    Some(match logo.cover {
        Some((a, b)) => (
            Point::new(a.x.min(b.x), a.y.min(b.y)),
            Point::new(a.x.max(b.x), a.y.max(b.y)),
        ),
        None => cover_logo_default(project, page),
    })
}

/// Every box the project's logo is drawn in on `page`: the title block's, and on the cover
/// page also its place there. Empty when the project has no logo. The picture is fitted into
/// each box keeping its proportions ([`fit_picture`]).
pub fn logo_boxes(project: &Project, page: &Page) -> Vec<(Point, Point)> {
    if project.meta.logo.is_none() {
        return Vec::new();
    }
    let mut out = title_logo_boxes(project, page);
    out.extend(cover_logo_box(project, page));
    out
}

/// The largest rectangle with the picture's proportions (`width` by `height` pixels) centred
/// in the box `min`..`max`.
pub fn fit_picture((min, max): (Point, Point), (width, height): (u32, u32)) -> (Point, Point) {
    let (bw, bh) = (max.x - min.x, max.y - min.y);
    if width == 0 || height == 0 || bw <= 0.0 || bh <= 0.0 {
        return (min, max);
    }
    let scale = (bw / f64::from(width)).min(bh / f64::from(height));
    let (w, h) = (f64::from(width) * scale, f64::from(height) * scale);
    let (x, y) = (min.x + (bw - w) / 2.0, min.y + (bh - h) / 2.0);
    (Point::new(x, y), Point::new(x + w, y + h))
}

/// From the top of the cover's drawing area to its default logo box.
const COVER_LOGO_TOP: f64 = 4.0;
/// The smallest height of the cover's default logo box.
const COVER_LOGO_MIN: f64 = 5.0;
/// From the bottom of the cover's default logo box to the project name's baseline.
const COVER_LOGO_TO_NAME: f64 = 10.0;
/// The smallest height a row of the cover's field table gets; below it, rows are left out.
const COVER_MIN_ROW: f64 = 4.0;
/// The room under the cover's table, above the title block.
const COVER_TABLE_GAP: f64 = 8.0;
/// From the project's name to the top of the cover's table: the description sits between.
const COVER_NAME_TO_TABLE: f64 = 18.0;

/// Where the project's name stands on a cover whose drawing area runs from `y0` to `y1` and
/// lists `rows` fields: a fifth of the way down, or higher on a short sheet so the table has
/// room for its rows, but never closer to the top than the name's height, or, with a `logo`,
/// than the smallest default logo box above it (see [`cover_logo_default`]).
fn cover_name_y(rows: usize, logo: bool, y0: f64, y1: f64) -> f64 {
    let ideal = y0 + (y1 - y0) * 0.2;
    let needed = rows as f64 * COVER_MIN_ROW + COVER_TABLE_GAP + COVER_NAME_TO_TABLE;
    let highest = y0
        + if logo {
            COVER_LOGO_TOP + COVER_LOGO_MIN + COVER_LOGO_TO_NAME
        } else {
            10.0
        };
    ideal.min(y1 - needed).max(highest)
}

/// Whether the cover shows the project's logo in its default place, above the name: only then
/// does the name leave room for it.
fn default_cover_logo(project: &Project) -> bool {
    project.meta.logo.as_ref().is_some_and(|l| l.cover.is_none())
}

/// How many rows of the cover's field table fit on a cover whose drawing area runs from `y0`
/// to `y1` and lists `rows` fields, at no less than [`COVER_MIN_ROW`] each.
fn cover_fits(rows: usize, logo: bool, y0: f64, y1: f64) -> usize {
    let room = y1 - COVER_TABLE_GAP - cover_name_y(rows, logo, y0, y1) - COVER_NAME_TO_TABLE;
    (room / COVER_MIN_ROW).floor().max(0.0) as usize
}

/// The rows of the cover's field table, label and value. Standard fields are listed even
/// when empty, so the cover reads as a form; other fields are listed when they have a value.
/// An empty date shows `date`.
fn cover_rows<'a>(project: &'a Project, date: &'a str) -> Vec<(&'a str, &'a str)> {
    let tb = &project.meta.title_block;
    let mut rows: Vec<(&str, &str)> = wc_core::project::TITLE_FIELDS
        .iter()
        .filter(|(k, _)| *k != "description")
        .map(|(k, label)| {
            let v = tb.get(*k).map(String::as_str).unwrap_or("");
            let v = if *k == "date" && v.trim().is_empty() { date } else { v };
            (*label, v)
        })
        .collect();
    for (k, v) in tb {
        if !wc_core::project::TITLE_FIELDS.iter().any(|(f, _)| f == k) && !v.trim().is_empty() {
            rows.push((k.as_str(), v.as_str()));
        }
    }
    rows
}

/// The cover page's content inside the drawing area `(x0, y0, x1, y1)`: the project's name
/// large, its description under it, and a table of the project fields. Every value comes from
/// the project, so it always matches the title blocks. Standard fields are listed even when
/// empty, so the cover reads as a form; other fields are listed when they have a value.
fn cover_marks(project: &Project, (x0, y0, x1, y1): (f64, f64, f64, f64), date: &str, out: &mut Vec<FrameMark>) {
    let tb = &project.meta.title_block;
    let cx = (x0 + x1) / 2.0;
    let push_text = |at: (f64, f64), height: f64, align: Align, t: &str, ink: Ink, out: &mut Vec<FrameMark>| {
        if !t.trim().is_empty() {
            out.push(FrameMark {
                shape: Shape::Text {
                    at: Point::new(at.0, at.1),
                    text: t.to_owned(),
                    height,
                    align,
                    angle: 0.0,
                },
                ink,
            });
        }
    };
    let fit = |t: &str, height: f64| fit_height(t, height, x1 - x0 - 10.0);
    let mut rows = cover_rows(project, date);
    let logo = default_cover_logo(project);
    let name_y = cover_name_y(rows.len(), logo, y0, y1);
    let name = &project.meta.name;
    push_text((cx, name_y), fit(name, 9.0), Align::Center, name, Ink::Thin, out);
    // On a sheet too short for it, the description is left out rather than drawn into the
    // title block.
    let desc = tb.get("description").map(String::as_str).unwrap_or("");
    if name_y + 9.0 + BELOW_BASELINE <= y1 {
        push_text((cx, name_y + 9.0), fit(desc, 4.0), Align::Center, desc, Ink::Thin, out);
    }

    // The table fits under the description; rows shrink on small sheets. When they would be
    // too small to read, empty fields are left out first, then the last filled-in ones, with a
    // row that says how many filled-in fields are not shown (empty ones have nothing to show).
    // When not even that row fits, the export warns (see `draw_page`).
    let top = name_y + COVER_NAME_TO_TABLE;
    let room = y1 - COVER_TABLE_GAP - top;
    let fits = cover_fits(rows.len(), logo, y0, y1);
    if rows.len() > fits {
        rows.retain(|(_, v)| !v.trim().is_empty());
    }
    let more;
    if rows.len() > fits {
        let shown = fits.saturating_sub(1);
        more = format!("{} more", rows.len() - shown);
        rows.truncate(shown);
        if fits > 0 {
            rows.push(("...", more.as_str()));
        }
    }
    if rows.is_empty() {
        return;
    }
    let row_h = (room / rows.len() as f64).min(8.0);
    let (tx0, tx1) = (
        cx - 90.0_f64.min((x1 - x0) / 2.0 - 5.0),
        cx + 90.0_f64.min((x1 - x0) / 2.0 - 5.0),
    );
    let split = tx0 + (tx1 - tx0) * 0.35;
    let rule = |y: f64, ink: Ink, out: &mut Vec<FrameMark>| {
        out.push(FrameMark {
            shape: Shape::Line {
                a: Point::new(tx0, y),
                b: Point::new(tx1, y),
            },
            ink,
        });
    };
    rule(top, Ink::Thick, out);
    let size = (row_h * 0.4).min(3.5);
    for (i, (label, value)) in rows.iter().enumerate() {
        let ry = top + row_h * i as f64;
        let base = ry + row_h / 2.0 + size * 0.35;
        let label_h = fit_height(label, size * 0.8, split - tx0 - 3.0);
        push_text((tx0 + 1.5, base), label_h, Align::Left, label, Ink::Label, out);
        let value_h = fit_height(value, size, tx1 - split - 3.0);
        push_text((split + 1.5, base), value_h, Align::Left, value, Ink::Thin, out);
        rule(ry + row_h, Ink::Thin, out);
    }
}

pub(crate) fn frame(pen: &mut Pen, project: &Project, page: &Page, index: usize, total: usize, date: &str) {
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
    if page.cover {
        let rows = cover_rows(project, &options.date);
        let band = project.meta.template.band_height();
        if rows.iter().any(|(_, v)| !v.trim().is_empty())
            && cover_fits(
                rows.len(),
                default_cover_logo(project),
                MARGIN + HEADER,
                h - MARGIN - band,
            ) == 0
        {
            warnings.push(format!(
                "page {}: the cover page is too short to list the project fields",
                page.number
            ));
        }
    }

    // Free graphics: texts, lines and rectangles drawn on the sheet, and imported drawings.
    pen.colour(0.0, 0.0, 0.0);
    pen.width(0.25);
    if !page.graphics.is_empty() {
        pen.begin_layer(Layer::Drawings);
        for g in &page.graphics {
            draw_shape(&mut pen, g);
        }
        pen.end_layer();
    }

    // Revision clouds and the revision table.
    if !page.clouds.is_empty() || page.revision_table.is_some() {
        pen.begin_layer(Layer::Drawings);
        for c in &page.clouds {
            if !c.is_valid() {
                warnings.push(format!("page {}: a revision cloud has no area", page.number));
            }
            for s in c.shapes() {
                draw_shape(&mut pen, &s);
            }
        }
        if let Some(at) = page.revision_table {
            for s in wc_core::revision::table_shapes(&project.meta.revisions, at) {
                draw_shape(&mut pen, &s);
            }
        }
        pen.end_layer();
    }
    for c in &page.clouds {
        if project.cloud_revision(&c.revision).is_none() {
            warnings.push(format!(
                "page {}: a cloud carries revision {}, which the project does not list",
                page.number, c.revision
            ));
        } else if !wc_core::revision::letter_prints(&c.revision) {
            warnings.push(format!(
                "page {}: the PDF cannot print revision {} on a cloud; it can read like another letter",
                page.number, c.revision
            ));
        }
    }
    if page.revision_table.is_some() {
        for letter in project.unprintable_revisions() {
            warnings.push(format!(
                "page {}: the PDF cannot print revision {letter} in the revision table; it can read like another letter",
                page.number
            ));
        }
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
            let core = w.cable.as_ref().map(|c| {
                if c.core.is_empty() {
                    c.cable.to_string()
                } else {
                    format!("{}:{}", c.cable, c.core)
                }
            });
            let label: Vec<&str> = [w.number.as_deref(), w.potential.as_deref(), core.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            if !label.is_empty() {
                // Left of a vertical first segment, above a horizontal one; pin numbers sit on the right.
                let (p, q) = (pts[0], pts[1]);
                let mid = ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
                pen.begin_layer(Layer::WireLabels);
                if (p.0 - q.0).abs() < (p.1 - q.1).abs() {
                    pen.text((mid.0 - 1.0, mid.1 + 0.8), 2.0, 2, &label.join(" "));
                } else {
                    pen.text((mid.0, mid.1 - 1.0), 2.0, 1, &label.join(" "));
                }
                pen.end_layer();
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
    }

    // Cross-references, as links to the page they point at.
    pen.colour(0.1, 0.3, 0.7);
    let marks = xref_marks(page, symbols, xref, warnings);
    if !marks.is_empty() {
        pen.begin_layer(Layer::CrossReferences);
    }
    for m in marks {
        let a = match m.align {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        };
        pen.text(tuple(m.at), XREF_HEIGHT, a, &m.text);
        let w = text_width_mm(&m.text, XREF_HEIGHT);
        let x0 = match m.align {
            Align::Left => m.at.x,
            Align::Center => m.at.x - w / 2.0,
            Align::Right => m.at.x - w,
        };
        links.push(Link {
            min: (x0, m.at.y - 2.0),
            max: (x0 + w, m.at.y + 0.5),
            target_page_id: m.target_page_id,
        });
    }
    pen.end_layer();
    pen.colour(0.0, 0.0, 0.0);
    Drawn {
        ops: pen.ops,
        links,
        prims: pen.prims,
        layers: pen.layers,
        underlay: None,
        pictures: Vec::new(),
    }
}

/// Points where three or more wire branches meet: wire ends and bends count once per wire
/// touching them, and a point in the middle of another wire's segment counts twice (a T).
pub fn junctions(polylines: &[Vec<(f64, f64)>]) -> Vec<(f64, f64)> {
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

/// How many table rows fit on one report page of this size (below the heading and header row),
/// above a title block band `band` mm high.
pub fn rows_per_page(size: PageSize, band: f64) -> usize {
    (report_rows(size, band).floor() as usize).max(1)
}

/// How many table rows the sheet height leaves room for, before rounding down.
fn report_rows(size: PageSize, band: f64) -> f64 {
    let (_, h) = size.mm();
    let usable = h - 2.0 * MARGIN - HEADER - band - 14.0 - ROW_H;
    usable / ROW_H
}

/// The sheet report pages are printed on: `wanted` (the first schematic page's size) when it
/// has room for its title block and at least one table row, otherwise the smallest ISO sheet
/// that does (A3 when none does).
pub fn report_size(wanted: PageSize, band: f64) -> PageSize {
    std::iter::once(wanted)
        .chain(PageSize::NAMED)
        .find(|s| report_rows(*s, band) >= 1.0)
        .unwrap_or(PageSize::A3)
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
    text_fit(s, TABLE_TEXT, width)
}

/// Cut text of `size` to fit a width, marking the cut with "...".
pub(crate) fn text_fit(s: &str, size: f64, width: f64) -> String {
    if text_width_mm(s, size) <= width {
        return s.to_owned();
    }
    let mut out: String = s.to_owned();
    while !out.is_empty() && text_width_mm(&format!("{out}..."), size) > width {
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
    let rows = table
        .rows
        .iter()
        .skip(first)
        .take(rows_per_page(page.size, project.meta.template.band_height()));
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
    let mut links = Vec::new();
    for (ri, r) in rows.enumerate() {
        // A row that describes a page jumps to it.
        if let Some(target) = table
            .row_pages
            .get(first.saturating_add(ri))
            .filter(|id| !id.is_empty())
        {
            links.push(Link {
                min: (x0, y),
                max: (x1, y + ROW_H),
                target_page_id: target.clone(),
            });
        }
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
        links,
        prims: pen.prims,
        layers: pen.layers,
        underlay: None,
        pictures: Vec::new(),
    }
}

/// One panel layout page: frame and title block, the mounting plate scaled to fit the drawing
/// area with its ducts (hatched grey), DIN rails and device footprints labelled with their
/// tags, and the scale and plate size under it.
pub fn draw_panel_page(
    project: &Project,
    page: &Page,
    index: usize,
    total: usize,
    panel: &wc_core::panel::Panel,
    date: &str,
) -> Drawn {
    use wc_core::panel::PanelItem;
    let (w, h) = page.size.mm();
    let mut pen = Pen::new(h);
    pen.ops.push_str("1 J 1 j\n");
    pen.colour(0.0, 0.0, 0.0);
    frame(&mut pen, project, page, index, total, date);
    // Drawing area inside the frame, above the title block, with room for the caption.
    let (ax0, ay0) = (MARGIN + 10.0, MARGIN + HEADER + 10.0);
    let band = project.meta.template.band_height();
    let (ax1, ay1) = (w - MARGIN - 10.0, h - MARGIN - band - 14.0);
    let (pw, ph) = (panel.width.max(1.0), panel.height.max(1.0));
    let fit = ((ax1 - ax0) / pw).min((ay1 - ay0) / ph);
    // A standard reduction (1:2, 1:5, 1:10, ...) that fits.
    let scale = [1.0, 2.0, 2.5, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0]
        .into_iter()
        .find(|d| 1.0 / d <= fit)
        .unwrap_or(100.0);
    let k = 1.0 / scale;
    let ox = ax0 + ((ax1 - ax0) - pw * k) / 2.0;
    let oy = ay0 + ((ay1 - ay0) - ph * k) / 2.0;
    let p = |x: f64, y: f64| (ox + x * k, oy + y * k);
    pen.width(0.5);
    pen.rect(p(0.0, 0.0), p(pw, ph));
    pen.width(0.18);
    for item in &panel.items {
        match item {
            PanelItem::Duct { min, max } => {
                pen.colour(0.86, 0.87, 0.89);
                pen.fill_rect(p(min.x, min.y), p(max.x, max.y));
                pen.colour(0.45, 0.45, 0.45);
                pen.rect(p(min.x, min.y), p(max.x, max.y));
            }
            PanelItem::Rail { at, length } => {
                // A 35 mm rail, drawn with its two flanges.
                pen.colour(0.3, 0.3, 0.3);
                pen.rect(p(at.x, at.y - 17.5), p(at.x + length, at.y + 17.5));
                pen.line(p(at.x, at.y - 12.5), p(at.x + length, at.y - 12.5));
                pen.line(p(at.x, at.y + 12.5), p(at.x + length, at.y + 12.5));
            }
            PanelItem::Device { .. } => {}
        }
    }
    pen.colour(0.0, 0.0, 0.0);
    pen.width(0.35);
    for item in &panel.items {
        if let PanelItem::Device { tag, at, width, height } = item {
            let (width, height) = (*width, *height);
            pen.colour(1.0, 1.0, 1.0);
            pen.fill_rect(p(at.x, at.y), p(at.x + width, at.y + height));
            pen.colour(0.0, 0.0, 0.0);
            pen.rect(p(at.x, at.y), p(at.x + width, at.y + height));
            // The product aspect is what fits on a narrow device; the full tag is in the list.
            let label = tag.product.join(".");
            let label = format!("-{label}");
            let per_char = label.chars().count().max(1) as f64 * 0.62;
            let across = (width * k / per_char).min(height * k * 0.5).min(2.5);
            let along = (height * k / per_char).min(width * k * 0.6).min(2.5);
            let (cx, cy) = p(at.x + width / 2.0, at.y + height / 2.0);
            if across >= along.min(1.2) {
                pen.text((cx, cy + across / 2.0), across.max(0.6), 1, &label);
            } else {
                pen.text_at_angle((cx + along / 2.0, cy), along.max(0.6), 1, 90.0, &label);
            }
        }
    }
    let devices = panel.device_tags().len();
    pen.text(
        (ax0, h - MARGIN - band - 5.0),
        2.5,
        0,
        &format!(
            "{}  ·  plate {} x {} mm  ·  scale 1:{}  ·  {} device(s)",
            panel.name, panel.width, panel.height, scale, devices
        ),
    );
    Drawn {
        ops: pen.ops,
        links: Vec::new(),
        prims: pen.prims,
        layers: pen.layers,
        underlay: None,
        pictures: Vec::new(),
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;
    use wc_core::template::{BoxText, PageTemplate, Side, TitleBox};

    // The title block exactly as it was drawn before the layout came from the page template.
    #[allow(clippy::too_many_lines)]
    pub(super) fn legacy_frame_marks(
        project: &Project,
        page: &Page,
        index: usize,
        total: usize,
        date: &str,
    ) -> Vec<FrameMark> {
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
                        angle: 0.0,
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
        let col_w = (x1 - x0) / wc_core::project::FRAME_COLUMNS as f64;
        for c in 0..wc_core::project::FRAME_COLUMNS {
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
        // Row letters down both sides, in the margin outside the frame, so nothing inside moves.
        let rows = page.size.frame_rows();
        let row_h = (y1 - y0) / rows as f64;
        for r in 0..rows {
            let ry = y0 + row_h * r as f64;
            if r > 0 {
                line((x0 - 3.0, ry), (x0, ry), Ink::Thin, &mut out);
                line((x1, ry), (x1 + 3.0, ry), Ink::Thin, &mut out);
            }
            let letter = wc_core::project::row_letter(r + 1).to_string();
            let cy = ry + row_h / 2.0 + 0.9;
            text(
                (x0 - MARGIN / 2.0, cy),
                2.5,
                Align::Center,
                &letter,
                Ink::Thin,
                &mut out,
            );
            text(
                (x1 + MARGIN / 2.0, cy),
                2.5,
                Align::Center,
                &letter,
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
        // The page's own fields (its revision, date, ...) win over the project's.
        let get = |k: &str| page.title_field(&project.meta, k);
        // Each value is shrunk to the width of its box (less padding), so a long one never runs into
        // the next box or over the frame.
        let field = |x: f64, w: f64, y: f64, label: &str, value: &str, height: f64, out: &mut Vec<FrameMark>| {
            text((x + PAD, y + 3.0), 1.8, Align::Left, label, Ink::Label, out);
            let h = fit_height(value, height, w - 2.0 * PAD);
            text((x + PAD, y + 9.5), h, Align::Left, value, Ink::Thin, out);
        };
        // A centred value in a box from `x` that is `w` wide.
        let centred = |x: f64, w: f64, y: f64, value: &str, height: f64, out: &mut Vec<FrameMark>| {
            let h = fit_height(value, height, w - 2.0 * PAD);
            text((x + w / 2.0, y), h, Align::Center, value, Ink::Thin, out);
        };
        field(fx[0], fx[1] - fx[0], ty, "Customer", get("customer"), 3.0, &mut out);
        field(fx[0], 40.0, mid, "Drawn by", get("drawn_by"), 2.5, &mut out);
        field(
            fx[0] + 40.0,
            fx[1] - fx[0] - 40.0,
            mid,
            "Checked by",
            get("checked_by"),
            2.5,
            &mut out,
        );
        text(
            (fx[1] + PAD, ty + 3.0),
            1.8,
            Align::Left,
            "Project",
            Ink::Label,
            &mut out,
        );
        // The project box's texts start 3 mm in.
        let project_room = fx[2] - fx[1] - 3.0 - PAD;
        let name = &project.meta.name;
        let name_h = fit_height(name, 4.0, project_room);
        text((fx[1] + 3.0, ty + 11.0), name_h, Align::Left, name, Ink::Thin, &mut out);
        let title_h = fit_height(&page.title, 3.0, project_room);
        text(
            (fx[1] + 3.0, ty + 19.5),
            title_h,
            Align::Left,
            &page.title,
            Ink::Thin,
            &mut out,
        );
        let numbers: Vec<&str> = [get("project_number"), get("plant")]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let numbers = numbers.join("  \u{b7}  ");
        let numbers_h = fit_height(&numbers, 2.2, fx[2] - fx[1] - 2.0 - PAD);
        text(
            (fx[2] - 2.0, ty + 22.0),
            numbers_h,
            Align::Right,
            &numbers,
            Ink::Thin,
            &mut out,
        );
        field(fx[2], fx[3] - fx[2], ty, "Revision", get("revision"), 2.5, &mut out);
        let d = if get("date").trim().is_empty() {
            date
        } else {
            get("date")
        };
        field(fx[2], fx[3] - fx[2], mid, "Date", d, 2.5, &mut out);
        text((fx[3] + PAD, ty + 3.0), 1.8, Align::Left, "Page", Ink::Label, &mut out);
        centred(fx[3], fx[4] - fx[3], ty + 10.5, &page.number, 4.0, &mut out);
        text(
            (fx[3] + PAD, mid + 3.0),
            1.8,
            Align::Left,
            "Location",
            Ink::Label,
            &mut out,
        );
        centred(fx[3], fx[4] - fx[3], mid + 9.5, get("location"), 2.5, &mut out);
        text((fx[4] + PAD, ty + 3.0), 1.8, Align::Left, "Sheet", Ink::Label, &mut out);
        let sheet = format!("{} / {}", index + 1, total);
        centred(fx[4], x1 - fx[4], ty + 10.0, &sheet, 3.0, &mut out);
        // The project's logo takes this box when it has one (see `title_logo_box`).
        if project.meta.logo.is_none() {
            text(
                (fx[4] + 15.0, mid + 9.5),
                2.0,
                Align::Center,
                "WiringCraft",
                Ink::Thin,
                &mut out,
            );
        }
        if page.cover {
            cover_marks(project, (x0, y0 + HEADER, x1, ty), date, &mut out);
        }
        out
    }

    fn key(m: &FrameMark) -> String {
        format!("{:?} {:?}", m.ink, m.shape)
    }

    /// Lines as sorted, rounded segments; texts in drawing order.
    fn split(marks: &[FrameMark]) -> (Vec<String>, Vec<String>) {
        let r = |v: f64| format!("{:.4}", v);
        let mut lines = Vec::new();
        let mut texts = Vec::new();
        for m in marks {
            match &m.shape {
                Shape::Line { a, b } => {
                    let (p, q) = if (a.x, a.y) <= (b.x, b.y) { (a, b) } else { (b, a) };
                    lines.push(format!("{:?} {} {} {} {}", m.ink, r(p.x), r(p.y), r(q.x), r(q.y)));
                }
                Shape::Text { .. } => texts.push(key(m)),
                _ => lines.push(key(m)),
            }
        }
        lines.sort();
        (lines, texts)
    }

    fn demo(size: PageSize, logo: bool) -> (Project, Page) {
        let mut p = Project::new("Pump station with a rather long project name that has to shrink");
        for (k, v) in [
            ("customer", "ACME"),
            ("drawn_by", "PH"),
            ("checked_by", "OK"),
            ("project_number", "P-1"),
            ("plant", "North"),
            ("revision", "B"),
            ("location", "+CAB1"),
        ] {
            p.meta.title_block.insert(k.into(), v.into());
        }
        if logo {
            p.meta.logo = Some(wc_core::Logo {
                file: "pictures/logo.png".into(),
                cover: None,
            });
        }
        let mut page = Page::new("p1", "12", "Motor feeders");
        page.size = size;
        (p, page)
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_default_template_draws_the_title_block_as_before() {
        // Every sheet wide enough for the layout draws it as before; the narrow ones are below.
        let custom = PageSize::Custom {
            width: 500,
            height: 700,
        };
        let wide = PageSize::NAMED
            .into_iter()
            .chain([custom])
            .filter(|s| s.mm().0 - 2.0 * MARGIN >= Project::new("").meta.template.needed_width());
        for size in wide {
            for logo in [false, true] {
                let (p, page) = demo(size, logo);
                for date in ["", "2026-11-17"] {
                    let now = split(&frame_marks(&p, &page, 2, 7, date));
                    let before = split(&legacy_frame_marks(&p, &page, 2, 7, date));
                    assert_eq!(now, before, "{size:?} logo {logo} date {date:?}");
                }
                let mut cover = page.clone();
                cover.cover = true;
                assert_eq!(
                    split(&frame_marks(&p, &cover, 0, 7, "")),
                    split(&legacy_frame_marks(&p, &cover, 0, 7, ""))
                );
                let logo_box = title_logo_box(&p, &page).unwrap();
                let (w, h) = page.size.mm();
                let (x1, y1) = (w - MARGIN, h - MARGIN);
                let mid = y1 - TITLE_H / 2.0;
                assert_eq!(
                    logo_box,
                    (Point::new(x1 - 30.0 + 1.0, mid + 1.0), Point::new(x1 - 1.0, y1 - 1.0))
                );
            }
        }
    }

    #[test]
    fn every_logo_box_gets_the_logo() {
        let (mut p, page) = demo(PageSize::A3, true);
        let mut second = TitleBox::new("", (180.0, Side::Right), (150.0, Side::Right), 0.0, 24.0);
        second.logo = true;
        p.meta.template.boxes.push(second);
        let boxes = logo_boxes(&p, &page);
        assert_eq!(boxes.len(), 2, "{boxes:?}");
        assert_eq!(title_logo_box(&p, &page), boxes.first().copied());
        let mut cover = page.clone();
        cover.cover = true;
        assert_eq!(logo_boxes(&p, &cover).len(), 3, "both title boxes and the cover");
    }

    #[test]
    fn a_new_box_shows_on_every_page_and_columns_follow_the_template() {
        let (mut p, page) = demo(PageSize::A3, false);
        p.meta.title_block.insert("order".into(), "ORD-77".into());
        let mut t = PageTemplate {
            columns: 12,
            ..Default::default()
        };
        // Make room for an order box left of the revision.
        t.boxes[3].right = 180.0;
        let mut order = TitleBox::new("Order", (180.0, Side::Right), (150.0, Side::Right), 0.0, 24.0);
        order.texts.push(BoxText::new(&["order"], 2.5, 9.5, Align::Left));
        t.boxes.push(order);
        // No logo box: the title block then has no logo.
        t.boxes.retain(|b| !b.logo);
        p.meta.template = t;
        let mut second = page.clone();
        second.id = "p2".into();
        for pg in [&page, &second] {
            let marks = frame_marks(&p, pg, 0, 2, "");
            let texts: Vec<&str> = marks
                .iter()
                .filter_map(|m| match &m.shape {
                    Shape::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(texts.contains(&"Order") && texts.contains(&"ORD-77"), "{texts:?}");
            assert!(texts.contains(&"12") && !texts.contains(&"13"), "12 columns");
            assert!(!texts.contains(&"WiringCraft"), "no logo box");
        }
        assert_eq!(title_logo_box(&p, &page), None);
        // Cross-references use the same 12 columns.
        let (w, _) = page.size.mm();
        assert_eq!(p.column_of(&page, w - MARGIN - 1.0), 12);
        assert_eq!(p.column_of(&page, MARGIN + (w - 2.0 * MARGIN) / 12.0 + 0.1), 2);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_taller_title_block_moves_its_line_and_keeps_boxes_inside() {
        let (mut p, page) = demo(PageSize::A3, true);
        let (w, h) = page.size.mm();
        let (x0, x1, y1) = (MARGIN, w - MARGIN, h - MARGIN);
        let before = rows_per_page(page.size, p.meta.template.band_height());
        let mut t = PageTemplate {
            height: 36.0,
            ..Default::default()
        };
        // A box filling the lower 12 mm the default layout leaves free.
        t.boxes.push(TitleBox::new(
            "Notes",
            (0.0, Side::Left),
            (0.0, Side::Right),
            24.0,
            36.0,
        ));
        p.meta.template = t;
        let marks = frame_marks(&p, &page, 0, 1, "");
        let thick: Vec<(Point, Point)> = marks
            .iter()
            .filter(|m| m.ink == Ink::Thick)
            .filter_map(|m| match &m.shape {
                Shape::Line { a, b } if (a.y - b.y).abs() < 1e-9 => Some((*a, *b)),
                _ => None,
            })
            .collect();
        let ty = y1 - 36.0;
        assert!(
            thick
                .iter()
                .any(|(a, b)| (a.y - ty).abs() < 1e-9 && a.x == x0 && b.x == x1),
            "the band's top line moved up: {thick:?}"
        );
        // The Notes box's top side is drawn 24 mm down the band, across the frame.
        assert!(marks.iter().any(|m| matches!(&m.shape,
            Shape::Line { a, b } if (a.y - (ty + 24.0)).abs() < 1e-9 && (b.y - a.y).abs() < 1e-9)));
        // Everything of the title block stays inside it.
        for m in &marks {
            if let Shape::Text { at, .. } = &m.shape
                && at.y > ty
            {
                assert!(at.y <= y1, "{at:?}");
            }
        }
        let (a, b) = title_logo_box(&p, &page).unwrap();
        assert!(a.y > ty && b.y < y1, "the logo stays in its box: {a:?} {b:?}");
        // Report pages fit fewer rows, and the cover logo stays above the band.
        assert!(rows_per_page(page.size, p.meta.template.band_height()) < before);
        assert!(cover_logo_default(&p, &page).1.y < ty);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_shorter_title_block_keeps_its_texts_inside_the_frame() {
        let (mut p, page) = demo(PageSize::A3, true);
        let (w, h) = page.size.mm();
        let (x0, x1, y1) = (MARGIN, w - MARGIN, h - MARGIN);
        // The default layout already fits its boxes, so the 24 mm band is drawn as before.
        let default = PageTemplate::default();
        for b in &default.boxes {
            let (_, t, _, bottom) = b.resolve(x0, x1, default.band_height()).unwrap();
            for bt in &b.texts {
                assert!(t + bt.y <= bottom - BELOW_BASELINE, "{} {:?}", b.label, bt.values);
                assert!(bt.size <= bt.y, "{} {:?}", b.label, bt.values);
            }
        }
        p.meta.template = PageTemplate {
            height: 20.0,
            ..Default::default()
        };
        let ty = y1 - 20.0;
        let marks = frame_marks(&p, &page, 0, 1, "");
        let mut seen = 0;
        for m in &marks {
            if let Shape::Text { at, height, text, .. } = &m.shape
                && at.y > ty
            {
                seen += 1;
                assert!(at.y <= y1 - BELOW_BASELINE + 1e-9, "{text} below the frame: {at:?}");
                assert!(at.y - height >= ty - 1e-9, "{text} above the band: {at:?} {height}");
            }
        }
        assert!(seen > 0, "the title block shows its texts");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_lowest_title_block_squeezes_its_texts_without_overprinting() {
        let (mut p, page) = demo(PageSize::A3, true);
        p.meta.name = "Pump station".into();
        let (_, h) = page.size.mm();
        let y1 = h - MARGIN;
        let band = wc_core::template::MIN_TITLE_HEIGHT;
        p.meta.template = PageTemplate {
            height: band,
            ..Default::default()
        };
        let ty = y1 - band;
        let marks = frame_marks(&p, &page, 0, 1, "");
        // Every title-block text as the rectangle it covers.
        let rects: Vec<(String, f64, f64, f64, f64)> = marks
            .iter()
            .filter_map(|m| match &m.shape {
                Shape::Text {
                    at,
                    text,
                    height,
                    align,
                    ..
                } if at.y > ty => {
                    let w = crate::text_width_mm(text, *height);
                    let left = match align {
                        Align::Left => at.x,
                        Align::Center => at.x - w / 2.0,
                        Align::Right => at.x - w,
                    };
                    Some((text.clone(), left, left + w, at.y - height, at.y))
                }
                _ => None,
            })
            .collect();
        assert!(rects.iter().any(|r| r.0 == "Pump station"), "{rects:?}");
        assert!(rects.iter().any(|r| r.0 == page.title), "{rects:?}");
        for (i, a) in rects.iter().enumerate() {
            assert!(
                a.3 >= ty - 1e-9 && a.4 <= y1 - BELOW_BASELINE + 1e-9,
                "{a:?} leaves the band"
            );
            for b in &rects[i + 1..] {
                let apart = a.2 <= b.1 + 1e-9 || b.2 <= a.1 + 1e-9 || a.4 <= b.3 + 1e-9 || b.4 <= a.3 + 1e-9;
                assert!(apart, "{a:?} overprints {b:?}");
            }
        }
    }

    fn texts(marks: &[FrameMark]) -> Vec<(String, f64)> {
        marks
            .iter()
            .filter_map(|m| match &m.shape {
                Shape::Text { text, height, .. } => Some((text.clone(), *height)),
                _ => None,
            })
            .collect()
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn labels_and_values_stay_inside_narrow_boxes() {
        let (mut p, page) = demo(PageSize::A3, false);
        let mut t = PageTemplate::default();
        // 2 mm wide: no room for any text once padded.
        let mut tiny = TitleBox::new("Tiny", (100.0, Side::Left), (102.0, Side::Left), 0.0, 12.0);
        tiny.texts.push(BoxText::new(&["customer"], 2.5, 9.5, Align::Left));
        // 12 mm wide with a long label: the label shrinks.
        let narrow = TitleBox::new(
            "A label far too long for its box",
            (110.0, Side::Left),
            (122.0, Side::Left),
            0.0,
            12.0,
        );
        t.boxes.push(tiny);
        t.boxes.push(narrow);
        p.meta.template = t;
        let all = texts(&frame_marks(&p, &page, 0, 1, ""));
        assert!(!all.iter().any(|(t, _)| t == "Tiny"), "no room for the label");
        assert_eq!(
            all.iter().filter(|(t, _)| t == "ACME").count(),
            1,
            "only the customer box shows the value, not the 2 mm box"
        );
        let (label, h) = all
            .iter()
            .find(|(t, _)| t.starts_with("A label"))
            .cloned()
            .expect("narrow label is rendered");
        assert!(h < 1.8, "{label} drawn at {h}");
        assert!(crate::writer::text_width_mm(&label, h) <= 12.0 - 2.0 * PAD + 1e-6);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn cross_references_use_the_templates_columns() {
        use wc_core::geom::Point;
        use wc_core::symbol::{Pin, PinDir, SymbolRole};
        let coil = Symbol {
            source: String::new(),
            id: "coil".into(),
            name: "coil".into(),
            category: String::new(),
            letter_code: "K".into(),
            role: SymbolRole::Main,
            graphics: Vec::new(),
            pins: vec![Pin {
                name: "A1".into(),
                at: Point::new(0.0, 0.0),
                dir: PinDir::Up,
            }],
            tag_at: Point::new(0.0, 0.0),
            description: String::new(),
        };
        let symbols = BTreeMap::from([("coil".to_owned(), coil)]);
        let (mut p, mut page) = demo(PageSize::A3, false);
        // 205 mm across an A3 frame: column 5 of 10, column 10 of 20.
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "coil".into(),
            at: Point::new(205.0, 100.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(DeviceTag::parse("-K1").unwrap()),
        });
        p.pages.push(page);
        let column = |p: &Project| CrossRefs::build(p, &symbols).mains.values().next().unwrap()[0].column;
        assert_eq!(column(&p), 5);
        p.meta.template.columns = 20;
        assert_eq!(column(&p), 10);
    }

    #[test]
    fn overlapping_box_sides_are_drawn_once() {
        let merged = merge_segments(vec![(5.0, 0.0, 2.0), (5.0, 3.0, 2.0), (5.0, 4.0, 6.0), (1.0, 0.0, 1.0)]);
        assert_eq!(merged, vec![(1.0, 0.0, 1.0), (5.0, 0.0, 3.0), (5.0, 4.0, 6.0)]);
    }
}
