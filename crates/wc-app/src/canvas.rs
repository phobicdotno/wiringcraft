//! The schematic canvas: paper, frame, grid, page graphics, wires and symbols, plus the
//! mouse handling for the select, place and wire tools.
//!
//! Page coordinates are millimetres with y pointing down, like the paper and the PDF export.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape as EShape, Stroke, Vec2};
use wc_core::geom::{Align, Point};
use wc_core::route::Route;
use wc_core::{Endpoint, Page, Shape};

use crate::layers::Layer;
use crate::state::{Open, Selection, Tool};
use crate::theme;

/// How close (in mm) a click must be to a pin to snap a wire to it.
const PIN_CATCH: f64 = 2.0;

/// Pan and zoom: `zoom` is screen points per millimetre, `offset` is where the page origin sits
/// relative to the canvas' top-left corner.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub zoom: f32,
    pub offset: Vec2,
    /// Fit the page on the next frame (after opening, or when asked).
    pub fit: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 3.0,
            offset: Vec2::new(20.0, 20.0),
            fit: true,
        }
    }
}

/// Everything the canvas needs besides the project.
pub struct Editor {
    pub page: usize,
    pub tool: Tool,
    /// Selected items; the properties panel edits the item when exactly one is selected.
    pub selection: Vec<Selection>,
    /// Library symbol the place tool puts down.
    pub place_symbol: Option<String>,
    /// A wire being drawn: its start and the bends so far.
    pub wire: Option<(Endpoint, Vec<Point>)>,
    pub view: View,
    pub cursor: Option<Point>,
    drag: Option<Drag>,
    /// The clicks so far of a line, rectangle, circle, arc or polyline being drawn.
    pub shape_points: Vec<Point>,
    /// The id of the page those clicks were made on: on another page they are dropped.
    pub shape_page: Option<String>,
    /// Enter was pressed (and no dialog or text field took it): finish the polyline being drawn.
    pub finish_shape: bool,
    /// Where and when (egui time, seconds) the last drawing click landed, to tell a quick
    /// repeat on a polyline's last point (which ends it) from a slow one.
    pub last_shape_click: Option<(Point, f64)>,
    /// The text tool was clicked here; the app asks for the text.
    pub ask_text: Option<Point>,
    /// The comment tool was clicked here; the app asks for the comment.
    pub ask_comment: Option<Point>,
    /// The review comment shown in the properties panel, by id.
    pub comment: Option<String>,
    pub message: String,
    /// The panel layout shown instead of a page, by index.
    pub panel: Option<usize>,
    /// The device item picked in the panel view, by index into the panel's items.
    pub panel_pick: Option<usize>,
    /// A device being dragged in the panel view: the grab offset and whether the undo
    /// checkpoint was taken.
    pub panel_drag: Option<(Point, bool)>,
    /// Wire pins that line up when a symbol is placed or moved (the app sets it from the
    /// preferences). Holding Alt skips it once.
    pub auto_wire: bool,
    /// In a shared session: pages other people have open (read-only here), page id to name.
    pub held_by: std::collections::BTreeMap<String, String>,
    /// The terminal strip a navigator menu asked to export markers of; the WAGO marking dialog
    /// opens with only that strip when it is set.
    pub marking_strip: Option<String>,
    /// Layers hidden or locked on the canvas (the app sets it from the preferences).
    pub layers: crate::layers::Layers,
    /// Findings of the check that runs while drawing, marked on their items; empty when it is
    /// off.
    pub live: Vec<wc_report::Finding>,
    /// Selected free drawings a sync showed the page without while this person's change was
    /// on its way to the host: selected again when the host accepts it.
    pub waiting_graphics: Option<crate::edit::WaitingGraphics>,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            page: 0,
            tool: Tool::Select,
            selection: Vec::new(),
            place_symbol: None,
            wire: None,
            view: View::default(),
            cursor: None,
            drag: None,
            shape_points: Vec::new(),
            shape_page: None,
            finish_shape: false,
            last_shape_click: None,
            ask_text: None,
            ask_comment: None,
            comment: None,
            message: String::new(),
            marking_strip: None,
            panel: None,
            panel_pick: None,
            panel_drag: None,
            auto_wire: false,
            held_by: Default::default(),
            layers: Default::default(),
            live: Vec::new(),
            waiting_graphics: None,
        }
    }
}

/// What a primary-button drag in the select tool is doing.
#[derive(Debug, Clone, Copy)]
enum Drag {
    /// Moving the selection: the grab point (snapped, page mm) and whether the undo checkpoint
    /// for this drag was taken.
    Move { from: Point, checkpoint: bool },
    /// Drawing a selection box from this corner.
    Box { from: Point },
    /// Moving the logo on the cover page, or with `size` sizing it from its bottom-right
    /// corner: the grab point, the logo's box when the drag started, and whether the undo
    /// checkpoint for this drag was taken.
    Logo {
        from: Point,
        start: (Point, Point),
        size: bool,
        checkpoint: bool,
    },
}

impl Editor {
    /// Who holds the page shown, in a shared session: then it is read-only here and only
    /// selecting, panning and zooming work on it. `None` in the panel view and on own pages.
    pub fn held_page<'a>(&'a self, project: &wc_core::Project) -> Option<&'a str> {
        if self.panel.is_some() {
            return None;
        }
        let page = project.pages.get(self.page)?;
        self.held_by.get(&page.id).map(String::as_str)
    }

    /// The selected item when exactly one is selected.
    pub fn one(&self) -> Option<&Selection> {
        match self.selection.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    pub fn select_only(&mut self, s: Selection) {
        self.selection = vec![s];
    }

    pub fn is_selected(&self, s: &Selection) -> bool {
        self.selection.contains(s)
    }
}

pub(crate) struct Xf {
    pub(crate) origin: Pos2,
    pub(crate) zoom: f32,
}

impl Xf {
    pub(crate) fn s(&self, p: Point) -> Pos2 {
        self.origin + Vec2::new(p.x as f32, p.y as f32) * self.zoom
    }
    pub(crate) fn mm(&self, p: Pos2) -> Point {
        let v = (p - self.origin) / self.zoom;
        Point::new(f64::from(v.x), f64::from(v.y))
    }
    pub(crate) fn len(&self, mm: f64) -> f32 {
        mm as f32 * self.zoom
    }
}

fn snap(p: Point, step: f64) -> Point {
    if step <= 0.0 {
        return p;
    }
    Point::new((p.x / step).round() * step, (p.y / step).round() * step)
}

/// The signed sweep from `start` to `end` in degrees as an arc is drawn: counterclockwise,
/// a full turn when they are equal. Angles stored past a full turn (from a file) can make it
/// negative, which draws the other way round.
pub(crate) fn arc_sweep(start: f64, end: f64) -> f64 {
    let sweep = end - start;
    if sweep <= 0.0 { sweep + 360.0 } else { sweep }
}

pub(crate) fn arc_points(center: Point, r: f64, start: f64, end: f64) -> Vec<Point> {
    let sweep = arc_sweep(start, end);
    let n = ((sweep.abs() / 10.0).ceil() as usize).clamp(2, 72);
    (0..=n)
        .map(|i| {
            let t = (start + sweep * i as f64 / n as f64).to_radians();
            Point::new(center.x + r * t.cos(), center.y + r * t.sin())
        })
        .collect()
}

/// Whether an outline turns the same way at every corner (straight corners do not count).
fn convex(pts: &[Pos2]) -> bool {
    let n = pts.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let (a, b, c) = (pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        let t = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
        if t.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = t.signum();
        } else if t.signum() != sign {
            return false;
        }
    }
    true
}

/// Triangles (vertex indexes) covering a simple polygon, by ear clipping. A polygon with a hole
/// cut in along a bridge (its two sides on top of each other) comes out right too.
fn triangulate(pts: &[Pos2]) -> Vec<[u32; 3]> {
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let area: f32 = (0..pts.len())
        .map(|i| cross(Pos2::ZERO, pts[i], pts[(i + 1) % pts.len()]))
        .sum();
    let turn = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < pts.len() * pts.len() {
        guard += 1;
        let n = idx.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (pts[idx[(i + n - 1) % n]], pts[idx[i]], pts[idx[(i + 1) % n]]);
            if cross(a, b, c) * turn <= 0.0 {
                return false;
            }
            // No other corner strictly inside the triangle.
            idx.iter().all(|&k| {
                let q = pts[k];
                q == a || q == b || q == c || {
                    let (d1, d2, d3) = (cross(a, b, q) * turn, cross(b, c, q) * turn, cross(c, a, q) * turn);
                    d1 < 0.0 || d2 < 0.0 || d3 < 0.0
                }
            })
        });
        match ear {
            Some(i) => {
                out.push([idx[(i + n - 1) % n] as u32, idx[i] as u32, idx[(i + 1) % n] as u32]);
                idx.remove(i);
            }
            // Only flat corners left (a bridge, collinear points): drop one and go on.
            None => {
                if let Some(i) =
                    (0..n).find(|&i| cross(pts[idx[(i + n - 1) % n]], pts[idx[i]], pts[idx[(i + 1) % n]]).abs() < 1e-6)
                {
                    idx.remove(i);
                } else {
                    break;
                }
            }
        }
    }
    if let [a, b, c] = idx[..] {
        out.push([a as u32, b as u32, c as u32]);
    }
    out
}

pub(crate) fn paint_shape(p: &egui::Painter, xf: &Xf, s: &Shape, stroke: Stroke) {
    match s {
        Shape::Line { a, b } => {
            p.line_segment([xf.s(*a), xf.s(*b)], stroke);
        }
        Shape::Polyline { points, closed, filled } => {
            let pts: Vec<Pos2> = points.iter().map(|q| xf.s(*q)).collect();
            // A filled shape is drawn without its outline: the cut that carries a hole would
            // show as a line.
            if *filled && convex(&pts) {
                p.add(EShape::convex_polygon(pts, stroke.color, Stroke::NONE));
            } else if *filled {
                // egui fills only convex paths itself, so cut the outline into triangles.
                let mut mesh = egui::Mesh::default();
                for p in &pts {
                    mesh.colored_vertex(*p, stroke.color);
                }
                for [a, b, c] in triangulate(&pts) {
                    mesh.add_triangle(a, b, c);
                }
                p.add(EShape::mesh(mesh));
            } else if *closed {
                p.add(EShape::closed_line(pts, stroke));
            } else {
                p.add(EShape::line(pts, stroke));
            }
        }
        Shape::Rect { min, max } => {
            p.rect_stroke(
                Rect::from_two_pos(xf.s(*min), xf.s(*max)),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Shape::Circle { center, radius, filled } => {
            if *filled {
                p.circle_filled(xf.s(*center), xf.len(*radius), stroke.color);
            } else {
                p.circle_stroke(xf.s(*center), xf.len(*radius), stroke);
            }
        }
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => {
            let pts = arc_points(*center, *radius, *start, *end)
                .into_iter()
                .map(|q| xf.s(q))
                .collect();
            p.add(EShape::line(pts, stroke));
        }
        Shape::Text {
            at,
            text,
            height,
            align,
            angle,
        } => {
            if *angle != 0.0 && angle.is_finite() {
                turned_text_mm(p, xf, *at, *height, *align, *angle, text, stroke.color);
                return;
            }
            let a = match align {
                Align::Left => Align2::LEFT_BOTTOM,
                Align::Center => Align2::CENTER_BOTTOM,
                Align::Right => Align2::RIGHT_BOTTOM,
            };
            text_mm(p, xf, *at, *height, a, text, stroke.color);
        }
    }
}

/// Text whose baseline sits at `at`, `height` mm tall (the PDF's font size).
fn text_mm(p: &egui::Painter, xf: &Xf, at: Point, height: f64, align: Align2, text: &str, c: Color32) {
    let size = xf.len(height) * 1.15;
    if size < MIN_TEXT_PX {
        return; // Too small to read: skip rather than smear.
    }
    // egui places the text box; nudge down so the baseline, not the box bottom, sits on `at`.
    let pos = xf.s(at) + Vec2::new(0.0, size * 0.22);
    p.text(pos, align, text, FontId::proportional(size), c);
}

/// Text turned `angle` degrees counter-clockwise around its baseline anchor `at`.
#[allow(clippy::too_many_arguments)]
fn turned_text_mm(
    p: &egui::Painter,
    xf: &Xf,
    at: Point,
    height: f64,
    align: Align,
    angle: f64,
    text: &str,
    c: Color32,
) {
    let size = xf.len(height) * 1.15;
    if size < MIN_TEXT_PX {
        return;
    }
    let galley = p.layout_no_wrap(text.to_owned(), FontId::proportional(size), c);
    let (w, h) = (galley.size().x, galley.size().y);
    let shift = match align {
        Align::Left => 0.0,
        Align::Center => -w / 2.0,
        Align::Right => -w,
    };
    // Galley top left in the text's own frame (baseline at 0), turned onto the screen; egui
    // turns clockwise on screen, which is counter-clockwise on paper negated.
    let theta = -(angle as f32).to_radians();
    let (sin, cos) = theta.sin_cos();
    let (ox, oy) = (shift, -0.8 * h);
    let offset = Vec2::new(ox * cos - oy * sin, ox * sin + oy * cos);
    let pos = xf.s(at) + offset;
    p.add(egui::epaint::TextShape::new(pos, galley, c).with_angle(theta));
}

/// Distance from `p` to the segment `a`-`b`.
fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

impl Open {
    /// The bounding box of a placed symbol in page mm.
    pub fn symbol_bounds(&self, page: &Page, id: &str) -> Option<(Point, Point)> {
        let placed = page.symbol(id)?;
        let Some(sym) = self.symbols.get(&placed.symbol) else {
            return Some(wc_core::Symbol::unknown_bounds(placed.at));
        };
        Some(sym.placed_bounds(placed.at, placed.rotation, placed.mirror))
    }

    /// Points of a wire, ends resolved; `None` when an end is on a missing symbol.
    pub fn wire_points(&self, page: &Page, w: &wc_core::Wire) -> Option<Vec<Point>> {
        let mut pts = vec![self.end_pos(page, &w.from)?];
        pts.extend(w.via.iter().copied());
        pts.push(self.end_pos(page, &w.to)?);
        Some(pts)
    }

    /// The nearest pin to `at` within `catch` mm.
    pub fn pin_near(&self, page: &Page, at: Point, catch: f64) -> Option<(String, String, Point)> {
        let mut best: Option<(f64, (String, String, Point))> = None;
        for s in &page.symbols {
            let Some(sym) = self.symbols.get(&s.symbol) else {
                continue;
            };
            for pin in &sym.pins {
                let pos = pin.at.transform(s.rotation, s.mirror) + s.at;
                let d = pos.distance(at);
                if d <= catch && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, (s.id.clone(), pin.name.clone(), pos)));
                }
            }
        }
        best.map(|(_, b)| b)
    }

    /// What is under `at` on a page: symbols win over wires.
    pub fn hit(&self, page: &Page, at: Point, tol: f64) -> Option<Selection> {
        for s in page.symbols.iter().rev() {
            if let Some((a, b)) = self.symbol_bounds(page, &s.id)
                && at.x >= a.x - tol
                && at.x <= b.x + tol
                && at.y >= a.y - tol
                && at.y <= b.y + tol
            {
                return Some(Selection::Symbol(s.id.clone()));
            }
        }
        for w in page.wires.iter().rev() {
            if let Some(pts) = self.wire_points(page, w)
                && pts.windows(2).any(|ab| seg_dist(at, ab[0], ab[1]) <= tol)
            {
                return Some(Selection::Wire(w.id.clone()));
            }
        }
        page.graphics
            .iter()
            .rposition(|g| crate::edit::graphic_hit(g, at, tol))
            .map(Selection::Graphic)
    }
}

fn paint_frame(p: &egui::Painter, xf: &Xf, open: &Open, page: &Page, c: Color32, faint: Color32) {
    let total = open.project.pages.len();
    let idx = open.project.pages.iter().position(|q| q.id == page.id).unwrap_or(0);
    for m in wc_pdf::frame_marks(&open.project, page, idx, total, "") {
        // A title-block value too small to read at this zoom still shows that it is there.
        if let Some((a, b)) = value_placeholder(&m, page.size.mm(), open.project.meta.template.band_height(), xf.zoom) {
            p.rect_filled(Rect::from_two_pos(xf.s(a), xf.s(b)), 0.0, faint.gamma_multiply(0.6));
            continue;
        }
        let stroke = match m.ink {
            wc_pdf::Ink::Thick => Stroke::new(xf.len(0.5).max(1.0), c),
            wc_pdf::Ink::Thin => Stroke::new(xf.len(0.25).max(0.6), c),
            wc_pdf::Ink::Label => Stroke::new(1.0_f32, faint),
        };
        paint_shape(p, xf, &m.shape, stroke);
    }
}

/// Texts smaller than this on screen (px) are not drawn: they would only smear.
const MIN_TEXT_PX: f32 = 3.0;

/// The bar (corners in mm) drawn instead of a title-block value that is too small to read at
/// `zoom` (px per mm) on a sheet of `size` mm: as wide as the text and a little lower than it,
/// so the user sees a value is there; `band` is the title block's height in mm. `None` for
/// every other mark, and for a value big enough to draw; other small texts (column numbers,
/// labels) are simply left out as before.
pub(crate) fn value_placeholder(
    m: &wc_pdf::FrameMark,
    size: (f64, f64),
    band: f64,
    zoom: f32,
) -> Option<(Point, Point)> {
    let Shape::Text {
        at,
        text,
        height,
        align,
        ..
    } = &m.shape
    else {
        return None;
    };
    if m.ink != wc_pdf::Ink::Thin || text.trim().is_empty() || (*height as f32) * zoom * 1.15 >= MIN_TEXT_PX {
        return None;
    }
    // Values sit in the title block band at the foot of the frame; column numbers and row
    // letters, drawn in the same ink, sit above it or outside the frame.
    let margin = wc_core::project::FRAME_MARGIN;
    let (x0, x1, y1) = (margin, size.0 - margin, size.1 - margin);
    let band = y1 - band;
    if at.x < x0 || at.x > x1 || at.y <= band || at.y > y1 {
        return None;
    }
    let w = wc_pdf::text_width_mm(text, *height);
    let left = match align {
        Align::Left => at.x,
        Align::Center => at.x - w / 2.0,
        Align::Right => at.x - w,
    };
    Some((Point::new(left, at.y - height * 0.7), Point::new(left + w, at.y)))
}

/// Show the canvas for the current page and handle the tools. Returns true when the project
/// changed.
pub fn show(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor, show_grid: bool, snap_step: f64) {
    drop_draft_of_other_page(open, ed);
    // An item on a layer that was just hidden or locked does not stay selected.
    let layers = ed.layers.clone();
    ed.selection.retain(|s| layers.may_select(s));
    let finish = std::mem::take(&mut ed.finish_shape);
    if let Some(panel) = ed.panel {
        crate::panel_view::show(ui, open, ed, panel, snap_step);
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let t = theme::Theme::of(ui.ctx());
    painter.rect_filled(rect, 0.0, t.backdrop);
    // A faint dot texture on the desk around the sheet, fixed to the screen.
    {
        let step = 18.0;
        let mut y = rect.top() + step / 2.0;
        while y < rect.bottom() {
            let mut x = rect.left() + step / 2.0;
            while x < rect.right() {
                painter.circle_filled(Pos2::new(x, y), 0.9, t.backdrop_dot);
                x += step;
            }
            y += step;
        }
    }

    let Some(page) = open.project.pages.get(ed.page).cloned() else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "This project has no pages yet. Use Pages → New page.",
            FontId::proportional(16.0),
            t.faint,
        );
        return;
    };
    let (pw, ph) = page.size.mm();

    if ed.view.fit {
        let z = ((rect.width() - 64.0) / pw as f32)
            .min((rect.height() - 72.0) / ph as f32)
            .max(0.2);
        ed.view.zoom = z;
        ed.view.offset = Vec2::new(
            (rect.width() - pw as f32 * z) / 2.0,
            (rect.height() - ph as f32 * z) / 2.0 + 6.0,
        );
        ed.view.fit = false;
    }

    // The view the user saw when they clicked: a zoom or pan in the same frame must not move
    // where a click lands.
    let shown = Xf {
        origin: rect.min + ed.view.offset,
        zoom: ed.view.zoom,
    };
    // Zoom about the pointer with the wheel; pan with the middle or right button.
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 200.0);
        if scroll.abs() > 0.0
            && let Some(hp) = resp.hover_pos()
        {
            let factor = (scroll / 300.0).exp();
            let new_zoom = (ed.view.zoom * factor).clamp(0.3, 60.0);
            let anchor = hp - rect.min - ed.view.offset;
            ed.view.offset += anchor - anchor * (new_zoom / ed.view.zoom);
            ed.view.zoom = new_zoom;
        }
    }
    if resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) {
        ed.view.offset += resp.drag_delta();
    }

    let xf = Xf {
        origin: rect.min + ed.view.offset,
        zoom: ed.view.zoom,
    };
    let paper = Rect::from_two_pos(xf.s(Point::new(0.0, 0.0)), xf.s(Point::new(pw, ph)));
    // The sheet: a soft drop shadow, the paper, and a hairline edge.
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: t.shadow,
        }
        .as_shape(paper, 2.0),
    );
    painter.rect_filled(paper, 2.0, t.paper);
    painter.rect_stroke(
        paper,
        2.0,
        Stroke::new(1.0_f32, t.shadow.gamma_multiply(0.5)),
        egui::StrokeKind::Outside,
    );
    // Page caption above the sheet.
    if paper.top() - rect.top() > 22.0 {
        painter.text(
            paper.left_top() + Vec2::new(0.0, -8.0),
            Align2::LEFT_BOTTOM,
            format!("Page {}  ·  {}", page.number, page.title),
            theme::strong(12.0),
            t.muted,
        );
    }

    if show_grid {
        // Minor dots on the snap-sized grid, with a small cross every 50 mm. Only the part of
        // the sheet that is on screen is drawn.
        let step = if xf.len(5.0) >= 8.0 { 5.0 } else { 10.0 };
        if xf.len(step) >= 6.0 {
            let vis = paper.intersect(rect);
            let lo = xf.mm(vis.min);
            let hi = xf.mm(vis.max);
            let first = |v: f64| ((v / step).floor().max(1.0)) * step;
            let r = (xf.len(0.35)).clamp(0.6, 1.1);
            let arm = xf.len(1.2).clamp(2.5, 5.0);
            let mut y = first(lo.y.min(hi.y));
            while y < ph && y <= lo.y.max(hi.y) {
                let mut x = first(lo.x);
                while x < pw && x <= hi.x {
                    let at = xf.s(Point::new(x, y));
                    let major = (x % 50.0).abs() < 1e-6 && (y % 50.0).abs() < 1e-6;
                    if major {
                        let st = Stroke::new(1.0_f32, t.grid_major);
                        painter.line_segment([at - Vec2::new(arm, 0.0), at + Vec2::new(arm, 0.0)], st);
                        painter.line_segment([at - Vec2::new(0.0, arm), at + Vec2::new(0.0, arm)], st);
                    } else {
                        painter.circle_filled(at, r, t.grid);
                    }
                    x += step;
                }
                y += step;
            }
        }
    }

    // A scanned sheet under the page, toned down so what is drawn over it stands out.
    if let Some(u) = page.underlay.as_ref().filter(|_| ed.layers.shown(Layer::Underlay)) {
        let area = Rect::from_two_pos(xf.s(u.min), xf.s(u.max));
        match open.underlay_texture(ui.ctx(), u) {
            Some(tex) => {
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(tex.id(), area, uv, Color32::from_white_alpha(150));
            }
            None => {
                painter.rect_stroke(area, 0.0, Stroke::new(1.0_f32, t.faint), egui::StrokeKind::Inside);
                painter.text(
                    area.center(),
                    Align2::CENTER_CENTER,
                    format!("Underlay picture {} cannot be shown", u.file),
                    FontId::proportional(13.0),
                    t.faint,
                );
            }
        }
    }

    paint_frame(&painter, &xf, open, &page, t.ink, t.faint);
    paint_logo(
        &painter,
        &xf,
        open,
        &page,
        ui.ctx(),
        &t,
        matches!(ed.tool, Tool::Select),
    );

    for (i, g) in page
        .graphics
        .iter()
        .enumerate()
        .filter(|_| ed.layers.shown(Layer::Drawings))
    {
        let selected = ed.is_selected(&Selection::Graphic(i));
        let c = if selected { t.selection } else { t.ink };
        paint_shape(&painter, &xf, g, Stroke::new(xf.len(0.25).max(0.8), c));
        if selected {
            let (a, b) = crate::edit::graphic_bounds(g);
            let r = Rect::from_two_pos(xf.s(a), xf.s(b)).expand(3.0);
            painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, t.selection), egui::StrokeKind::Outside);
        }
    }

    // Revision clouds and the revision table, drawn from the same shapes as the PDF.
    let revision_ink = Stroke::new(xf.len(0.25).max(0.8), t.ink);
    for c in &page.clouds {
        for s in c.shapes() {
            paint_shape(&painter, &xf, &s, revision_ink);
        }
    }
    if let Some(at) = page.revision_table {
        for s in wc_core::revision::table_shapes(&open.project.meta.revisions, at) {
            paint_shape(&painter, &xf, &s, revision_ink);
        }
    }

    for w in &page.wires {
        let Some(pts) = open.wire_points(&page, w) else {
            continue;
        };
        let selected = ed.is_selected(&Selection::Wire(w.id.clone()));
        let c = if selected { t.selection } else { t.wire };
        let stroke = Stroke::new(xf.len(0.35).max(1.2) * if selected { 2.0 } else { 1.0 }, c);
        painter.add(EShape::line(pts.iter().map(|q| xf.s(*q)).collect(), stroke));
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
        if !label.is_empty() && pts.len() >= 2 && ed.layers.shown(Layer::WireNumbers) {
            let (a, b) = (pts[0], pts[1]);
            let mid = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            if (a.x - b.x).abs() < (a.y - b.y).abs() {
                text_mm(
                    &painter,
                    &xf,
                    mid + Point::new(-1.0, 0.8),
                    2.0,
                    Align2::RIGHT_BOTTOM,
                    &label.join(" "),
                    t.wire_label,
                );
            } else {
                text_mm(
                    &painter,
                    &xf,
                    mid + Point::new(0.0, -1.0),
                    2.0,
                    Align2::CENTER_BOTTOM,
                    &label.join(" "),
                    t.wire_label,
                );
            }
        }
    }

    // Jumpers: a bar through the jumpered terminals drawn on this page, in strip order.
    let numbers = if open.project.meta.jumpers.is_empty() {
        Default::default()
    } else {
        wc_core::project::terminal_numbers(&open.project, |id| open.symbols.get(id).map(|s| s.role))
    };
    for j in &open.project.meta.jumpers {
        let at = jumper_points(&page, &open.symbols, &numbers, j);
        if at.len() < 2 {
            continue;
        }
        let stroke = Stroke::new(xf.len(0.6).max(1.5), t.wire);
        for pair in at.windows(2) {
            if let [a, b] = pair {
                painter.line_segment([xf.s(*a), xf.s(*b)], stroke);
            }
        }
        for p in &at {
            painter.circle_filled(xf.s(*p), xf.len(0.9).max(2.0), t.wire);
        }
    }

    for s in &page.symbols {
        let selected = ed.is_selected(&Selection::Symbol(s.id.clone()));
        let c = if selected { t.selection } else { t.ink };
        let stroke = Stroke::new(xf.len(0.3).max(1.0), c);
        let Some(sym) = open.symbols.get(&s.symbol) else {
            let r = Rect::from_center_size(xf.s(s.at), Vec2::splat(xf.len(8.0)));
            painter.rect_stroke(r, 0.0, Stroke::new(1.5_f32, t.error), egui::StrokeKind::Middle);
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                "?",
                FontId::proportional(xf.len(4.0)),
                t.error,
            );
            continue;
        };
        for g in &sym.graphics {
            paint_shape(&painter, &xf, &g.placed(s.at, s.rotation, s.mirror), stroke);
        }
        for pin in &sym.pins {
            let pos = pin.at.transform(s.rotation, s.mirror) + s.at;
            painter.circle_filled(xf.s(pos), xf.len(0.45).max(1.5), t.pin);
        }
        if let Some(tag) = &s.tag {
            let ta = sym.tag_at.transform(s.rotation, s.mirror) + s.at;
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
                text_mm(
                    &painter,
                    &xf,
                    Point::new(ta.x, top + 3.0 * i as f64),
                    2.5,
                    Align2::RIGHT_BOTTOM,
                    l,
                    c,
                );
            }
        }
        if selected && let Some((a, b)) = open.symbol_bounds(&page, &s.id) {
            let r = Rect::from_two_pos(xf.s(a), xf.s(b)).expand(4.0);
            painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, t.selection), egui::StrokeKind::Outside);
        }
    }

    // Junction dots where three or more wire branches meet.
    let polylines: Vec<Vec<(f64, f64)>> = page
        .wires
        .iter()
        .filter_map(|w| open.wire_points(&page, w))
        .map(|pts| pts.iter().map(|q| (q.x, q.y)).collect())
        .collect();
    for (x, y) in wc_pdf::junctions(&polylines) {
        painter.circle_filled(xf.s(Point::new(x, y)), xf.len(0.6).max(2.0), t.wire);
    }

    // Cross-references (coil to contacts, contact to coil, interruption points). Clicking one
    // opens the page it points at.
    let xref = wc_pdf::CrossRefs::build(&open.project, &open.symbols);
    let mut xref_warnings = Vec::new();
    let marks = if ed.layers.shown(Layer::CrossReferences) {
        wc_pdf::xref_marks(&page, &open.symbols, &xref, &mut xref_warnings)
    } else {
        Vec::new()
    };
    let mut jump = None;
    for m in &marks {
        let (a, b) = crate::edit::text_box(m.at, wc_pdf::XREF_HEIGHT, m.align, &m.text);
        let r = Rect::from_two_pos(xf.s(a), xf.s(b));
        let hovered = ed.layers.pickable(Layer::CrossReferences) && resp.hover_pos().is_some_and(|h| r.contains(h));
        let c = if hovered { t.selection } else { t.wire_label };
        let al = match m.align {
            Align::Left => Align2::LEFT_BOTTOM,
            Align::Center => Align2::CENTER_BOTTOM,
            Align::Right => Align2::RIGHT_BOTTOM,
        };
        text_mm(&painter, &xf, m.at, wc_pdf::XREF_HEIGHT, al, &m.text, c);
        if hovered {
            painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0_f32, c));
            if ed.tool == Tool::Select && resp.clicked_by(egui::PointerButton::Primary) {
                jump = Some(m.target_page_id.clone());
            }
        }
    }
    if let Some(target) = jump
        && let Some(i) = open.project.pages.iter().position(|p| p.id == target)
    {
        ed.page = i;
        ed.selection.clear();
        ed.view.fit = true;
        ed.message = format!("Page {}", open.project.pages[i].number);
        return;
    }

    paint_live_findings(&painter, &xf, open, &page, ed, resp.hover_pos(), &t);

    // Review comments pinned on the page: a numbered bubble whose tail points at the spot,
    // greyed once resolved. Hovering shows the start of the text; clicking opens it in the
    // properties panel.
    let mut clicked_comment = None;
    for c in open
        .project
        .page_comments(&page.id)
        .filter(|_| ed.layers.shown(Layer::Comments))
    {
        let Some(at) = c.at else { continue };
        let spot = xf.s(at);
        let r = 9.0_f32;
        let center = spot + Vec2::new(r + 2.0, -r - 2.0);
        let hovered =
            ed.layers.pickable(Layer::Comments) && resp.hover_pos().is_some_and(|h| h.distance(center) <= r + 2.0);
        let chosen = ed.comment.as_deref() == Some(c.id.as_str());
        let fill = if c.resolved { t.faint } else { t.accent };
        painter.add(EShape::convex_polygon(
            vec![
                spot,
                center + Vec2::new(-r * 0.2, r * 0.9),
                center + Vec2::new(-r * 0.9, r * 0.2),
            ],
            fill,
            Stroke::NONE,
        ));
        painter.circle_filled(center, r, fill);
        if chosen || hovered {
            painter.circle_stroke(center, r + 2.5, Stroke::new(2.0_f32, t.selection));
        }
        painter.text(
            center,
            Align2::CENTER_CENTER,
            c.id.trim_start_matches('c'),
            FontId::proportional(10.0),
            Color32::WHITE,
        );
        if hovered {
            let mut preview: String = c.text.lines().next().unwrap_or_default().chars().take(60).collect();
            if preview.chars().count() < c.text.chars().count() {
                preview.push('…');
            }
            let galley = painter.layout_no_wrap(format!("{}: {preview}", c.author), FontId::proportional(12.0), t.ink);
            let size = galley.size() + Vec2::splat(10.0);
            let rect = Rect::from_min_size(center + Vec2::new(r + 6.0, -size.y / 2.0), size);
            painter.rect_filled(rect, 4.0, t.paper);
            painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, t.faint), egui::StrokeKind::Inside);
            painter.galley(rect.min + Vec2::splat(5.0), galley, t.ink);
            if ed.tool == Tool::Select && resp.clicked_by(egui::PointerButton::Primary) {
                clicked_comment = Some(c.id.clone());
            }
        }
    }
    if let Some(id) = clicked_comment {
        ed.selection.clear();
        ed.comment = Some(id);
        return;
    }
    if ed.tool == Tool::Select && resp.clicked_by(egui::PointerButton::Primary) {
        ed.comment = None;
    }

    // On a page someone else holds, only selecting (and comments, which are not part of the
    // page) work: a tool that draws or places falls back to Select.
    let held = ed.held_page(&open.project).is_some();
    if held && !matches!(ed.tool, Tool::Select | Tool::Comment) {
        ed.tool = Tool::Select;
        ed.wire = None;
        ed.shape_points.clear();
    }
    leave_unpickable_layer(ed);

    // Pointer position in page mm, snapped.
    let hover = resp.hover_pos().map(|hp| xf.mm(hp));
    ed.cursor = hover.map(|p| snap(p, snap_step));
    let tol = (6.0 / f64::from(xf.zoom)).max(0.8);

    match ed.tool {
        Tool::Select => select_tool(&painter, &resp, open, ed, &page, &xf, tol, snap_step, held),
        Tool::Place => place_tool(&painter, &resp, open, ed, &xf, &t),
        Tool::Wire => wire_tool(&painter, &resp, open, ed, &page, &xf, &t),
        Tool::Text => {
            if resp.clicked_by(egui::PointerButton::Primary)
                && let Some(at) = ed.cursor
                && on_paper(open, ed.page, at)
            {
                ed.ask_text = Some(at);
            }
        }
        Tool::Comment => {
            if resp.clicked_by(egui::PointerButton::Primary)
                && let Some(at) = hover
                && on_paper(open, ed.page, at)
            {
                ed.ask_comment = Some(at);
            }
        }
        Tool::Line | Tool::Rect | Tool::Circle | Tool::Arc | Tool::Polyline => {
            shape_tool(&painter, &resp, open, ed, &xf, &shown, &t, finish, snap_step)
        }
    }
}

/// What is under `at` that can be picked: a free drawing only while its layer is shown and
/// unlocked.
/// Where a finding of the check while drawing is marked: around a placed symbol, or at the
/// middle of a wire. `None` for anything else, or an item no longer on the page.
fn live_spot(open: &Open, page: &Page, item: &str) -> Option<(Point, Point)> {
    if let Some(b) = open.symbol_bounds(page, item) {
        return Some(b);
    }
    let w = page.wires.iter().find(|w| w.id == item)?;
    let pts = open.wire_points(page, w)?;
    // The middle of the middle segment, or the one point of a wire that has only one.
    let i = pts.len().saturating_sub(1) / 2;
    let a = *pts.get(i)?;
    let b = pts.get(i + 1).copied().unwrap_or(a);
    let mid = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    Some((mid, mid))
}

/// The findings of the check while drawing on this page: a ring in the colour of the worst
/// finding around each item, with a badge that lists the findings when hovered.
fn paint_live_findings(
    painter: &egui::Painter,
    xf: &Xf,
    open: &Open,
    page: &Page,
    ed: &Editor,
    hover: Option<Pos2>,
    t: &crate::theme::Theme,
) {
    use wc_report::Severity;
    let mut by_item: std::collections::BTreeMap<&str, Vec<&wc_report::Finding>> = Default::default();
    for f in &ed.live {
        if f.page_id.as_deref() == Some(page.id.as_str())
            && f.severity != Severity::Note
            && let Some(item) = f.item.as_deref()
        {
            by_item.entry(item).or_default().push(f);
        }
    }
    let mut tip = None;
    for (item, found) in by_item {
        let Some((a, b)) = live_spot(open, page, item) else {
            continue;
        };
        let worst = found.iter().map(|f| f.severity).min().unwrap_or(Severity::Warning);
        let c = if worst == Severity::Error { t.error } else { t.warning };
        let ring = Rect::from_two_pos(xf.s(a), xf.s(b)).expand(6.0);
        painter.rect_stroke(ring, 4.0, Stroke::new(1.5_f32, c), egui::StrokeKind::Outside);
        let badge = ring.right_top();
        let r = 6.5_f32;
        painter.circle_filled(badge, r, c);
        painter.text(
            badge,
            Align2::CENTER_CENTER,
            "!",
            crate::theme::strong(10.0),
            Color32::WHITE,
        );
        if hover.is_some_and(|h| h.distance(badge) <= r + 2.0 || ring.contains(h)) {
            tip = Some((badge, found));
        }
    }
    if let Some((badge, found)) = tip {
        let text = found
            .iter()
            .map(|f| format!("{}: {}", f.severity.label(), f.message))
            .collect::<Vec<_>>()
            .join("\n");
        let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), t.ink);
        let size = galley.size() + Vec2::splat(10.0);
        let rect = Rect::from_min_size(badge + Vec2::new(10.0, -size.y / 2.0), size);
        painter.rect_filled(rect, 4.0, t.paper);
        painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, t.faint), egui::StrokeKind::Inside);
        painter.galley(rect.min + Vec2::splat(5.0), galley, t.ink);
    }
}

fn pick(open: &Open, ed: &Editor, page: &Page, at: Point, tol: f64) -> Option<Selection> {
    open.hit(page, at, tol).filter(|s| ed.layers.may_select(s))
}

/// A tool that adds to a hidden or locked layer (free drawings and texts, or review comments)
/// falls back to Select, so nothing is drawn that cannot be seen or picked afterwards.
fn leave_unpickable_layer(ed: &mut Editor) {
    let layer = match ed.tool {
        Tool::Text | Tool::Line | Tool::Rect | Tool::Circle | Tool::Arc | Tool::Polyline => Layer::Drawings,
        Tool::Comment => Layer::Comments,
        Tool::Select | Tool::Place | Tool::Wire => return,
    };
    let Some(why) = ed.layers.refusal(layer) else {
        return;
    };
    ed.tool = Tool::Select;
    ed.shape_points.clear();
    ed.shape_page = None;
    ed.message = why;
}

#[allow(clippy::too_many_arguments)]
fn select_tool(
    painter: &egui::Painter,
    resp: &egui::Response,
    open: &mut Open,
    ed: &mut Editor,
    page: &Page,
    xf: &Xf,
    tol: f64,
    snap_step: f64,
    held: bool,
) {
    let t = theme::Theme::of(&resp.ctx);
    let shift = resp.ctx.input(|i| i.modifiers.shift || i.modifiers.command);
    if resp.clicked_by(egui::PointerButton::Primary)
        && let Some(at) = resp.interact_pointer_pos().map(|p| xf.mm(p))
    {
        match (pick(open, ed, page, at, tol), shift) {
            (Some(h), true) => {
                if let Some(i) = ed.selection.iter().position(|s| *s == h) {
                    ed.selection.remove(i);
                } else {
                    ed.selection.push(h);
                }
            }
            (Some(h), false) => ed.select_only(h),
            (None, true) => {}
            (None, false) => ed.selection.clear(),
        }
    }
    // Right-click on a wire: select it (keeping a selection it is part of) and offer the route
    // styles for every selected wire.
    if resp.secondary_clicked()
        && let Some(at) = resp.interact_pointer_pos().map(|p| xf.mm(p))
    {
        let hit = pick(open, ed, page, at, tol);
        right_clicked(ed, hit);
    }
    if !held && !selected_wires(ed).is_empty() {
        resp.context_menu(|ui| route_menu(ui, open, ed));
    }
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(at) = resp.interact_pointer_pos().map(|p| xf.mm(p))
    {
        ed.drag = Some(drag_start(open, ed, page, at, tol, snap_step, shift, held));
    }
    stop_if_held(ed, held);
    match ed.drag {
        Some(Drag::Logo {
            from,
            start,
            size,
            checkpoint,
        }) if resp.dragged_by(egui::PointerButton::Primary) => {
            // The raw pointer, like `from`: `ed.cursor` is snapped to the grid, and mixing the
            // two would jump the logo on the first move.
            if let Some(cur) = resp.interact_pointer_pos().map(|p| xf.mm(p)) {
                let (a, b) = logo_dragged(start, size, Point::new(cur.x - from.x, cur.y - from.y));
                let now = wc_pdf::cover_logo_box(&open.project, page);
                if now != Some((a, b)) {
                    if !checkpoint {
                        open.checkpoint();
                    }
                    open.set_logo_place(a, b);
                    ed.drag = Some(Drag::Logo {
                        from,
                        start,
                        size,
                        checkpoint: true,
                    });
                }
            }
        }
        Some(Drag::Move { from, checkpoint }) if resp.dragged_by(egui::PointerButton::Primary) => {
            if let Some(cur) = ed.cursor {
                // Move by whole snap steps so pins stay on the grid.
                let target = snap(cur, snap_step);
                if target != from {
                    if !checkpoint {
                        open.checkpoint();
                    }
                    let d = Point::new(target.x - from.x, target.y - from.y);
                    let sel = ed.selection.clone();
                    open.move_by(ed.page, &sel, d);
                    ed.drag = Some(Drag::Move {
                        from: target,
                        checkpoint: true,
                    });
                }
            }
        }
        Some(Drag::Box { from }) => {
            if let Some(cur) = resp.interact_pointer_pos().or(resp.hover_pos()).map(|p| xf.mm(p)) {
                let r = Rect::from_two_pos(xf.s(from), xf.s(cur));
                painter.rect_filled(r, 0.0, t.selection.gamma_multiply(0.08));
                painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, t.selection), egui::StrokeKind::Middle);
                if resp.drag_stopped() {
                    let found = ed.layers.keep(open.select_in(ed.page, from, cur));
                    if shift {
                        for f in found {
                            if !ed.is_selected(&f) {
                                ed.selection.push(f);
                            }
                        }
                    } else {
                        ed.selection = found;
                    }
                }
            }
        }
        _ => {}
    }
    if resp.drag_stopped() {
        let alt = resp.ctx.input(|i| i.modifiers.alt);
        drag_released(open, ed, alt);
    }
}

/// Whether `at` is on the sizing corner of a logo box whose bottom-right corner is `max`.
/// What a select-tool drag starting at `at` does: size or move the cover logo, move the
/// items under it (selecting them first), or draw a selection box. On a page someone else
/// holds (`held`) nothing moves: every drag is a selection box.
#[allow(clippy::too_many_arguments)]
fn drag_start(
    open: &Open,
    ed: &mut Editor,
    page: &Page,
    at: Point,
    tol: f64,
    snap_step: f64,
    shift: bool,
    held: bool,
) -> Drag {
    if held {
        return Drag::Box { from: at };
    }
    let logo = wc_pdf::cover_logo_box(&open.project, page);
    let hit = pick(open, ed, page, at, tol);
    match (logo, hit) {
        // The sizing corner wins over whatever lies under it; elsewhere the page's items do.
        (Some((a, b)), _) if logo_corner(b, at, tol) => Drag::Logo {
            from: at,
            start: (a, b),
            size: true,
            checkpoint: false,
        },
        (Some((a, b)), None) if a.x <= at.x && at.x <= b.x && a.y <= at.y && at.y <= b.y => Drag::Logo {
            from: at,
            start: (a, b),
            size: false,
            checkpoint: false,
        },
        (_, Some(h)) => {
            if !ed.is_selected(&h) {
                if shift {
                    ed.selection.push(h);
                } else {
                    ed.select_only(h);
                }
            }
            Drag::Move {
                from: snap(at, snap_step),
                checkpoint: false,
            }
        }
        (_, None) => Drag::Box { from: at },
    }
}

fn logo_corner(max: Point, at: Point, tol: f64) -> bool {
    let r = tol.max(2.0);
    (at.x - max.x).abs() <= r && (at.y - max.y).abs() <= r
}

/// The logo's box after a drag by `d` (whole millimetres) from `start`: moved, or with `size`
/// its bottom-right corner moved, keeping it at least 5 mm each way.
fn logo_dragged((a, b): (Point, Point), size: bool, d: Point) -> (Point, Point) {
    let d = Point::new(d.x.round(), d.y.round());
    if size {
        (a, Point::new((b.x + d.x).max(a.x + 5.0), (b.y + d.y).max(a.y + 5.0)))
    } else {
        (Point::new(a.x + d.x, a.y + d.y), Point::new(b.x + d.x, b.y + d.y))
    }
}

/// The project logo in the title block and, on the cover page, in its place there. With the
/// select tool the cover's logo shows its box and sizing corner.
fn paint_logo(
    p: &egui::Painter,
    xf: &Xf,
    open: &mut Open,
    page: &Page,
    ctx: &egui::Context,
    t: &theme::Theme,
    select: bool,
) {
    let Some(file) = open.project.meta.logo.as_ref().map(|l| l.file.clone()) else {
        return;
    };
    let tex = open.picture_texture(ctx, &file);
    for b in wc_pdf::logo_boxes(&open.project, page) {
        match &tex {
            Some(tex) => {
                let [w, h] = tex.size();
                let (a, z) = wc_pdf::fit_picture(b, (w as u32, h as u32));
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                p.image(tex.id(), Rect::from_two_pos(xf.s(a), xf.s(z)), uv, Color32::WHITE);
            }
            None => {
                let area = Rect::from_two_pos(xf.s(b.0), xf.s(b.1));
                p.rect_stroke(area, 0.0, Stroke::new(1.0_f32, t.faint), egui::StrokeKind::Inside);
                let label = match open.picture_problems.get(&file).map(String::as_str) {
                    None | Some("the file is missing") => "Logo missing".to_owned(),
                    Some(why) => format!("Logo cannot be shown: {why}"),
                };
                p.text(
                    area.center(),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(xf.len(2.0).clamp(8.0, 13.0)),
                    t.faint,
                );
            }
        }
    }
    if select && let Some((a, b)) = wc_pdf::cover_logo_box(&open.project, page) {
        let area = Rect::from_two_pos(xf.s(a), xf.s(b));
        p.rect_stroke(
            area,
            0.0,
            Stroke::new(1.0_f32, t.faint.gamma_multiply(0.6)),
            egui::StrokeKind::Middle,
        );
        p.rect_filled(Rect::from_center_size(area.max, Vec2::splat(7.0)), 1.0, t.faint);
    }
}

/// The end of a select-tool drag: a finished move wires the moved symbols' pins that now line
/// up (see [`auto_wire`]).
/// The page became held during a move: the move stops where it is, and its release does
/// nothing more (no wiring of pins that now line up). A selection box goes on.
fn stop_if_held(ed: &mut Editor, held: bool) {
    if held && matches!(ed.drag, Some(Drag::Move { .. } | Drag::Logo { .. })) {
        ed.drag = None;
    }
}

fn drag_released(open: &mut Open, ed: &mut Editor, alt: bool) {
    if let Some(Drag::Move { checkpoint: true, .. }) = ed.drag {
        let moved = ed
            .selection
            .iter()
            .filter_map(|s| match s {
                Selection::Symbol(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let wired = auto_wire(open, ed, alt, moved);
        if wired > 0 {
            ed.message = wired_note(wired).trim_start().to_owned();
        }
    }
    ed.drag = None;
}

/// Wire the pins of `items` that now line up, unless auto-wiring is off or Alt is held.
fn auto_wire(open: &mut Open, ed: &Editor, alt: bool, items: std::collections::BTreeSet<String>) -> usize {
    if !ed.auto_wire || alt || items.is_empty() {
        return 0;
    }
    open.auto_wire(ed.page, &items)
}

fn wired_note(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => " Connected 1 wire to pins that line up (hold Alt to skip).".into(),
        n => format!(" Connected {n} wires to pins that line up (hold Alt to skip)."),
    }
}

/// A right-click on `hit`: a wire that is not selected becomes the only selection; a selected
/// one keeps the selection it is part of, so the menu acts on all of it. Anything else leaves
/// the selection alone.
fn right_clicked(ed: &mut Editor, hit: Option<Selection>) {
    if let Some(h @ Selection::Wire(_)) = hit
        && !ed.is_selected(&h)
    {
        ed.select_only(h);
    }
}

/// The ids of the selected wires.
fn selected_wires(ed: &Editor) -> Vec<String> {
    ed.selection
        .iter()
        .filter_map(|s| match s {
            Selection::Wire(id) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// The route style shared by all of `wires`, or `None` when they differ.
fn shared_route(page: &Page, wires: &[String]) -> Option<Route> {
    let mut styles = page.wires.iter().filter(|w| wires.contains(&w.id)).map(|w| w.route);
    let first = styles.next()?;
    styles.all(|r| r == first).then_some(first)
}

/// The wire right-click menu: the route styles, the current one marked.
fn route_menu(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor) {
    let wires = selected_wires(ed);
    let Some(page) = open.project.pages.get(ed.page) else {
        return;
    };
    let current = shared_route(page, &wires);
    ui.label(if wires.len() == 1 {
        "Route".to_owned()
    } else {
        format!("Route of {} wires", wires.len())
    });
    for style in Route::ALL {
        if ui.radio(current == Some(style), style.label()).clicked() {
            ed.message = route_note(open.set_route(ed.page, &wires, style), style);
            ui.close_menu();
        }
    }
    ui.separator();
    if ui.add(egui::Button::new("Delete").shortcut_text("Del")).clicked() {
        delete_selected_wires(open, ed);
        ui.close_menu();
    }
}

/// The wire menu's Delete: remove the selected wires, as one undo step. Other selected items
/// (symbols, drawings) stay, and symbols keep their pins.
fn delete_selected_wires(open: &mut Open, ed: &mut Editor) {
    let wires: Vec<Selection> = selected_wires(ed).into_iter().map(Selection::Wire).collect();
    if wires.is_empty() {
        return;
    }
    open.delete_many(ed.page, &wires);
    ed.selection.retain(|s| !wires.contains(s));
    ed.message = match wires.len() {
        1 => "Deleted 1 wire.".to_owned(),
        n => format!("Deleted {n} wires."),
    };
}

fn route_note(changed: usize, style: Route) -> String {
    let what = style.label().to_lowercase();
    match changed {
        0 => "No wire routes changed.".to_owned(),
        1 => format!("Route changed to {what} on 1 wire."),
        n => format!("Route changed to {what} on {n} wires."),
    }
}

/// How many clicks a drawing tool takes (`None`: a polyline takes clicks until it is finished).
pub(crate) fn clicks_needed(tool: Tool) -> Option<usize> {
    match tool {
        Tool::Arc => Some(3),
        Tool::Polyline => None,
        _ => Some(2),
    }
}

/// The shape `tool` draws from the clicks `pts`, or `None` while it is too small to draw
/// (both clicks on one point, a circle without a radius, an arc that ends at its centre).
/// A line, rectangle and circle take two clicks (the circle: centre, then a point on it), an arc
/// three (centre, start, end; it runs clockwise on paper) and a polyline two or more.
pub(crate) fn drawn_shape(tool: Tool, pts: &[Point]) -> Option<Shape> {
    match (tool, pts) {
        (Tool::Line, &[a, b]) if a != b => Some(Shape::Line { a, b }),
        (Tool::Rect, &[a, b]) if a != b => Some(Shape::Rect {
            min: Point::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point::new(a.x.max(b.x), a.y.max(b.y)),
        }),
        (Tool::Circle, &[c, p]) if c != p => Some(Shape::Circle {
            center: c,
            radius: c.distance(p),
            filled: false,
        }),
        // An end on the start ray would make a full turn, which is a circle: not an arc.
        (Tool::Arc, &[c, s, e])
            if c != s
                && c != e
                && (crate::symbol_editor::angle(c, s) - crate::symbol_editor::angle(c, e)).abs() > 1e-9 =>
        {
            Some(Shape::Arc {
                center: c,
                radius: c.distance(s),
                start: crate::symbol_editor::angle(c, s),
                end: crate::symbol_editor::angle(c, e),
            })
        }
        (Tool::Polyline, pts) if pts.len() >= 2 => {
            // A polyline that ends where it started is closed.
            let mut points = pts.to_vec();
            let closed = points.len() >= 4 && points.first() == points.last();
            if closed {
                points.pop();
            }
            Some(Shape::Polyline {
                points,
                closed,
                filled: false,
            })
        }
        _ => None,
    }
}

/// What to click next, for the status line.
pub(crate) fn shape_hint(tool: Tool, clicks: usize) -> &'static str {
    match (tool, clicks) {
        (Tool::Circle, 0) => "Click the centre of the circle.",
        (Tool::Circle, _) => "Click a point on the circle (Esc cancels).",
        (Tool::Arc, 0) => "Click the centre of the arc.",
        (Tool::Arc, 1) => "Click where the arc starts (Esc cancels).",
        (Tool::Arc, _) => "Click where the arc ends; it runs clockwise (Esc cancels).",
        (Tool::Polyline, 0) => "Click the first point.",
        (Tool::Polyline, _) => {
            "Click the next point; double-click or Enter ends it, a click on the first point closes it (Esc cancels)."
        }
        (_, 0) => "Click the first point.",
        _ => "Click the other end (Esc cancels).",
    }
}

/// Put the shape drawn from `ed.shape_points` on the page and select it.
pub(crate) fn finish_shape(open: &mut Open, ed: &mut Editor) {
    let pts = std::mem::take(&mut ed.shape_points);
    if let Some(shape) = drawn_shape(ed.tool, &pts)
        && let Some(sel) = open.add_graphic(ed.page, shape)
    {
        ed.select_only(sel);
    }
    ed.message = shape_hint(ed.tool, 0).into();
}

/// The drawing tools: line, rectangle, circle, arc and polyline. Each click adds a point; the
/// shape goes on the page when it has all its clicks (a polyline: on a double-click, on Enter
/// or on a click back on its first point).
#[allow(clippy::too_many_arguments)]
fn shape_tool(
    painter: &egui::Painter,
    resp: &egui::Response,
    open: &mut Open,
    ed: &mut Editor,
    xf: &Xf,
    shown: &Xf,
    t: &theme::Theme,
    enter: bool,
    snap_step: f64,
) {
    // Enter ends a polyline wherever the pointer is.
    if enter && ed.tool == Tool::Polyline && ed.shape_points.len() >= 2 {
        finish_shape(open, ed);
        return;
    }
    // A click places its point where the button was released (a tap on a touch screen has
    // no hover, and the pointer may have moved on before this frame).
    if resp.clicked_by(egui::PointerButton::Primary)
        && let Some(pos) = primary_release_pos(&resp.ctx).or(resp.interact_pointer_pos())
    {
        let now = resp.ctx.input(|i| i.time);
        // Unsnapped, a repeat may land a little off the last point.
        let tol = if snap_step > 0.0 {
            0.0
        } else {
            f64::from(REPEAT_SLOP_PX / shown.zoom)
        };
        shape_input(open, ed, snap(shown.mm(pos), snap_step), now, tol);
    }
    let Some(cur) = ed.cursor else { return };
    let ghost = Stroke::new(1.5_f32, t.ghost);
    if !ed.shape_points.is_empty() {
        let mut pts = ed.shape_points.clone();
        pts.push(cur);
        match drawn_shape(ed.tool, &pts) {
            Some(s) => paint_shape(painter, xf, &s, ghost),
            // An arc with its centre and start only: show the full circle it lies on.
            None if ed.tool == Tool::Arc && pts.len() == 2 && pts[0] != pts[1] => {
                painter.circle_stroke(xf.s(pts[0]), xf.len(pts[0].distance(pts[1])), ghost);
            }
            None => {}
        }
    }
}

/// A drawing started on another page does not go on this one: its clicks are dropped when
/// the page shown changes (by id, so the same page moved in the navigator keeps them).
pub(crate) fn drop_draft_of_other_page(open: &Open, ed: &mut Editor) {
    let page_id = open.project.pages.get(ed.page).map(|p| p.id.as_str());
    if !ed.shape_points.is_empty() && ed.shape_page.as_deref() != page_id {
        ed.shape_points.clear();
        ed.message = shape_hint(ed.tool, 0).into();
    }
}

/// Longest time between two clicks on a polyline's last point that ends it, in seconds (egui's
/// double-click time).
const REPEAT_SECS: f64 = 0.3;

/// How far (in screen points) a repeat click may land from the last point with snapping off.
const REPEAT_SLOP_PX: f32 = 4.0;

/// Where the primary button was last released this frame, if it was.
fn primary_release_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    ctx.input(|i| {
        i.events.iter().rev().find_map(|e| match e {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            } => Some(*pos),
            _ => None,
        })
    })
}

/// A primary click at `at` at time `now` (seconds) with a drawing tool. A second click on a
/// polyline's last point within [`REPEAT_SECS`] ends it; a slower one does nothing, and a quick
/// click on another point adds that point. Points within `tol` (mm) of each other count as the
/// same (0 when snapping, so a quick click on the next grid point still adds it).
pub(crate) fn shape_input(open: &mut Open, ed: &mut Editor, at: Point, now: f64, tol: f64) {
    let same = |p: &Point| p.distance(at) <= tol;
    let repeat = ed.shape_points.last().is_some_and(same);
    let quick = ed
        .last_shape_click
        .is_some_and(|(p, t)| same(&p) && now - t <= REPEAT_SECS);
    ed.last_shape_click = Some((at, now));
    if ed.tool == Tool::Polyline && repeat {
        if quick && ed.shape_points.len() >= 2 {
            finish_shape(open, ed);
        }
        return;
    }
    // A click near the first point of a polyline of three or more closes it on that point.
    let at = match ed.shape_points.first() {
        Some(&first) if ed.tool == Tool::Polyline && ed.shape_points.len() >= 3 && same(&first) => first,
        _ => at,
    };
    shape_click(open, ed, at);
}

/// A click at `at` with a drawing tool: add the point, and put the shape on the page when it
/// has all its clicks (a polyline: when the click is back on its first point).
pub(crate) fn shape_click(open: &mut Open, ed: &mut Editor, at: Point) {
    // A polyline does not take the same point twice in a row (a double-click on it ends it).
    if ed.tool == Tool::Polyline && ed.shape_points.last() == Some(&at) {
        return;
    }
    // An arc's start on its centre would refuse every end: ask for another start.
    if ed.tool == Tool::Arc && ed.shape_points.len() == 1 && ed.shape_points[0] == at {
        ed.message = "The start must be away from the centre; click another point (Esc cancels).".into();
        return;
    }
    if ed.shape_points.is_empty() {
        ed.shape_page = open.project.pages.get(ed.page).map(|p| p.id.clone());
    }
    ed.shape_points.push(at);
    let n = ed.shape_points.len();
    let done = match clicks_needed(ed.tool) {
        Some(k) => n >= k,
        None => n >= 4 && ed.shape_points.first() == Some(&at),
    };
    if done && drawn_shape(ed.tool, &ed.shape_points).is_none() {
        // The last click makes no shape (a line or circle of no length, an arc that ends on its
        // centre or on its start ray): keep the others and ask for another point.
        ed.shape_points.pop();
        ed.message = "That point makes no shape; click another one (Esc cancels).".into();
    } else if done {
        finish_shape(open, ed);
    } else {
        ed.message = shape_hint(ed.tool, n).into();
    }
}

fn on_paper(open: &Open, page: usize, at: Point) -> bool {
    open.project.pages.get(page).is_some_and(|p| {
        let (w, h) = p.size.mm();
        at.x >= 0.0 && at.y >= 0.0 && at.x <= w && at.y <= h
    })
}

fn place_tool(
    painter: &egui::Painter,
    resp: &egui::Response,
    open: &mut Open,
    ed: &mut Editor,
    xf: &Xf,
    t: &theme::Theme,
) {
    let Some(id) = ed.place_symbol.clone() else {
        ed.message = "Pick a symbol in the library panel to place it.".into();
        return;
    };
    let Some(at) = ed.cursor else { return };
    // Ghost preview of the symbol under the pointer.
    if let Some(sym) = open.symbols.get(&id) {
        let stroke = Stroke::new(xf.len(0.3).max(1.0), t.ghost);
        for g in &sym.graphics {
            paint_shape(painter, xf, &g.placed(at, wc_core::geom::Rotation::R0, false), stroke);
        }
    } else {
        painter.rect_stroke(
            Rect::from_center_size(xf.s(at), Vec2::splat(xf.len(8.0))),
            0.0,
            Stroke::new(1.0_f32, t.ghost),
            egui::StrokeKind::Middle,
        );
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        if !on_paper(open, ed.page, at) {
            ed.message = "Click on the sheet to place the symbol.".into();
            return;
        }
        match open.place(ed.page, &id, at) {
            Ok(sid) => {
                let wired = auto_wire(
                    open,
                    ed,
                    resp.ctx.input(|i| i.modifiers.alt),
                    std::iter::once(sid.clone()).collect(),
                );
                ed.select_only(Selection::Symbol(sid));
                ed.message = format!("Placed {id}.{}", wired_note(wired));
            }
            Err(e) => ed.message = format!("Could not place {id}: {e}"),
        }
    }
}

fn wire_tool(
    painter: &egui::Painter,
    resp: &egui::Response,
    open: &mut Open,
    ed: &mut Editor,
    page: &Page,
    xf: &Xf,
    t: &theme::Theme,
) {
    let Some(cur) = ed.cursor else { return };
    let raw = resp.hover_pos().map(|p| xf.mm(p)).unwrap_or(cur);
    let near = open.pin_near(page, raw, PIN_CATCH);
    let target = near.as_ref().map(|(_, _, p)| *p).unwrap_or(cur);
    if let Some((_, _, p)) = &near {
        painter.circle_stroke(xf.s(*p), 6.0, Stroke::new(1.5_f32, t.selection));
    }
    if let Some((start, via)) = &ed.wire {
        let mut pts = vec![open.end_pos(page, start).unwrap_or(target)];
        pts.extend(via.iter().copied());
        // Rubber band: an orthogonal elbow from the last point to the pointer.
        if let Some(last) = pts.last().copied()
            && last.x != target.x
            && last.y != target.y
        {
            pts.push(Point::new(last.x, target.y));
        }
        pts.push(target);
        painter.add(EShape::line(
            pts.iter().map(|q| xf.s(*q)).collect(),
            Stroke::new(1.5_f32, t.ghost),
        ));
    }
    let end = match &near {
        Some((item, pin, _)) => Endpoint::Pin {
            item: item.clone(),
            pin: pin.clone(),
        },
        None => Endpoint::Point { at: cur },
    };
    if resp.double_clicked() {
        if let Some((start, mut via)) = ed.wire.take() {
            // The first click of the double click already added this point as a bend.
            if via.last() == Some(&cur) {
                via.pop();
            }
            finish_wire(open, ed, page, start, via, end);
        }
        return;
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        match ed.wire.take() {
            None => {
                ed.wire = Some((end, Vec::new()));
                ed.message =
                    "Click bends, then a pin to finish (double-click ends on a free point, Esc cancels).".into();
            }
            Some((start, mut via)) => {
                if near.is_some() {
                    finish_wire(open, ed, page, start, via, end);
                } else {
                    let last = via.last().copied().or_else(|| open.end_pos(page, &start));
                    if let Some(l) = last
                        && l.x != cur.x
                        && l.y != cur.y
                    {
                        via.push(Point::new(l.x, cur.y));
                    }
                    via.push(cur);
                    ed.wire = Some((start, via));
                }
            }
        }
    }
}

fn finish_wire(open: &mut Open, ed: &mut Editor, page: &Page, start: Endpoint, mut via: Vec<Point>, end: Endpoint) {
    // Keep it orthogonal: add an elbow before the end if the last leg is diagonal.
    let last = via.last().copied().or_else(|| open.end_pos(page, &start));
    if let (Some(l), Some(e)) = (last, open.end_pos(page, &end))
        && l.x != e.x
        && l.y != e.y
    {
        via.push(Point::new(l.x, e.y));
    }
    if start == end {
        ed.message = "A wire needs two different ends.".into();
        return;
    }
    open.add_wire(ed.page, start, end, via);
    ed.message = "Wire added.".into();
}

/// Where the jumper's bar passes on `page`: the terminal symbols of its terminals, in strip order.
/// Other symbols with a terminal's tag (a device drawn with it) are no bar end.
fn jumper_points(
    page: &Page,
    symbols: &std::collections::BTreeMap<String, wc_core::symbol::Symbol>,
    numbers: &wc_core::project::TerminalNumbers,
    j: &wc_core::project::Jumper,
) -> Vec<Point> {
    let mut at: Vec<(&str, Point)> = page
        .symbols
        .iter()
        .filter(|s| {
            symbols
                .get(&s.symbol)
                .is_some_and(|y| y.role == wc_core::symbol::SymbolRole::Terminal)
                && j.has(numbers, page, s, true)
        })
        .filter_map(|s| {
            let n = match numbers.get(&(page.id.clone(), s.id.clone())) {
                Some((_, n)) => n.as_str(),
                None => j.terminal_of(s.tag.as_ref()?)?,
            };
            Some((n, s.at))
        })
        .collect();
    let key = wc_core::project::natural_key;
    at.sort_by(|a, b| key(a.0).cmp(&key(b.0)).then_with(|| a.0.cmp(b.0)));
    at.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::state::tests::project;

    /// Click `pts` with `tool` as the canvas does.
    fn draw(open: &mut Open, ed: &mut Editor, tool: Tool, pts: &[(f64, f64)]) {
        ed.tool = tool;
        for &(x, y) in pts {
            shape_click(open, ed, Point::new(x, y));
        }
    }

    #[test]
    fn a_polyline_ends_on_a_quick_repeat_at_its_last_point_only() {
        let (_tmp, mut open) = project();
        let mut ed = Editor {
            tool: Tool::Polyline,
            ..Default::default()
        };
        let p = Point::new;
        shape_input(&mut open, &mut ed, p(10.0, 10.0), 0.0, 0.0);
        shape_input(&mut open, &mut ed, p(20.0, 10.0), 0.08, 0.0);
        // A quick click on another point adds that point.
        shape_input(&mut open, &mut ed, p(20.0, 20.0), 0.16, 0.0);
        assert_eq!(ed.shape_points.len(), 3);
        // A slow repeat on the last point (egui may still call it a triple-click) does nothing.
        shape_input(&mut open, &mut ed, p(20.0, 20.0), 0.62, 0.0);
        assert_eq!(ed.shape_points.len(), 3);
        assert!(open.project.pages[0].graphics.is_empty());
        // A quick repeat on the last point ends it with three points.
        shape_input(&mut open, &mut ed, p(20.0, 20.0), 0.8, 0.0);
        assert!(ed.shape_points.is_empty());
        assert!(matches!(
            &open.project.pages[0].graphics[..],
            [Shape::Polyline { points, closed: false, .. }] if points.len() == 3
        ));
        // Unsnapped, a quick repeat a hair off the last point still ends the line; with no
        // tolerance (snapping on) a quick click on a nearby grid point adds a vertex.
        shape_input(&mut open, &mut ed, p(0.0, 0.0), 1.0, 0.5);
        shape_input(&mut open, &mut ed, p(10.0, 0.0), 2.0, 0.5);
        shape_input(&mut open, &mut ed, p(10.2, 0.1), 2.1, 0.5);
        assert!(ed.shape_points.is_empty());
        assert_eq!(open.project.pages[0].graphics.len(), 2);
        shape_input(&mut open, &mut ed, p(0.0, 0.0), 3.0, 0.0);
        shape_input(&mut open, &mut ed, p(10.0, 0.0), 4.0, 0.0);
        shape_input(&mut open, &mut ed, p(11.0, 0.0), 4.1, 0.0);
        assert_eq!(ed.shape_points.len(), 3);
        // Unsnapped, a click a hair off the first point closes the line on that point.
        ed.shape_points.clear();
        for (i, q) in [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.1, -0.2)]
            .into_iter()
            .enumerate()
        {
            shape_input(&mut open, &mut ed, q, 10.0 + i as f64, 0.5);
        }
        assert!(ed.shape_points.is_empty());
        assert!(matches!(
            open.project.pages[0].graphics.last(),
            Some(Shape::Polyline { points, closed: true, .. }) if points.len() == 3
        ));
    }

    #[test]
    fn enter_ends_a_polyline_through_the_canvas_with_the_pointer_away() {
        let (_tmp, mut open) = project();
        let page_id = open.project.pages[0].id.clone();
        let mut ed = Editor {
            tool: Tool::Polyline,
            shape_points: vec![Point::new(10.0, 10.0), Point::new(30.0, 10.0)],
            shape_page: Some(page_id),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let frame = |open: &mut Open, ed: &mut Editor| {
            // No pointer over the canvas at all.
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 600.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(ui, open, ed, false, 1.0));
            });
        };
        frame(&mut open, &mut ed);
        assert_eq!(ed.shape_points.len(), 2, "nothing happens without Enter");
        // The keyboard shortcut sets the flag (not while a dialog or text field has the keys).
        ed.finish_shape = true;
        frame(&mut open, &mut ed);
        assert!(ed.shape_points.is_empty() && !ed.finish_shape);
        assert!(matches!(
            &open.project.pages[0].graphics[..],
            [Shape::Polyline { points, .. }] if points.len() == 2
        ));
    }

    #[test]
    fn a_quick_repeat_through_egui_pointer_events_ends_a_polyline() {
        let (_tmp, mut open) = project();
        let mut ed = Editor {
            tool: Tool::Polyline,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 600.0));
        let frame = |open: &mut Open, ed: &mut Editor, time: f64, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(ui, open, ed, false, 1.0));
            });
        };
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // A click: move there, press, release a moment later.
        let click = |open: &mut Open, ed: &mut Editor, pos: egui::Pos2, t: f64| {
            frame(open, ed, t, vec![egui::Event::PointerMoved(pos)]);
            frame(open, ed, t + 0.01, vec![button(pos, true)]);
            frame(open, ed, t + 0.02, vec![button(pos, false)]);
        };
        let (a, b) = (egui::pos2(300.0, 300.0), egui::pos2(400.0, 300.0));
        click(&mut open, &mut ed, a, 1.0);
        click(&mut open, &mut ed, b, 2.0);
        assert_eq!(ed.shape_points.len(), 2, "two points placed");
        // A slow repeat on the last point does nothing.
        click(&mut open, &mut ed, b, 3.0);
        assert_eq!(ed.shape_points.len(), 2);
        assert!(open.project.pages[0].graphics.is_empty());
        // A quick repeat on it ends the polyline.
        click(&mut open, &mut ed, b, 3.1);
        assert!(ed.shape_points.is_empty());
        assert!(matches!(
            &open.project.pages[0].graphics[..],
            [Shape::Polyline { points, .. }] if points.len() == 2
        ));
        // A quick click on a new point C after B adds it (even though egui may count it as a
        // double- or triple-click); a quick second click on C ends the line.
        let c = egui::pos2(400.0, 400.0);
        click(&mut open, &mut ed, a, 4.0);
        click(&mut open, &mut ed, b, 4.1);
        click(&mut open, &mut ed, c, 4.2);
        assert_eq!(ed.shape_points.len(), 3, "C is added");
        click(&mut open, &mut ed, c, 4.3);
        assert!(ed.shape_points.is_empty());
        assert!(matches!(
            &open.project.pages[0].graphics[..],
            [_, Shape::Polyline { points, .. }] if points.len() == 3
        ));
        // Released at A, then moved on to B in the same frame: the point goes at A (at B it
        // would be a slow repeat and add nothing).
        click(&mut open, &mut ed, b, 5.0);
        frame(&mut open, &mut ed, 6.0, vec![egui::Event::PointerMoved(b)]);
        frame(
            &mut open,
            &mut ed,
            6.01,
            vec![egui::Event::PointerMoved(a), button(a, true)],
        );
        frame(
            &mut open,
            &mut ed,
            6.02,
            vec![button(a, false), egui::Event::PointerMoved(b)],
        );
        assert_eq!(ed.shape_points.len(), 2);
        assert_ne!(ed.shape_points[0], ed.shape_points[1], "the second point is at A");
        // Released at A, then moved to B and zoomed in the same frame: the point still goes
        // where A was in the view the user saw.
        ed.shape_points.clear();
        click(&mut open, &mut ed, b, 8.0);
        let (p0, zoom) = (ed.shape_points[0], ed.view.zoom);
        frame(
            &mut open,
            &mut ed,
            9.0,
            vec![egui::Event::PointerMoved(a), button(a, true)],
        );
        frame(
            &mut open,
            &mut ed,
            9.01,
            vec![button(a, false), egui::Event::PointerMoved(b), egui::Event::Zoom(2.0)],
        );
        assert!(ed.view.zoom > zoom, "the view zoomed");
        let want = Point::new(p0.x - 100.0 / f64::from(zoom), p0.y);
        assert!(ed.shape_points[1].distance(want) <= 1.0, "{:?}", ed.shape_points); // 1 mm snap
    }

    #[test]
    fn an_arc_ending_on_its_centre_keeps_its_first_clicks() {
        let (_tmp, mut open) = project();
        let mut ed = Editor {
            tool: Tool::Arc,
            ..Default::default()
        };
        let c = Point::new(10.0, 10.0);
        shape_click(&mut open, &mut ed, c);
        shape_click(&mut open, &mut ed, Point::new(20.0, 10.0));
        shape_click(&mut open, &mut ed, c);
        assert_eq!(ed.shape_points.len(), 2, "centre and start stay");
        assert!(ed.message.contains("no shape"), "{}", ed.message);
        assert!(open.project.pages[0].graphics.is_empty());
        // On the start point or further out on its ray: a full turn, refused the same way.
        for end in [Point::new(20.0, 10.0), Point::new(30.0, 10.0)] {
            shape_click(&mut open, &mut ed, end);
            assert_eq!(ed.shape_points.len(), 2);
            assert!(ed.message.contains("no shape"), "{}", ed.message);
        }
        shape_click(&mut open, &mut ed, Point::new(10.0, 20.0));
        assert!(matches!(open.project.pages[0].graphics[..], [Shape::Arc { .. }]));
        // A start on the centre is refused at once; the centre stays for a proper start.
        shape_click(&mut open, &mut ed, c);
        shape_click(&mut open, &mut ed, c);
        assert_eq!(ed.shape_points, vec![c]);
        assert!(ed.message.contains("away from the centre"), "{}", ed.message);
        shape_click(&mut open, &mut ed, Point::new(20.0, 10.0));
        shape_click(&mut open, &mut ed, Point::new(10.0, 20.0));
        assert!(ed.shape_points.is_empty());
        assert_eq!(open.project.pages[0].graphics.len(), 2);
    }

    #[test]
    fn a_drawing_started_on_one_page_is_dropped_on_another() {
        let (_tmp, mut open) = project();
        let mut ed = Editor {
            tool: Tool::Arc,
            ..Default::default()
        };
        shape_click(&mut open, &mut ed, Point::new(10.0, 10.0));
        shape_click(&mut open, &mut ed, Point::new(20.0, 10.0));
        // The same page, moved to another place in the list: the clicks stay.
        open.project.pages.swap(0, 1);
        ed.page = 1;
        drop_draft_of_other_page(&open, &mut ed);
        assert_eq!(ed.shape_points.len(), 2);
        // Another page: they go, and the next click starts a new arc there.
        ed.page = 0;
        drop_draft_of_other_page(&open, &mut ed);
        assert!(ed.shape_points.is_empty());
        assert_eq!(ed.message, "Click the centre of the arc.");
        shape_click(&mut open, &mut ed, Point::new(50.0, 50.0));
        assert!(open.project.pages.iter().all(|p| p.graphics.is_empty()));
    }

    #[test]
    fn drawing_tools_make_their_shapes() {
        let p = Point::new;
        assert_eq!(
            drawn_shape(Tool::Circle, &[p(10.0, 10.0), p(13.0, 14.0)]),
            Some(Shape::Circle {
                center: p(10.0, 10.0),
                radius: 5.0,
                filled: false
            })
        );
        // Centre, start to the right, end below: a quarter turn clockwise on paper.
        assert_eq!(
            drawn_shape(Tool::Arc, &[p(10.0, 10.0), p(15.0, 10.0), p(10.0, 20.0)]),
            Some(Shape::Arc {
                center: p(10.0, 10.0),
                radius: 5.0,
                start: 0.0,
                end: 90.0
            })
        );
        let open = drawn_shape(Tool::Polyline, &[p(0.0, 0.0), p(10.0, 0.0), p(10.0, 5.0)]);
        assert!(matches!(open, Some(Shape::Polyline { ref points, closed: false, .. }) if points.len() == 3));
        let closed = drawn_shape(Tool::Polyline, &[p(0.0, 0.0), p(10.0, 0.0), p(10.0, 5.0), p(0.0, 0.0)]);
        assert!(matches!(closed, Some(Shape::Polyline { ref points, closed: true, .. }) if points.len() == 3));
        // Too small to draw: nothing.
        assert_eq!(drawn_shape(Tool::Circle, &[p(1.0, 1.0), p(1.0, 1.0)]), None);
        assert_eq!(drawn_shape(Tool::Arc, &[p(1.0, 1.0), p(1.0, 1.0), p(3.0, 1.0)]), None);
        assert_eq!(drawn_shape(Tool::Polyline, &[p(1.0, 1.0)]), None);
        assert_eq!(drawn_shape(Tool::Rect, &[p(1.0, 1.0), p(1.0, 1.0)]), None);
    }

    #[test]
    fn drawn_circles_arcs_and_polylines_are_saved_picked_and_exported() {
        let (_tmp, mut open) = project();
        let mut ed = Editor::default();
        draw(&mut open, &mut ed, Tool::Circle, &[(50.0, 50.0), (55.0, 50.0)]);
        draw(
            &mut open,
            &mut ed,
            Tool::Arc,
            &[(100.0, 50.0), (110.0, 50.0), (100.0, 60.0)],
        );
        // The polyline ends with a click back on its first point, which closes it.
        draw(
            &mut open,
            &mut ed,
            Tool::Polyline,
            &[
                (150.0, 50.0),
                (170.0, 50.0),
                (170.0, 70.0),
                (170.0, 70.0),
                (150.0, 50.0),
            ],
        );
        assert!(ed.shape_points.is_empty());
        assert_eq!(ed.selection, vec![Selection::Graphic(2)]);
        assert_eq!(open.project.pages[0].graphics.len(), 3);
        let g = open.project.pages[0].graphics.clone();
        assert_eq!(g.len(), 3, "{g:?}");
        assert!(matches!(g[2], Shape::Polyline { closed: true, ref points, .. } if points.len() == 3));

        // Picked on its line; the open part of the arc does not pick it.
        use crate::edit::graphic_hit;
        assert!(graphic_hit(&g[0], Point::new(50.0, 55.0), 0.5));
        assert!(graphic_hit(&g[1], Point::new(107.07, 57.07), 0.5), "inside the sweep");
        assert!(!graphic_hit(&g[1], Point::new(90.0, 50.0), 0.5), "the open part");
        // A large arc is picked on the chords the canvas draws, not only on the true circle.
        let big = Shape::Arc {
            center: Point::new(0.0, 0.0),
            radius: 250.0,
            start: 0.0,
            end: 90.0,
        };
        let chord_mid = {
            let pts = arc_points(Point::new(0.0, 0.0), 250.0, 0.0, 90.0);
            Point::new((pts[0].x + pts[1].x) / 2.0, (pts[0].y + pts[1].y) / 2.0)
        };
        assert!(graphic_hit(&big, chord_mid, 0.3), "on the drawn stroke");
        // An arc past a full turn is picked all round, as it is drawn.
        let more_than_full = Shape::Arc {
            center: Point::new(0.0, 0.0),
            radius: 10.0,
            start: 0.0,
            end: 450.0,
        };
        assert!(graphic_hit(&more_than_full, Point::new(-10.0, 0.0), 0.5));
        // A negative sweep (720° to 100°, from a file) is drawn the long way back round: 260°
        // in 26 steps, picked on that path and not on the 100° it leaves out.
        let back = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 720.0,
            end: 100.0,
        };
        assert_eq!(arc_points(Point::new(150.0, 100.0), 9.0, 720.0, 100.0).len(), 27);
        assert!(graphic_hit(&back, Point::new(141.0, 100.0), 0.3), "180° is drawn");
        assert!(!graphic_hit(&back, Point::new(155.79, 106.89), 0.3), "50° is not");
        // A box around the drawn quarter arc holds it (not the whole circle).
        let quarter = Shape::Arc {
            center: Point::new(100.0, 100.0),
            radius: 10.0,
            start: 0.0,
            end: 90.0,
        };
        let (a, b) = crate::edit::graphic_bounds(&quarter);
        assert!(
            a.x >= 99.0 && a.y >= 99.0 && b.x <= 111.0 && b.y <= 111.0,
            "{a:?} {b:?}"
        );
        assert!(graphic_hit(&g[2], Point::new(160.0, 60.0), 0.5), "the closing side");

        // Undo takes the last one back off; moving and deleting work like other drawings.
        open.undo();
        assert_eq!(open.project.pages[0].graphics.len(), 2);
        open.redo();
        open.delete_many(0, &[Selection::Graphic(0)]);
        assert_eq!(open.project.pages[0].graphics.len(), 2);
        open.undo();

        // Saved in the page file and read back the same.
        open.save().unwrap();
        let again = Open::load(&open.dir).unwrap();
        assert_eq!(again.project.pages[0].graphics, g);

        // In the SVG and DXF drawings.
        let (pages, _) = wc_pdf::page_drawings(&again.project, &again.symbols, &wc_pdf::Options::default());
        let svg = wc_pdf::svg(&pages[0]);
        let dxf = wc_pdf::dxf(&pages[0]);
        assert!(
            svg.contains("<circle") && svg.contains(" A10 10 "),
            "circle and arc in the SVG"
        );
        assert!(
            svg.contains("<polygon points=\"150,50 170,50 170,70\""),
            "the closed polyline in the SVG"
        );
        let mut bare = again.project.clone();
        bare.pages[0].graphics.clear();
        let (bare_pages, _) = wc_pdf::page_drawings(&bare, &again.symbols, &wc_pdf::Options::default());
        let bare_dxf = wc_pdf::dxf(&bare_pages[0]);
        // One more of each than the same page without the drawings (the frame has polylines too).
        for kind in ["CIRCLE", "ARC", "POLYLINE"] {
            let count = |d: &str| d.lines().filter(|l| l.trim() == kind).count();
            assert_eq!(count(&dxf), count(&bare_dxf) + 1, "{kind} in the DXF");
        }
        // In the PDF: each drawing starts one more path than the page without them.
        let paths = |p: &wc_core::Project| {
            let (pdf, _) = wc_pdf::export(p, &again.symbols, &wc_pdf::Options::default());
            String::from_utf8_lossy(&pdf).matches(" m\n").count()
        };
        assert_eq!(paths(&again.project), paths(&bare) + 3, "three more paths in the PDF");

        // An arc stored past a full turn (720° to 100°) is exported along the path the canvas
        // draws: 260° back round, through 180° and not through 50°.
        let mut back = bare.clone();
        back.pages[0].graphics = vec![Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 720.0,
            end: 100.0,
        }];
        let (back_pages, _) = wc_pdf::page_drawings(&back, &again.symbols, &wc_pdf::Options::default());
        let svg = wc_pdf::svg(&back_pages[0]);
        // From 100° clockwise to 0° (360°): the large arc.
        assert!(svg.contains(" A9 9 0 1 1 "), "the 260° arc in the SVG: {svg}");
        let dxf = wc_pdf::dxf(&back_pages[0]);
        let arc: Vec<&str> = dxf.lines().map(str::trim).skip_while(|l| *l != "ARC").collect();
        let value = |code: &str| {
            arc.iter()
                .position(|l| *l == code)
                .and_then(|k| arc.get(k + 1))
                .copied()
        };
        // Counter-clockwise with y up from 0° to 260°: page angles 100° to 360°.
        assert_eq!((value("50"), value("51")), (Some("0"), Some("260")), "{dxf}");
        let curves = |p: &wc_core::Project| {
            let (pdf, _) = wc_pdf::export(p, &again.symbols, &wc_pdf::Options::default());
            String::from_utf8_lossy(&pdf).matches(" c\n").count()
        };
        assert_eq!(curves(&back), curves(&bare) + 3, "260° in three curves in the PDF");

        // A full turn backwards (720° to 0°) is a circle in the SVG and DXF.
        back.pages[0].graphics = vec![Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 720.0,
            end: 0.0,
        }];
        let (full_pages, _) = wc_pdf::page_drawings(&back, &again.symbols, &wc_pdf::Options::default());
        assert!(wc_pdf::svg(&full_pages[0]).contains("<circle"), "a circle in the SVG");
        let count = |d: &str, kind: &str| d.lines().filter(|l| l.trim() == kind).count();
        let full_dxf = wc_pdf::dxf(&full_pages[0]);
        assert_eq!(
            count(&full_dxf, "CIRCLE"),
            count(&bare_dxf, "CIRCLE") + 1,
            "a circle in the DXF"
        );
        // Angles stored far out draw one turn at most: a circle's four curves, not millions.
        back.pages[0].graphics = vec![Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 1e9,
            end: 0.0,
        }];
        assert!(curves(&back) <= curves(&bare) + 4, "at most a full turn in the PDF");
    }

    #[test]
    fn a_long_title_block_value_shows_as_a_bar_when_too_small_to_read() {
        let mut p = wc_core::Project::new("Pump station");
        let long =
            "Customer with a name of sixty characters that has to shrink, and a department and a site after it as well";
        p.meta.title_block.insert("customer".into(), long.into());
        let page = wc_core::Page::new("p1", "1", "Main");
        p.pages.push(page.clone());
        let size = page.size.mm();
        let marks = wc_pdf::frame_marks(&p, &page, 0, 1, "");
        let customer = marks
            .iter()
            .find(|m| matches!(&m.shape, Shape::Text { text, .. } if text.contains("sixty")))
            .unwrap();
        let Shape::Text { at, height, .. } = &customer.shape else {
            unreachable!()
        };
        assert!(*height < 1.25, "the value is shrunk: {height}");
        // At zoom 2 the text is under 3 px: a bar as wide as the text, ending on its baseline.
        let (a, b) = value_placeholder(customer, size, wc_core::template::TITLE_HEIGHT, 2.0).unwrap();
        let w = wc_pdf::text_width_mm(long, *height);
        assert!((b.x - a.x - w).abs() < 1e-9 && b.y == at.y && a.y < b.y, "{a:?} {b:?}");
        // Zoomed in far enough the text itself is drawn.
        assert_eq!(
            value_placeholder(customer, size, wc_core::template::TITLE_HEIGHT, 4.0),
            None
        );
        // Column numbers (along the top) and row letters (outside the frame), as small at zoom
        // 0.5, are left out as before.
        let mut checked = 0;
        for m in &marks {
            if let Shape::Text { text, at, .. } = &m.shape
                && ((text == "1" && at.y < size.1 / 2.0) || text == "A")
            {
                assert_eq!(
                    value_placeholder(m, size, wc_core::template::TITLE_HEIGHT, 0.5),
                    None,
                    "{text}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 3, "column 1 and row A on both sides");
        // So are labels and lines.
        assert!(
            marks
                .iter()
                .filter(|m| m.ink != wc_pdf::Ink::Thin || !matches!(m.shape, Shape::Text { .. }))
                .all(|m| value_placeholder(m, size, wc_core::template::TITLE_HEIGHT, 0.5).is_none())
        );
    }

    #[test]
    fn a_value_in_the_added_band_area_shows_as_a_bar_only_for_a_taller_band() {
        let page = wc_core::Page::new("p1", "1", "Main");
        let size = page.size.mm();
        let y1 = size.1 - wc_core::project::FRAME_MARGIN;
        // A small value 30 mm above the frame's foot: inside a 36 mm band, above a 24 mm one.
        let m = wc_pdf::FrameMark {
            shape: Shape::Text {
                at: Point::new(100.0, y1 - 30.0),
                text: "Notes".into(),
                height: 1.0,
                align: Align::Left,
                angle: 0.0,
            },
            ink: wc_pdf::Ink::Thin,
        };
        assert!(value_placeholder(&m, size, 36.0, 0.5).is_some());
        assert_eq!(value_placeholder(&m, size, 24.0, 0.5), None);
    }

    #[test]
    fn the_cover_logo_moves_and_sizes_by_whole_millimetres_and_stays_usable() {
        let start = (Point::new(10.0, 10.0), Point::new(40.0, 30.0));
        assert_eq!(
            logo_dragged(start, false, Point::new(5.4, -2.6)),
            (Point::new(15.0, 7.0), Point::new(45.0, 27.0))
        );
        assert_eq!(
            logo_dragged(start, true, Point::new(10.0, 5.0)),
            (Point::new(10.0, 10.0), Point::new(50.0, 35.0))
        );
        assert_eq!(
            logo_dragged(start, true, Point::new(-100.0, -100.0)),
            (Point::new(10.0, 10.0), Point::new(15.0, 15.0)),
            "never smaller than 5 mm"
        );
        assert!(logo_corner(Point::new(40.0, 30.0), Point::new(41.5, 29.0), 0.5));
        assert!(!logo_corner(Point::new(40.0, 30.0), Point::new(35.0, 25.0), 0.5));
    }

    #[test]
    fn a_notched_outline_leaves_the_notch_empty() {
        let pts: Vec<Pos2> = [
            (0.0, 0.0),
            (60.0, 0.0),
            (60.0, 60.0),
            (40.0, 60.0),
            (40.0, 20.0),
            (20.0, 20.0),
            (20.0, 60.0),
            (0.0, 60.0),
        ]
        .iter()
        .map(|(x, y)| Pos2::new(*x, *y))
        .collect();
        assert!(!convex(&pts));
        let inside = |q: Pos2, [a, b, c]: [u32; 3]| {
            let (a, b, c) = (pts[a as usize], pts[b as usize], pts[c as usize]);
            let s = |p: Pos2, r: Pos2| (r.x - p.x) * (q.y - p.y) - (r.y - p.y) * (q.x - p.x);
            let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
            (d1 > 0.0 && d2 > 0.0 && d3 > 0.0) || (d1 < 0.0 && d2 < 0.0 && d3 < 0.0)
        };
        let tris = triangulate(&pts);
        assert!(
            !tris.iter().any(|t| inside(Pos2::new(30.0, 40.0), *t)),
            "the notch is not filled"
        );
        assert!(tris.iter().any(|t| inside(Pos2::new(7.0, 13.0), *t)));
        let area: f32 = tris
            .iter()
            .map(|[a, b, c]| {
                let (a, b, c) = (pts[*a as usize], pts[*b as usize], pts[*c as usize]);
                ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() / 2.0
            })
            .sum();
        assert!((area - 2800.0).abs() < 1e-2, "{area}");
    }

    #[test]
    fn a_square_with_a_hole_cut_in_fills_only_the_ring() {
        // Outer 0..10 counter-clockwise, bridged at (0,0)-(4,4) into the hole 4..6 clockwise.
        let pts: Vec<Pos2> = [
            (0.0, 0.0),
            (4.0, 4.0),
            (4.0, 6.0),
            (6.0, 6.0),
            (6.0, 4.0),
            (4.0, 4.0),
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
        ]
        .iter()
        .map(|(x, y)| Pos2::new(*x, *y))
        .collect();
        let area: f32 = triangulate(&pts)
            .iter()
            .map(|[a, b, c]| {
                let (a, b, c) = (pts[*a as usize], pts[*b as usize], pts[*c as usize]);
                ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() / 2.0
            })
            .sum();
        assert!((area - 96.0).abs() < 1e-3, "the ring only: {area}");
    }

    #[test]
    fn a_concave_outline_is_cut_into_triangles_inside_it() {
        // An L: 0..2 wide at the bottom, 0..1 wide above.
        let pts: Vec<Pos2> = [(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0), (1.0, 2.0), (0.0, 2.0)]
            .iter()
            .map(|(x, y)| Pos2::new(*x, *y))
            .collect();
        let tris = triangulate(&pts);
        assert_eq!(tris.len(), 4);
        let area: f32 = tris
            .iter()
            .map(|[a, b, c]| {
                let (a, b, c) = (pts[*a as usize], pts[*b as usize], pts[*c as usize]);
                ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() / 2.0
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-5, "covers the L exactly: {area}");
    }

    use wc_core::project::Endpoint;

    #[test]
    fn a_jumper_bar_runs_through_terminal_symbols_only() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
        let mut put = |symbol: &str, at: Point, tag: &str| {
            let id = o.place(page, symbol, at).unwrap();
            let t = Some(wc_core::DeviceTag::parse(tag).unwrap());
            o.project.pages[page]
                .symbols
                .iter_mut()
                .find(|s| s.id == id)
                .unwrap()
                .tag = t;
        };
        put("wc.terminal", Point::new(40.0, 20.0), "-X1.2");
        put("wc.terminal", Point::new(20.0, 20.0), "-X1.1");
        put("wc.coil", Point::new(90.0, 60.0), "-X1.1");
        o.project.meta.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        let j = &o.project.meta.jumpers[0];
        let numbers = wc_core::project::terminal_numbers(&o.project, |id| o.symbols.get(id).map(|s| s.role));
        let at = jumper_points(&o.project.pages[page], &o.symbols, &numbers, j);
        assert_eq!(
            at,
            [Point::new(20.0, 20.0), Point::new(40.0, 20.0)],
            "in strip order, no coil"
        );
    }

    /// A contact at (50, 20) and a coil placed off to the side, ready to be dragged under it.
    fn moving_coil() -> (tempfile::TempDir, Open, Editor, String) {
        let (tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        o.place(page, "wc.contact.no", Point::new(50.0, 20.0)).unwrap();
        let coil = o.place(page, "wc.coil", Point::new(90.0, 60.0)).unwrap();
        let ed = Editor {
            page,
            auto_wire: true,
            selection: vec![Selection::Symbol(coil.clone())],
            ..Editor::default()
        };
        (tmp, o, ed, coil)
    }

    /// Drag the selection by `d` the way the select tool does: one checkpoint, then the move.
    #[test]
    fn a_drag_on_a_page_someone_else_holds_moves_nothing() {
        let (_tmp, mut o) = project();
        o.project.pages[0].graphics.push(Shape::Line {
            a: Point::new(50.0, 50.0),
            b: Point::new(80.0, 50.0),
        });
        let page = o.project.pages[0].clone();
        let mut ed = Editor::default();
        let at = Point::new(60.0, 50.0);
        assert!(o.hit(&page, at, 1.0).is_some(), "the line is under the mouse");

        // Own page: the drag picks the line up to move it.
        assert_eq!(ed.held_page(&o.project), None);
        let d = drag_start(&o, &mut ed, &page, at, 1.0, 2.5, false, false);
        assert!(matches!(d, Drag::Move { .. }));
        assert_eq!(ed.selection, vec![Selection::Graphic(0)]);

        // Held by Ola: the same drag only draws a selection box, and nothing is picked up.
        ed.selection.clear();
        ed.held_by.insert(page.id.clone(), "Ola".into());
        assert_eq!(ed.held_page(&o.project), Some("Ola"));
        let held = ed.held_page(&o.project).is_some();
        let d = drag_start(&o, &mut ed, &page, at, 1.0, 2.5, false, held);
        assert!(matches!(d, Drag::Box { .. }));
        assert!(ed.selection.is_empty());
        assert_eq!(o.project.pages[0], page, "the page is unchanged");

        // The panel view is not the page: it is not held.
        ed.panel = Some(0);
        assert_eq!(ed.held_page(&o.project), None);
    }

    #[test]
    fn a_drawing_on_a_hidden_or_locked_layer_cannot_be_picked() {
        let (_tmp, mut o) = project();
        o.project.pages[0].graphics.push(Shape::Line {
            a: Point::new(50.0, 50.0),
            b: Point::new(80.0, 50.0),
        });
        let page = o.project.pages[0].clone();
        let at = Point::new(60.0, 50.0);
        let mut ed = Editor::default();
        assert_eq!(pick(&o, &ed, &page, at, 1.0), Some(Selection::Graphic(0)));

        for hide in [true, false] {
            let mut ed = Editor::default();
            if hide {
                ed.layers.set_shown(Layer::Drawings, false);
            } else {
                ed.layers.set_locked(Layer::Drawings, true);
            }
            // A click finds nothing, and a drag there draws a selection box instead of
            // picking the line up.
            assert_eq!(pick(&o, &ed, &page, at, 1.0), None);
            let d = drag_start(&o, &mut ed, &page, at, 1.0, 2.5, false, false);
            assert!(matches!(d, Drag::Box { .. }));
            assert!(ed.selection.is_empty());
            // A selection box or Select all leaves it out.
            let boxed = o.select_in(0, Point::new(0.0, 0.0), Point::new(200.0, 200.0));
            assert!(boxed.contains(&Selection::Graphic(0)));
            assert!(!ed.layers.keep(boxed).contains(&Selection::Graphic(0)));
            assert!(!ed.layers.keep(o.select_all(0)).contains(&Selection::Graphic(0)));
        }

        // Hiding the layer while the line is selected drops it from the selection on the next
        // frame, and a drawing tool falls back to Select so nothing invisible is drawn.
        ed.selection = vec![Selection::Graphic(0)];
        ed.tool = Tool::Line;
        ed.layers.set_shown(Layer::Drawings, false);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut o, &mut ed, false, 1.0));
        });
        assert!(ed.selection.is_empty());
        assert_eq!(ed.tool, Tool::Select);
        assert!(ed.message.contains("Drawings and notes"), "{}", ed.message);
        assert_eq!(o.project.pages[0], page, "the page is unchanged");
    }

    /// Draw one frame of the canvas with `events`; returns every text painted and where.
    fn painted(
        ctx: &egui::Context,
        o: &mut Open,
        ed: &mut Editor,
        time: f64,
        events: Vec<egui::Event>,
    ) -> Vec<(String, Rect)> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, o, ed, false, 1.0));
        });
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                EShape::Text(t) => Some((t.galley.text().to_owned(), t.visual_bounding_rect())),
                _ => None,
            })
            .collect()
    }

    /// Click at `pos`: move there, press, release.
    fn click_at(ctx: &egui::Context, o: &mut Open, ed: &mut Editor, pos: egui::Pos2, t: f64) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        painted(ctx, o, ed, t, vec![egui::Event::PointerMoved(pos)]);
        painted(ctx, o, ed, t + 0.01, vec![button(true)]);
        painted(ctx, o, ed, t + 0.02, vec![button(false)]);
    }

    /// Page 2 with a free text, a numbered wire, a coil -K1 whose contact is on page 3 (so a
    /// cross-reference shows under the coil), a review comment and a scanned sheet whose
    /// picture is missing (so its placeholder text shows).
    fn annotated() -> (tempfile::TempDir, Open, Editor) {
        let (tmp, mut o) = project();
        o.project.pages.push(Page::new("p0003", "3", "Contacts"));
        let coil = o.place(1, "wc.coil", Point::new(100.0, 60.0)).unwrap();
        let contact = o.place(2, "wc.contact.no", Point::new(100.0, 60.0)).unwrap();
        for (page, id) in [(1, &coil), (2, &contact)] {
            let s = o.project.pages[page].symbols.iter_mut().find(|s| &s.id == id).unwrap();
            s.tag = Some(wc_core::DeviceTag::parse("-K1").unwrap());
        }
        let page = &mut o.project.pages[1];
        page.underlay = Some(wc_core::Underlay {
            file: "scans/missing-scan.png".into(),
            min: Point::new(20.0, 100.0),
            max: Point::new(120.0, 180.0),
        });
        page.graphics.push(
            serde_json::from_value(serde_json::json!({
                "kind": "text", "at": {"x": 40.0, "y": 150.0}, "text": "Hello note", "height": 5.0
            }))
            .unwrap(),
        );
        page.wires.push(
            serde_json::from_value(serde_json::json!({
                "id": "w1",
                "from": {"at": {"x": 200.0, "y": 40.0}},
                "to": {"at": {"x": 300.0, "y": 40.0}},
                "number": "W101"
            }))
            .unwrap(),
        );
        o.project.comments.push(wc_core::Comment {
            id: "c742".into(),
            page: "p0002".into(),
            at: Some(Point::new(250.0, 150.0)),
            author: "Kari".into(),
            time: 0,
            text: "Check this".into(),
            resolved: false,
            replies: Vec::new(),
        });
        let ed = Editor {
            page: 1,
            ..Editor::default()
        };
        (tmp, o, ed)
    }

    #[test]
    fn a_hidden_layer_is_not_painted_and_a_shown_one_is() {
        let (_tmp, mut o, mut ed) = annotated();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let has = |texts: &[(String, Rect)], want: &str| texts.iter().any(|(t, _)| t.contains(want));
        // A first frame fits the page; the second is drawn as it stays.
        painted(&ctx, &mut o, &mut ed, 0.0, vec![]);
        let all = painted(&ctx, &mut o, &mut ed, 0.1, vec![]);
        let xref = all
            .iter()
            .find(|(t, _)| t.contains("3."))
            .map(|(t, _)| t.clone())
            .unwrap_or_default();
        assert!(!xref.is_empty(), "a cross-reference to page 3 is drawn: {all:?}");
        for want in ["Hello note", "W101", "742", "missing-scan", xref.as_str()] {
            assert!(has(&all, want), "{want} is drawn: {all:?}");
        }
        for (layer, gone) in [
            (Layer::Drawings, "Hello note"),
            (Layer::WireNumbers, "W101"),
            (Layer::CrossReferences, xref.as_str()),
            (Layer::Comments, "742"),
            (Layer::Underlay, "missing-scan"),
        ] {
            ed.layers = Default::default();
            ed.layers.set_shown(layer, false);
            let texts = painted(&ctx, &mut o, &mut ed, 0.2, vec![]);
            assert!(!has(&texts, gone), "{layer:?} hidden: {gone} is not drawn");
            for other in ["Hello note", "W101", "742", "missing-scan", xref.as_str()]
                .into_iter()
                .filter(|w| *w != gone)
            {
                assert!(has(&texts, other), "{layer:?} hidden: {other} is still drawn");
            }
            // Locked is still drawn.
            ed.layers = Default::default();
            ed.layers.set_locked(layer, true);
            let texts = painted(&ctx, &mut o, &mut ed, 0.3, vec![]);
            assert!(has(&texts, gone), "{layer:?} locked: {gone} is still drawn");
        }
    }

    #[test]
    fn a_locked_comment_or_cross_reference_does_not_open_on_a_click() {
        let (_tmp, mut o, mut ed) = annotated();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        painted(&ctx, &mut o, &mut ed, 0.0, vec![]);
        let texts = painted(&ctx, &mut o, &mut ed, 0.1, vec![]);
        let at = |want: &str| {
            texts
                .iter()
                .find(|(t, _)| t == want || (want == "xref" && t.contains("3.")))
                .map(|(_, r)| r.center())
                .unwrap()
        };
        let (bubble, xref) = (at("742"), at("xref"));

        // Locked: a click on the bubble opens nothing, a click on the cross-reference stays.
        ed.layers.set_locked(Layer::Comments, true);
        ed.layers.set_locked(Layer::CrossReferences, true);
        click_at(&ctx, &mut o, &mut ed, bubble, 1.0);
        assert_eq!(ed.comment, None);
        click_at(&ctx, &mut o, &mut ed, xref, 2.0);
        assert_eq!(ed.page, 1, "still on page 2");

        // Unlocked: the bubble opens its comment, the cross-reference goes to page 3.
        ed.layers = Default::default();
        click_at(&ctx, &mut o, &mut ed, bubble, 3.0);
        assert_eq!(ed.comment.as_deref(), Some("c742"));
        click_at(&ctx, &mut o, &mut ed, xref, 4.0);
        assert_eq!(ed.page, 2, "on page 3");
    }

    #[test]
    fn the_comment_tool_leaves_a_hidden_comment_layer() {
        let mut ed = Editor {
            tool: Tool::Comment,
            ..Editor::default()
        };
        leave_unpickable_layer(&mut ed);
        assert_eq!(ed.tool, Tool::Comment, "shown and unlocked: the tool stays");
        ed.layers.set_locked(Layer::Comments, true);
        leave_unpickable_layer(&mut ed);
        assert_eq!(ed.tool, Tool::Select);
        // Hiding wire numbers keeps the wire tool: wires are on no layer.
        ed.tool = Tool::Wire;
        ed.layers.set_shown(Layer::WireNumbers, false);
        leave_unpickable_layer(&mut ed);
        assert_eq!(ed.tool, Tool::Wire);
    }

    fn drag(o: &mut Open, ed: &mut Editor, d: Point) {
        o.checkpoint();
        o.move_by(ed.page, &ed.selection.clone(), d);
        ed.drag = Some(Drag::Move {
            from: Point::new(0.0, 0.0),
            checkpoint: true,
        });
    }

    #[test]
    fn right_click_keeps_a_selection_it_is_part_of() {
        let wire = |id: &str| Selection::Wire(id.into());
        let mut ed = Editor {
            selection: vec![wire("w1"), wire("w2")],
            ..Editor::default()
        };
        right_clicked(&mut ed, Some(wire("w1")));
        assert_eq!(ed.selection, [wire("w1"), wire("w2")]);
        right_clicked(&mut ed, Some(Selection::Symbol("s1".into())));
        right_clicked(&mut ed, None);
        assert_eq!(ed.selection, [wire("w1"), wire("w2")], "only wires change it");
        right_clicked(&mut ed, Some(wire("w3")));
        assert_eq!(ed.selection, [wire("w3")]);
        assert_eq!(selected_wires(&ed), ["w3"]);
    }

    #[test]
    fn the_wire_menu_deletes_the_clicked_wire_or_the_selection() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let free = |x: f64| Endpoint::Point {
            at: Point::new(x, 100.0),
        };
        for x in [10.0, 30.0, 50.0] {
            o.add_wire(page, free(x), free(x + 10.0), Vec::new());
        }
        let ids: Vec<String> = o.project.pages[page].wires.iter().map(|w| w.id.clone()).collect();
        let symbols = o.project.pages[page].symbols.clone();
        let wire = |i: usize| Selection::Wire(ids[i].clone());

        // One wire, right-clicked while not selected.
        ed.selection = vec![Selection::Symbol(coil.clone())];
        right_clicked(&mut ed, Some(wire(0)));
        delete_selected_wires(&mut o, &mut ed);
        let left: Vec<&str> = o.project.pages[page].wires.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(left, [ids[1].as_str(), ids[2].as_str()]);
        assert!(ed.selection.is_empty());
        assert_eq!(ed.message, "Deleted 1 wire.");

        // The rest, right-clicking one of a selection that also holds a symbol.
        ed.selection = vec![wire(1), wire(2), Selection::Symbol(coil.clone())];
        right_clicked(&mut ed, Some(wire(2)));
        let before = o.project.pages[page].clone();
        delete_selected_wires(&mut o, &mut ed);
        assert!(o.project.pages[page].wires.is_empty());
        assert_eq!(o.project.pages[page].symbols, symbols, "symbols and their pins stay");
        assert_eq!(ed.selection, [Selection::Symbol(coil)]);
        assert_eq!(ed.message, "Deleted 2 wires.");
        assert!(o.undo());
        assert_eq!(o.project.pages[page], before, "one undo brings both back");
    }

    #[test]
    fn a_finished_move_wires_pins_that_line_up() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let before = o.project.pages.clone();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        drag_released(&mut o, &mut ed, false);
        let wires = &o.project.pages[ed.page].wires;
        assert_eq!(wires.len(), 1);
        assert!(matches!(&wires[0].from, Endpoint::Pin { item, .. } if *item == coil));
        assert!(ed.message.contains("1 wire"), "{}", ed.message);
        assert!(ed.drag.is_none());
        assert!(o.undo());
        assert_eq!(o.project.pages, before, "one undo takes back the move and its wire");
    }

    #[test]
    fn a_move_stops_when_its_page_becomes_held_and_wires_nothing() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        // Not held: the move goes on.
        stop_if_held(&mut ed, false);
        assert!(matches!(ed.drag, Some(Drag::Move { .. })));
        // Ola takes the page mid-move: the move stops, and its release wires nothing.
        stop_if_held(&mut ed, true);
        assert!(ed.drag.is_none());
        drag_released(&mut o, &mut ed, false);
        assert!(o.project.pages[ed.page].wires.is_empty());

        // A logo drag stops the same way; a selection box goes on.
        ed.drag = Some(Drag::Logo {
            from: Point::new(0.0, 0.0),
            start: (Point::new(0.0, 0.0), Point::new(10.0, 10.0)),
            size: false,
            checkpoint: true,
        });
        stop_if_held(&mut ed, true);
        assert!(ed.drag.is_none());
        ed.drag = Some(Drag::Box {
            from: Point::new(0.0, 0.0),
        });
        stop_if_held(&mut ed, true);
        assert!(matches!(ed.drag, Some(Drag::Box { .. })));
    }

    #[test]
    fn a_move_released_with_alt_adds_no_wires() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        drag_released(&mut o, &mut ed, true);
        assert!(o.project.pages[ed.page].wires.is_empty());
        assert!(ed.message.is_empty());
    }

    #[test]
    fn a_move_keeps_the_wires_that_were_there() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let other = o.place(page, "wc.coil", Point::new(150.0, 60.0)).unwrap();
        o.add_wire(
            page,
            Endpoint::Pin {
                item: coil,
                pin: "A2".into(),
            },
            Endpoint::Pin {
                item: other,
                pin: "A2".into(),
            },
            Vec::new(),
        );
        let w = o.project.pages[page].wires[0].clone();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        drag_released(&mut o, &mut ed, false);
        let wires = &o.project.pages[page].wires;
        assert_eq!(wires.len(), 2);
        assert!(wires.contains(&w));
    }
}
