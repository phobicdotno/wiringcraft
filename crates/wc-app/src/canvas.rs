//! The schematic canvas: paper, frame, grid, page graphics, wires and symbols, plus the
//! mouse handling for the select, place and wire tools.
//!
//! Page coordinates are millimetres with y pointing down, like the paper and the PDF export.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape as EShape, Stroke, Vec2};
use wc_core::geom::{Align, Point, Rotation};
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
    /// The size of the canvas when it was last drawn: Zoom in and Zoom out keep its centre.
    pub size: Vec2,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 3.0,
            offset: Vec2::new(20.0, 20.0),
            fit: true,
            size: Vec2::ZERO,
        }
    }
}

/// The schematic canvas' zoom limits, in screen points per millimetre.
pub const ZOOM_LIMITS: (f32, f32) = (0.3, 60.0);

impl View {
    /// Zoom by `factor` about `anchor` (relative to the canvas' top-left corner), so the page
    /// point under it stays put, within `limits`. Zooming out never zooms in, nor the other way.
    pub fn zoom_about(&mut self, anchor: Vec2, factor: f32, (min, max): (f32, f32)) {
        let new_zoom = (self.zoom * factor).clamp(min, max);
        let new_zoom = if factor < 1.0 {
            new_zoom.min(self.zoom)
        } else {
            new_zoom.max(self.zoom)
        };
        let a = anchor - self.offset;
        self.offset += a - a * (new_zoom / self.zoom);
        self.zoom = new_zoom;
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
    /// The turn and mirror the place tool puts the symbol down with (R and M set them while
    /// the symbol hangs on the pointer). Set back when another tool is picked.
    pub place_turn: (Rotation, bool),
    /// A wire being drawn: its start and the bends so far.
    pub wire: Option<(Endpoint, Vec<Point>)>,
    /// The id of the page that wire was started on: on another page it is dropped.
    pub wire_page: Option<String>,
    pub view: View,
    pub cursor: Option<Point>,
    drag: Option<Drag>,
    /// The clicks so far of a line, rectangle, circle, arc or polyline being drawn.
    pub shape_points: Vec<Point>,
    /// The id of the page those clicks were made on: on another page they are dropped.
    pub shape_page: Option<String>,
    /// Enter was pressed (and no dialog or text field took it): finish the polyline being drawn.
    pub finish_shape: bool,
    /// Escape was pressed during a select-tool drag (see [`Editor::dragging`]): cancel it.
    pub cancel_drag: bool,
    /// Where and when (egui time, seconds) the last drawing click landed, to tell a quick
    /// repeat on a polyline's last point (which ends it) from a slow one.
    pub last_shape_click: Option<(Point, f64)>,
    /// The text tool was clicked here; the app asks for the text.
    pub ask_text: Option<Point>,
    /// The comment tool was clicked here; the app asks for the comment.
    pub ask_comment: Option<Point>,
    /// A black box being drawn with the black box tool.
    pub black_box: Option<crate::black_box_tool::BoxDraft>,
    /// The black box tool was clicked on an edge of its box; the app asks for the connection
    /// point's name.
    pub ask_box_pin: Option<(wc_core::BoxSide, f64)>,
    /// The black box tool is done adding connection points; the app asks for the tag.
    pub ask_box_tag: bool,
    /// The panel layout's Attach footprint drawing / Attach 3D model: the file to ask for, and
    /// the part it goes with.
    pub ask_attach: Option<(crate::panels::Attach, String)>,
    /// The review comment shown in the properties panel, by id.
    pub comment: Option<String>,
    pub message: String,
    /// The panel layout shown instead of a page, by index.
    pub panel: Option<usize>,
    /// The device item picked in the panel view, by index into the panel's items.
    pub panel_pick: Option<usize>,
    /// An item being dragged in the panel view: the grab offset, the undo history count
    /// right after the drag's checkpoint (`None` until it is taken), and the item as it was
    /// when the drag started (Escape puts it back so). The drag ends when the history count
    /// moves on without it.
    pub panel_drag: Option<(Point, Option<u64>, wc_core::panel::PanelItem)>,
    /// The panel drag changes a rail's length or a duct's size by its handle instead of
    /// moving it; the grab offset is then from the handle.
    pub panel_resize: bool,
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
    /// A command picked in the canvas right-click menu (Rotate, Mirror, Copy, Cut, Delete):
    /// the app runs it like the toolbar button.
    pub menu_action: Option<crate::actions::Action>,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            page: 0,
            tool: Tool::Select,
            selection: Vec::new(),
            place_symbol: None,
            place_turn: (Rotation::R0, false),
            wire: None,
            view: View::default(),
            cursor: None,
            drag: None,
            shape_points: Vec::new(),
            shape_page: None,
            wire_page: None,
            finish_shape: false,
            cancel_drag: false,
            last_shape_click: None,
            ask_text: None,
            ask_comment: None,
            black_box: None,
            ask_box_pin: None,
            ask_box_tag: false,
            ask_attach: None,
            comment: None,
            message: String::new(),
            marking_strip: None,
            panel: None,
            panel_pick: None,
            panel_drag: None,
            panel_resize: false,
            auto_wire: false,
            held_by: Default::default(),
            layers: Default::default(),
            live: Vec::new(),
            waiting_graphics: None,
            menu_action: None,
        }
    }
}

/// What a primary-button drag in the select tool is doing.
#[derive(Debug, Clone)]
enum Drag {
    /// Moving the selection: the grab point (snapped, page mm) and whether the undo checkpoint
    /// for this drag was taken. Escape (#737, #863) needs the rest: the id of the page it moves
    /// on, the history count right after its checkpoint (any other count means a change was
    /// recorded during the drag, a rotate say), that checkpoint's undo step, and the selection
    /// when the drag began, or `None` once a frame moved another selection. With no change
    /// recorded during the drag, Escape puts the whole page back as it was, whatever moved;
    /// after one (a rotate), it only takes back a move that moved that same selection all
    /// along, by [`Open::take_back_move`].
    Move {
        from: Point,
        checkpoint: bool,
        page: String,
        step: u64,
        id: u64,
        items: Option<Vec<Selection>>,
    },
    /// Drawing a selection box from this corner.
    Box { from: Point },
    /// Moving the logo on the cover page, or with `size` sizing it from its bottom-right
    /// corner: the grab point, the logo's box when the drag started, and whether the undo
    /// checkpoint for this drag was taken, and the history count right after it (see `Move`).
    Logo {
        from: Point,
        start: (Point, Point),
        size: bool,
        checkpoint: bool,
        step: u64,
    },
    /// Sizing the selected black box from one of its corners (#524, #529): the box's item id
    /// on the page, the corner held, the grab point (page mm, unsnapped), its size when the drag
    /// started, its size now and how far its origin moves in its own frame (a near corner moves
    /// it so the opposite corner stays put). The box changes once, on release; a box no longer
    /// on the page changes nothing.
    BoxSize {
        id: String,
        corner: crate::black_box_tool::Corner,
        from: Point,
        start: (f64, f64),
        size: (f64, f64),
        shift: Point,
    },
}

impl Editor {
    /// Change the tool. The turn for the symbol on the pointer belongs to the place tool:
    /// any other tool sets it back.
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        if tool != Tool::Place {
            self.reset_place_turn();
        }
    }

    /// Set the turn for the symbol on the pointer back to upright. A status that still tells
    /// of the old turn goes with it; any other status stays.
    pub fn reset_place_turn(&mut self) {
        if self.place_turn == (Rotation::R0, false) {
            return;
        }
        self.place_turn = (Rotation::R0, false);
        if self.message.ends_with(TURN_STATUS_END) || self.message.contains(TURN_KEPT_STATUS) {
            self.message.clear();
        }
    }

    /// Whether a select-tool drag (a move, a selection box, a logo or black box sizing drag)
    /// is going on, so Escape cancels it instead of doing what it does otherwise.
    pub fn select_dragging(&self) -> bool {
        self.tool == Tool::Select && self.drag.is_some()
    }

    /// Escape on the canvas: cancel the select-tool drag going on (the canvas takes it back on
    /// its next frame, #737), else drop the black box, shape or wire being drawn, else clear the
    /// Select tool's selection (#736). Returns true when there was nothing to drop, and Escape
    /// goes back to the Select tool.
    pub fn escape(&mut self) -> bool {
        if self.select_dragging() {
            self.cancel_drag = true;
        } else if self.tool == Tool::BlackBox && self.black_box.is_some() {
            self.black_box = None;
            self.message = crate::black_box_tool::hint(None).into();
        } else if !self.shape_points.is_empty() {
            self.shape_points.clear();
            self.message = shape_hint(self.tool, 0).into();
        } else if self.wire.is_some() {
            self.wire = None;
        } else if self.tool == Tool::Select && !self.selection.is_empty() {
            self.selection.clear();
        } else {
            return true;
        }
        false
    }

    /// Zoom in or out by `factor` from the keys or buttons, about the centre of the visible
    /// canvas, within the zoom limits of the schematic or of the panel view.
    pub fn zoom_keys(&mut self, factor: f32) {
        let limits = if self.panel.is_some() {
            crate::panel_view::ZOOM_LIMITS
        } else {
            ZOOM_LIMITS
        };
        self.view.zoom_about(self.view.size / 2.0, factor, limits);
    }

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

    /// Show page `page` instead of the page or panel shown now: the panel view closes, and
    /// the selection is cleared and the view fitted. Nothing changes when `page` is shown
    /// already; returns whether something did.
    pub fn show_page(&mut self, page: usize) -> bool {
        if self.page == page && self.panel.is_none() {
            return false;
        }
        self.page = page;
        self.panel = None;
        self.panel_pick = None;
        self.selection.clear();
        // A drag still held belonged to the page left behind (as `stop_if_held` drops it).
        self.drag = None;
        self.view.fit = true;
        true
    }

    pub fn select_only(&mut self, s: Selection) {
        self.selection = vec![s];
    }

    pub fn is_selected(&self, s: &Selection) -> bool {
        self.selection.contains(s)
    }

    /// Whether a select-tool drag (a move, a logo or box sizing, a selection box) or a panel
    /// device drag is under way: keys that edit the selection wait until it ends.
    pub fn dragging(&self) -> bool {
        self.drag.is_some() || self.panel_drag.is_some()
    }

    /// Start a move drag as a press on a selected item does (for tests of what waits for it).
    #[cfg(test)]
    pub(crate) fn start_move_drag(&mut self) {
        self.drag = Some(Drag::Move {
            from: Point::new(0.0, 0.0),
            checkpoint: false,
            page: String::new(),
            step: 0,
            id: 0,
            items: Some(Vec::new()),
        });
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

/// How many corner tests [`triangulate`] may spend on one outline. Ear tests look only at the
/// corners in the grid cells under the ear, so ordinary fills of tens of thousands of points
/// stay far below this; only a pathological outline (every corner piled into a few cells) runs
/// out, and is then drawn as its outline instead of freezing the editor on its first paint.
const TRIANGULATE_BUDGET: usize = 20_000_000;

/// Triangles (vertex indexes) covering a simple polygon, by ear clipping. A polygon with a hole
/// cut in along a bridge (its two sides on top of each other) comes out right too. `None` when
/// cutting it up would take more than [`TRIANGULATE_BUDGET`] corner tests.
///
/// The corners are kept in a ring that is walked round, clipping each ear as it comes (instead
/// of searching from the start again), and an ear is checked only against the corners that are
/// not convex (only those can lie inside it) in the grid cells its box covers, so a fill costs
/// about its point count, not the square of it.
fn triangulate(pts: &[Pos2]) -> Option<Vec<[u32; 3]>> {
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let n = pts.len();
    let mut out = Vec::new();
    if n < 3 {
        return Some(out);
    }
    // Summed in f64 (each f32 product is exact there), so the winding does not cancel out.
    let area: f64 = pts
        .iter()
        .zip(pts.iter().cycle().skip(1))
        .map(|(a, b)| f64::from(a.x) * f64::from(b.y) - f64::from(a.y) * f64::from(b.x))
        .sum();
    let turn = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = vec![true; n];
    // Every index below comes from the ring, so it is in range; an index that is not (a bug)
    // gives `None`, and the shape is drawn as its outline rather than panicking.
    let pt = |i: usize| pts.get(i).copied();
    let link = |v: &[usize], i: usize| v.get(i).copied();
    let set = |v: &mut [usize], i: usize, to: usize| v.get_mut(i).map(|x| *x = to);

    // The corners that are not convex, bucketed in a square grid over the outline's box. A
    // corner only ever turns from reflex to convex as ears are clipped, so the grid is built
    // once; clipped corners are skipped when looked up.
    let first = pt(0)?;
    let (mut lo, mut hi) = (first, first);
    for q in pts {
        lo = lo.min(*q);
        hi = hi.max(*q);
    }
    let corner = |prev: &[usize], next: &[usize], i: usize| -> Option<f32> {
        Some(cross(pt(link(prev, i)?)?, pt(i)?, pt(link(next, i)?)?))
    };
    let mut reflex = Vec::new();
    for i in 0..n {
        if corner(&prev, &next, i)? * turn <= 0.0 {
            reflex.push(i);
        }
    }
    let side = ((reflex.len() as f32).sqrt().ceil() as usize).clamp(1, 1024);
    let cell_w = ((hi.x - lo.x) / side as f32).max(f32::MIN_POSITIVE);
    let cell_h = ((hi.y - lo.y) / side as f32).max(f32::MIN_POSITIVE);
    let cell = |q: Pos2| {
        let cx = (((q.x - lo.x) / cell_w) as usize).min(side - 1);
        let cy = (((q.y - lo.y) / cell_h) as usize).min(side - 1);
        (cx, cy)
    };
    let mut grid: Vec<Vec<usize>> = vec![Vec::new(); side * side];
    for &i in &reflex {
        let (cx, cy) = cell(pt(i)?);
        grid.get_mut(cy * side + cx)?.push(i);
    }

    let mut work = 0usize;
    let mut left = n;
    let mut ear = 0usize;
    // A full walk round the ring without an ear ends at `stop`.
    let mut stop = ear;
    while left > 3 {
        // Every step counts, so a walk that finds no ear is bounded too.
        work += 1;
        if work > TRIANGULATE_BUDGET {
            return None;
        }
        let (ia, ic) = (link(&prev, ear)?, link(&next, ear)?);
        let (a, b, c) = (pt(ia)?, pt(ear)?, pt(ic)?);
        let is_ear = cross(a, b, c) * turn > 0.0 && {
            // No other corner strictly inside the triangle.
            let (c0x, c0y) = cell(a.min(b).min(c));
            let (c1x, c1y) = cell(a.max(b).max(c));
            let mut clear = true;
            'cells: for cy in c0y..=c1y {
                for cx in c0x..=c1x {
                    // Empty cells count too: an ear with a box over most of the grid costs
                    // its cells even with no corner in them.
                    work += 1;
                    if work > TRIANGULATE_BUDGET {
                        return None;
                    }
                    for &k in grid.get(cy * side + cx)? {
                        work += 1;
                        if !alive.get(k).copied()? || k == ia || k == ear || k == ic {
                            continue;
                        }
                        let q = pt(k)?;
                        if q == a || q == b || q == c {
                            continue;
                        }
                        let (d1, d2, d3) = (cross(a, b, q) * turn, cross(b, c, q) * turn, cross(c, a, q) * turn);
                        if d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0 {
                            clear = false;
                            break 'cells;
                        }
                    }
                }
            }
            clear
        };
        if is_ear {
            out.push([ia as u32, ear as u32, ic as u32]);
            set(&mut next, ia, ic)?;
            set(&mut prev, ic, ia)?;
            *alive.get_mut(ear)? = false;
            left -= 1;
            // Go on from the corner after the ear, as earcut does.
            ear = link(&next, ic)?;
            stop = ear;
            continue;
        }
        ear = link(&next, ear)?;
        if ear == stop {
            // Only flat corners left (a bridge, collinear points): drop one and go on.
            let mut k = ear;
            let mut flat = None;
            for _ in 0..left {
                work += 1;
                if corner(&prev, &next, k)?.abs() < 1e-6 {
                    flat = Some(k);
                    break;
                }
                k = link(&next, k)?;
            }
            let Some(k) = flat else { break };
            let (p, q) = (link(&prev, k)?, link(&next, k)?);
            set(&mut next, p, q)?;
            set(&mut prev, q, p)?;
            *alive.get_mut(k)? = false;
            left -= 1;
            ear = q;
            stop = ear;
        }
    }
    if left == 3 {
        let b = link(&next, ear)?;
        out.push([ear as u32, b as u32, link(&next, b)? as u32]);
    }
    Some(out)
}

/// The outline of a filled polyline in drawing millimetres, as the key of [`TriCache`]: the same
/// points give the same key, so any edit, replaced shape or reloaded symbol misses the cache.
#[derive(Clone, Copy)]
struct FillKey<'a>(&'a [Point]);

impl std::hash::Hash for FillKey<'_> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        for q in self.0 {
            q.x.to_bits().hash(h);
            q.y.to_bits().hash(h);
        }
    }
}

#[derive(Default)]
struct Triangulator;

/// The triangles of one fill, or `None` when it is too large to cut up (drawn as its outline).
type FillTriangles = Option<std::sync::Arc<[[u32; 3]]>>;

impl egui::util::cache::ComputerMut<FillKey<'_>, FillTriangles> for Triangulator {
    fn compute(&mut self, key: FillKey<'_>) -> FillTriangles {
        #[cfg(test)]
        TRIANGULATIONS.with(|n| n.set(n.get() + 1));
        // In millimetres, not screen pixels, so panning and zooming keep the cached triangles.
        // Taken from the first point (in f64), so an outline far from the sheet origin keeps
        // its precision in f32.
        let o = key.0.first().copied().unwrap_or_default();
        let pts: Vec<Pos2> = key
            .0
            .iter()
            .map(|q| Pos2::new((q.x - o.x) as f32, (q.y - o.y) as f32))
            .collect();
        triangulate(&pts).map(Into::into)
    }
}

/// Triangles of the filled polylines painted last frame, so a large fill is not cut up again on
/// every frame (nor a too large one tried again). Shapes not painted in a frame drop out.
type TriCache = egui::util::cache::FrameCache<FillTriangles, Triangulator>;

#[cfg(test)]
thread_local! {
    /// How many fills this thread has triangulated (the cache's misses), for tests.
    static TRIANGULATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn paint_shape(p: &egui::Painter, xf: &Xf, s: &Shape, stroke: Stroke) {
    match s {
        Shape::Dimension { .. } => {
            for part in s.dimension_parts().unwrap_or_default() {
                paint_shape(p, xf, &part, stroke);
            }
        }
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
                let tris = p
                    .ctx()
                    .memory_mut(|m| m.caches.cache::<TriCache>().get(FillKey(points)));
                if let Some(tris) = tris {
                    let mut mesh = egui::Mesh::default();
                    for p in &pts {
                        mesh.colored_vertex(*p, stroke.color);
                    }
                    for &[a, b, c] in tris.iter() {
                        mesh.add_triangle(a, b, c);
                    }
                    p.add(EShape::mesh(mesh));
                } else {
                    // Too many points to cut up without stalling the editor: its outline at least.
                    p.add(EShape::closed_line(pts, stroke));
                }
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
        // Revision clouds and the table lie over the drawing, so they are found before it.
        if let Some(i) = page.clouds.iter().rposition(|c| c.hit(at, tol)) {
            return Some(Selection::Cloud(i));
        }
        if let Some((a, b)) = self.revision_table_bounds(page)
            && at.x >= a.x - tol
            && at.x <= b.x + tol
            && at.y >= a.y - tol
            && at.y <= b.y + tol
        {
            return Some(Selection::RevisionTable);
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
/// changed. `keyboard_edits` is false while an app dialog has the keyboard: the panel view's
/// arrow keys then do nothing.
pub fn show(
    ui: &mut egui::Ui,
    open: &mut Open,
    ed: &mut Editor,
    show_grid: bool,
    snap_step: f64,
    keyboard_edits: bool,
) {
    drop_draft_of_other_page(open, ed);
    // A black box being drawn belongs to its tool: another tool drops it.
    if ed.tool != Tool::BlackBox {
        ed.black_box = None;
    }
    // An item on a layer that was just hidden or locked does not stay selected.
    let layers = ed.layers.clone();
    ed.selection.retain(|s| layers.may_select(s));
    let finish = std::mem::take(&mut ed.finish_shape);
    // The turn for the symbol on the pointer belongs to the place tool: another tool sets it
    // back, also while a panel is shown.
    if ed.tool != Tool::Place {
        ed.reset_place_turn();
    }
    if let Some(panel) = ed.panel {
        crate::panel_view::show(ui, open, ed, panel, snap_step, keyboard_edits);
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    ed.view.size = rect.size();
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
            ed.view.zoom_about(hp - rect.min, (scroll / 300.0).exp(), ZOOM_LIMITS);
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
    // A selected one is drawn in the selection colour, as a graphic is.
    let revision_ink = |s: &Selection| {
        let c = if ed.is_selected(s) { t.selection } else { t.ink };
        Stroke::new(xf.len(0.25).max(0.8), c)
    };
    for (i, c) in page.clouds.iter().enumerate() {
        let ink = revision_ink(&Selection::Cloud(i));
        for s in c.shapes() {
            paint_shape(&painter, &xf, &s, ink);
        }
    }
    if let Some(at) = page.revision_table {
        let ink = revision_ink(&Selection::RevisionTable);
        for s in wc_core::revision::table_shapes(&open.project.meta.revisions, at) {
            paint_shape(&painter, &xf, &s, ink);
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
        ed.set_tool(Tool::Select);
        ed.wire = None;
        ed.shape_points.clear();
        ed.black_box = None;
    }
    leave_unpickable_layer(ed);

    // Pointer position in page mm, snapped.
    let hover = resp.hover_pos().map(|hp| xf.mm(hp));
    ed.cursor = hover.map(|p| snap(p, snap_step));
    let tol = (6.0 / f64::from(xf.zoom)).max(0.8);

    end_drag_off_select(ed);
    match ed.tool {
        Tool::Select => select_tool(&painter, &resp, open, ed, &page, &xf, tol, snap_step, held),
        Tool::Place => place_tool(&painter, &resp, open, ed, &xf, &t),
        Tool::Wire => wire_tool(&painter, &resp, open, ed, &page, &xf, &shown, &t, snap_step),
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
        Tool::Line | Tool::Rect | Tool::Circle | Tool::Arc | Tool::Polyline | Tool::Dimension | Tool::Cloud => {
            shape_tool(&painter, &resp, open, ed, &xf, &shown, &t, finish, snap_step)
        }
        Tool::RevisionTable => {
            if resp.clicked_by(egui::PointerButton::Primary)
                && let Some(at) = primary_release_pos(&resp.ctx)
                    .or(resp.interact_pointer_pos())
                    .map(|p| snap(shown.mm(p), snap_step))
            {
                revision_table_click(open, ed, at);
            }
            // The table as it would be placed, under the pointer.
            if let Some(cur) = ed.cursor {
                let ghost = Stroke::new(1.5_f32, t.ghost);
                for s in wc_core::revision::table_shapes(&open.project.meta.revisions, cur) {
                    paint_shape(&painter, &xf, &s, ghost);
                }
            }
        }
        Tool::BlackBox => black_box_tool(&painter, &resp, ed, &page, &xf, &shown, &t, finish, tol, snap_step),
    }
}

/// The black box tool: a drag (or two clicks) draws the rectangle; then a click on an edge
/// asks for a connection point there, and a click inside the box (or Enter) asks for the tag.
#[allow(clippy::too_many_arguments)]
fn black_box_tool(
    painter: &egui::Painter,
    resp: &egui::Response,
    ed: &mut Editor,
    page: &Page,
    xf: &Xf,
    shown: &Xf,
    t: &theme::Theme,
    enter: bool,
    tol: f64,
    snap_step: f64,
) {
    use crate::black_box_tool::{BoxDraft, edge_at, hint, inside};
    if ed.black_box.as_ref().is_some_and(|d| d.page_id != page.id) {
        ed.black_box = None;
        ed.message = hint(None).into();
    }
    // A dialog for this box is waiting: the canvas takes no clicks for it meanwhile.
    let asking = ed.ask_box_pin.is_some() || ed.ask_box_tag;
    let cur = ed.cursor;
    // Where a click or a drag ended, unsnapped: the release itself, since a touch screen
    // clears the hover position (and so `ed.cursor`) in the frame it lets go.
    // Pointer input is read in the view the user saw (`shown`): a zoom in the same frame must
    // not move it. `xf` is the view drawn now.
    let released_at = primary_release_pos(&resp.ctx)
        .or(resp.interact_pointer_pos())
        .map(|p| shown.mm(p));
    // How near an edge a click must land, in that view too; `tol` (the view drawn now) is
    // for the hover ring.
    let click_tol = (6.0 / f64::from(shown.zoom)).max(0.8);
    match ed.black_box.as_mut() {
        None if !asking => {
            // A drag is only seen once the pointer has moved: its corner is where the button
            // went down, not where the pointer is by then.
            let start = if resp.drag_started_by(egui::PointerButton::Primary) {
                resp.ctx
                    .input(|i| i.pointer.press_origin())
                    .or(resp.interact_pointer_pos())
                    .map(|p| snap(shown.mm(p), snap_step))
            } else if resp.clicked_by(egui::PointerButton::Primary) {
                released_at.map(|p| snap(p, snap_step))
            } else {
                None
            };
            if let Some(at) = start {
                ed.black_box = Some(BoxDraft::new(&page.id, at));
                ed.message = hint(ed.black_box.as_ref()).into();
            }
        }
        Some(d) if d.to.is_none() && !asking => {
            let released =
                resp.drag_stopped_by(egui::PointerButton::Primary) || resp.clicked_by(egui::PointerButton::Primary);
            let at = released_at.map(|p| snap(p, snap_step));
            if released && let Some(at) = at.filter(|at| at.x != d.from.x && at.y != d.from.y) {
                d.to = Some(at);
                ed.message = hint(Some(d)).into();
            }
        }
        Some(d) if !asking => {
            let Some((min, max)) = d.rect() else { return };
            if resp.clicked_by(egui::PointerButton::Primary)
                && let Some(at) = released_at
            {
                if let Some(hit) = edge_at(min, max, at, click_tol) {
                    // Snapped along the edge, like every other point on the page.
                    let snapped = edge_at(min, max, snap(at, snap_step), f64::INFINITY);
                    ed.ask_box_pin = Some(match snapped {
                        Some((side, off)) if side == hit.0 => (side, off),
                        _ => hit,
                    });
                } else if inside(min, max, at, click_tol) {
                    ed.ask_box_tag = true;
                }
            } else if enter {
                ed.ask_box_tag = true;
            }
        }
        _ => {}
    }

    // The box so far, its points, and where a click on an edge would put the next one.
    let ghost = Stroke::new(1.5_f32, t.ghost);
    let Some(d) = ed.black_box.as_ref() else { return };
    let (min, max) = match d.rect() {
        Some(r) => r,
        None => match cur {
            Some(c) => crate::black_box_tool::corners(d.from, c),
            None => return,
        },
    };
    painter.rect_stroke(
        Rect::from_two_pos(xf.s(min), xf.s(max)),
        0.0,
        ghost,
        egui::StrokeKind::Middle,
    );
    for pin in &d.pins {
        if let Some(at) = d.pin_at(pin) {
            painter.circle_filled(xf.s(at), 3.0, t.accent);
            painter.text(
                xf.s(at) + Vec2::new(4.0, -4.0),
                Align2::LEFT_BOTTOM,
                &pin.name,
                FontId::proportional(11.0),
                t.ink,
            );
        }
    }
    if d.to.is_some()
        && !asking
        && let Some(c) = cur
        && let Some((side, off)) = edge_at(min, max, c, tol.max(1.0))
    {
        let at = d.pin_at(&wc_core::BoxPin {
            name: String::new(),
            side,
            offset: off,
        });
        if let Some(at) = at {
            painter.circle_stroke(xf.s(at), 4.0, Stroke::new(1.5_f32, t.accent));
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
    // A revision cloud: the area it goes around.
    if let Some(c) = wc_report::cloud_index(item).and_then(|n| page.clouds.get(n)) {
        return Some((c.min, c.max));
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
        Tool::Text | Tool::Line | Tool::Rect | Tool::Circle | Tool::Arc | Tool::Polyline | Tool::Dimension => {
            Layer::Drawings
        }
        Tool::Comment => Layer::Comments,
        // Revision clouds and the revision table are on no layer.
        Tool::Select | Tool::Place | Tool::Wire | Tool::Cloud | Tool::RevisionTable | Tool::BlackBox => return,
    };
    let Some(why) = ed.layers.refusal(layer) else {
        return;
    };
    ed.set_tool(Tool::Select);
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
    // Right-click on a wire, symbol, free text or drawing: select it (keeping a selection it is
    // part of) and offer the route styles for every selected wire and the usual edits for the
    // selected symbols, text and drawings.
    if resp.secondary_clicked()
        && let Some(at) = resp.interact_pointer_pos().map(|p| xf.mm(p))
    {
        let hit = pick(open, ed, page, at, tol);
        right_clicked(ed, hit);
    }
    if !held && (!edit_menu_actions(ed).is_empty() || !selected_wires(ed).is_empty()) {
        resp.context_menu(|ui| {
            if !selected_wires(ed).is_empty() {
                route_menu(ui, open, ed);
            }
            edit_menu(ui, ed);
        });
    }
    // A drag is only seen once the pointer has moved: it starts where the button went down,
    // so a small target (a sizing corner) is not missed by a quick drag.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(at) = resp
            .ctx
            .input(|i| i.pointer.press_origin())
            .or(resp.interact_pointer_pos())
            .map(|p| xf.mm(p))
    {
        ed.drag = Some(drag_start(open, ed, page, at, tol, snap_step, shift, held));
    }
    stop_if_held(ed, held);
    if std::mem::take(&mut ed.cancel_drag) {
        cancel_drag(open, ed);
    }
    if matches!(ed.drag, Some(Drag::Move { .. })) {
        // Not cloned like the other drags: a move carries the selection it began with.
        if resp.dragged_by(egui::PointerButton::Primary)
            && let Some(cur) = ed.cursor
        {
            // Move by whole snap steps so pins stay on the grid.
            move_drag_to(open, ed, snap(cur, snap_step));
        }
    } else {
        match ed.drag.clone() {
            Some(Drag::Logo {
                from,
                start,
                size,
                checkpoint,
                step,
            }) if resp.dragged_by(egui::PointerButton::Primary) => {
                // The raw pointer, like `from`: `ed.cursor` is snapped to the grid, and mixing the
                // two would jump the logo on the first move.
                if let Some(cur) = resp.interact_pointer_pos().map(|p| xf.mm(p)) {
                    let (a, b) = logo_dragged(start, size, Point::new(cur.x - from.x, cur.y - from.y));
                    let now = wc_pdf::cover_logo_box(&open.project, page);
                    if now != Some((a, b)) {
                        let step = if checkpoint {
                            step
                        } else {
                            open.checkpoint();
                            open.history_count()
                        };
                        open.set_logo_place(a, b);
                        ed.drag = Some(Drag::Logo {
                            from,
                            start,
                            size,
                            checkpoint: true,
                            step,
                        });
                    }
                }
            }
            // Also in the frame the button is let go: a last move that comes with the release counts.
            Some(Drag::BoxSize {
                id,
                corner,
                from,
                start,
                size,
                shift,
            }) if resp.dragged_by(egui::PointerButton::Primary) || resp.drag_stopped() => {
                if let Some(cur) = primary_release_pos(&resp.ctx)
                    .or(resp.interact_pointer_pos())
                    .map(|p| xf.mm(p))
                    && let Some(s) = page.symbol(&id)
                {
                    let d = Point::new(cur.x - from.x, cur.y - from.y);
                    let (now, moved) =
                        crate::black_box_tool::dragged_box(start, corner, d, s.rotation, s.mirror, snap_step);
                    if now != size || moved != shift {
                        ed.drag = Some(Drag::BoxSize {
                            id,
                            corner,
                            from,
                            start,
                            size: now,
                            shift: moved,
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
    }
    paint_box_sizing(painter, open, ed, page, xf, &t, held);
    if resp.drag_stopped() {
        let alt = resp.ctx.input(|i| i.modifiers.alt);
        drag_released(open, ed, alt);
    }
}

/// The black box `id` on `page` that can be sized by its corners: its size, and each of its
/// four sizing corners (far corner first) with where it is in page mm, turned and mirrored with
/// the placement. `None` for any other item.
type BoxCorners = ((f64, f64), [(crate::black_box_tool::Corner, Point); 4]);

fn box_corners(open: &Open, page: &Page, id: &str) -> Option<BoxCorners> {
    let s = page.symbol(id)?;
    let (w, h, _) = wc_core::black_box_layout(open.symbols.get(&s.symbol)?)?;
    let on_page = |c| {
        (
            c,
            crate::black_box_tool::corner_at(c, w, h).transform(s.rotation, s.mirror) + s.at,
        )
    };
    Some(((w, h), crate::black_box_tool::CORNERS.map(on_page)))
}

/// The sizing handle of the one selected black box, and while it is dragged the box's outline
/// at the size it will get. Not on a page someone else holds.
fn paint_box_sizing(
    painter: &egui::Painter,
    open: &Open,
    ed: &Editor,
    page: &Page,
    xf: &Xf,
    t: &theme::Theme,
    held: bool,
) {
    if held {
        return;
    }
    if let Some(Drag::BoxSize {
        id,
        corner,
        size,
        shift,
        ..
    }) = &ed.drag
        && let Some(s) = page.symbol(id)
    {
        let (w, h) = *size;
        let on_page = |p: Point| xf.s((p + *shift).transform(s.rotation, s.mirror) + s.at);
        let pts: Vec<Pos2> = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h), (0.0, 0.0)]
            .into_iter()
            .map(|(x, y)| on_page(Point::new(x, y)))
            .collect();
        painter.add(egui::Shape::line(pts, Stroke::new(1.5_f32, t.ghost)));
        painter.rect_filled(
            Rect::from_center_size(
                on_page(crate::black_box_tool::corner_at(*corner, w, h)),
                Vec2::splat(7.0),
            ),
            1.0,
            t.selection,
        );
        return;
    }
    if let Some(Selection::Symbol(id)) = ed.one()
        && let Some((_, corners)) = box_corners(open, page, id)
    {
        for (_, at) in corners {
            painter.rect_filled(Rect::from_center_size(xf.s(at), Vec2::splat(7.0)), 1.0, t.selection);
        }
    }
}

/// Whether `at` is on a sizing corner `c` of a black box of `size`: within reach of it, but
/// no further than a quarter of the box's shorter side, so a small box keeps a middle part
/// that moves it.
fn box_corner_hit(c: Point, at: Point, tol: f64, size: (f64, f64)) -> bool {
    let r = tol.max(2.0).min(size.0.min(size.1) / 4.0);
    (at.x - c.x).abs() <= r && (at.y - c.y).abs() <= r
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
    // A sizing corner of the one selected black box wins over whatever lies under it; the
    // nearest one when a small box puts several within reach.
    if let Some(Selection::Symbol(id)) = ed.one()
        && let Some((start, corners)) = box_corners(open, page, id)
        && let Some((corner, _)) = corners
            .into_iter()
            .filter(|(_, c)| box_corner_hit(*c, at, tol, start))
            .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)))
    {
        return Drag::BoxSize {
            id: id.clone(),
            corner,
            from: at,
            start,
            size: start,
            shift: Point::new(0.0, 0.0),
        };
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
            step: 0,
        },
        (Some((a, b)), None) if a.x <= at.x && at.x <= b.x && a.y <= at.y && at.y <= b.y => Drag::Logo {
            from: at,
            start: (a, b),
            size: false,
            checkpoint: false,
            step: 0,
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
                page: page.id.clone(),
                step: 0,
                id: 0,
                items: Some(ed.selection.clone()),
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

/// A frame of a select-tool move drag with the grab point now at `target`: the selection moves
/// by whole steps from the last grab point, the drag's checkpoint taken on its first move. When
/// this moves another selection than the drag began with, the drag forgets its own, so Escape
/// does not take back from it a part of the move it did not get.
fn move_drag_to(open: &mut Open, ed: &mut Editor, target: Point) {
    let Some(Drag::Move {
        from,
        checkpoint,
        page,
        step,
        id,
        items,
    }) = ed.drag.take()
    else {
        return;
    };
    // Nothing selected moves nothing: the grab point follows the pointer, and the drag keeps
    // its record for Escape.
    if target == from || ed.selection.is_empty() {
        ed.drag = Some(Drag::Move {
            from: if target == from { from } else { target },
            checkpoint,
            page,
            step,
            id,
            items,
        });
        return;
    }
    let sel = ed.selection.clone();
    // An Undo during the drag took its checkpoint away: what moves from here on is a move of
    // its own, with a checkpoint of its own for Escape to find.
    let (step, id, items) = if checkpoint && open.has_undo_step(id) {
        (step, id, items)
    } else {
        open.begin_drag_move();
        (
            open.history_count(),
            open.last_step_id().unwrap_or(0),
            Some(sel.clone()),
        )
    };
    let d = Point::new(target.x - from.x, target.y - from.y);
    open.drag_move_by(ed.page, &sel, d);
    ed.drag = Some(Drag::Move {
        from: target,
        checkpoint: true,
        page,
        step,
        id,
        items: items.filter(|i| same_items(i, &sel)),
    });
}

/// Whether two selections hold the same items, in any order (as [`crate::edit::move_items`]
/// sees them).
fn same_items(a: &[Selection], b: &[Selection]) -> bool {
    a.iter().all(|x| b.contains(x)) && b.iter().all(|x| a.contains(x))
}

/// The end of a select-tool drag: a finished move wires the moved symbols' pins that now line
/// up (see [`auto_wire`]).
/// The page became held during a move: the move stops where it is, and its release does
/// nothing more (no wiring of pins that now line up). A selection box goes on.
fn stop_if_held(ed: &mut Editor, held: bool) {
    if held
        && matches!(
            ed.drag,
            Some(Drag::Move { .. } | Drag::Logo { .. } | Drag::BoxSize { .. })
        )
    {
        ed.drag = None;
    }
}

/// Escape during a select-tool drag (#737): a move puts the page it began on back as it was
/// when the drag started, and a logo drag puts the logo back; the drag's undo checkpoint is
/// dropped, so it is as if the drag never happened. When another change was recorded since that
/// checkpoint (a rotate during the move, #863), only the move is taken back: the moved items go
/// back to where they started and keep what was done to them, and the undo and redo steps
/// recorded since get the start position too, so Undo then takes back the rotate alone. Once the
/// move itself was undone during the drag, or a frame moved another selection, the drag is only
/// ended. A black box sizing drag (which only changes the box on release) and a selection box are
/// just dropped. The drag ends, so the release that follows does nothing (no wiring of pins that
/// line up).
fn cancel_drag(open: &mut Open, ed: &mut Editor) {
    let drag = ed.drag.take();
    let moving = matches!(drag, Some(Drag::Move { .. }));
    match drag {
        Some(Drag::Move {
            checkpoint: true,
            page,
            step,
            ..
        }) if open.history_count() == step => {
            let Some(at) = open.project.pages.iter().position(|p| p.id == page) else {
                return;
            };
            let was = open
                .before_last_change()
                .and_then(|b| b.pages.iter().find(|p| p.id == page))
                .cloned();
            // Only the page moved on: a sync of other pages since the drag began stays.
            if let Some(was) = was
                && at < open.project.pages.len()
            {
                // The jumpers, strip layouts and spare device data the moved terminals took
                // along (followed every frame of the drag) follow them back to where they were,
                // the same way: whatever else changed meanwhile (a sync) stays.
                open.follow_terminals();
                let moved = open.terminal_numbers();
                open.project.pages[at] = was;
                open.follow_terminals_from(moved);
                open.forget_terminal_change();
                open.forget_last_change();
                // A save while the button was held wrote the moved page: this is unsaved again.
                open.edited();
            }
        }
        Some(Drag::Move {
            checkpoint: true,
            page,
            id,
            items: Some(items),
            ..
            // `None` would mean a frame moved another selection, leaving what each item moved
            // unknown: then nothing is taken back. A selection changed without moving (a right
            // click on another item) still takes the drag's own items back.
        }) => {
            // Said only when the move is still there to undo: a move already undone has
            // nothing to take back.
            if open.has_undo_step(id)
                && let Err(why) = open.take_back_move(id, &page, &items)
            {
                ed.message = format!("The move could not be taken back: {why}. Undo goes back step by step.");
            }
        }
        Some(Drag::Logo {
            checkpoint: true, step, ..
        }) if open.history_count() == step => {
            let was = open
                .before_last_change()
                .and_then(|b| b.meta.logo.as_ref())
                .map(|l| l.cover);
            if let (Some(was), Some(logo)) = (was, open.project.meta.logo.as_mut()) {
                logo.cover = was;
                open.forget_last_change();
                open.edited();
            }
        }
        _ => {}
    }
    if moving {
        open.end_drag_move();
    }
}

/// A select-tool drag ends when the tool changes (W during a move, say): what moved stays
/// where it is, without the wiring a release does, so no drag is left for a later release or
/// Escape to act on.
fn end_drag_off_select(ed: &mut Editor) {
    if ed.tool != Tool::Select {
        ed.drag = None;
        ed.cancel_drag = false;
    }
}

fn drag_released(open: &mut Open, ed: &mut Editor, alt: bool) {
    if let Some(Drag::BoxSize {
        id, start, size, shift, ..
    }) = ed.drag.clone()
        && (size != start || shift != Point::new(0.0, 0.0))
    {
        box_sized(open, ed, &id, size, shift);
    }
    if let Some(Drag::Move { .. }) = ed.drag {
        open.end_drag_move();
    }
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

/// The end of a drag on a black box's sizing corner: the box gets `size` and its origin moves
/// by `shift` in its own frame (a near corner was dragged), its points kept where they were on
/// the page where they can and on their edges, as one undo step with the same warnings as the
/// properties panel's Apply.
fn box_sized(open: &mut Open, ed: &mut Editor, id: &str, size: (f64, f64), shift: Point) {
    let Some(s) = open.project.pages.get(ed.page).and_then(|p| p.symbol(id)) else {
        ed.message = format!("The box was not sized: {id} is no longer on this page.");
        return;
    };
    let Some((_, _, pins)) = open.symbols.get(&s.symbol).and_then(wc_core::black_box_layout) else {
        return;
    };
    let pins = crate::black_box_tool::shifted_pins(&pins, shift, size.0, size.1);
    let moved = shift.transform(s.rotation, s.mirror);
    let none = std::collections::BTreeSet::new();
    let none_renamed = std::collections::BTreeMap::new();
    ed.message = match open.edit_black_box_moved(ed.page, id, size, &pins, &none_renamed, &none, moved) {
        Ok(warnings) if warnings.is_empty() => format!("Black box {id} sized to {} x {} mm.", size.0, size.1),
        Ok(warnings) => format!(
            "Black box {id} sized to {} x {} mm. Check: {}.",
            size.0,
            size.1,
            warnings.join("; ")
        ),
        Err(e) => format!("The box was not sized: {e}"),
    };
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

/// A right-click on `hit`: a wire, symbol, free text or drawing that is not selected becomes the
/// only selection; a selected one keeps the selection it is part of, so the menu acts on all of
/// it. Anything else leaves the selection alone.
fn right_clicked(ed: &mut Editor, hit: Option<Selection>) {
    if let Some(h @ (Selection::Wire(_) | Selection::Symbol(_) | Selection::Graphic(_))) = hit
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

/// True when a symbol is selected.
fn has_symbols(ed: &Editor) -> bool {
    ed.selection.iter().any(|s| matches!(s, Selection::Symbol(_)))
}

/// The edits the right-click menu offers for the selection, run as the toolbar runs them:
/// Rotate, Mirror, Copy, Cut, Duplicate and Delete with a symbol selected; Copy, Cut, Duplicate
/// and Delete with only free text or drawings (Rotate and Mirror turn symbols only); none
/// otherwise.
fn edit_menu_actions(ed: &Editor) -> &'static [crate::actions::Action] {
    use crate::actions::Action;
    if has_symbols(ed) {
        &[
            Action::Rotate,
            Action::Mirror,
            Action::Copy,
            Action::Cut,
            Action::Duplicate,
            Action::Delete,
        ]
    } else if ed.selection.iter().any(|s| matches!(s, Selection::Graphic(_))) {
        &[Action::Copy, Action::Cut, Action::Duplicate, Action::Delete]
    } else {
        &[]
    }
}

/// The edit entries of the right-click menu (see [`edit_menu_actions`]). The picked command is
/// left in [`Editor::menu_action`] for the app to run.
fn edit_menu(ui: &mut egui::Ui, ed: &mut Editor) {
    let actions = edit_menu_actions(ed);
    if actions.is_empty() {
        return;
    }
    if !selected_wires(ed).is_empty() {
        ui.separator();
    }
    for &a in actions {
        let info = a.info();
        if ui
            .add(egui::Button::new(info.label).shortcut_text(info.shortcut))
            .clicked()
        {
            ed.menu_action = Some(a);
            ui.close_menu();
        }
    }
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
    // With symbols, text or drawings selected too, the edit entries' Delete removes the whole
    // selection.
    if !edit_menu_actions(ed).is_empty() {
        return;
    }
    ui.separator();
    if ui.add(egui::Button::new("Delete").shortcut_text("Del")).clicked() {
        if ed.dragging() {
            // As the Delete key: Escape must still be able to take the move back (#863).
            ed.message = "Let go of the move first, then delete.".into();
        } else {
            delete_selected_wires(open, ed);
        }
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
        Tool::Arc | Tool::Dimension => Some(3),
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
        // A cloud is shown as the rectangle it goes around while it is drawn.
        (Tool::Rect | Tool::Cloud, &[a, b]) if a != b => Some(Shape::Rect {
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
        // Two clicks show it on the line measured; the third sets how far off the line goes.
        (Tool::Dimension, &[a, b]) if a != b => Some(Shape::Dimension {
            a,
            b,
            offset: 0.0,
            height: DIMENSION_TEXT_HEIGHT,
        }),
        (Tool::Dimension, &[a, b, at]) if a != b => Some(Shape::Dimension {
            a,
            b,
            offset: dimension_offset(a, b, at),
            height: DIMENSION_TEXT_HEIGHT,
        }),
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

/// Text height of a dimension drawn by hand, in mm.
pub(crate) const DIMENSION_TEXT_HEIGHT: f64 = 2.5;

/// How far `at` lies to the left of the line from `a` to `b` (as seen on paper; negative: to
/// the right), in mm: the offset of a dimension line through `at`.
pub(crate) fn dimension_offset(a: Point, b: Point, at: Point) -> f64 {
    let len = a.distance(b);
    if len <= 0.0 {
        return 0.0;
    }
    // Left normal on a y-down page is (uy, -ux).
    let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
    let off = (at.x - a.x) * uy - (at.y - a.y) * ux;
    (off * 1e6).round() / 1e6
}

/// What to click next, for the status line.
pub(crate) fn shape_hint(tool: Tool, clicks: usize) -> &'static str {
    match (tool, clicks) {
        (Tool::Circle, 0) => "Click the centre of the circle.",
        (Tool::Circle, _) => "Click a point on the circle (Esc cancels).",
        (Tool::Arc, 0) => "Click the centre of the arc.",
        (Tool::Arc, 1) => "Click where the arc starts (Esc cancels).",
        (Tool::Arc, _) => "Click where the arc ends; it runs clockwise (Esc cancels).",
        (Tool::Cloud, 0) => "Click one corner of the area the cloud goes around.",
        (Tool::Cloud, _) => "Click the opposite corner; the cloud gets the latest revision (Esc cancels).",
        (Tool::Dimension, 0) => "Click the first point to measure from.",
        (Tool::Dimension, 1) => "Click the second point to measure to (Esc cancels).",
        (Tool::Dimension, _) => "Click where the dimension line goes (Esc cancels).",
        (Tool::Polyline, 0) => "Click the first point.",
        (Tool::Polyline, _) => {
            "Click the next point; double-click or Enter ends it, a click on the first point closes it (Esc cancels)."
        }
        (_, 0) => "Click the first point.",
        _ => "Click the other end (Esc cancels).",
    }
}

/// A click with the revision table tool: the page's table goes there (moved when it is on
/// the page already) and is selected.
pub(crate) fn revision_table_click(open: &mut Open, ed: &mut Editor, at: Point) {
    match open.place_revision_table(ed.page, at) {
        Ok(sel) => {
            ed.select_only(sel);
            ed.message = "Revision table placed. Drag it to move it; Delete takes it off.".into();
        }
        Err(why) => ed.message = why,
    }
}

/// Put the shape drawn from `ed.shape_points` on the page and select it.
pub(crate) fn finish_shape(open: &mut Open, ed: &mut Editor) {
    let pts = std::mem::take(&mut ed.shape_points);
    if ed.tool == Tool::Cloud {
        if let &[a, b] = pts.as_slice() {
            match open.add_cloud(ed.page, a, b) {
                Ok(sel) => {
                    ed.select_only(sel);
                    ed.message = shape_hint(ed.tool, 0).into();
                }
                Err(why) => ed.message = why,
            }
        }
        return;
    }
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

/// A drawing or wire started on another page does not go on this one: its clicks are dropped
/// when the page shown changes (by id, so the same page moved in the navigator keeps them).
pub(crate) fn drop_draft_of_other_page(open: &Open, ed: &mut Editor) {
    let page_id = open.project.pages.get(ed.page).map(|p| p.id.as_str());
    if !ed.shape_points.is_empty() && ed.shape_page.as_deref() != page_id {
        ed.shape_points.clear();
        ed.message = shape_hint(ed.tool, 0).into();
    }
    // A wire started on the page left behind would end here from a pin or point of that page.
    if ed.wire.is_some() && ed.wire_page.is_some() && ed.wire_page.as_deref() != page_id {
        ed.wire = None;
        ed.wire_page = None;
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
    // A dimension measures between two different points.
    if ed.tool == Tool::Dimension && ed.shape_points.len() == 1 && ed.shape_points[0] == at {
        ed.message = "Click a point away from the first to measure to (Esc cancels).".into();
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

/// How the status after R or M ends.
const TURN_STATUS_END: &str = ", R turns again, M mirrors.";
/// What the status after placing a turned symbol says about the turn staying on.
const TURN_KEPT_STATUS: &str = "The next ones are too until you change it with R or M";

/// R (`mirror` false) or M (`mirror` true) with the place tool: turn the symbol on the pointer
/// a quarter clockwise, or mirror it, for this and the next symbols placed.
pub fn turn_place(ed: &mut Editor, mirror: bool) {
    let (rotation, mirrored) = &mut ed.place_turn;
    if mirror {
        *mirrored = !*mirrored;
    } else {
        *rotation = Rotation::from_degrees(rotation.degrees() + 90).unwrap_or_default();
    }
    let turn = turn_words(*rotation, *mirrored).unwrap_or_else(|| "not turned".into());
    let mut first = turn.chars();
    let turn: String = first
        .next()
        .map(|c| c.to_ascii_uppercase())
        .into_iter()
        .chain(first)
        .collect();
    ed.message = format!("{turn}{TURN_STATUS_END}");
}

/// How a turn reads in a status message ("turned 90° and mirrored"); `None` when not turned.
fn turn_words(rotation: Rotation, mirrored: bool) -> Option<String> {
    match (rotation.degrees(), mirrored) {
        (0, false) => None,
        (0, true) => Some("mirrored".into()),
        (d, false) => Some(format!("turned {d}°")),
        (d, true) => Some(format!("turned {d}° and mirrored")),
    }
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
            paint_shape(painter, xf, &g.placed(at, ed.place_turn.0, ed.place_turn.1), stroke);
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
        let (rotation, mirror) = ed.place_turn;
        match open.place_turned(ed.page, &id, at, rotation, mirror) {
            Ok(sid) => {
                let wired = auto_wire(
                    open,
                    ed,
                    resp.ctx.input(|i| i.modifiers.alt),
                    std::iter::once(sid.clone()).collect(),
                );
                ed.select_only(Selection::Symbol(sid));
                let turn = turn_words(rotation, mirror)
                    .map(|w| format!(", {w}. {TURN_KEPT_STATUS}"))
                    .unwrap_or_default();
                ed.message = format!("Placed {id}{turn}.{}", wired_note(wired));
            }
            Err(e) => ed.message = format!("Could not place {id}: {e}"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn wire_tool(
    painter: &egui::Painter,
    resp: &egui::Response,
    open: &mut Open,
    ed: &mut Editor,
    page: &Page,
    xf: &Xf,
    shown: &Xf,
    t: &theme::Theme,
    snap_step: f64,
) {
    // The pin under the pointer and the rubber band follow the hover position.
    if let Some(cur) = ed.cursor {
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
    }
    if !resp.double_clicked() && !resp.clicked_by(egui::PointerButton::Primary) {
        return;
    }
    // A click places its point where the button was released (the pointer may have moved on
    // before this frame), in the view the user saw.
    let Some(pos) = primary_release_pos(&resp.ctx).or(resp.interact_pointer_pos()) else {
        return;
    };
    let raw = shown.mm(pos);
    let cur = snap(raw, snap_step);
    let near = open.pin_near(page, raw, PIN_CATCH);
    let end = match &near {
        Some((item, pin, _)) => Endpoint::Pin {
            item: item.clone(),
            pin: pin.clone(),
        },
        None => Endpoint::Point { at: cur },
    };
    // A pin is always on the sheet; a free start, bend or end must be too.
    let off_sheet = near.is_none() && !on_paper(open, ed.page, cur);
    if off_sheet && (resp.double_clicked() || resp.clicked_by(egui::PointerButton::Primary)) {
        ed.message = "Click on the sheet to draw the wire.".into();
        return;
    }
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
                ed.wire_page = Some(page.id.clone());
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
    // A spot clicked twice is one bend: drop bends on the point before them or on the end, so
    // the wire has no zero-length segment (and no junction dot on a plain bend, #1236).
    let mut prev = open.end_pos(page, &start);
    via.retain(|p| {
        let keep = prev != Some(*p);
        prev = Some(*p);
        keep
    });
    if let Some(e) = open.end_pos(page, &end) {
        while via.last() == Some(&e) {
            via.pop();
        }
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
    #[test]
    fn showing_another_page_drops_a_drag_still_held() {
        let from = Point::new(0.0, 0.0);
        let mut ed = Editor {
            drag: Some(Drag::Logo {
                from,
                start: (from, from),
                size: false,
                checkpoint: true,
                step: 0,
            }),
            ..Editor::default()
        };
        assert!(!ed.show_page(0), "the page shown: nothing changes");
        assert!(ed.drag.is_some());
        assert!(ed.show_page(1));
        assert!(ed.drag.is_none(), "the logo drag stays on the page it started on");
    }

    use super::*;
    use crate::state::tests::project;

    /// Click `pts` with `tool` as the canvas does.
    fn draw(open: &mut Open, ed: &mut Editor, tool: Tool, pts: &[(f64, f64)]) {
        ed.tool = tool;
        for &(x, y) in pts {
            shape_click(open, ed, Point::new(x, y));
        }
    }

    /// Zoom in and Zoom out keep the page point at the centre of the visible canvas in place
    /// (#826), and Zoom out from below the lowest zoom does not zoom in.
    #[test]
    fn zoom_keys_keep_the_view_centre() {
        let mut ed = Editor {
            view: View {
                zoom: 2.0,
                offset: Vec2::new(30.0, -40.0),
                fit: false,
                size: Vec2::new(800.0, 600.0),
            },
            ..Default::default()
        };
        let centre = |v: &View| (v.size / 2.0 - v.offset) / v.zoom;
        let before = centre(&ed.view);
        ed.zoom_keys(1.25);
        assert!((ed.view.zoom - 2.5).abs() < 1e-5, "{}", ed.view.zoom);
        assert!(
            (centre(&ed.view) - before).length() < 1e-3,
            "{:?} {before:?}",
            centre(&ed.view)
        );
        ed.zoom_keys(0.8);
        assert!((centre(&ed.view) - before).length() < 1e-3);

        ed.view.zoom = 0.2;
        ed.zoom_keys(0.8);
        assert!(
            (ed.view.zoom - 0.2).abs() < 1e-6,
            "Zoom out zoomed in to {}",
            ed.view.zoom
        );
    }

    #[test]
    fn a_revision_is_added_a_cloud_drawn_and_the_table_placed_moved_and_taken_off() {
        use crate::state::RevisionRow;
        let (_tmp, mut open) = project();
        let mut ed = Editor {
            page: 1,
            ..Editor::default()
        };
        // Without a revision the cloud tool says what to do first and draws nothing.
        draw(&mut open, &mut ed, Tool::Cloud, &[(20.0, 20.0), (60.0, 40.0)]);
        assert!(open.project.pages[1].clouds.is_empty());
        assert!(ed.message.contains("Add a revision first"), "{}", ed.message);

        let name = open.project.meta.name.clone();
        let row = |letter: &str, text: &str| RevisionRow {
            letter: letter.into(),
            date: "2026-11-19".into(),
            text: text.into(),
            ..RevisionRow::default()
        };
        let mark = open.undo_mark();
        open.set_project_props(
            &name,
            &[],
            None,
            Some(&[row("A", "First issue"), row("B", "Pumps added")]),
        )
        .unwrap();
        assert_eq!(open.undo_mark(), mark + 1, "one undo step");
        assert!(
            open.set_project_props(&name, &[], None, Some(&[row("A", ""), row("A", "")]))
                .is_err(),
            "a letter twice is refused"
        );

        // Two clicks draw a cloud with the latest revision, selected.
        let mark = open.undo_mark();
        draw(&mut open, &mut ed, Tool::Cloud, &[(60.0, 40.0), (20.0, 20.0)]);
        let clouds = &open.project.pages[1].clouds;
        assert_eq!(clouds.len(), 1);
        assert_eq!(
            (clouds[0].min, clouds[0].max, clouds[0].revision.as_str()),
            (Point::new(20.0, 20.0), Point::new(60.0, 40.0), "B")
        );
        assert_eq!(ed.selection, [Selection::Cloud(0)]);
        assert_eq!(open.undo_mark(), mark + 1);
        // Picked on its outline, retagged, moved and removed like a drawing.
        let page = open.project.pages[1].clone();
        let c = &page.clouds[0];
        // A point on its revision tag, above the top-right corner.
        let tag = (0..200)
            .map(|k| Point::new(c.max.x, c.min.y - f64::from(k) * 0.1))
            .find(|p| c.hit(*p, 0.2))
            .unwrap();
        assert_eq!(open.hit(&page, tag, 0.2), Some(Selection::Cloud(0)));
        assert_eq!(
            open.hit(&page, Point::new(40.0, 30.0), 0.2),
            None,
            "inside it is not the cloud"
        );
        assert!(open.set_cloud_revision(1, 0, "A"));
        assert!(!open.set_cloud_revision(1, 0, "C"), "no such revision");
        assert_eq!(open.project.pages[1].clouds[0].revision, "A");
        // A revision a cloud carries cannot be removed.
        let rows = RevisionRow::of(&open.project.meta.revisions);
        let err = open.set_project_props(&name, &[], None, Some(&rows[1..])).unwrap_err();
        assert!(err.contains("cloud of revision A"), "{err}");
        open.move_by(1, &[Selection::Cloud(0)], Point::new(5.0, 5.0));
        assert_eq!(open.project.pages[1].clouds[0].min, Point::new(25.0, 25.0));

        // The table goes where the tool clicks, and a second click moves it.
        ed.tool = Tool::RevisionTable;
        revision_table_click(&mut open, &mut ed, Point::new(200.0, 150.0));
        revision_table_click(&mut open, &mut ed, Point::new(210.0, 160.0));
        assert_eq!(open.project.pages[1].revision_table, Some(Point::new(210.0, 160.0)));
        assert_eq!(ed.selection, [Selection::RevisionTable]);
        let page = open.project.pages[1].clone();
        assert_eq!(
            open.hit(&page, Point::new(212.0, 162.0), 1.0),
            Some(Selection::RevisionTable)
        );
        assert!(open.select_all(1).contains(&Selection::RevisionTable));

        // Saved and opened again, both are kept.
        open.save().unwrap();
        let mut again = Open::load(&open.dir).unwrap();
        assert_eq!(again.project.pages[1].clouds, open.project.pages[1].clouds);
        assert_eq!(again.project.pages[1].revision_table, Some(Point::new(210.0, 160.0)));
        assert_eq!(again.project.meta.revisions.len(), 2);

        // Delete takes both off; Undo puts them back.
        again.delete_many(1, &[Selection::Cloud(0), Selection::RevisionTable]);
        assert!(again.project.pages[1].clouds.is_empty() && again.project.pages[1].revision_table.is_none());
        assert!(again.undo());
        assert_eq!(again.project.pages[1].clouds.len(), 1);
        assert!(again.project.pages[1].revision_table.is_some());
    }

    #[test]
    fn a_cloud_with_an_unknown_revision_is_marked_around_its_area() {
        let (_tmp, mut o) = project();
        let cloud = wc_core::RevisionCloud {
            min: Point::new(40.0, 30.0),
            max: Point::new(90.0, 60.0),
            revision: "Z".into(),
        };
        o.project.pages[1].clouds.push(cloud);
        let found = crate::run_check(&o);
        let f = found.iter().find(|f| f.message.contains("revision Z")).unwrap();
        let page = &o.project.pages[1];
        assert_eq!(f.page_id.as_deref(), Some(page.id.as_str()));
        let spot = f.item.as_deref().and_then(|item| live_spot(&o, page, item));
        assert_eq!(spot, Some((Point::new(40.0, 30.0), Point::new(90.0, 60.0))));
        assert_eq!(live_spot(&o, page, "cloud 7"), None, "no such cloud");
    }

    #[test]
    fn r_and_m_turn_and_mirror_the_symbol_before_it_is_placed() {
        let (_tmp, mut open) = project();
        let mut ed = Editor {
            tool: Tool::Place,
            place_symbol: Some("wc.coil".into()),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        painted(&ctx, &mut open, &mut ed, 0.5, vec![]);
        // R twice and M while the symbol hangs on the pointer.
        turn_place(&mut ed, false);
        turn_place(&mut ed, false);
        turn_place(&mut ed, true);
        assert_eq!(ed.place_turn, (Rotation::R180, true));
        assert_eq!(ed.message, "Turned 180° and mirrored, R turns again, M mirrors.");
        let steps = |o: &Open| {
            o.project.pages[0]
                .symbols
                .iter()
                .map(|s| (s.rotation, s.mirror))
                .collect::<Vec<_>>()
        };
        click_at(&ctx, &mut open, &mut ed, egui::pos2(400.0, 300.0), 1.0);
        assert_eq!(steps(&open), [(Rotation::R180, true)]);
        // The status says the turn is still on for the next symbol.
        assert!(
            ed.message.starts_with(
                "Placed wc.coil, turned 180° and mirrored. The next ones are too until you change it with R or M."
            ),
            "{}",
            ed.message
        );
        // One undo step takes the placed symbol back off.
        assert!(open.undo());
        assert!(open.project.pages[0].symbols.is_empty());
        // The turn stays for the next symbols placed with the tool.
        click_at(&ctx, &mut open, &mut ed, egui::pos2(400.0, 300.0), 2.0);
        turn_place(&mut ed, false);
        click_at(&ctx, &mut open, &mut ed, egui::pos2(500.0, 300.0), 3.0);
        assert_eq!(steps(&open), [(Rotation::R180, true), (Rotation::R270, true)]);
        // Another tool sets it back.
        ed.tool = Tool::Select;
        painted(&ctx, &mut open, &mut ed, 4.0, vec![]);
        assert_eq!(ed.place_turn, (Rotation::R0, false));
        // Also while a panel is shown: the panel view returns early, the reset still runs.
        ed.tool = Tool::Place;
        turn_place(&mut ed, false);
        ed.tool = Tool::Select;
        ed.panel = Some(0);
        painted(&ctx, &mut open, &mut ed, 5.0, vec![]);
        assert_eq!(ed.place_turn, (Rotation::R0, false));
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
                egui::CentralPanel::default().show(ctx, |ui| show(ui, open, ed, false, 1.0, true));
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
    fn the_black_box_tool_takes_a_drag_edge_clicks_and_a_click_inside_through_egui_events() {
        let (_tmp, mut open) = project();
        let mut ed = Editor {
            tool: Tool::BlackBox,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let (a, b) = (egui::pos2(300.0, 200.0), egui::pos2(500.0, 350.0));
        // A first frame lays the canvas out, so the pointer is over it from then on.
        painted(&ctx, &mut open, &mut ed, 0.5, vec![]);
        painted(&ctx, &mut open, &mut ed, 1.0, vec![egui::Event::PointerMoved(a)]);
        let corner = ed.cursor.unwrap();
        painted(&ctx, &mut open, &mut ed, 1.01, vec![button(a, true)]);
        // One move all the way: the drag is seen only now, with the pointer already at `b`.
        painted(&ctx, &mut open, &mut ed, 1.05, vec![egui::Event::PointerMoved(b)]);
        let other = ed.cursor.unwrap();
        painted(&ctx, &mut open, &mut ed, 1.1, vec![button(b, false)]);
        let d = ed.black_box.clone().unwrap();
        assert_eq!(d.from, corner, "the drag's corner is where the button went down");
        assert_eq!(d.to, Some(other));
        assert!(ed.ask_box_pin.is_none() && !ed.ask_box_tag);

        // A click on the top edge asks for a connection point there.
        click_at(&ctx, &mut open, &mut ed, egui::pos2(400.0, 200.0), 2.0);
        let Some((side, offset)) = ed.ask_box_pin.take() else {
            unreachable!("a click on the edge asks for a point")
        };
        assert_eq!(side, wc_core::BoxSide::Top);
        assert!(offset > 0.0 && offset < other.x - corner.x, "{offset}");
        ed.black_box.as_mut().unwrap().add_pin(side, offset, "1").unwrap();
        // A click inside asks for the tag; the page is not changed by any of it.
        click_at(&ctx, &mut open, &mut ed, egui::pos2(400.0, 280.0), 3.0);
        assert!(ed.ask_box_tag);
        assert!(open.project.pages[0].symbols.is_empty());

        // On a touch screen the release comes with PointerGone in the same frame, which clears
        // the hover position: a tap still starts a box and a second one finishes it.
        ed.ask_box_tag = false;
        ed.black_box = None;
        let tap = |open: &mut Open, ed: &mut Editor, pos: egui::Pos2, t: f64| {
            painted(
                &ctx,
                open,
                ed,
                t,
                vec![egui::Event::PointerMoved(pos), button(pos, true)],
            );
            painted(
                &ctx,
                open,
                ed,
                t + 0.02,
                vec![button(pos, false), egui::Event::PointerGone],
            );
        };
        tap(&mut open, &mut ed, a, 5.0);
        assert!(ed.cursor.is_none(), "the hover position is gone after the tap");
        assert_eq!(ed.black_box.as_ref().map(|d| d.from), Some(corner));
        tap(&mut open, &mut ed, b, 6.0);
        assert_eq!(ed.black_box.as_ref().and_then(|d| d.to), Some(other));

        // Released at `b`, then moved and zoomed in the same frame: the corner is where `b` was
        // in the view the user saw.
        ed.black_box = None;
        click_at(&ctx, &mut open, &mut ed, a, 7.0);
        let zoom = ed.view.zoom;
        painted(
            &ctx,
            &mut open,
            &mut ed,
            8.0,
            vec![egui::Event::PointerMoved(b), button(b, true)],
        );
        painted(
            &ctx,
            &mut open,
            &mut ed,
            8.01,
            // Moved on to `a` before the zoom, so the zoom is about `a` and moves `b`.
            vec![button(b, false), egui::Event::PointerMoved(a), egui::Event::Zoom(2.0)],
        );
        assert!(ed.view.zoom > zoom, "the view zoomed");
        assert_eq!(ed.black_box.as_ref().and_then(|d| d.to), Some(other));

        // A click near an edge, released as the view zooms in: it is judged at the zoom the
        // user saw, so it still lands on the edge (at the new zoom it would be inside).
        ed.view = View {
            zoom: 1.0,
            offset: Vec2::ZERO,
            fit: false,
            ..Default::default()
        };
        painted(
            &ctx,
            &mut open,
            &mut ed,
            8.5,
            vec![egui::Event::PointerMoved(egui::pos2(100.0, 100.0))],
        );
        let seen = ed.cursor.unwrap();
        let origin = egui::pos2(100.0 - seen.x as f32, 100.0 - seen.y as f32);
        let mut d = crate::black_box_tool::BoxDraft::new(&open.project.pages[0].id, Point::new(50.0, 50.0));
        d.to = Some(Point::new(150.0, 150.0));
        ed.black_box = Some(d);
        let near_top = origin + Vec2::new(100.0, 54.0);
        painted(
            &ctx,
            &mut open,
            &mut ed,
            8.6,
            vec![egui::Event::PointerMoved(near_top), button(near_top, true)],
        );
        painted(
            &ctx,
            &mut open,
            &mut ed,
            8.62,
            vec![
                button(near_top, false),
                egui::Event::PointerMoved(origin + Vec2::new(400.0, 400.0)),
                egui::Event::Zoom(2.0),
            ],
        );
        assert!(ed.view.zoom > 1.5, "the view zoomed");
        assert!(!ed.ask_box_tag, "not taken for a click inside");
        assert_eq!(ed.ask_box_pin.take().map(|(side, _)| side), Some(wc_core::BoxSide::Top));

        // Another tool drops the box being drawn.
        ed.ask_box_tag = false;
        ed.tool = Tool::Line;
        painted(&ctx, &mut open, &mut ed, 9.0, vec![]);
        assert!(ed.black_box.is_none());
    }

    #[test]
    fn a_small_selected_black_box_keeps_a_middle_part_that_moves_it() {
        let (_tmp, mut open) = project();
        let id = open
            .add_black_box(0, Point::new(40.0, 40.0), Point::new(50.0, 50.0), &[], None)
            .unwrap();
        let mut ed = Editor {
            selection: vec![Selection::Symbol(id.clone())],
            ..Default::default()
        };
        let page = open.project.pages[0].clone();
        // The reach at zoom 1 (6 mm) would cover the whole 10 mm box: a quarter of it is kept.
        let drag = drag_start(&open, &mut ed, &page, Point::new(42.0, 43.0), 6.0, 1.0, false, false);
        assert!(matches!(drag, Drag::Move { .. }), "{drag:?}");
        let drag = drag_start(&open, &mut ed, &page, Point::new(41.0, 41.0), 6.0, 1.0, false, false);
        assert!(
            matches!(
                drag,
                Drag::BoxSize {
                    corner: (false, false),
                    ..
                }
            ),
            "{drag:?}"
        );
    }

    /// A black box from (40, 40) to (60, 50) with `pins`, selected, on a canvas at zoom 1
    /// painted once: the project, the box's id, the editor, the context and where the page
    /// origin is on the screen.
    fn selected_box(pins: &[wc_core::BoxPin]) -> (tempfile::TempDir, Open, String, Editor, egui::Context, egui::Pos2) {
        let (tmp, mut open) = project();
        let id = open
            .add_black_box(0, Point::new(40.0, 40.0), Point::new(60.0, 50.0), pins, None)
            .unwrap();
        let mut ed = Editor {
            selection: vec![Selection::Symbol(id.clone())],
            view: View {
                zoom: 1.0,
                offset: Vec2::ZERO,
                fit: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        // Where the page origin is on the screen, from the pointer's place on the page.
        painted(&ctx, &mut open, &mut ed, 0.5, vec![]);
        painted(
            &ctx,
            &mut open,
            &mut ed,
            1.0,
            vec![egui::Event::PointerMoved(egui::pos2(300.0, 300.0))],
        );
        let seen = ed.cursor.unwrap();
        let origin = egui::pos2(300.0 - seen.x as f32, 300.0 - seen.y as f32);
        (tmp, open, id, ed, ctx, origin)
    }

    /// The symbol that item `id` on the first page places.
    fn box_symbol(o: &Open, id: &str) -> wc_core::Symbol {
        o.symbols[&o.project.pages[0].symbol(id).unwrap().symbol].clone()
    }

    #[test]
    fn a_selected_black_box_is_sized_from_its_near_corner_through_egui_events() {
        let pin = |name: &str, side, offset| wc_core::BoxPin {
            name: name.into(),
            side,
            offset,
        };
        let pins = [
            pin("1", wc_core::BoxSide::Top, 15.0),
            pin("2", wc_core::BoxSide::Right, 5.0),
        ];
        let (_tmp, mut open, id, mut ed, ctx, origin) = selected_box(&pins);
        // A wire on the right point: that edge does not move, so the point keeps its place on
        // the page and there is no warning.
        open.add_wire(
            0,
            Endpoint::Pin {
                item: id.clone(),
                pin: "2".into(),
            },
            Endpoint::Point {
                at: Point::new(80.0, 45.0),
            },
            vec![],
        );
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let at = |x: f32, y: f32| origin + Vec2::new(x, y);
        let symbol = |o: &Open| box_symbol(o, &id);
        let undo_steps = open.undo_steps();

        // Each of the four corners can be grabbed.
        for (x, y, corner) in [
            (60.0, 50.0, (true, true)),
            (40.0, 50.0, (false, true)),
            (60.0, 40.0, (true, false)),
            (40.0, 40.0, (false, false)),
        ] {
            let held = Point::new(x, y);
            let Drag::BoxSize { corner: got, .. } = drag_start(
                &open,
                &mut ed,
                &open.project.pages[0].clone(),
                held,
                1.0,
                1.0,
                false,
                false,
            ) else {
                unreachable!("a sizing drag at {held:?}")
            };
            assert_eq!(got, corner, "{held:?}");
        }

        // Drag the top-left corner up and left: bigger, moved, the bottom-right corner in place.
        let (from, to) = (at(40.0, 40.0), at(35.0, 36.0));
        painted(
            &ctx,
            &mut open,
            &mut ed,
            2.0,
            vec![egui::Event::PointerMoved(from), button(from, true)],
        );
        painted(
            &ctx,
            &mut open,
            &mut ed,
            2.02,
            vec![egui::Event::PointerMoved(at(37.0, 38.0))],
        );
        painted(&ctx, &mut open, &mut ed, 2.04, vec![egui::Event::PointerMoved(to)]);
        painted(&ctx, &mut open, &mut ed, 2.06, vec![button(to, false)]);
        let Some((w, h, now)) = wc_core::black_box_layout(&symbol(&open)) else {
            unreachable!("still a black box")
        };
        assert_eq!((w, h), (25.0, 14.0), "{}", ed.message);
        assert_eq!(open.project.pages[0].symbol(&id).unwrap().at, Point::new(35.0, 36.0));
        assert_eq!(
            now,
            vec![
                pin("1", wc_core::BoxSide::Top, 20.0),
                pin("2", wc_core::BoxSide::Right, 9.0)
            ],
            "the points keep their place on the page"
        );
        assert_eq!(open.undo_steps(), undo_steps + 1, "one undo step");
        assert!(ed.message.contains("25 x 14 mm"), "{}", ed.message);
        assert!(!ed.message.contains("Check"), "{}", ed.message);

        // One Undo takes back both the size and the move.
        assert!(open.undo());
        let Some((w, h, _)) = wc_core::black_box_layout(&symbol(&open)) else {
            unreachable!("still a black box")
        };
        assert_eq!((w, h), (20.0, 10.0));
        assert_eq!(open.project.pages[0].symbol(&id).unwrap().at, Point::new(40.0, 40.0));
    }

    #[test]
    fn a_selected_black_box_is_sized_by_dragging_its_corner_through_egui_events() {
        let pin = |name: &str, side, offset| wc_core::BoxPin {
            name: name.into(),
            side,
            offset,
        };
        let pins = [
            pin("1", wc_core::BoxSide::Top, 15.0),
            pin("2", wc_core::BoxSide::Right, 5.0),
        ];
        let (_tmp, mut open, id, mut ed, ctx, origin) = selected_box(&pins);
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let at = |x: f32, y: f32| origin + Vec2::new(x, y);
        let symbol = |o: &Open| box_symbol(o, &id);
        let undo_steps = open.undo_steps();
        let wires = open.project.pages[0].wires.clone();

        // Grab the corner, drag in two moves, release: one change, at the size let go at.
        let drag_to = |o: &mut Open, ed: &mut Editor, from: egui::Pos2, to: egui::Pos2, t: f64| {
            painted(
                &ctx,
                o,
                ed,
                t,
                vec![egui::Event::PointerMoved(from), button(from, true)],
            );
            let mid = from + (to - from) / 2.0;
            painted(&ctx, o, ed, t + 0.02, vec![egui::Event::PointerMoved(mid)]);
            painted(&ctx, o, ed, t + 0.04, vec![egui::Event::PointerMoved(to)]);
            painted(&ctx, o, ed, t + 0.06, vec![button(to, false)]);
        };
        drag_to(&mut open, &mut ed, at(60.0, 50.0), at(70.0, 58.0), 2.0);
        let Some((w, h, now)) = wc_core::black_box_layout(&symbol(&open)) else {
            unreachable!("still a black box")
        };
        assert_eq!((w, h), (30.0, 18.0), "{}", ed.message);
        assert_eq!(now, pins.to_vec(), "the points stay where they were on their edges");
        assert_eq!(open.undo_steps(), undo_steps + 1, "one undo step");
        assert_eq!(
            open.project.pages[0].symbol(&id).unwrap().at,
            Point::new(40.0, 40.0),
            "not moved"
        );
        assert!(ed.message.contains("30 x 18 mm"), "{}", ed.message);

        // Escape while the corner is held: the release afterwards changes nothing (#737).
        let held = symbol(&open);
        let steps = open.undo_steps();
        painted(
            &ctx,
            &mut open,
            &mut ed,
            2.5,
            vec![egui::Event::PointerMoved(at(70.0, 58.0)), button(at(70.0, 58.0), true)],
        );
        painted(
            &ctx,
            &mut open,
            &mut ed,
            2.52,
            vec![egui::Event::PointerMoved(at(80.0, 66.0))],
        );
        assert!(matches!(ed.drag, Some(Drag::BoxSize { .. })));
        ed.cancel_drag = true;
        painted(
            &ctx,
            &mut open,
            &mut ed,
            2.54,
            vec![egui::Event::PointerMoved(at(82.0, 68.0))],
        );
        assert!(ed.drag.is_none());
        painted(&ctx, &mut open, &mut ed, 2.56, vec![button(at(82.0, 68.0), false)]);
        assert_eq!(symbol(&open), held, "not sized");
        assert_eq!(open.undo_steps(), steps, "no undo step");

        // Smaller than its points: they are kept on their edges, at the ends.
        drag_to(&mut open, &mut ed, at(70.0, 58.0), at(45.0, 45.0), 3.0);
        let Some((w, h, now)) = wc_core::black_box_layout(&symbol(&open)) else {
            unreachable!("still a black box")
        };
        assert_eq!((w, h), (5.0, 5.0));
        assert_eq!(
            now,
            vec![
                pin("1", wc_core::BoxSide::Top, 5.0),
                pin("2", wc_core::BoxSide::Right, 5.0)
            ]
        );
        assert_eq!(open.undo_steps(), undo_steps + 2);
        assert_eq!(open.project.pages[0].wires, wires);
        assert!(open.undo());
        let Some((w, h, _)) = wc_core::black_box_layout(&symbol(&open)) else {
            unreachable!("still a black box")
        };
        assert_eq!((w, h), (30.0, 18.0), "Undo takes the size back");

        // A drag that starts away from the corner moves the box instead of sizing it.
        drag_to(&mut open, &mut ed, at(50.0, 45.0), at(60.0, 45.0), 4.0);
        assert_eq!(open.project.pages[0].symbol(&id).unwrap().at, Point::new(50.0, 40.0));
        let Some((w, h, _)) = wc_core::black_box_layout(&symbol(&open)) else {
            unreachable!("still a black box")
        };
        assert_eq!((w, h), (30.0, 18.0));

        // On a page someone else holds, the corner sizes nothing.
        let held = open.project.pages[0].id.clone();
        ed.held_by.insert(held, "Ola".into());
        let steps = open.undo_steps();
        drag_to(&mut open, &mut ed, at(80.0, 58.0), at(90.0, 70.0), 5.0);
        assert_eq!(open.undo_steps(), steps);
        let Some((w, h, _)) = wc_core::black_box_layout(&symbol(&open)) else {
            unreachable!("still a black box")
        };
        assert_eq!((w, h), (30.0, 18.0));
        ed.held_by.clear();
        // The selection box drawn there picked nothing: select the box again.
        ed.select_only(Selection::Symbol(id.clone()));

        // A last move that comes in the same frame as the release counts.
        let (corner, end) = (at(80.0, 58.0), at(90.0, 62.0));
        painted(
            &ctx,
            &mut open,
            &mut ed,
            6.0,
            vec![egui::Event::PointerMoved(corner), button(corner, true)],
        );
        painted(
            &ctx,
            &mut open,
            &mut ed,
            6.02,
            vec![egui::Event::PointerMoved(at(88.0, 58.0))],
        );
        painted(
            &ctx,
            &mut open,
            &mut ed,
            6.04,
            vec![egui::Event::PointerMoved(end), button(end, false)],
        );
        let Some((w, h, _)) = wc_core::black_box_layout(&symbol(&open)) else {
            unreachable!("still a black box")
        };
        assert_eq!((w, h), (40.0, 22.0), "{}", ed.message);

        // The box is deleted while its corner is held: another box that takes its place among
        // the page's items is not sized on release.
        let other = open
            .add_black_box(0, Point::new(120.0, 40.0), Point::new(140.0, 50.0), &pins, None)
            .unwrap();
        ed.select_only(Selection::Symbol(id.clone()));
        let corner = at(90.0, 62.0);
        painted(
            &ctx,
            &mut open,
            &mut ed,
            7.0,
            vec![egui::Event::PointerMoved(corner), button(corner, true)],
        );
        painted(
            &ctx,
            &mut open,
            &mut ed,
            7.02,
            vec![egui::Event::PointerMoved(at(95.0, 66.0))],
        );
        assert!(matches!(ed.drag, Some(Drag::BoxSize { .. })));
        open.delete_many(0, &[Selection::Symbol(id.clone())]);
        let before = open.symbols[&open.project.pages[0].symbol(&other).unwrap().symbol].clone();
        let steps = open.undo_steps();
        painted(&ctx, &mut open, &mut ed, 7.04, vec![button(at(95.0, 66.0), false)]);
        assert_eq!(open.undo_steps(), steps, "nothing changed");
        assert_eq!(
            open.symbols[&open.project.pages[0].symbol(&other).unwrap().symbol],
            before
        );
        assert!(ed.message.contains("no longer on this page"), "{}", ed.message);
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
                egui::CentralPanel::default().show(ctx, |ui| show(ui, open, ed, false, 1.0, true));
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
    fn a_dimension_is_drawn_in_three_clicks_and_edited_like_any_drawing() {
        let (_tmp, mut open) = project();
        let mut ed = Editor::default();
        let before = open.project.pages[0].graphics.len();
        // A second click on the first point is refused; the first stays.
        draw(&mut open, &mut ed, Tool::Dimension, &[(40.0, 120.0), (40.0, 120.0)]);
        assert_eq!(ed.shape_points.len(), 1);
        assert!(ed.message.contains("away from the first"), "{}", ed.message);
        draw(&mut open, &mut ed, Tool::Dimension, &[(65.0, 120.0), (50.0, 110.0)]);
        assert!(ed.shape_points.is_empty());
        let page = &open.project.pages[0];
        assert_eq!(page.graphics.len(), before + 1);
        let i = page.graphics.len() - 1;
        let g = page.graphics[i].clone();
        assert_eq!(
            g,
            Shape::Dimension {
                a: Point::new(40.0, 120.0),
                b: Point::new(65.0, 120.0),
                offset: 10.0,
                height: DIMENSION_TEXT_HEIGHT,
            }
        );
        assert_eq!(
            ed.selection,
            vec![Selection::Graphic(i)],
            "the new dimension is selected"
        );
        // Picked on its dimension line, not far from it.
        assert!(crate::edit::graphic_hit(&g, Point::new(52.0, 110.0), 0.5));
        assert!(!crate::edit::graphic_hit(&g, Point::new(52.0, 130.0), 0.5));
        let (min, max) = crate::edit::graphic_bounds(&g);
        assert!(min.x <= 40.0 && max.x >= 65.0 && min.y < 110.0 && max.y > 115.0);
        // Moved, then deleted, each one undo step.
        open.checkpoint();
        open.move_by(0, &[Selection::Graphic(i)], Point::new(5.0, 0.0));
        assert!(
            matches!(open.project.pages[0].graphics[i], Shape::Dimension { a, .. } if a == Point::new(45.0, 120.0))
        );
        open.delete_many(0, &[Selection::Graphic(i)]);
        assert_eq!(open.project.pages[0].graphics.len(), before);
        assert!(open.undo());
        assert!(
            matches!(open.project.pages[0].graphics[i], Shape::Dimension { a, .. } if a == Point::new(45.0, 120.0))
        );
        assert!(open.undo());
        assert_eq!(open.project.pages[0].graphics[i], g);
        // Kept in the page file.
        open.save().unwrap();
        let saved = std::fs::read_dir(open.dir.join("pages"))
            .unwrap()
            .filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
            .any(|text| text.contains("\"kind\": \"dimension\"") || text.contains("\"kind\":\"dimension\""));
        assert!(saved, "a page file holds the dimension");
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
        let Some(tris) = triangulate(&pts) else {
            unreachable!("a small outline is cut up")
        };
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
            .unwrap_or_default()
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
        let Some(tris) = triangulate(&pts) else {
            unreachable!("a small outline is cut up")
        };
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

    #[test]
    fn a_filled_outline_is_cut_into_triangles_once_until_it_changes() {
        let ell = |w: f64| Shape::Polyline {
            points: [(0.0, 0.0), (w, 0.0), (w, 1.0), (1.0, 1.0), (1.0, 2.0), (0.0, 2.0)]
                .iter()
                .map(|(x, y)| Point::new(*x, *y))
                .collect(),
            closed: true,
            filled: true,
        };
        let ctx = egui::Context::default();
        let xf = Xf {
            origin: Pos2::ZERO,
            zoom: 4.0,
        };
        let paint = |s: &Shape| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                let p = ctx.layer_painter(egui::LayerId::background());
                paint_shape(&p, &xf, s, Stroke::new(1.0_f32, Color32::BLACK));
            });
        };
        let runs = || TRIANGULATIONS.with(|n| n.get());
        let start = runs();
        paint(&ell(2.0));
        paint(&ell(2.0));
        paint(&ell(2.0));
        assert_eq!(runs() - start, 1, "an unchanged fill reuses its triangles");
        paint(&ell(3.0));
        assert_eq!(runs() - start, 2, "a changed fill is cut up again");
        paint(&ell(3.0));
        assert_eq!(runs() - start, 2);
    }

    /// A closed zigzag round a circle of radius 100: every other corner pointing in, to radius
    /// `inner`, so it is not convex.
    fn star_to(n: usize, inner: f64) -> Vec<Point> {
        (0..n)
            .map(|i| {
                let t = std::f64::consts::TAU * i as f64 / n as f64;
                let r = if i % 2 == 0 { 100.0 } else { inner };
                Point::new(r * t.cos(), r * t.sin())
            })
            .collect()
    }

    fn star(n: usize) -> Vec<Point> {
        star_to(n, 99.0)
    }

    fn screen(pts: &[Point]) -> Vec<Pos2> {
        pts.iter().map(|q| Pos2::new(q.x as f32, q.y as f32)).collect()
    }

    /// The area the triangles cover and the outline's own area (shoelace), both in f64.
    fn areas(pts: &[Pos2], tris: &[[u32; 3]]) -> (f64, f64) {
        let at = |i: u32| (f64::from(pts[i as usize].x), f64::from(pts[i as usize].y));
        let covered = tris
            .iter()
            .map(|&[a, b, c]| {
                let (a, b, c) = (at(a), at(b), at(c));
                ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs() / 2.0
            })
            .sum();
        let outline = pts
            .iter()
            .zip(pts.iter().cycle().skip(1))
            .map(|(a, b)| f64::from(a.x) * f64::from(b.y) - f64::from(a.y) * f64::from(b.x))
            .sum::<f64>()
            .abs()
            / 2.0;
        (covered, outline)
    }

    #[test]
    fn a_fill_of_a_few_thousand_points_is_still_cut_into_triangles() {
        let pts = screen(&star(2_000));
        let Some(tris) = triangulate(&pts) else {
            unreachable!("2000 points fit in the budget")
        };
        assert_eq!(tris.len(), pts.len() - 2);
    }

    #[test]
    fn a_fill_of_twenty_thousand_points_is_cut_into_triangles_covering_it() {
        let pts = screen(&star(20_000));
        let started = std::time::Instant::now();
        let Some(tris) = triangulate(&pts) else {
            unreachable!("20 000 points fit in the budget")
        };
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(tris.len(), pts.len() - 2);
        let (covered, outline) = areas(&pts, &tris);
        assert!((covered - outline).abs() < outline * 1e-4, "{covered} vs {outline}");
    }

    #[test]
    fn a_fill_of_a_hundred_thousand_points_is_filled_once_without_stalling() {
        let shape = Shape::Polyline {
            points: star(100_000),
            closed: true,
            filled: true,
        };
        let ctx = egui::Context::default();
        let xf = Xf {
            origin: Pos2::new(200.0, 200.0),
            zoom: 1.0,
        };
        let paint = || {
            let out = ctx.run(egui::RawInput::default(), |ctx| {
                let p = ctx.layer_painter(egui::LayerId::background());
                paint_shape(&p, &xf, &shape, Stroke::new(1.0_f32, Color32::BLACK));
            });
            out.shapes
                .iter()
                .any(|c| matches!(&c.shape, EShape::Mesh(m) if m.indices.len() == 3 * (100_000 - 2)))
        };
        let runs = || TRIANGULATIONS.with(|n| n.get());
        let start = runs();
        let started = std::time::Instant::now();
        assert!(paint(), "the shape is filled");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        assert!(paint(), "and filled on the next frame too");
        assert_eq!(runs() - start, 1, "from the cached triangles");
    }

    #[test]
    fn a_fill_too_costly_to_cut_up_is_drawn_as_its_outline_without_stalling() {
        // Every inward corner piled up next to the centre, under the box of every ear: each
        // ear test has to look at all of them, so this runs out of budget.
        let shape = Shape::Polyline {
            points: star_to(100_000, 0.001),
            closed: true,
            filled: true,
        };
        let ctx = egui::Context::default();
        let xf = Xf {
            origin: Pos2::new(200.0, 200.0),
            zoom: 1.0,
        };
        let paint = || {
            let out = ctx.run(egui::RawInput::default(), |ctx| {
                let p = ctx.layer_painter(egui::LayerId::background());
                paint_shape(&p, &xf, &shape, Stroke::new(1.0_f32, Color32::BLACK));
            });
            out.shapes
                .iter()
                .any(|c| matches!(&c.shape, EShape::Path(path) if path.closed && path.points.len() == 100_000))
        };
        let runs = || TRIANGULATIONS.with(|n| n.get());
        let start = runs();
        let started = std::time::Instant::now();
        assert!(paint(), "the outline is painted");
        // Unbounded, this takes minutes; the budget keeps it to a fraction of that even in a
        // debug build.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        assert!(paint(), "the outline is painted on the next frame too");
        assert!(paint());
        assert_eq!(
            runs() - start,
            1,
            "a fill too costly to cut up is not tried again every frame"
        );
    }

    #[test]
    fn a_clockwise_outline_far_from_the_origin_fills_the_outline_not_its_notch() {
        // A clockwise L near (10000, 10000) mm: the notch is the square 10001..10002 on both axes.
        let pts: Vec<Point> = [
            (10000.0, 10002.0),
            (10001.0, 10002.0),
            (10001.0, 10001.0),
            (10002.0, 10001.0),
            (10002.0, 10000.0),
            (10000.0, 10000.0),
        ]
        .iter()
        .map(|(x, y)| Point::new(*x, *y))
        .collect();
        let Some(tris) = egui::util::cache::ComputerMut::compute(&mut Triangulator, FillKey(&pts)) else {
            unreachable!("a small outline is cut up")
        };
        let at = |i: u32| {
            let q = pts[i as usize];
            (q.x - 10000.0, q.y - 10000.0)
        };
        let inside = |(x, y): (f64, f64), &[a, b, c]: &[u32; 3]| {
            let (a, b, c) = (at(a), at(b), at(c));
            let s = |p: (f64, f64), r: (f64, f64)| (r.0 - p.0) * (y - p.1) - (r.1 - p.1) * (x - p.0);
            let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
            (d1 > 0.0 && d2 > 0.0 && d3 > 0.0) || (d1 < 0.0 && d2 < 0.0 && d3 < 0.0)
        };
        for q in [(0.37, 0.71), (1.63, 0.29), (0.29, 1.63)] {
            assert!(tris.iter().any(|t| inside(q, t)), "{q:?} is filled");
        }
        assert!(!tris.iter().any(|t| inside((1.43, 1.61), t)), "the notch is not filled");
        let area: f64 = tris
            .iter()
            .map(|&[a, b, c]| {
                let (a, b, c) = (at(a), at(b), at(c));
                ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs() / 2.0
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-6, "covers the L exactly: {area}");
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
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut o, &mut ed, false, 1.0, true));
        });
        assert!(ed.selection.is_empty());
        assert_eq!(ed.tool, Tool::Select);
        assert!(ed.message.contains("Drawings and notes"), "{}", ed.message);
        assert_eq!(o.project.pages[0], page, "the page is unchanged");
    }

    #[test]
    fn a_selected_cloud_and_revision_table_are_painted_in_the_selection_colour() {
        let (_tmp, mut o) = project();
        o.project.add_revision("A", "2026-11-19", "First issue", "").unwrap();
        o.add_cloud(1, Point::new(20.0, 20.0), Point::new(60.0, 40.0)).unwrap();
        o.place_revision_table(1, Point::new(200.0, 150.0)).unwrap();
        let mut ed = Editor {
            page: 1,
            ..Editor::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let colour = crate::theme::Theme::of(&ctx).selection;
        // How many lines of one frame are drawn in the selection colour.
        let selected_lines = |o: &mut Open, ed: &mut Editor| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(ui, o, ed, false, 1.0, true));
            });
            out.shapes
                .iter()
                .filter(|c| match &c.shape {
                    EShape::Path(p) => p.stroke.color == egui::epaint::ColorMode::Solid(colour),
                    EShape::LineSegment { stroke, .. } => stroke.color == colour,
                    _ => false,
                })
                .count()
        };
        assert_eq!(selected_lines(&mut o, &mut ed), 0, "nothing selected");
        ed.selection = vec![Selection::Cloud(0)];
        let cloud = selected_lines(&mut o, &mut ed);
        assert!(cloud > 0, "the selected cloud");
        ed.selection = vec![Selection::RevisionTable];
        let table = selected_lines(&mut o, &mut ed);
        assert!(table > 0, "the selected table");
        ed.selection = vec![Selection::Cloud(0), Selection::RevisionTable];
        assert_eq!(selected_lines(&mut o, &mut ed), cloud + table, "each in its own colour");
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
            egui::CentralPanel::default().show(ctx, |ui| show(ui, o, ed, false, 1.0, true));
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

    #[test]
    fn the_wire_tool_takes_no_start_bend_or_free_end_outside_the_sheet() {
        let (_tmp, mut o) = project();
        let mut ed = Editor {
            tool: Tool::Wire,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        painted(&ctx, &mut o, &mut ed, 0.5, vec![]);
        // Pan the sheet right and down so the top-left of the canvas is off the paper.
        ed.view = View {
            zoom: 3.0,
            offset: Vec2::new(300.0, 300.0),
            fit: false,
            ..View::default()
        };
        let (off, on) = (egui::pos2(40.0, 40.0), egui::pos2(500.0, 450.0));
        let wires = |o: &Open| o.project.pages[0].wires.len();

        // A click outside the sheet starts no wire.
        click_at(&ctx, &mut o, &mut ed, off, 1.0);
        assert!(!on_paper(&o, 0, ed.cursor.unwrap()), "{:?}", ed.cursor);
        assert!(ed.wire.is_none());
        assert_eq!(ed.message, "Click on the sheet to draw the wire.");

        // Started on the sheet, a click outside adds no bend and a double-click there no end.
        click_at(&ctx, &mut o, &mut ed, on, 2.0);
        assert!(on_paper(&o, 0, ed.cursor.unwrap()), "{:?}", ed.cursor);
        assert!(ed.wire.as_ref().is_some_and(|(_, via)| via.is_empty()));
        click_at(&ctx, &mut o, &mut ed, off, 3.0);
        click_at(&ctx, &mut o, &mut ed, off, 3.1);
        assert!(ed.wire.as_ref().is_some_and(|(_, via)| via.is_empty()), "{:?}", ed.wire);
        assert_eq!(wires(&o), 0);

        // A double-click on the sheet still ends the wire on a free point.
        let end = on + Vec2::new(60.0, 40.0);
        click_at(&ctx, &mut o, &mut ed, end, 4.0);
        click_at(&ctx, &mut o, &mut ed, end, 4.1);
        assert!(ed.wire.is_none());
        assert_eq!(wires(&o), 1);
    }

    #[test]
    fn the_wire_tool_places_points_where_the_button_was_released() {
        let (_tmp, mut o) = project();
        let mut ed = Editor {
            tool: Tool::Wire,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        painted(&ctx, &mut o, &mut ed, 0.5, vec![]);
        ed.view = View {
            zoom: 3.0,
            offset: Vec2::new(300.0, 300.0),
            fit: false,
            ..Default::default()
        };
        let off = egui::pos2(40.0, 40.0);
        // Press at `pos`, then release there and move off the sheet in the same frame; returns
        // where the pointer was when the button went down, in page mm.
        let click_then_leave = |o: &mut Open, ed: &mut Editor, pos: egui::Pos2, t: f64| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            painted(&ctx, o, ed, t, vec![egui::Event::PointerMoved(pos)]);
            let at = ed.cursor.unwrap();
            painted(&ctx, o, ed, t + 0.01, vec![button(true)]);
            painted(
                &ctx,
                o,
                ed,
                t + 0.02,
                vec![button(false), egui::Event::PointerMoved(off)],
            );
            assert!(!on_paper(o, 0, ed.cursor.unwrap()), "{:?}", ed.cursor);
            at
        };

        let start = click_then_leave(&mut o, &mut ed, egui::pos2(500.0, 450.0), 1.0);
        assert!(on_paper(&o, 0, start), "{start:?}");
        assert_ne!(ed.message, "Click on the sheet to draw the wire.");
        assert_eq!(ed.wire, Some((Endpoint::Point { at: start }, Vec::new())));

        let bend = click_then_leave(&mut o, &mut ed, egui::pos2(500.0, 550.0), 2.0);
        assert_eq!(ed.wire, Some((Endpoint::Point { at: start }, vec![bend])));
    }

    /// A bend clicked twice on the same spot leaves no zero-length segment in the finished
    /// wire, so no junction dot shows on that plain bend (#1236).
    #[test]
    fn a_bend_clicked_twice_leaves_no_zero_length_segment() {
        let (_tmp, mut o) = project();
        let mut ed = Editor {
            tool: Tool::Wire,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        painted(&ctx, &mut o, &mut ed, 0.5, vec![]);
        ed.view = View {
            zoom: 3.0,
            offset: Vec2::new(300.0, 300.0),
            fit: false,
            ..Default::default()
        };
        let start = egui::pos2(500.0, 450.0);
        let bend = start + Vec2::new(0.0, 90.0);
        let end = bend + Vec2::new(90.0, 0.0);
        click_at(&ctx, &mut o, &mut ed, start, 1.0);
        // The start spot again, then the bend twice, each a separate single click.
        click_at(&ctx, &mut o, &mut ed, start, 2.0);
        click_at(&ctx, &mut o, &mut ed, bend, 3.0);
        click_at(&ctx, &mut o, &mut ed, bend, 4.0);
        click_at(&ctx, &mut o, &mut ed, end, 5.0);
        click_at(&ctx, &mut o, &mut ed, end, 5.1);
        assert!(ed.wire.is_none());
        let page = o.project.pages[0].clone();
        assert_eq!(page.wires.len(), 1, "{:?}", page.wires);
        let w = &page.wires[0];
        let pts = o.wire_points(&page, w).unwrap();
        assert_eq!(pts.len(), 3, "start, one bend, end: {pts:?}");
        assert!(pts.windows(2).all(|ab| ab[0] != ab[1]), "{pts:?}");
        let lines = vec![pts.iter().map(|q| (q.x, q.y)).collect::<Vec<_>>()];
        assert!(wc_pdf::junctions(&lines).is_empty(), "{pts:?}");
    }

    /// A wire started on one page is dropped when another page is shown, also by a way that
    /// sets the page shown directly (New page, Duplicate page, the device list), so its next
    /// click does not end it on the new page (#1154).
    #[test]
    fn a_wire_started_on_one_page_is_dropped_on_another() {
        let (_tmp, mut o) = project();
        let mut ed = Editor {
            tool: Tool::Wire,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        painted(&ctx, &mut o, &mut ed, 0.5, vec![]);
        ed.view = View {
            zoom: 3.0,
            offset: Vec2::new(300.0, 300.0),
            fit: false,
            ..View::default()
        };
        let on = egui::pos2(500.0, 450.0);
        click_at(&ctx, &mut o, &mut ed, on, 1.0);
        painted(&ctx, &mut o, &mut ed, 1.5, vec![]);
        assert!(ed.wire.is_some(), "kept while its page is shown");

        // As Pages > New page does: the new page is shown with the wire tool still on.
        ed.page = o.add_page();
        painted(&ctx, &mut o, &mut ed, 2.0, vec![]);
        assert!(ed.wire.is_none(), "the wire begun on the other page is dropped");
        let end = on + Vec2::new(60.0, 40.0);
        click_at(&ctx, &mut o, &mut ed, end, 3.0);
        click_at(&ctx, &mut o, &mut ed, end, 3.1);
        assert!(
            o.project.pages[ed.page].wires.is_empty(),
            "no wire from the other page's start: {:?}",
            o.project.pages[ed.page].wires
        );
        assert!(o.project.pages[0].wires.is_empty());
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
        o.begin_drag_move();
        o.drag_move_by(ed.page, &ed.selection.clone(), d);
        ed.drag = Some(Drag::Move {
            from: d,
            checkpoint: true,
            page: o.project.pages[ed.page].id.clone(),
            step: o.history_count(),
            id: o.last_step_id().unwrap_or(0),
            items: Some(ed.selection.clone()),
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
        right_clicked(&mut ed, Some(Selection::Cloud(0)));
        right_clicked(&mut ed, Some(Selection::RevisionTable));
        right_clicked(&mut ed, None);
        assert_eq!(
            ed.selection,
            [wire("w1"), wire("w2")],
            "only wires, symbols and drawings change it"
        );
        right_clicked(&mut ed, Some(wire("w3")));
        assert_eq!(ed.selection, [wire("w3")]);
        assert_eq!(selected_wires(&ed), ["w3"]);
    }

    #[test]
    fn right_click_on_a_symbol_selects_it_or_keeps_its_selection() {
        let sym = |id: &str| Selection::Symbol(id.into());
        let mut ed = Editor {
            selection: vec![sym("s1"), Selection::Wire("w1".into())],
            ..Editor::default()
        };
        right_clicked(&mut ed, Some(sym("s1")));
        assert_eq!(
            ed.selection,
            [sym("s1"), Selection::Wire("w1".into())],
            "a selected one keeps it"
        );
        assert!(has_symbols(&ed));
        right_clicked(&mut ed, Some(sym("s2")));
        assert_eq!(ed.selection, [sym("s2")], "one not selected becomes the only selection");
        assert!(selected_wires(&ed).is_empty());
        let labels = |ed: &Editor| -> Vec<&str> { edit_menu_actions(ed).iter().map(|a| a.info().label).collect() };
        assert_eq!(labels(&ed), ["Rotate", "Mirror", "Copy", "Cut", "Duplicate", "Delete"]);
    }

    #[test]
    fn right_click_on_text_or_a_drawing_offers_copy_cut_duplicate_and_delete() {
        let labels = |ed: &Editor| -> Vec<&str> { edit_menu_actions(ed).iter().map(|a| a.info().label).collect() };
        // Not selected: it becomes the only selection, and the menu has the edits that work on it.
        let mut ed = Editor {
            selection: vec![Selection::Wire("w1".into())],
            ..Editor::default()
        };
        assert!(labels(&ed).is_empty(), "wires alone keep only the route menu");
        right_clicked(&mut ed, Some(Selection::Graphic(2)));
        assert_eq!(ed.selection, [Selection::Graphic(2)]);
        assert_eq!(labels(&ed), ["Copy", "Cut", "Duplicate", "Delete"]);
        // Selected: the selection it is part of stays.
        let mut ed = Editor {
            selection: vec![
                Selection::Graphic(0),
                Selection::Graphic(1),
                Selection::Wire("w1".into()),
            ],
            ..Editor::default()
        };
        right_clicked(&mut ed, Some(Selection::Graphic(1)));
        assert_eq!(
            ed.selection,
            [
                Selection::Graphic(0),
                Selection::Graphic(1),
                Selection::Wire("w1".into())
            ]
        );
        assert_eq!(labels(&ed), ["Copy", "Cut", "Duplicate", "Delete"]);
        // With a symbol among them, Rotate and Mirror come back.
        ed.selection.push(Selection::Symbol("s1".into()));
        assert_eq!(labels(&ed), ["Rotate", "Mirror", "Copy", "Cut", "Duplicate", "Delete"]);
    }

    /// Right-click a symbol through egui events, then click Duplicate in the menu: the command
    /// is left for the app to run, as Ctrl+D runs it (#1122).
    #[test]
    fn duplicate_is_picked_from_the_right_click_menu_of_a_symbol() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        ed.selection.clear();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        painted(&ctx, &mut o, &mut ed, 0.5, vec![]);
        ed.view = View {
            zoom: 3.0,
            offset: Vec2::new(100.0, 100.0),
            fit: false,
            ..View::default()
        };
        // Where page mm (0, 0) is on the screen, from the cursor seen on a grid point.
        let probe = egui::pos2(400.0, 400.0);
        painted(&ctx, &mut o, &mut ed, 0.6, vec![egui::Event::PointerMoved(probe)]);
        let seen = ed.cursor.unwrap();
        let screen = |p: Point| probe + Vec2::new((p.x - seen.x) as f32, (p.y - seen.y) as f32) * 3.0;
        let at = o.project.pages[ed.page].symbol(&coil).unwrap().at;
        let on_coil = screen(at);
        let button = |pos, button, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        painted(&ctx, &mut o, &mut ed, 1.0, vec![egui::Event::PointerMoved(on_coil)]);
        painted(
            &ctx,
            &mut o,
            &mut ed,
            1.01,
            vec![button(on_coil, egui::PointerButton::Secondary, true)],
        );
        painted(
            &ctx,
            &mut o,
            &mut ed,
            1.02,
            vec![button(on_coil, egui::PointerButton::Secondary, false)],
        );
        assert_eq!(
            ed.selection,
            [Selection::Symbol(coil.clone())],
            "the right-click selects it"
        );
        let texts = painted(&ctx, &mut o, &mut ed, 1.1, vec![]);
        let item = texts.iter().find(|(t, _)| t == "Duplicate");
        assert!(item.is_some(), "no Duplicate in the menu: {texts:?}");
        let item = item.unwrap().1.center();
        let left = |pressed| button(item, egui::PointerButton::Primary, pressed);
        painted(&ctx, &mut o, &mut ed, 1.2, vec![egui::Event::PointerMoved(item)]);
        painted(&ctx, &mut o, &mut ed, 1.21, vec![left(true)]);
        painted(&ctx, &mut o, &mut ed, 1.22, vec![left(false)]);
        assert_eq!(ed.menu_action, Some(crate::actions::Action::Duplicate));
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
            step: 0,
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
    fn escape_during_a_move_puts_it_back_and_wires_nothing() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        let before = o.project.clone();
        let steps = o.undo_steps();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        assert_ne!(o.project.pages, before.pages);
        assert!(ed.select_dragging());
        // Saved while the button is held: the file has the moved page.
        o.dirty = false;
        cancel_drag(&mut o, &mut ed);
        assert!(o.dirty, "the page put back is unsaved again");
        assert!(!ed.select_dragging());
        assert_eq!(o.project.pages, before.pages, "the move is taken back");
        assert_eq!(o.undo_steps(), steps, "and leaves no undo step behind");
        // The release that follows adds no wires.
        drag_released(&mut o, &mut ed, false);
        assert!(o.project.pages[ed.page].wires.is_empty());
        assert!(ed.message.is_empty());
    }

    #[test]
    fn escape_during_a_move_puts_followed_jumpers_back_and_keeps_other_changes() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
        // Terminals numbered by position: A 1, B 2, C 3; the jumper joins B and C.
        let mut put = |x: f64| {
            let id = o.place(page, "wc.terminal", Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let a = put(20.0);
        put(40.0);
        put(60.0);
        o.project.meta.add_jumper(&x1, &["2".into(), "3".into()]).unwrap();
        // A rail order that names the terminals by number: C, A, B.
        o.project.meta.strips.push(wc_core::project::StripLayout {
            strip: x1.clone(),
            order: vec!["3".into(), "1".into(), "2".into()],
            spares: Vec::new(),
        });
        let before = o.project.clone();
        let mut ed = Editor {
            page,
            selection: vec![Selection::Symbol(a)],
            ..Editor::default()
        };
        // A goes past C: B is 1, C 2, A 3, and the jumper follows B and C to 1 and 2.
        drag(&mut o, &mut ed, Point::new(60.0, 0.0));
        o.follow_terminals();
        let jumper = |o: &Open| o.project.meta.jumpers[0].terminals.clone();
        assert_eq!(jumper(&o), ["1", "2"], "followed during the drag");
        assert_ne!(o.project.meta.strips, before.meta.strips, "the rail order followed too");
        // A sync brings someone else's device in meanwhile.
        let other = wc_core::DeviceTag::parse("-K9").unwrap();
        o.project.devices.insert(other.clone(), Default::default());
        cancel_drag(&mut o, &mut ed);
        assert_eq!(o.project.pages, before.pages);
        assert_eq!(jumper(&o), ["2", "3"], "the jumper joins B and C again");
        assert_eq!(
            o.project.meta.strips, before.meta.strips,
            "and the rail order is C, A, B again"
        );
        assert!(o.project.devices.contains_key(&other), "the synced device stays");
    }

    #[test]
    fn escape_after_a_rotate_during_a_move_puts_the_items_back_turned() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| (s.at, s.rotation));
        let Some((start, r0)) = at(&o) else { unreachable!() };
        let steps = o.undo_steps();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        o.rotate_many(page, &ed.selection.clone());
        let Some((moved, turned)) = at(&o) else { unreachable!() };
        assert_ne!(moved, start);
        assert_ne!(turned, r0);
        o.dirty = false;
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        assert!(o.dirty, "unsaved again");
        assert_eq!(at(&o), Some((start, turned)), "back at the start, still turned");
        assert_eq!(o.undo_steps(), steps + 1, "only the rotate's undo step is left");
        assert!(o.undo());
        assert_eq!(at(&o), Some((start, r0)), "Undo takes back only the rotate");
        assert_eq!(o.undo_steps(), steps);
        assert!(o.redo());
        assert_eq!(at(&o), Some((start, turned)), "Redo turns it again at the start");
        // The release that follows adds no wires.
        drag_released(&mut o, &mut ed, false);
        assert!(o.project.pages[page].wires.is_empty());
    }

    #[test]
    fn escape_after_a_rotate_and_more_moving_takes_each_step_back_by_its_own_part() {
        for more in [Point::new(-20.0, 0.0), Point::new(40.0, 0.0)] {
            let (_tmp, mut o, mut ed, coil) = moving_coil();
            let page = ed.page;
            let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| (s.at, s.rotation));
            let Some((start, r0)) = at(&o) else { unreachable!() };
            let steps = o.undo_steps();
            drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
            o.rotate_many(page, &ed.selection.clone());
            let Some((_, turned)) = at(&o) else { unreachable!() };
            // The drag goes on after the rotate (back to the start with +40).
            o.drag_move_by(page, &ed.selection.clone(), more);
            cancel_drag(&mut o, &mut ed);
            assert_eq!(at(&o), Some((start, turned)), "back at the start, turned ({more:?})");
            assert_eq!(o.undo_steps(), steps + 1);
            assert!(o.undo());
            assert_eq!(at(&o), Some((start, r0)), "Undo leaves it at the start ({more:?})");
            assert!(o.redo());
            assert_eq!(at(&o), Some((start, turned)));
        }
    }

    #[test]
    fn escape_after_another_change_during_a_move_puts_followed_jumpers_back_in_every_step() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
        let mut put = |x: f64| {
            let id = o.place(page, "wc.terminal", Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let a = put(20.0);
        put(40.0);
        put(60.0);
        o.project.meta.add_jumper(&x1, &["2".into(), "3".into()]).unwrap();
        o.project.meta.strips.push(wc_core::project::StripLayout {
            strip: x1.clone(),
            order: vec!["3".into(), "1".into(), "2".into()],
            spares: Vec::new(),
        });
        let before = o.project.clone();
        let mut ed = Editor {
            page,
            selection: vec![Selection::Symbol(a)],
            ..Editor::default()
        };
        // A goes past C, then another change is recorded during the drag.
        drag(&mut o, &mut ed, Point::new(60.0, 0.0));
        let other = wc_core::DeviceTag::parse("-K9").unwrap();
        o.checkpoint();
        o.project.devices.insert(other.clone(), Default::default());
        let jumper = |o: &Open| o.project.meta.jumpers[0].terminals.clone();
        assert_eq!(jumper(&o), ["1", "2"], "followed during the drag");
        cancel_drag(&mut o, &mut ed);
        let check = |o: &Open, what: &str| {
            assert_eq!(o.project.pages, before.pages, "{what}");
            assert_eq!(jumper(o), ["2", "3"], "{what}: the jumper joins B and C again");
            assert_eq!(o.project.meta.strips, before.meta.strips, "{what}: rail order C, A, B");
        };
        check(&o, "after Escape");
        assert!(o.project.devices.contains_key(&other));
        assert!(o.undo());
        check(&o, "after Undo");
        assert!(
            !o.project.devices.contains_key(&other),
            "Undo takes back the other change"
        );
        assert!(o.redo());
        check(&o, "after Redo");
    }

    #[test]
    fn escape_after_moving_another_selection_during_the_drag_only_ends_it() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| (s.at, s.rotation));
        let own = ed.selection.clone();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        // A frame of the drag moves another selection (the contact): the drag forgets its own.
        let Some(contact) = o.project.pages[page]
            .symbols
            .iter()
            .find(|s| s.id != coil)
            .map(|s| s.id.clone())
        else {
            unreachable!("the contact is on the page")
        };
        ed.selection = vec![Selection::Symbol(contact)];
        move_drag_to(&mut o, &mut ed, Point::new(-60.0, 0.0));
        // The first selection comes back and is turned.
        ed.selection = own;
        o.rotate_many(page, &ed.selection.clone());
        let now = at(&o);
        let steps = o.undo_steps();
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        assert_eq!(at(&o), now, "nothing is moved back by a part it did not move");
        assert_eq!(o.undo_steps(), steps);
    }

    #[test]
    fn escape_after_moving_another_selection_with_nothing_recorded_puts_the_page_back() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let before = o.project.pages[page].clone();
        let steps = o.undo_steps();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        let contact = o.project.pages[page]
            .symbols
            .iter()
            .find(|s| s.id != coil)
            .map(|s| s.id.clone());
        let Some(contact) = contact else {
            unreachable!("the contact is on the page")
        };
        ed.selection = vec![Selection::Symbol(contact)];
        move_drag_to(&mut o, &mut ed, Point::new(-60.0, 0.0));
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        assert_eq!(
            o.project.pages[page], before,
            "both moves are taken back, nothing else changed"
        );
        assert_eq!(o.undo_steps(), steps, "the drag leaves no undo step");
    }

    #[test]
    fn escape_after_the_move_was_undone_and_redone_takes_it_back() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| s.at);
        let start = at(&o);
        let steps = o.undo_steps();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        o.rotate_many(page, &ed.selection.clone());
        assert!(o.undo() && o.undo(), "the rotate and the move are undone");
        assert!(o.redo(), "the move is redone");
        cancel_drag(&mut o, &mut ed);
        assert_eq!(at(&o), start, "Escape still finds the redone move");
        assert_eq!(o.undo_steps(), steps);
        assert!(ed.message.is_empty(), "{}", ed.message);
    }

    #[test]
    fn escape_after_an_assistant_turn_folded_into_one_step_only_ends_the_drag() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        let page = ed.page;
        let line = |x: f64| Shape::Line {
            a: Point::new(x, 100.0),
            b: Point::new(x + 10.0, 100.0),
        };
        let Some(g) = o.add_graphic(page, line(10.0)) else {
            unreachable!()
        };
        ed.selection.push(g.clone());
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        // One assistant turn deletes the line and draws another: the same count of drawings,
        // and the step in between is folded away.
        let mark = o.start_turn();
        o.delete_many(page, &[g]);
        o.add_graphic(page, line(200.0));
        o.squash_undo(mark);
        let now = o.project.pages[page].clone();
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        assert_eq!(
            o.project.pages[page], now,
            "the new line is not moved by the old one's offset"
        );
        assert!(!ed.message.is_empty(), "the editor says why");
    }

    #[test]
    fn escape_after_a_long_assistant_turn_trimmed_into_the_move_only_ends_the_drag() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        let page = ed.page;
        // The move is the first step of an assistant turn that runs past the undo limit: the
        // steps after it are folded into it as the oldest ones are trimmed.
        let _mark = o.start_turn();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        for _ in 0..450 {
            o.checkpoint();
        }
        let now = o.project.pages[page].clone();
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        assert_eq!(o.project.pages[page], now, "nothing is moved");
        assert!(ed.message.contains("one step"), "{}", ed.message);
    }

    #[test]
    fn escape_after_frames_with_nothing_selected_still_takes_the_move_back() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| (s.at, s.rotation));
        let Some((start, _)) = at(&o) else { unreachable!() };
        let contact = o.project.pages[page]
            .symbols
            .iter()
            .find(|s| s.id != coil)
            .map(|s| s.id.clone());
        let Some(contact) = contact else {
            unreachable!("the contact is on the page")
        };
        ed.selection.push(Selection::Symbol(contact));
        let own = ed.selection.clone();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        o.rotate_many(page, &ed.selection.clone());
        let Some((_, turned)) = at(&o) else { unreachable!() };
        // Nothing selected, and the pointer goes on: nothing moves, the drag keeps its record.
        ed.selection.clear();
        move_drag_to(&mut o, &mut ed, Point::new(-60.0, 0.0));
        // The same items again, in another order of the list, moving on.
        ed.selection = own.into_iter().rev().collect();
        move_drag_to(&mut o, &mut ed, Point::new(-70.0, 0.0));
        cancel_drag(&mut o, &mut ed);
        assert_eq!(at(&o), Some((start, turned)), "the coil goes back, still turned");
        assert!(ed.message.is_empty(), "{}", ed.message);
    }

    #[test]
    fn escape_after_a_rotate_takes_back_the_drags_own_items_when_the_selection_changed() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| (s.at, s.rotation));
        let Some((start, _)) = at(&o) else { unreachable!() };
        let steps = o.undo_steps();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        o.rotate_many(page, &ed.selection.clone());
        let Some((_, turned)) = at(&o) else { unreachable!() };
        // Another item picked (a right click) while the button is held, nothing moved since.
        ed.selection.clear();
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        assert_eq!(at(&o), Some((start, turned)), "the coil goes back, still turned");
        assert_eq!(o.undo_steps(), steps + 1, "the move's step is gone, the rotate's stays");
    }

    #[test]
    fn escape_after_a_rotate_and_its_undo_during_a_move_keeps_the_rotate_for_redo() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| (s.at, s.rotation));
        let Some((start, r0)) = at(&o) else { unreachable!() };
        let steps = o.undo_steps();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        o.rotate_many(page, &ed.selection.clone());
        let Some((_, turned)) = at(&o) else { unreachable!() };
        assert!(o.undo(), "the rotate is undone, the move is not");
        cancel_drag(&mut o, &mut ed);
        assert_eq!(at(&o), Some((start, r0)), "the move is taken back");
        assert_eq!(o.undo_steps(), steps, "with its undo step");
        assert!(o.redo());
        assert_eq!(at(&o), Some((start, turned)), "Redo turns it at the start");
    }

    #[test]
    fn escape_after_the_move_was_undone_and_a_rotate_made_only_ends_the_drag() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| (s.at, s.rotation));
        let Some((start, _)) = at(&o) else { unreachable!() };
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        assert!(o.undo(), "the move is undone");
        // The rotate's checkpoint takes the place the move's step had in the history.
        o.rotate_many(page, &ed.selection.clone());
        let now = at(&o);
        let steps = o.undo_steps();
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        assert_eq!(at(&o), now, "nothing moves");
        assert_eq!(now.map(|(p, _)| p), Some(start));
        assert_eq!(o.undo_steps(), steps, "the rotate keeps its undo step");
    }

    #[test]
    fn escape_after_an_undo_during_a_move_changes_nothing_more() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        let before = o.project.pages.clone();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        assert!(o.undo());
        assert_eq!(o.project.pages, before);
        let steps = o.undo_steps();
        cancel_drag(&mut o, &mut ed);
        assert_eq!(o.project.pages, before);
        assert_eq!(o.undo_steps(), steps, "an earlier change keeps its undo step");
    }

    #[test]
    fn a_move_after_an_undo_during_the_drag_is_taken_back_by_escape() {
        let (_tmp, mut o, mut ed, coil) = moving_coil();
        let page = ed.page;
        let at = |o: &Open| o.project.pages[page].symbol(&coil).map(|s| s.at);
        let start = at(&o);
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        // Undo takes the move back; the same item is selected again and the pointer moves on.
        assert!(o.undo());
        assert_eq!(at(&o), start);
        ed.selection = vec![Selection::Symbol(coil.clone())];
        move_drag_to(&mut o, &mut ed, Point::new(-60.0, 0.0));
        assert_ne!(at(&o), start, "the new frame moved the coil");
        cancel_drag(&mut o, &mut ed);
        assert_eq!(at(&o), start, "Escape takes the new move back");
    }

    #[test]
    fn escape_puts_a_moved_or_sized_logo_back_and_drops_its_step() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        let start = (Point::new(10.0, 10.0), Point::new(40.0, 30.0));
        o.project.meta.logo = Some(wc_core::Logo {
            file: "pictures/logo.png".into(),
            cover: Some(start),
        });
        for size in [false, true] {
            let steps = o.undo_steps();
            o.checkpoint();
            o.set_logo_place(Point::new(20.0, 15.0), Point::new(60.0, 45.0));
            ed.drag = Some(Drag::Logo {
                from: Point::new(0.0, 0.0),
                start,
                size,
                checkpoint: true,
                step: o.history_count(),
            });
            o.dirty = false;
            cancel_drag(&mut o, &mut ed);
            assert!(ed.drag.is_none());
            assert!(o.dirty, "unsaved again");
            assert_eq!(o.project.meta.logo.as_ref().and_then(|l| l.cover), Some(start));
            assert_eq!(o.undo_steps(), steps, "no step left behind");
        }
    }

    #[test]
    fn escape_takes_back_a_move_on_the_page_it_began_on() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        let before = o.project.pages.clone();
        let page = ed.page;
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        // Page Up during the move: another page is shown when Escape comes.
        ed.page = if page == 0 { 1 } else { 0 };
        cancel_drag(&mut o, &mut ed);
        assert_eq!(o.project.pages, before);
    }

    #[test]
    fn a_tool_change_ends_a_drag_so_escape_leaves_the_new_tool() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        drag(&mut o, &mut ed, Point::new(-40.0, 0.0));
        assert!(ed.select_dragging());
        // W during the move, the button let go, a wire started, then Escape twice.
        ed.tool = Tool::Wire;
        end_drag_off_select(&mut ed);
        assert!(ed.drag.is_none(), "the tool change ends the drag");
        let moved = o.project.pages.clone();
        drag_released(&mut o, &mut ed, false);
        assert_eq!(o.project.pages, moved, "its release does nothing");
        ed.wire = Some((
            Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            Vec::new(),
        ));
        assert!(!ed.escape(), "the first Escape drops the wire being drawn");
        assert!(ed.wire.is_none());
        assert!(!ed.cancel_drag, "and leaves no drag to cancel");
        assert!(ed.escape(), "the next leaves the wire tool");
    }

    #[test]
    fn escape_before_a_move_has_moved_or_during_a_selection_box_just_ends_the_drag() {
        let (_tmp, mut o, mut ed, _) = moving_coil();
        let before = o.project.clone();
        let steps = o.undo_steps();
        ed.drag = Some(Drag::Move {
            from: Point::new(0.0, 0.0),
            checkpoint: false,
            page: o.project.pages[ed.page].id.clone(),
            step: 0,
            id: 0,
            items: Some(ed.selection.clone()),
        });
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        ed.drag = Some(Drag::Box {
            from: Point::new(0.0, 0.0),
        });
        cancel_drag(&mut o, &mut ed);
        assert!(ed.drag.is_none());
        assert_eq!(o.project, before);
        assert_eq!(o.undo_steps(), steps);
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
