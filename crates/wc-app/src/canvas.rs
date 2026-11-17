//! The schematic canvas: paper, frame, grid, page graphics, wires and symbols, plus the
//! mouse handling for the select, place and wire tools.
//!
//! Page coordinates are millimetres with y pointing down, like the paper and the PDF export.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape as EShape, Stroke, Vec2};
use wc_core::geom::{Align, Point};
use wc_core::route::Route;
use wc_core::{Endpoint, Page, Shape};

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
    /// First corner of a line or rectangle being drawn.
    pub shape_start: Option<Point>,
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
            shape_start: None,
            ask_text: None,
            ask_comment: None,
            comment: None,
            message: String::new(),
            panel: None,
            panel_pick: None,
            panel_drag: None,
            auto_wire: false,
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
}

impl Editor {
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

struct Xf {
    origin: Pos2,
    zoom: f32,
}

impl Xf {
    fn s(&self, p: Point) -> Pos2 {
        self.origin + Vec2::new(p.x as f32, p.y as f32) * self.zoom
    }
    fn mm(&self, p: Pos2) -> Point {
        let v = (p - self.origin) / self.zoom;
        Point::new(f64::from(v.x), f64::from(v.y))
    }
    fn len(&self, mm: f64) -> f32 {
        mm as f32 * self.zoom
    }
}

fn snap(p: Point, step: f64) -> Point {
    if step <= 0.0 {
        return p;
    }
    Point::new((p.x / step).round() * step, (p.y / step).round() * step)
}

fn arc_points(center: Point, r: f64, start: f64, end: f64) -> Vec<Point> {
    let mut sweep = end - start;
    if sweep <= 0.0 {
        sweep += 360.0;
    }
    let n = ((sweep / 10.0).ceil() as usize).clamp(2, 72);
    (0..=n)
        .map(|i| {
            let t = (start + sweep * i as f64 / n as f64).to_radians();
            Point::new(center.x + r * t.cos(), center.y + r * t.sin())
        })
        .collect()
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

fn paint_shape(p: &egui::Painter, xf: &Xf, s: &Shape, stroke: Stroke) {
    match s {
        Shape::Line { a, b } => {
            p.line_segment([xf.s(*a), xf.s(*b)], stroke);
        }
        Shape::Polyline { points, closed, filled } => {
            let pts: Vec<Pos2> = points.iter().map(|q| xf.s(*q)).collect();
            if *filled {
                // egui fills only convex paths itself, so cut the outline into triangles.
                let mut mesh = egui::Mesh::default();
                for p in &pts {
                    mesh.colored_vertex(*p, stroke.color);
                }
                for [a, b, c] in triangulate(&pts) {
                    mesh.add_triangle(a, b, c);
                }
                p.add(EShape::mesh(mesh));
                p.add(EShape::closed_line(pts, stroke));
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
    if size < 3.0 {
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
    if size < 3.0 {
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
        let stroke = match m.ink {
            wc_pdf::Ink::Thick => Stroke::new(xf.len(0.5).max(1.0), c),
            wc_pdf::Ink::Thin => Stroke::new(xf.len(0.25).max(0.6), c),
            wc_pdf::Ink::Label => Stroke::new(1.0_f32, faint),
        };
        paint_shape(p, xf, &m.shape, stroke);
    }
}

/// Show the canvas for the current page and handle the tools. Returns true when the project
/// changed.
pub fn show(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor, show_grid: bool, snap_step: f64) {
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
    if let Some(u) = &page.underlay {
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

    for (i, g) in page.graphics.iter().enumerate() {
        let selected = ed.is_selected(&Selection::Graphic(i));
        let c = if selected { t.selection } else { t.ink };
        paint_shape(&painter, &xf, g, Stroke::new(xf.len(0.25).max(0.8), c));
        if selected {
            let (a, b) = crate::edit::graphic_bounds(g);
            let r = Rect::from_two_pos(xf.s(a), xf.s(b)).expand(3.0);
            painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, t.selection), egui::StrokeKind::Outside);
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
        if !label.is_empty() && pts.len() >= 2 {
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
    let marks = wc_pdf::xref_marks(&page, &open.symbols, &xref, &mut xref_warnings);
    let mut jump = None;
    for m in &marks {
        let (a, b) = crate::edit::text_box(m.at, wc_pdf::XREF_HEIGHT, m.align, &m.text);
        let r = Rect::from_two_pos(xf.s(a), xf.s(b));
        let hovered = resp.hover_pos().is_some_and(|h| r.contains(h));
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

    // Review comments pinned on the page: a numbered bubble whose tail points at the spot,
    // greyed once resolved. Hovering shows the start of the text; clicking opens it in the
    // properties panel.
    let mut clicked_comment = None;
    for c in open.project.page_comments(&page.id) {
        let Some(at) = c.at else { continue };
        let spot = xf.s(at);
        let r = 9.0_f32;
        let center = spot + Vec2::new(r + 2.0, -r - 2.0);
        let hovered = resp.hover_pos().is_some_and(|h| h.distance(center) <= r + 2.0);
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

    // Pointer position in page mm, snapped.
    let hover = resp.hover_pos().map(|hp| xf.mm(hp));
    ed.cursor = hover.map(|p| snap(p, snap_step));
    let tol = (6.0 / f64::from(xf.zoom)).max(0.8);

    match ed.tool {
        Tool::Select => select_tool(&painter, &resp, open, ed, &page, &xf, tol, snap_step),
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
        Tool::Line | Tool::Rect => shape_tool(&painter, &resp, open, ed, &xf, &t),
    }
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
) {
    let t = theme::Theme::of(&resp.ctx);
    let shift = resp.ctx.input(|i| i.modifiers.shift || i.modifiers.command);
    if resp.clicked_by(egui::PointerButton::Primary)
        && let Some(at) = resp.interact_pointer_pos().map(|p| xf.mm(p))
    {
        match (open.hit(page, at, tol), shift) {
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
        right_clicked(ed, open.hit(page, at, tol));
    }
    if !selected_wires(ed).is_empty() {
        resp.context_menu(|ui| route_menu(ui, open, ed));
    }
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(at) = resp.interact_pointer_pos().map(|p| xf.mm(p))
    {
        ed.drag = match open.hit(page, at, tol) {
            Some(h) => {
                if !ed.is_selected(&h) {
                    if shift {
                        ed.selection.push(h);
                    } else {
                        ed.select_only(h);
                    }
                }
                Some(Drag::Move {
                    from: snap(at, snap_step),
                    checkpoint: false,
                })
            }
            None => Some(Drag::Box { from: at }),
        };
    }
    match ed.drag {
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
                    let found = open.select_in(ed.page, from, cur);
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

/// The end of a select-tool drag: a finished move wires the moved symbols' pins that now line
/// up (see [`auto_wire`]).
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

/// Line and rectangle tools: click the first corner, then the second.
fn shape_tool(
    painter: &egui::Painter,
    resp: &egui::Response,
    open: &mut Open,
    ed: &mut Editor,
    xf: &Xf,
    t: &theme::Theme,
) {
    let Some(cur) = ed.cursor else { return };
    let make = |a: Point, b: Point, tool: Tool| match tool {
        Tool::Rect => Shape::Rect {
            min: Point::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point::new(a.x.max(b.x), a.y.max(b.y)),
        },
        _ => Shape::Line { a, b },
    };
    if let Some(a) = ed.shape_start {
        paint_shape(painter, xf, &make(a, cur, ed.tool), Stroke::new(1.5_f32, t.ghost));
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        match ed.shape_start.take() {
            None => {
                ed.shape_start = Some(cur);
                ed.message = "Click the other end (Esc cancels).".into();
            }
            Some(a) if a != cur => {
                if let Some(sel) = open.add_graphic(ed.page, make(a, cur, ed.tool)) {
                    ed.select_only(sel);
                }
                ed.message.clear();
            }
            Some(_) => {}
        }
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::state::tests::project;

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
