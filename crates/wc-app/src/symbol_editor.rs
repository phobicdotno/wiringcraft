//! The symbol editor: draw a symbol's lines, rectangles, circles, arcs and texts, place and name
//! its pins, set where the device tag goes, and save it into one of the project's library
//! folders. [`SymbolEditor`] holds the state and does the work without any UI, so it is
//! tested on its own; [`SymbolEditor::show`] is the window around it.

use std::path::PathBuf;

use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke, Vec2};
use wc_core::Symbol;
use wc_core::geom::{Align, Point, Shape};
use wc_core::symbol::{Pin, PinDir, SymbolRole};

use crate::canvas::{Xf, paint_shape};

/// Everything snaps to this grid (mm), so pins line up with wires on the page.
pub const SNAP: f64 = 2.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Line,
    Rect,
    Circle,
    /// Points one after another; Enter or a double-click ends it, a click on the first point
    /// closes it.
    Polyline,
    /// Centre, then the start of the arc, then its end (clockwise on paper).
    Arc,
    Text,
    Pin,
    /// Where the device tag is printed.
    Tag,
}

impl Tool {
    const ALL: [Tool; 9] = [
        Tool::Select,
        Tool::Line,
        Tool::Rect,
        Tool::Circle,
        Tool::Polyline,
        Tool::Arc,
        Tool::Text,
        Tool::Pin,
        Tool::Tag,
    ];

    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Line => "Line",
            Tool::Rect => "Rectangle",
            Tool::Circle => "Circle",
            Tool::Polyline => "Polyline",
            Tool::Arc => "Arc",
            Tool::Text => "Text",
            Tool::Pin => "Pin",
            Tool::Tag => "Tag position",
        }
    }

    /// What to do next, for the status line.
    fn hint(self, clicks: usize) -> &'static str {
        match (self, clicks) {
            (Tool::Select, _) => {
                "Click a shape or pin to select it; drag it, nudge it with the arrow keys, or drag a line end or corner."
            }
            (Tool::Line, 0) => "Click the start of the line.",
            (Tool::Line, _) => "Click the end of the line.",
            (Tool::Rect, 0) => "Click one corner.",
            (Tool::Rect, _) => "Click the opposite corner.",
            (Tool::Circle, 0) => "Click the centre.",
            (Tool::Circle, _) => "Click a point on the circle.",
            (Tool::Polyline, 0) => "Click the first point.",
            (Tool::Polyline, _) => {
                "Click the next point; double-click or Enter ends it, a click on the first point closes it (Esc cancels)."
            }
            (Tool::Arc, 0) => "Click the centre of the arc.",
            (Tool::Arc, 1) => "Click where the arc starts.",
            (Tool::Arc, _) => "Click where the arc ends (it runs clockwise).",
            (Tool::Text, _) => "Click where the text starts.",
            (Tool::Pin, _) => "Click where a wire connects.",
            (Tool::Tag, _) => "Click where the device tag is printed.",
        }
    }
}

/// What is selected in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Shape(usize),
    Pin(usize),
}

/// Reads a library symbol by id: the symbol and the source it came from, or why not.
pub type OpenSymbol<'a> = dyn FnMut(&str) -> Result<(Symbol, String), String> + 'a;

/// A library folder the symbol can be saved into: `(source name, folder)`.
pub type Target = (String, PathBuf);

pub struct SymbolEditor {
    pub symbol: Symbol,
    pub tool: Tool,
    /// The clicks so far of a shape being drawn.
    clicks: Vec<Point>,
    /// Where the last click closed a polyline on its first point, so the second click of a
    /// double-click there does not start a new one.
    closed_at: Option<Point>,
    pub selected: Option<Item>,
    /// Folders the symbol can be saved into, and the chosen one.
    pub targets: Vec<Target>,
    pub target: usize,
    /// The id the symbol had when it was opened (`None` for a new symbol).
    pub opened_as: Option<String>,
    /// The library folder it was opened from (or last saved into). Only that file may be
    /// replaced by a save; a symbol with the same id in another folder is not overwritten.
    opened_from: Option<PathBuf>,
    /// The project folder the editor belongs to; it closes when another project is opened.
    pub owner: Option<PathBuf>,
    /// True while property edits run on: they join one undo step until something else happens.
    props_dirty: bool,
    /// Text the Text tool places.
    pub text: String,
    /// Name the Pin tool gives the next pin.
    pub pin_name: String,
    pub message: String,
    undo: Vec<Symbol>,
    /// Steps taken back by Undo, newest last; any new change clears them.
    redo: Vec<Symbol>,
    /// A drag of the selected item under way.
    drag: Option<Drag>,
    /// Library symbols that can be opened: `(id, name)`.
    pub choices: Vec<(String, String)>,
}

/// A drag in progress: where it started and the item as it was then.
#[derive(Debug, Clone)]
struct Drag {
    from: Point,
    item: Item,
    /// A line end, rectangle corner, circle edge or arc end being dragged (index into
    /// [`handles`]) instead of the whole item.
    handle: Option<usize>,
    before: Symbol,
}

/// The points of a shape that can be dragged on their own: a line's two ends, a rectangle's
/// four corners (top-left, top-right, bottom-right, bottom-left), a circle's edge at angle 0,
/// an arc's start and end.
pub fn handles(shape: &Shape) -> Vec<Point> {
    let on = |c: Point, r: f64, deg: f64| {
        let a = deg.to_radians();
        Point::new(c.x + r * a.cos(), c.y + r * a.sin())
    };
    match shape {
        Shape::Line { a, b } => vec![*a, *b],
        Shape::Rect { min, max } => vec![*min, Point::new(max.x, min.y), *max, Point::new(min.x, max.y)],
        Shape::Circle { center, radius, .. } => vec![on(*center, *radius, 0.0)],
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => vec![on(*center, *radius, *start), on(*center, *radius, *end)],
        _ => Vec::new(),
    }
}

/// `shape` with handle `k` moved to `to`. A rectangle keeps the opposite corner; a circle's
/// radius becomes the distance from its centre to `to`, rounded to the grid; an arc's end
/// turns to point at `to`, keeping the radius and the other end. Nothing changes when the
/// result would be a line of no length, a rectangle without area, a circle of radius zero or
/// an arc whose ends meet.
fn with_handle(shape: &Shape, k: usize, to: Point) -> Option<Shape> {
    let new = match shape {
        Shape::Line { a, b } => match k {
            0 => Shape::Line { a: to, b: *b },
            1 => Shape::Line { a: *a, b: to },
            _ => return None,
        },
        Shape::Rect { .. } => {
            let corners = handles(shape);
            let fixed = *corners.get((k + 2) % 4)?;
            if k > 3 || to.x == fixed.x || to.y == fixed.y {
                return None;
            }
            Shape::Rect {
                min: Point::new(to.x.min(fixed.x), to.y.min(fixed.y)),
                max: Point::new(to.x.max(fixed.x), to.y.max(fixed.y)),
            }
        }
        Shape::Circle { center, filled, .. } => {
            let d = ((to.x - center.x).powi(2) + (to.y - center.y).powi(2)).sqrt();
            let radius = (d / SNAP).round() * SNAP;
            if k != 0 || radius == 0.0 {
                return None;
            }
            Shape::Circle {
                center: *center,
                radius,
                filled: *filled,
            }
        }
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => {
            // On the centre (give or take rounding from the handle's sin/cos) has no direction.
            if (to.x - center.x).hypot(to.y - center.y) < 1e-6 {
                return None;
            }
            let a = angle(*center, to);
            let (start, end) = match k {
                0 => (a, *end),
                1 => (*start, a),
                _ => return None,
            };
            // The same direction, also when one is stored as 360 and the other as 0.
            if (start - end).rem_euclid(360.0) == 0.0 {
                return None;
            }
            Shape::Arc {
                center: *center,
                radius: *radius,
                start,
                end,
            }
        }
        _ => return None,
    };
    match new {
        Shape::Line { a, b } if a == b => None,
        n => Some(n),
    }
}

/// What a save did, for the window around the editor.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub id: String,
    pub path: PathBuf,
    /// The library source it was saved into.
    pub source: String,
    /// The symbol as saved.
    pub symbol: Symbol,
}

fn blank() -> Symbol {
    Symbol {
        source: String::new(),
        id: String::new(),
        name: String::new(),
        category: String::new(),
        letter_code: String::new(),
        role: SymbolRole::Standalone,
        graphics: Vec::new(),
        pins: Vec::new(),
        tag_at: Point::new(-7.5, 0.0),
        description: String::new(),
    }
}

/// Round to the editor's grid.
pub fn snap(p: Point) -> Point {
    Point::new((p.x / SNAP).round() * SNAP, (p.y / SNAP).round() * SNAP)
}

/// The next pin name after `name`: the trailing number goes up by one (`A1` → `A2`, `13` → `14`);
/// a name without a number stays.
fn next_name(name: &str) -> String {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return name.to_owned();
    }
    let (head, num) = name.split_at(name.len() - digits);
    match num.parse::<u64>().ok().and_then(|n| n.checked_add(1)) {
        Some(n) => format!("{head}{n}"),
        None => name.to_owned(),
    }
}

/// The angle (degrees, y down, as [`Shape::Arc`] uses) of `p` seen from `c`.
/// The angle of `p` seen from `c`, in degrees from 0 to 360 as [`Shape::Arc`] counts them.
pub(crate) fn angle(c: Point, p: Point) -> f64 {
    let a = (p.y - c.y).atan2(p.x - c.x).to_degrees();
    if a < 0.0 { a + 360.0 } else { a }
}

/// The side of the drawing a pin at `at` sits on, so wires leave it outwards.
fn dir_for(at: Point, graphics: &[Shape]) -> PinDir {
    let mut pts = Vec::new();
    for g in graphics {
        let (a, b) = g.bounds();
        pts.extend([a, b]);
    }
    if pts.is_empty() {
        return PinDir::Down;
    }
    let (min, max) = wc_core::geom::bounds_of(&pts);
    let c = Point::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
    let (dx, dy) = (at.x - c.x, at.y - c.y);
    let (w, h) = ((max.x - min.x).max(1.0), (max.y - min.y).max(1.0));
    // Compare relative to the drawing's size, so a tall symbol still gets pins at top and bottom.
    if (dx / w).abs() > (dy / h).abs() {
        if dx < 0.0 { PinDir::Left } else { PinDir::Right }
    } else if dy < 0.0 {
        PinDir::Up
    } else {
        PinDir::Down
    }
}

impl SymbolEditor {
    /// A new, empty symbol to be saved into one of `targets`.
    pub fn new(targets: Vec<Target>) -> Self {
        Self {
            symbol: blank(),
            tool: Tool::Line,
            clicks: Vec::new(),
            closed_at: None,
            selected: None,
            targets,
            target: 0,
            opened_as: None,
            opened_from: None,
            owner: None,
            props_dirty: false,
            text: String::new(),
            pin_name: "1".into(),
            message: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            drag: None,
            choices: Vec::new(),
        }
    }

    /// Edit an existing symbol, saving back into the source it came from when that is one of
    /// `targets`.
    pub fn edit(symbol: Symbol, source: &str, targets: Vec<Target>) -> Self {
        let mut e = Self::new(targets);
        let from = e.targets.iter().position(|(n, _)| n == source);
        e.target = from.unwrap_or(0);
        e.opened_from = from.and_then(|i| e.targets.get(i)).map(|(_, root)| root.clone());
        e.opened_as = Some(symbol.id.clone());
        e.pin_name = symbol.pins.last().map_or_else(|| "1".into(), |p| next_name(&p.name));
        e.symbol = symbol;
        e.tool = Tool::Select;
        e
    }

    fn checkpoint(&mut self) {
        self.props_dirty = false;
        self.push_undo(self.symbol.clone());
    }

    /// Record `before` as an undo step for a new change; what was undone can no longer be redone.
    fn push_undo(&mut self, before: Symbol) {
        self.redo.clear();
        self.undo.push(before);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.end_drag();
        if let Some(s) = self.undo.pop() {
            let now = std::mem::replace(&mut self.symbol, s);
            self.redo.push(now);
            self.after_history_step();
        }
    }

    /// Take back the last Undo.
    pub fn redo(&mut self) {
        self.end_drag();
        if let Some(s) = self.redo.pop() {
            let now = std::mem::replace(&mut self.symbol, s);
            self.undo.push(now);
            self.after_history_step();
        }
    }

    fn after_history_step(&mut self) {
        self.props_dirty = false;
        self.selected = None;
        self.clicks.clear();
    }

    /// Change the tool; a shape half drawn is dropped.
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.clicks.clear();
    }

    /// The properties (id, name, a pin's name or direction...) were changed from `before` by
    /// the form. Typing on in the form stays one undo step until something else happens.
    pub fn note_property_edit(&mut self, before: Symbol) {
        if self.symbol == before {
            return;
        }
        if !self.props_dirty {
            self.push_undo(before);
            self.props_dirty = true;
        }
    }

    /// A click on the drawing at `p` (mm, symbol coordinates). Drawing snaps to the grid;
    /// selecting uses the point as clicked, so shapes off the grid can be picked too.
    pub fn click(&mut self, p: Point) {
        let raw = p;
        let p = snap(p);
        self.message.clear();
        match self.tool {
            Tool::Select => {
                let picked = self.pick(raw);
                // Editing another item's properties is a new undo step.
                if picked != self.selected {
                    self.props_dirty = false;
                }
                self.selected = picked;
            }
            Tool::Line | Tool::Rect | Tool::Circle => {
                let Some(&a) = self.clicks.first() else {
                    self.clicks.push(p);
                    return;
                };
                self.clicks.clear();
                if a == p {
                    return;
                }
                let shape = match self.tool {
                    Tool::Line => Shape::Line { a, b: p },
                    Tool::Rect => Shape::Rect {
                        min: Point::new(a.x.min(p.x), a.y.min(p.y)),
                        max: Point::new(a.x.max(p.x), a.y.max(p.y)),
                    },
                    _ => Shape::Circle {
                        center: a,
                        radius: ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt(),
                        filled: false,
                    },
                };
                self.add_shape(shape);
            }
            Tool::Polyline => {
                // A click on the first point of three or more closes it; the same point twice
                // in a row is not taken (a double-click on it ends the polyline).
                match (self.clicks.first(), self.clicks.last()) {
                    (Some(&first), _) if first == p && self.clicks.len() >= 3 => {
                        self.closed_at = Some(p);
                        let points = std::mem::take(&mut self.clicks);
                        self.add_shape(Shape::Polyline {
                            points,
                            closed: true,
                            filled: false,
                        });
                    }
                    (_, Some(&last)) if last == p => {}
                    _ => self.clicks.push(p),
                }
            }
            Tool::Arc => {
                self.clicks.push(p);
                if let [c, s, e] = self.clicks[..] {
                    let radius = ((s.x - c.x).powi(2) + (s.y - c.y).powi(2)).sqrt();
                    if radius == 0.0 || e == c {
                        self.clicks.clear();
                        return;
                    }
                    let (start, end) = (angle(c, s), angle(c, e));
                    // An end in the start's direction would make a full circle: keep the
                    // centre and start and wait for another end, as dragging an end does.
                    let turn = (end - start).rem_euclid(360.0);
                    if !(1e-9..=360.0 - 1e-9).contains(&turn) {
                        self.clicks.pop();
                        self.message = "The arc would end where it starts; click another end.".into();
                        return;
                    }
                    self.clicks.clear();
                    self.add_shape(Shape::Arc {
                        center: c,
                        radius,
                        start,
                        end,
                    });
                }
            }
            Tool::Text => {
                let text = self.text.trim();
                if text.is_empty() {
                    self.message = "Type the text first.".into();
                    return;
                }
                let text = text.to_owned();
                self.add_shape(Shape::Text {
                    at: p,
                    text,
                    height: 2.5,
                    align: Default::default(),
                    angle: 0.0,
                });
            }
            Tool::Pin => {
                let name = self.pin_name.trim().to_owned();
                if name.is_empty() {
                    self.message = "Give the pin a name first.".into();
                    return;
                }
                if self.symbol.pin(&name).is_some() {
                    self.message = format!("There is already a pin {name}.");
                    return;
                }
                if let Some(q) = self.symbol.pins.iter().find(|q| q.at == p) {
                    self.message = format!("Pin {} is already there.", q.name);
                    return;
                }
                self.checkpoint();
                let dir = dir_for(p, &self.symbol.graphics);
                self.symbol.pins.push(Pin {
                    name: name.clone(),
                    at: p,
                    dir,
                });
                self.selected = Some(Item::Pin(self.symbol.pins.len() - 1));
                self.pin_name = next_name(&name);
            }
            Tool::Tag => {
                if self.symbol.tag_at != p {
                    self.checkpoint();
                    self.symbol.tag_at = p;
                }
            }
        }
    }

    /// End the polyline being drawn where it is (Enter or a double-click), open. Fewer than
    /// two points draw nothing.
    pub fn finish_polyline(&mut self) {
        if self.tool != Tool::Polyline {
            return;
        }
        let points = std::mem::take(&mut self.clicks);
        if points.len() >= 2 {
            self.add_shape(Shape::Polyline {
                points,
                closed: false,
                filled: false,
            });
        }
    }

    /// True while a shape is half drawn: Enter and Escape are then the editor's.
    pub fn drawing_shape(&self) -> bool {
        !self.clicks.is_empty()
    }

    /// Drop the shape being drawn (Escape or a right-click).
    pub fn cancel_shape(&mut self) {
        self.clicks.clear();
    }

    /// Move `item` of `self.symbol` by `d` (mm). A pin stays on the grid.
    fn shift(&mut self, item: Item, d: Point) {
        match item {
            Item::Shape(i) => {
                if let Some(g) = self.symbol.graphics.get_mut(i) {
                    *g = g.translate(d);
                }
            }
            Item::Pin(i) => {
                let others: Vec<Point> = self
                    .symbol
                    .pins
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != i)
                    .map(|(_, q)| q.at)
                    .collect();
                if let Some(pin) = self.symbol.pins.get_mut(i) {
                    let at = snap(pin.at + d);
                    // Two pins on one point would connect each other's wires.
                    if !others.contains(&at) {
                        pin.at = at;
                    }
                }
            }
        }
    }

    /// Move the selected shape or pin by `d` (mm), as one undo step: the arrow keys.
    pub fn nudge(&mut self, d: Point) {
        let Some(item) = self.selected else { return };
        if self.drag.is_some() {
            return;
        }
        let before = self.symbol.clone();
        self.shift(item, d);
        if self.symbol != before {
            self.symbol = before.clone();
            self.checkpoint();
            self.shift(item, d);
        }
    }

    /// The mouse went down on the drawing at `p` (mm) and started dragging. With the Select
    /// tool, the item under it is selected (the selected one wins when it is under it too) and
    /// follows the mouse until [`Self::end_drag`].
    pub fn start_drag(&mut self, p: Point) {
        self.drag = None;
        if self.tool != Tool::Select {
            return;
        }
        self.message.clear();
        // A handle of the selected shape wins: it reshapes instead of moving.
        if let Some(Item::Shape(i)) = self.selected
            && let Some(k) = self.symbol.graphics.get(i).and_then(|g| {
                handles(g)
                    .iter()
                    .position(|h| (h.x - p.x).abs() < 1.0 && (h.y - p.y).abs() < 1.0)
            })
        {
            self.drag = Some(Drag {
                from: p,
                item: Item::Shape(i),
                handle: Some(k),
                before: self.symbol.clone(),
            });
            return;
        }
        let hit = self.pick(p);
        let item = match self.selected {
            Some(sel) if self.under(sel, p) => sel,
            _ => {
                if hit != self.selected {
                    self.props_dirty = false;
                }
                self.selected = hit;
                match hit {
                    Some(h) => h,
                    None => return,
                }
            }
        };
        self.drag = Some(Drag {
            from: p,
            item,
            handle: None,
            before: self.symbol.clone(),
        });
    }

    /// The mouse moved to `p` during a drag: the item moves by whole grid steps; a
    /// dragged handle lands on the grid point nearest the pointer.
    pub fn drag_to(&mut self, p: Point) {
        let Some(drag) = self.drag.as_ref() else { return };
        let raw = Point::new(p.x - drag.from.x, p.y - drag.from.y);
        let d = snap(raw);
        let (item, handle) = (drag.item, drag.handle);
        self.symbol = drag.before.clone();
        match (item, handle) {
            (Item::Shape(i), Some(k)) => {
                if let Some(g) = self.symbol.graphics.get_mut(i)
                    // The pointer itself on an arc's centre has no direction, wherever on the
                    // handle the drag started: test it before the grab offset is added.
                    && !matches!(g, Shape::Arc { center, .. }
                        if (p.x - center.x).hypot(p.y - center.y) < 1e-6)
                    && let Some(h) = handles(g).get(k).copied()
                    // A circle or arc rounds its own radius; snapping the point first would
                    // lose radius and angle when dragging off the axes.
                    && let to = match g {
                        Shape::Circle { .. } | Shape::Arc { .. } => h + raw,
                        _ => snap(h + raw),
                    }
                    && let Some(new) = with_handle(g, k, to)
                {
                    *g = new;
                }
            }
            _ => self.shift(item, d),
        }
    }

    /// The drag ended: a move is one undo step; a drag that moved nothing leaves none.
    pub fn end_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        if self.symbol != drag.before {
            self.props_dirty = false;
            self.push_undo(drag.before);
        }
    }

    /// Escape during a drag: everything goes back to where it was before the drag, with no
    /// undo step, and the mouse moving on changes nothing.
    pub fn cancel_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.symbol = drag.before;
        }
    }

    /// True while a drag is under way.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether `p` is on `item`: within 1.3 mm of a pin, or inside a shape's box made 1 mm
    /// wider. [`Self::pick`] and dragging both use it.
    fn under(&self, item: Item, p: Point) -> bool {
        match item {
            Item::Pin(i) => self
                .symbol
                .pins
                .get(i)
                .is_some_and(|q| (q.at.x - p.x).abs() < 1.3 && (q.at.y - p.y).abs() < 1.3),
            Item::Shape(i) => self.symbol.graphics.get(i).is_some_and(|g| {
                let (a, b) = g.bounds();
                p.x >= a.x - 1.0 && p.x <= b.x + 1.0 && p.y >= a.y - 1.0 && p.y <= b.y + 1.0
            }),
        }
    }

    /// The selected shape, to change it from the properties box. Call
    /// [`Self::note_property_edit`] with the symbol from before the change, so it can be undone.
    pub fn selected_shape_mut(&mut self) -> Option<&mut Shape> {
        match self.selected {
            Some(Item::Shape(i)) => self.symbol.graphics.get_mut(i),
            _ => None,
        }
    }

    fn add_shape(&mut self, shape: Shape) {
        self.checkpoint();
        self.symbol.graphics.push(shape);
        self.selected = Some(Item::Shape(self.symbol.graphics.len() - 1));
    }

    /// The pin at `p`, else the last drawn shape whose box (1 mm wider) holds `p`.
    fn pick(&self, p: Point) -> Option<Item> {
        let pins = (0..self.symbol.pins.len()).map(Item::Pin);
        let shapes = (0..self.symbol.graphics.len()).rev().map(Item::Shape);
        pins.chain(shapes).find(|&item| self.under(item, p))
    }

    pub fn delete_selected(&mut self) {
        self.end_drag();
        let Some(item) = self.selected.take() else { return };
        match item {
            Item::Shape(i) if i < self.symbol.graphics.len() => {
                self.checkpoint();
                self.symbol.graphics.remove(i);
            }
            Item::Pin(i) if i < self.symbol.pins.len() => {
                self.checkpoint();
                self.symbol.pins.remove(i);
            }
            _ => {}
        }
    }

    /// What stops the symbol from being saved; empty when it can be.
    pub fn problems(&self) -> Vec<String> {
        let s = &self.symbol;
        let mut out = Vec::new();
        if s.id.trim().is_empty() {
            out.push("Give the symbol an id, for example my.contact.nc.".into());
        } else if let Err(e) = wc_core::store::check_id(&s.id) {
            out.push(format!("The id cannot be used: {e}"));
        }
        if s.name.trim().is_empty() {
            out.push("Give the symbol a name.".into());
        }
        if s.graphics.is_empty() {
            out.push("Draw the symbol: it has no lines or shapes yet.".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for p in &s.pins {
            // Spaces around a name are dropped on save, so `A1` and `A1 ` are one name.
            let name = p.name.trim();
            if name.is_empty() {
                out.push("Every pin needs a name.".into());
            } else if !names.insert(name) {
                out.push(format!("Two pins are called {name}."));
            }
        }
        // Two pins on one point would connect each other's wires.
        for (i, p) in s.pins.iter().enumerate() {
            if let Some(q) = s.pins[..i].iter().find(|q| q.at == p.at) {
                out.push(format!("Pins {} and {} are at the same place.", q.name, p.name));
            }
        }
        if self.targets.is_empty() {
            out.push("This project has no library folder to save into; add one in the project properties.".into());
        }
        out
    }

    /// Write the symbol into the chosen library folder. A new symbol (or one renamed here) may
    /// not replace a symbol that is already in that folder.
    pub fn save(&mut self) -> Result<Saved, String> {
        if let Some(p) = self.problems().into_iter().next() {
            return Err(p);
        }
        let (source, root) = self
            .targets
            .get(self.target)
            .ok_or("No library folder is chosen.")?
            .clone();
        self.symbol.id = self.symbol.id.trim().to_owned();
        for p in &mut self.symbol.pins {
            p.name = p.name.trim().to_owned();
        }
        let id = self.symbol.id.clone();
        let file = root.join("symbols").join(format!("{id}.json"));
        let same =
            self.opened_as.as_deref() == Some(id.as_str()) && self.opened_from.as_deref() == Some(root.as_path());
        if !same && file.exists() {
            return Err(format!(
                "Library {source} already has a symbol {id}; choose another id, or open that symbol to change it."
            ));
        }
        let path = wc_library::write_symbol(&root, &self.symbol).map_err(|e| e.to_string())?;
        self.opened_as = Some(id.clone());
        self.opened_from = Some(root.clone());
        self.message = format!("Saved {id} into library {source}.");
        Ok(Saved {
            id,
            path,
            source,
            symbol: self.symbol.clone(),
        })
    }

    /// The editor window. Returns what was saved this frame, and false once it was closed.
    /// `open_symbol` is asked for a library symbol picked in "Open", by id.
    pub fn show(&mut self, ctx: &egui::Context, open_symbol: &mut OpenSymbol<'_>) -> (Option<Saved>, bool) {
        let mut keep = true;
        let mut saved = None;
        let mut close = false;
        egui::Window::new("Symbol editor")
            .open(&mut keep)
            .default_size([820.0, 560.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for t in Tool::ALL {
                        if ui.selectable_label(self.tool == t, t.label()).clicked() {
                            self.set_tool(t);
                        }
                    }
                    ui.separator();
                    if ui.add_enabled(self.can_undo(), egui::Button::new("Undo")).clicked() {
                        self.undo();
                    }
                    if ui.add_enabled(self.can_redo(), egui::Button::new("Redo")).clicked() {
                        self.redo();
                    }
                    if ui
                        .add_enabled(self.selected.is_some(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        self.delete_selected();
                    }
                });
                ui.separator();
                // Arrow keys nudge the selection by one grid step, unless a field is being typed in.
                if self.selected.is_some() && ctx.memory(|m| m.focused().is_none()) {
                    let d = ui.input(|i| {
                        let mut d = Point::new(0.0, 0.0);
                        for (k, x, y) in [
                            (egui::Key::ArrowLeft, -SNAP, 0.0),
                            (egui::Key::ArrowRight, SNAP, 0.0),
                            (egui::Key::ArrowUp, 0.0, -SNAP),
                            (egui::Key::ArrowDown, 0.0, SNAP),
                        ] {
                            if i.key_pressed(k) {
                                d = d + Point::new(x, y);
                            }
                        }
                        d
                    });
                    if d != Point::new(0.0, 0.0) {
                        self.nudge(d);
                    }
                }
                // Enter ends a polyline, Escape drops the shape being drawn or cancels a drag.
                if (self.drawing_shape() || self.dragging()) && ctx.memory(|m| m.focused().is_none()) {
                    let (enter, escape) =
                        ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                    if escape && self.dragging() {
                        self.cancel_drag();
                    } else if escape {
                        self.cancel_shape();
                    } else if enter {
                        self.finish_polyline();
                    }
                }
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(260.0);
                        self.properties(ui, open_symbol);
                        ui.separator();
                        let problems = self.problems();
                        let save = ui
                            .add_enabled(problems.is_empty(), egui::Button::new("Save to library"))
                            .on_disabled_hover_text(problems.join("\n"));
                        if save.clicked() {
                            match self.save() {
                                Ok(s) => saved = Some(s),
                                Err(e) => self.message = e,
                            }
                        }
                        if let Some(p) = problems.first() {
                            ui.label(RichText::new(p).weak());
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                    ui.separator();
                    self.drawing(ui);
                });
                let hint = if self.message.is_empty() {
                    self.tool.hint(self.clicks.len()).to_owned()
                } else {
                    self.message.clone()
                };
                ui.label(hint);
            });
        (saved, keep && !close)
    }

    fn properties(&mut self, ui: &mut egui::Ui, open_symbol: &mut OpenSymbol<'_>) {
        if !self.choices.is_empty() {
            let mut pick = None;
            egui::ComboBox::from_id_salt("symbol_editor_open")
                .selected_text("Open a library symbol…")
                .width(250.0)
                .show_ui(ui, |ui| {
                    for (id, name) in &self.choices {
                        if ui.selectable_label(false, format!("{id} · {name}")).clicked() {
                            pick = Some(id.clone());
                        }
                    }
                });
            if let Some(id) = pick {
                match open_symbol(&id) {
                    Ok((s, source)) => {
                        let choices = std::mem::take(&mut self.choices);
                        let owner = self.owner.take();
                        *self = Self::edit(s, &source, std::mem::take(&mut self.targets));
                        self.choices = choices;
                        self.owner = owner;
                    }
                    Err(e) => self.message = format!("Symbol {id} could not be read: {e}"),
                }
            }
        }
        let before = self.symbol.clone();
        egui::Grid::new("symbol_editor_props").num_columns(2).show(ui, |ui| {
            ui.label("Id");
            ui.text_edit_singleline(&mut self.symbol.id);
            ui.end_row();
            ui.label("Name");
            ui.text_edit_singleline(&mut self.symbol.name);
            ui.end_row();
            ui.label("Category");
            ui.text_edit_singleline(&mut self.symbol.category);
            ui.end_row();
            ui.label("Letter code");
            ui.text_edit_singleline(&mut self.symbol.letter_code);
            ui.end_row();
            ui.label("Role");
            egui::ComboBox::from_id_salt("symbol_editor_role")
                .selected_text(role_label(self.symbol.role))
                .show_ui(ui, |ui| {
                    for r in [
                        SymbolRole::Standalone,
                        SymbolRole::Main,
                        SymbolRole::Auxiliary,
                        SymbolRole::Terminal,
                        SymbolRole::Interruption,
                    ] {
                        ui.selectable_value(&mut self.symbol.role, r, role_label(r));
                    }
                });
            ui.end_row();
            ui.label("Description");
            ui.text_edit_singleline(&mut self.symbol.description);
            ui.end_row();
            ui.label("Save into");
            let current = self.targets.get(self.target).map_or("(none)", |(n, _)| n.as_str());
            egui::ComboBox::from_id_salt("symbol_editor_target")
                .selected_text(current.to_owned())
                .show_ui(ui, |ui| {
                    for (i, (n, _)) in self.targets.iter().enumerate() {
                        ui.selectable_value(&mut self.target, i, n.as_str());
                    }
                });
            ui.end_row();
            ui.label("Text");
            ui.text_edit_singleline(&mut self.text);
            ui.end_row();
            ui.label("Next pin");
            ui.text_edit_singleline(&mut self.pin_name);
            ui.end_row();
        });
        if let Some(Item::Pin(i)) = self.selected
            && let Some(pin) = self.symbol.pins.get_mut(i)
        {
            ui.separator();
            ui.label(RichText::new(format!("Pin at {:.1}, {:.1}", pin.at.x, pin.at.y)).strong());
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut pin.name);
            });
            ui.horizontal(|ui| {
                ui.label("Wire leaves");
                for (d, l) in [
                    (PinDir::Up, "up"),
                    (PinDir::Down, "down"),
                    (PinDir::Left, "left"),
                    (PinDir::Right, "right"),
                ] {
                    ui.selectable_value(&mut pin.dir, d, l);
                }
            });
        }
        if let Some(shape) = self.selected_shape_mut() {
            ui.separator();
            shape_properties(ui, shape);
        }
        self.note_property_edit(before);
    }

    fn drawing(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size().max(Vec2::new(320.0, 320.0)) - Vec2::new(0.0, 24.0);
        let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = resp.rect;
        let visuals = ui.visuals();
        painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
        // 40 mm across the shorter side, the symbol origin in the middle.
        let zoom = (rect.width().min(rect.height()) / 40.0).max(2.0);
        let xf = Xf {
            origin: rect.center(),
            zoom,
        };
        let grid = visuals.weak_text_color().gamma_multiply(0.5);
        let half = Vec2::new(rect.width(), rect.height()) / (2.0 * zoom);
        let steps = |h: f32| (f64::from(h) / SNAP).ceil() as i32;
        for ix in -steps(half.x)..=steps(half.x) {
            for iy in -steps(half.y)..=steps(half.y) {
                let p = xf.s(Point::new(f64::from(ix) * SNAP, f64::from(iy) * SNAP));
                if rect.contains(p) {
                    let r = if ix % 2 == 0 && iy % 2 == 0 { 1.2 } else { 0.6 };
                    painter.circle_filled(p, r, grid);
                }
            }
        }
        let axis = Stroke::new(1.0_f32, grid);
        painter.line_segment(
            [
                Pos2::new(rect.left(), xf.origin.y),
                Pos2::new(rect.right(), xf.origin.y),
            ],
            axis,
        );
        painter.line_segment(
            [
                Pos2::new(xf.origin.x, rect.top()),
                Pos2::new(xf.origin.x, rect.bottom()),
            ],
            axis,
        );

        let ink = visuals.text_color();
        let pick = visuals.selection.stroke.color;
        let stroke = Stroke::new((zoom * 0.25).max(1.2), ink);
        for (i, g) in self.symbol.graphics.iter().enumerate() {
            let s = if self.selected == Some(Item::Shape(i)) {
                Stroke::new(stroke.width + 1.0, pick)
            } else {
                stroke
            };
            paint_shape(&painter, &xf, g, s);
            if self.selected == Some(Item::Shape(i)) {
                for h in handles(g) {
                    let r = egui::Rect::from_center_size(xf.s(h), Vec2::splat(7.0));
                    painter.rect_filled(r, 1.0, visuals.extreme_bg_color);
                    painter.rect_stroke(r, 1.0, Stroke::new(1.5_f32, pick), egui::StrokeKind::Middle);
                }
            }
        }
        let pin_col = Color32::from_rgb(200, 60, 60);
        for (i, p) in self.symbol.pins.iter().enumerate() {
            let c = xf.s(p.at);
            let col = if self.selected == Some(Item::Pin(i)) {
                pick
            } else {
                pin_col
            };
            painter.circle_stroke(c, 4.0, Stroke::new(1.5_f32, col));
            let out = match p.dir {
                PinDir::Up => Vec2::new(0.0, -1.0),
                PinDir::Down => Vec2::new(0.0, 1.0),
                PinDir::Left => Vec2::new(-1.0, 0.0),
                PinDir::Right => Vec2::new(1.0, 0.0),
            };
            painter.line_segment([c, c + out * 10.0], Stroke::new(1.5_f32, col));
            painter.text(
                c + Vec2::new(5.0, -5.0),
                egui::Align2::LEFT_BOTTOM,
                &p.name,
                egui::FontId::proportional(12.0),
                col,
            );
        }
        let tag = xf.s(self.symbol.tag_at);
        painter.text(
            tag,
            egui::Align2::RIGHT_CENTER,
            "-K?",
            egui::FontId::proportional((zoom * 2.5).max(10.0)),
            visuals.weak_text_color(),
        );
        // The clicks so far of a shape being drawn, and the cursor snapped to the grid.
        for c in &self.clicks {
            painter.circle_filled(xf.s(*c), 3.0, pick);
        }
        if self.tool == Tool::Polyline {
            for w in self.clicks.windows(2) {
                if let [a, b] = w {
                    painter.line_segment([xf.s(*a), xf.s(*b)], Stroke::new(1.0_f32, pick));
                }
            }
        }
        if let Some(h) = resp.hover_pos() {
            let at = snap(xf.mm(h));
            painter.circle_stroke(xf.s(at), 3.0, Stroke::new(1.0_f32, pick));
            // A polyline goes on from its last point; the other shapes from their first.
            let from = if self.tool == Tool::Polyline {
                self.clicks.last()
            } else {
                self.clicks.first()
            };
            if let Some(a) = from {
                painter.line_segment([xf.s(*a), xf.s(at)], Stroke::new(1.0_f32, pick));
            }
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let at = xf.mm(pos);
            let closed_at = self.closed_at.take();
            // egui calls a quick click a double- or triple-click wherever it lands (A, B, B in
            // a row makes the last a triple-click): only a quick click on the polyline's last
            // point ends it, and the quick click after one that closed it on its first point
            // does nothing more.
            let double = (resp.double_clicked() || resp.triple_clicked()) && self.tool == Tool::Polyline;
            let paired = double && self.clicks.is_empty() && closed_at == Some(snap(at));
            if double && self.clicks.len() >= 2 && self.clicks.last() == Some(&snap(at)) {
                self.finish_polyline();
            } else if !paired {
                self.click(at);
            }
        }
        if resp.secondary_clicked() {
            self.cancel_shape();
        }
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(pos) = ui.input(|i| i.pointer.press_origin())
        {
            self.start_drag(xf.mm(pos));
        }
        if self.dragging()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            self.drag_to(xf.mm(pos));
        }
        if resp.drag_stopped() {
            self.end_drag();
        }
    }
}

/// The properties box of the selected shape: what can be changed after drawing it.
fn shape_properties(ui: &mut egui::Ui, shape: &mut Shape) {
    fn mm(v: &mut f64, min: f64) -> egui::DragValue<'_> {
        egui::DragValue::new(v)
            .speed(0.1)
            .range(min..=1000.0)
            .clamp_existing_to_range(false)
            .suffix(" mm")
    }
    fn deg(v: &mut f64) -> egui::DragValue<'_> {
        egui::DragValue::new(v)
            .speed(1.0)
            .range(0.0..=360.0)
            .clamp_existing_to_range(false)
            .suffix("°")
    }
    match shape {
        Shape::Dimension { a, b, .. } => {
            ui.label(RichText::new("Dimension").strong());
            ui.label(format!("{} mm", wc_core::geom::format_length(a.distance(*b))));
        }
        Shape::Line { a, b } => {
            ui.label(RichText::new("Line").strong());
            ui.label(format!("From {:.1}, {:.1} to {:.1}, {:.1}", a.x, a.y, b.x, b.y));
        }
        Shape::Polyline { points, closed, filled } => {
            polyline_properties(ui, points.len(), closed, filled);
        }
        Shape::Rect { min, max } => {
            ui.label(RichText::new("Rectangle").strong());
            ui.label(format!(
                "{:.1} × {:.1} mm at {:.1}, {:.1}",
                max.x - min.x,
                max.y - min.y,
                min.x,
                min.y
            ));
        }
        Shape::Circle { radius, filled, .. } => {
            ui.label(RichText::new("Circle").strong());
            egui::Grid::new("symbol_editor_shape").num_columns(2).show(ui, |ui| {
                ui.label("Radius");
                ui.add(mm(radius, 0.1));
                ui.end_row();
                ui.label("Filled");
                ui.checkbox(filled, "");
                ui.end_row();
            });
        }
        Shape::Arc { radius, start, end, .. } => {
            ui.label(RichText::new("Arc").strong());
            egui::Grid::new("symbol_editor_shape").num_columns(2).show(ui, |ui| {
                ui.label("Radius");
                ui.add(mm(radius, 0.1));
                ui.end_row();
                ui.label("Starts at");
                ui.add(deg(start));
                ui.end_row();
                ui.label("Ends at");
                ui.add(deg(end));
                ui.end_row();
            });
        }
        Shape::Text {
            text,
            height,
            align,
            angle,
            ..
        } => {
            ui.label(RichText::new("Text").strong());
            egui::Grid::new("symbol_editor_shape").num_columns(2).show(ui, |ui| {
                ui.label("Text");
                ui.text_edit_singleline(text);
                ui.end_row();
                ui.label("Height");
                ui.add(mm(height, 0.5));
                ui.end_row();
                ui.label("Align");
                ui.horizontal(|ui| {
                    for (a, l) in [
                        (Align::Left, "left"),
                        (Align::Center, "centre"),
                        (Align::Right, "right"),
                    ] {
                        ui.selectable_value(align, a, l);
                    }
                });
                ui.end_row();
                ui.label("Angle");
                egui::ComboBox::from_id_salt("symbol_editor_text_angle")
                    .selected_text(format!("{angle}°"))
                    .show_ui(ui, |ui| {
                        for a in [0.0, 90.0, 180.0, 270.0] {
                            ui.selectable_value(angle, a, format!("{a}°"));
                        }
                    });
                ui.end_row();
            });
        }
    }
}

/// A polyline's Closed and Filled boxes. Opening a polyline also clears Filled, in the same
/// edit: only a closed outline has an inside to fill. Returns the Closed box.
fn polyline_properties(ui: &mut egui::Ui, points: usize, closed: &mut bool, filled: &mut bool) -> egui::Response {
    ui.label(RichText::new("Polyline").strong());
    ui.label(format!("{points} points"));
    egui::Grid::new("symbol_editor_shape")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Closed");
            let can = *closed || points >= 3;
            let r = ui
                .add_enabled(can, egui::Checkbox::without_text(closed))
                .on_disabled_hover_text("A polyline needs three points or more to close.");
            if r.changed() && !*closed {
                *filled = false;
            }
            ui.end_row();
            ui.label("Filled");
            ui.add_enabled(*closed || *filled, egui::Checkbox::without_text(filled))
                .on_disabled_hover_text("Close the polyline to fill it.");
            ui.end_row();
            r
        })
        .inner
}

fn role_label(r: SymbolRole) -> &'static str {
    match r {
        SymbolRole::Standalone => "Standalone",
        SymbolRole::Main => "Main (coil)",
        SymbolRole::Auxiliary => "Auxiliary (contact)",
        SymbolRole::Terminal => "Terminal",
        SymbolRole::Interruption => "Interruption point",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn draw_coil(e: &mut SymbolEditor) {
        e.set_tool(Tool::Rect);
        e.click(Point::new(-5.0, -2.6));
        e.click(Point::new(5.1, 2.4));
        e.set_tool(Tool::Line);
        e.click(Point::new(0.0, -2.5));
        e.click(Point::new(0.0, -5.0));
        e.click(Point::new(0.0, 2.5));
        e.click(Point::new(0.0, 5.0));
        e.set_tool(Tool::Pin);
        e.pin_name = "A1".into();
        e.click(Point::new(0.0, -5.0));
        e.click(Point::new(0.1, 5.0));
        e.set_tool(Tool::Tag);
        e.click(Point::new(-10.0, 0.0));
        e.symbol.id = "test.coil".into();
        e.symbol.name = "Test coil".into();
        e.symbol.letter_code = "K".into();
        e.symbol.role = SymbolRole::Main;
    }

    #[test]
    fn cancel_shape_drops_a_half_drawn_line_and_nothing_else() {
        let mut e = SymbolEditor::new(Vec::new());
        e.set_tool(Tool::Line);
        e.click(Point::new(0.0, 0.0));
        assert!(e.drawing_shape());
        assert_eq!(e.tool.hint(e.clicks.len()), "Click the end of the line.");
        e.cancel_shape();
        assert!(!e.drawing_shape());
        assert_eq!(e.tool.hint(e.clicks.len()), "Click the start of the line.");
        assert!(e.symbol.graphics.is_empty());
        // Nothing half drawn: nothing changes.
        e.cancel_shape();
        assert_eq!(e.tool, Tool::Line);
        assert!(e.symbol.graphics.is_empty());
    }

    #[test]
    fn drawing_snaps_and_names_pins_in_turn() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        let s = &e.symbol;
        assert_eq!(s.graphics.len(), 3);
        assert_eq!(
            s.graphics[0],
            Shape::Rect {
                min: Point::new(-5.0, -2.5),
                max: Point::new(5.0, 2.5)
            }
        );
        let pins: Vec<_> = s.pins.iter().map(|p| (p.name.as_str(), p.at, p.dir)).collect();
        assert_eq!(
            pins,
            [
                ("A1", Point::new(0.0, -5.0), PinDir::Up),
                ("A2", Point::new(0.0, 5.0), PinDir::Down)
            ]
        );
        assert_eq!(s.tag_at, Point::new(-10.0, 0.0));
        assert_eq!(e.pin_name, "A3");
        // A pin name in use, or a place taken, is refused.
        e.set_tool(Tool::Pin);
        e.pin_name = "A1".into();
        e.click(Point::new(10.0, 0.0));
        assert_eq!(e.symbol.pins.len(), 2);
        assert!(e.message.contains("already a pin A1"), "{}", e.message);
        e.pin_name = "X9".into();
        e.click(Point::new(0.0, 5.0));
        assert_eq!(e.symbol.pins.len(), 2);
        assert!(e.message.contains("Pin A2 is already there"), "{}", e.message);
        // The last possible number does not overflow.
        assert_eq!(next_name("18446744073709551615"), "18446744073709551615");
        // No library folder: cannot be saved, and says why.
        assert!(e.problems().iter().any(|p| p.contains("library folder")));
    }

    #[test]
    fn arcs_circles_texts_select_delete_and_undo() {
        let mut e = SymbolEditor::new(Vec::new());
        e.set_tool(Tool::Arc);
        for p in [(0.0, 0.0), (5.0, 0.0), (0.0, 5.0)] {
            e.click(Point::new(p.0, p.1));
        }
        assert_eq!(
            e.symbol.graphics,
            [Shape::Arc {
                center: Point::new(0.0, 0.0),
                radius: 5.0,
                start: 0.0,
                end: 90.0
            }]
        );
        e.set_tool(Tool::Circle);
        e.click(Point::new(10.0, 10.0));
        e.click(Point::new(12.5, 10.0));
        e.set_tool(Tool::Text);
        e.click(Point::new(0.0, 0.0));
        assert_eq!(e.symbol.graphics.len(), 2, "no text typed, nothing placed");
        e.text = "M".into();
        e.click(Point::new(-2.5, 2.5));
        assert_eq!(e.symbol.graphics.len(), 3);

        e.set_tool(Tool::Select);
        e.click(Point::new(10.0, 12.5));
        assert_eq!(e.selected, Some(Item::Shape(1)));
        // A shape off the grid (from an import) is picked where it is clicked, not snapped.
        e.symbol.graphics.push(Shape::Line {
            a: Point::new(20.0, 1.25),
            b: Point::new(30.0, 1.25),
        });
        e.click(Point::new(25.0, 1.3));
        assert_eq!(e.selected, Some(Item::Shape(3)));
        e.symbol.graphics.pop();
        e.selected = Some(Item::Shape(1));
        e.delete_selected();
        assert_eq!(e.symbol.graphics.len(), 2);
        e.undo();
        assert_eq!(e.symbol.graphics.len(), 3);
        assert!(matches!(e.symbol.graphics[1], Shape::Circle { radius, .. } if radius == 2.5));
    }

    #[test]
    fn saved_symbol_reopens_and_can_be_placed_and_wired() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let lib = tmp.path().join("lib");
        let mut e = SymbolEditor::new(vec![("mine".into(), lib.clone())]);
        draw_coil(&mut e);
        assert!(e.problems().is_empty(), "{:?}", e.problems());
        let saved = e.save()?;
        assert_eq!(saved.path, lib.join("symbols").join("test.coil.json"));

        // Read back through the library, as the project does.
        let dir = tmp.path().join("proj");
        let mut project = wc_core::store::create(&dir, "Demo")?;
        project.meta.libraries = vec![wc_core::project::LibrarySource {
            name: "mine".into(),
            kind: "folder".into(),
            location: lib.display().to_string(),
            reference: None,
        }];
        let library = wc_library::Library::open(&dir, &project)?;
        let (back, source) = library.symbol("test.coil")?;
        assert_eq!(source, "mine");
        assert_eq!(back, e.symbol);
        let hash = back.hash();

        // Placed on a page through the same path as the app, and wired pin to pin.
        wc_library::take(
            &dir,
            &mut project,
            &library,
            wc_core::lock::ItemKind::Symbol,
            "test.coil",
        )?;
        assert!(wc_core::store::read_vendored_symbol(&dir, "test.coil").is_ok());
        let mut page = wc_core::Page::new("p0001", "1", "Test");
        for (id, y) in [("s1", 50.0), ("s2", 60.0)] {
            page.symbols.push(wc_core::PlacedSymbol {
                id: id.into(),
                symbol: "test.coil".into(),
                at: Point::new(50.0, y),
                rotation: Default::default(),
                mirror: false,
                tag: None,
            });
        }
        let items: std::collections::BTreeSet<String> = ["s1".into(), "s2".into()].into();
        let wires = wc_core::autowire::suggest(&page, |_| Some(&back), &items);
        assert_eq!(wires.len(), 1, "A2 of the upper coil meets A1 of the lower one");

        // Opened again, changed and saved: same file, new hash; a new symbol with that id is refused.
        let mut again = SymbolEditor::edit(back, &source, vec![("mine".into(), lib.clone())]);
        again.set_tool(Tool::Line);
        again.click(Point::new(-5.0, 0.0));
        again.click(Point::new(5.0, 0.0));
        again.save()?;
        let (changed, _) = wc_library::Library::open(&dir, &project)?.symbol("test.coil")?;
        assert_ne!(changed.hash(), hash);
        let status = wc_library::status(&dir, &project, &wc_library::Library::open(&dir, &project)?);
        assert!(
            status
                .iter()
                .any(|s| s.id == "test.coil" && s.state == wc_library::State::Changed)
        );

        let mut dup = SymbolEditor::new(vec![("mine".into(), lib)]);
        draw_coil(&mut dup);
        assert!(dup.save().is_err_and(|m| m.contains("already has")));
        Ok(())
    }

    #[test]
    fn a_dragged_shape_moves_by_grid_steps_and_is_one_undo_step() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        let drawn = e.symbol.clone();
        let undo_steps = e.undo.len();
        e.set_tool(Tool::Select);
        // Grab the rectangle's left side and move the mouse around before letting go.
        e.start_drag(Point::new(-5.0, 0.0));
        assert_eq!(e.selected, Some(Item::Shape(0)));
        assert!(e.dragging());
        e.drag_to(Point::new(-4.0, 0.4));
        e.drag_to(Point::new(-1.9, 1.2));
        e.end_drag();
        assert!(!e.dragging());
        assert_eq!(
            e.symbol.graphics[0],
            Shape::Rect {
                min: Point::new(-2.5, -2.5),
                max: Point::new(7.5, 2.5)
            },
            "moved 2.5 mm right; the 1.2 mm down is less than half a step"
        );
        assert_eq!(e.symbol.graphics[1..], drawn.graphics[1..], "nothing else moved");
        assert_eq!(e.undo.len(), undo_steps + 1, "the whole drag is one step");
        e.undo();
        assert_eq!(e.symbol, drawn);

        // A drag that ends where it started leaves no undo step.
        e.start_drag(Point::new(-5.0, 0.0));
        e.drag_to(Point::new(-3.0, 0.0));
        e.drag_to(Point::new(-5.2, 0.3));
        e.end_drag();
        assert_eq!(e.symbol, drawn);
        assert_eq!(e.undo.len(), undo_steps);

        // Dragging needs the Select tool, and empty paper selects nothing.
        e.set_tool(Tool::Line);
        e.start_drag(Point::new(-5.0, 0.0));
        assert!(!e.dragging());
        e.set_tool(Tool::Select);
        e.start_drag(Point::new(15.0, 15.0));
        assert!(!e.dragging());
        assert_eq!(e.selected, None);
    }

    #[test]
    fn a_corner_dragged_onto_the_opposite_side_leaves_the_rectangle_as_it_was() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        e.set_tool(Tool::Select);
        e.click(Point::new(-5.0, 0.0));
        assert_eq!(e.selected, Some(Item::Shape(0)));
        let drawn = e.symbol.clone();
        let steps = e.undo.len();
        // The bottom-right corner (5,2.5) onto the opposite corner's x, then onto its y.
        for to in [Point::new(-5.0, 5.0), Point::new(10.0, -2.5)] {
            e.start_drag(Point::new(5.0, 2.5));
            e.drag_to(to);
            e.end_drag();
            assert_eq!(e.symbol, drawn, "no rectangle of no area at {to:?}");
            assert_eq!(e.undo.len(), steps, "and no undo step");
        }
    }

    #[test]
    fn an_off_grid_line_end_lands_on_the_grid_point_under_the_pointer() {
        let mut e = SymbolEditor::new(Vec::new());
        e.symbol.graphics.push(Shape::Line {
            a: Point::new(20.0, 1.25),
            b: Point::new(20.0, -10.0),
        });
        e.set_tool(Tool::Select);
        e.click(Point::new(20.0, -5.0));
        assert_eq!(e.selected, Some(Item::Shape(0)));
        e.start_drag(Point::new(20.0, 1.25));
        e.drag_to(Point::new(20.0, 5.0));
        e.end_drag();
        assert_eq!(
            e.symbol.graphics[0],
            Shape::Line {
                a: Point::new(20.0, 5.0),
                b: Point::new(20.0, -10.0)
            }
        );
    }

    #[test]
    fn dragging_a_line_end_or_a_corner_reshapes_it() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        let drawn = e.symbol.clone();
        e.set_tool(Tool::Select);

        // The upper lead (line 1, from (0,-2.5) to (0,-5)): longer by dragging its outer end.
        e.click(Point::new(0.0, -3.5));
        assert_eq!(e.selected, Some(Item::Shape(1)));
        let steps = e.undo.len();
        e.start_drag(Point::new(0.3, -5.2));
        e.drag_to(Point::new(0.4, -9.8));
        e.end_drag();
        assert_eq!(
            e.symbol.graphics[1],
            Shape::Line {
                a: Point::new(0.0, -2.5),
                b: Point::new(0.0, -10.0)
            },
            "only that end moved, on the grid"
        );
        assert_eq!(e.symbol.pins, drawn.pins, "the pin there stays");
        assert_eq!(e.undo.len(), steps + 1, "one handle drag is one step");
        // Onto the other end: a line of no length is not made.
        e.start_drag(Point::new(0.0, -10.0));
        e.drag_to(Point::new(0.0, -2.5));
        e.end_drag();
        assert_eq!(e.undo.len(), steps + 1);

        // The rectangle (-5,-2.5)-(5,2.5): drag its bottom-right corner, then past the opposite
        // corner, where it turns over and stays ordered.
        e.click(Point::new(-5.0, 0.0));
        assert_eq!(e.selected, Some(Item::Shape(0)));
        e.start_drag(Point::new(5.0, 2.5));
        e.drag_to(Point::new(10.0, 5.0));
        e.end_drag();
        assert_eq!(
            e.symbol.graphics[0],
            Shape::Rect {
                min: Point::new(-5.0, -2.5),
                max: Point::new(10.0, 5.0)
            }
        );
        e.start_drag(Point::new(10.0, 5.0));
        e.drag_to(Point::new(-10.0, -10.0));
        e.end_drag();
        assert_eq!(
            e.symbol.graphics[0],
            Shape::Rect {
                min: Point::new(-10.0, -10.0),
                max: Point::new(-5.0, -2.5)
            }
        );
        // Grabbing the side, not a corner, still moves the whole rectangle.
        e.start_drag(Point::new(-7.5, -2.5));
        e.drag_to(Point::new(-5.0, -2.5));
        e.end_drag();
        assert_eq!(
            e.symbol.graphics[0],
            Shape::Rect {
                min: Point::new(-7.5, -10.0),
                max: Point::new(-2.5, -2.5)
            }
        );
        e.undo();
        e.undo();
        e.undo();
        e.undo();
        assert_eq!(e.symbol, drawn, "each drag was one step");
    }

    #[test]
    fn a_circle_edge_and_arc_ends_reshape_them() {
        let c = Point::new(0.0, 0.0);
        let circle = Shape::Circle {
            center: c,
            radius: 5.0,
            filled: true,
        };
        assert_eq!(handles(&circle), [Point::new(5.0, 0.0)]);
        // The radius is the distance to the point, on the grid; the fill stays.
        assert_eq!(
            with_handle(&circle, 0, Point::new(0.0, 7.6)),
            Some(Shape::Circle {
                center: c,
                radius: 7.5,
                filled: true
            })
        );
        assert_eq!(with_handle(&circle, 0, Point::new(1.0, 0.0)), None, "radius zero");
        assert_eq!(with_handle(&circle, 1, Point::new(5.0, 5.0)), None);

        let arc = Shape::Arc {
            center: c,
            radius: 5.0,
            start: 0.0,
            end: 90.0,
        };
        let [s, e] = handles(&arc)[..] else { unreachable!() };
        assert_eq!(s, Point::new(5.0, 0.0));
        assert!((e.x).abs() < 1e-9 && (e.y - 5.0).abs() < 1e-9);
        // Each end turns to the point; radius and the other end stay.
        assert_eq!(
            with_handle(&arc, 0, Point::new(-10.0, 0.0)),
            Some(Shape::Arc {
                center: c,
                radius: 5.0,
                start: 180.0,
                end: 90.0
            })
        );
        assert_eq!(
            with_handle(&arc, 1, Point::new(0.0, -2.5)),
            Some(Shape::Arc {
                center: c,
                radius: 5.0,
                start: 0.0,
                end: 270.0
            })
        );
        assert_eq!(with_handle(&arc, 1, Point::new(2.5, 0.0)), None, "ends meet");
        assert_eq!(with_handle(&arc, 0, c), None, "no direction");
        // An end stored as 360 meets an end dragged to 0 degrees.
        let full = Shape::Arc {
            center: c,
            radius: 5.0,
            start: 360.0,
            end: 90.0,
        };
        assert_eq!(with_handle(&full, 1, Point::new(5.0, 0.0)), None, "360 and 0 meet");
        assert_eq!(
            with_handle(&full, 1, Point::new(-5.0, 0.0)),
            Some(Shape::Arc {
                center: c,
                radius: 5.0,
                start: 360.0,
                end: 180.0
            }),
            "another direction is taken; the stored 360 stays"
        );
    }

    #[test]
    fn dragging_a_circle_edge_is_one_undo_step() {
        let mut e = SymbolEditor::new(Vec::new());
        e.set_tool(Tool::Circle);
        e.click(Point::new(0.0, 0.0));
        e.click(Point::new(5.0, 0.0));
        let drawn = e.symbol.clone();
        let steps = e.undo.len();
        e.set_tool(Tool::Select);
        e.selected = Some(Item::Shape(0));
        e.start_drag(Point::new(5.0, 0.0));
        e.drag_to(Point::new(10.1, 0.0));
        e.drag_to(Point::new(0.0, 12.4));
        e.end_drag();
        assert_eq!(
            e.symbol.graphics[0],
            Shape::Circle {
                center: Point::new(0.0, 0.0),
                radius: 12.5,
                filled: false
            }
        );
        assert_eq!(e.undo.len(), steps + 1, "one drag is one step");
        e.undo();
        assert_eq!(e.symbol, drawn, "undo brings the old circle back");
        // Off the axes: the pointer is not snapped first (that would give (5, 2.5) and keep
        // radius 5); its distance 6.64 rounds to 7.5.
        e.selected = Some(Item::Shape(0));
        e.start_drag(Point::new(5.0, 0.0));
        e.drag_to(Point::new(6.2, 2.4));
        e.end_drag();
        assert!(matches!(e.symbol.graphics[0], Shape::Circle { radius, .. } if radius == 7.5));
    }

    #[test]
    fn dragging_an_arc_end_onto_its_centre_changes_nothing() {
        let mut e = SymbolEditor::new(Vec::new());
        let arc = Shape::Arc {
            center: Point::new(0.0, 0.0),
            radius: 5.0,
            start: 0.0,
            end: 180.0,
        };
        e.symbol.graphics.push(arc.clone());
        e.set_tool(Tool::Select);
        e.selected = Some(Item::Shape(0));
        // The end at 180 degrees sits at y = 6e-16, not 0, so the drag lands just off the centre.
        e.start_drag(Point::new(-5.0, 0.0));
        e.drag_to(Point::new(0.0, 0.0));
        e.end_drag();
        assert_eq!(e.symbol.graphics[0], arc);
        assert!(e.undo.is_empty());
        assert_eq!(with_handle(&arc, 0, Point::new(1e-12, -1e-12)), None);
        // Grabbed half a millimetre off the handle: the pointer on the centre still changes
        // nothing (the handle plus the pointer's move would be (0, -0.5), about 270 degrees).
        e.selected = Some(Item::Shape(0));
        e.start_drag(Point::new(-5.0, 0.5));
        assert!(e.drag.as_ref().is_some_and(|d| d.handle == Some(1)));
        e.drag_to(Point::new(0.0, 0.0));
        e.end_drag();
        assert_eq!(e.symbol.graphics[0], arc);
        assert!(e.undo.is_empty());
    }

    #[test]
    fn an_arc_end_clicked_in_the_start_direction_draws_no_circle() {
        let mut e = SymbolEditor::new(Vec::new());
        e.set_tool(Tool::Arc);
        // The end on the start itself, then further out on the same line from the centre.
        for p in [(0.0, 0.0), (5.0, 0.0), (5.0, 0.0), (10.0, 0.0)] {
            e.click(Point::new(p.0, p.1));
        }
        assert!(e.symbol.graphics.is_empty());
        assert!(e.undo.is_empty());
        assert!(!e.message.is_empty());
        assert_eq!(e.clicks, [Point::new(0.0, 0.0), Point::new(5.0, 0.0)]);
        // Another end still finishes the arc from the same centre and start.
        e.click(Point::new(0.0, 5.0));
        assert_eq!(
            e.symbol.graphics,
            [Shape::Arc {
                center: Point::new(0.0, 0.0),
                radius: 5.0,
                start: 0.0,
                end: 90.0
            }]
        );
    }

    #[test]
    fn escape_cancels_a_handle_drag() {
        let mut e = SymbolEditor::new(Vec::new());
        e.set_tool(Tool::Circle);
        e.click(Point::new(0.0, 0.0));
        e.click(Point::new(5.0, 0.0));
        let drawn = e.symbol.clone();
        let steps = e.undo.len();
        e.set_tool(Tool::Select);
        e.selected = Some(Item::Shape(0));
        e.start_drag(Point::new(5.0, 0.0));
        e.drag_to(Point::new(10.0, 0.0));
        assert_ne!(e.symbol, drawn);
        e.cancel_drag();
        assert!(!e.dragging());
        assert_eq!(e.symbol, drawn, "the circle is back as it was");
        // The mouse moving on and being let go changes nothing and leaves no undo step.
        e.drag_to(Point::new(15.0, 0.0));
        e.end_drag();
        assert_eq!(e.symbol, drawn);
        assert_eq!(e.undo.len(), steps);
    }

    #[test]
    fn dragging_an_arc_end_off_the_axes_keeps_its_angle() {
        let mut e = SymbolEditor::new(Vec::new());
        e.symbol.graphics.push(Shape::Arc {
            center: Point::new(0.0, 0.0),
            radius: 5.0,
            start: 90.0,
            end: 180.0,
        });
        e.set_tool(Tool::Select);
        e.selected = Some(Item::Shape(0));
        // Grab the end at (-5, 0) and drag it to (5, 1): about 11.3 degrees, not 0.
        e.start_drag(Point::new(-5.0, 0.0));
        e.drag_to(Point::new(5.0, 1.0));
        e.end_drag();
        let Shape::Arc { radius, start, end, .. } = e.symbol.graphics[0] else {
            unreachable!()
        };
        assert_eq!((radius, start), (5.0, 90.0));
        assert!((end - 1.0_f64.atan2(5.0).to_degrees()).abs() < 1e-9, "end is {end}");
    }

    #[test]
    fn two_pins_at_the_same_place_cannot_be_saved() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let mut drawn = SymbolEditor::new(Vec::new());
        draw_coil(&mut drawn);
        // An opened symbol (from an import, say) with pin A2 on top of pin A1.
        let mut symbol = drawn.symbol;
        symbol.pins[1].at = symbol.pins[0].at;
        let mut e = SymbolEditor::edit(symbol, "mine", vec![("mine".into(), tmp.path().join("lib"))]);
        assert_eq!(e.problems(), ["Pins A1 and A2 are at the same place."]);
        assert!(e.save().is_err_and(|m| m.contains("Pins A1 and A2")));
        // Moving A2 off A1 clears it.
        e.selected = Some(Item::Pin(1));
        e.nudge(Point::new(SNAP, 0.0));
        assert!(e.problems().is_empty(), "{:?}", e.problems());
        e.save()?;
        Ok(())
    }

    #[test]
    fn pin_names_are_saved_without_spaces_around_them() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let mut e = SymbolEditor::new(vec![("mine".into(), tmp.path().join("lib"))]);
        draw_coil(&mut e);
        e.symbol.id = "my.coil".into();
        e.symbol.name = "Coil".into();
        // A2 renamed in its Name field to A1 with a space after it.
        e.symbol.pins[1].name = "A1 ".into();
        assert_eq!(e.problems(), ["Two pins are called A1."]);
        e.symbol.pins[1].name = " A2 ".into();
        assert!(e.problems().is_empty(), "{:?}", e.problems());
        let saved = e.save()?;
        assert_eq!(saved.symbol.pins[1].name, "A2");
        let back: Symbol = serde_json::from_str(&std::fs::read_to_string(&saved.path)?)?;
        assert!(back.pin("A2").is_some(), "{:?}", back.pins);
        Ok(())
    }

    #[test]
    fn a_dragged_pin_stays_on_the_grid_and_off_other_pins() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        e.set_tool(Tool::Select);
        e.start_drag(Point::new(0.0, 5.0));
        assert_eq!(e.selected, Some(Item::Pin(1)), "the pin wins over the line under it");
        e.drag_to(Point::new(3.0, 6.0));
        e.end_drag();
        assert_eq!(e.symbol.pins[1].at, Point::new(2.5, 5.0));
        // Onto the other pin: it stays where it was.
        e.start_drag(Point::new(2.5, 5.0));
        e.drag_to(Point::new(0.0, -5.0));
        assert_eq!(e.symbol.pins[1].at, Point::new(2.5, 5.0));
        e.end_drag();
    }

    #[test]
    fn arrow_keys_nudge_the_selection_one_step_each() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        let drawn = e.symbol.clone();
        e.set_tool(Tool::Select);
        e.click(Point::new(0.0, -3.5));
        assert_eq!(e.selected, Some(Item::Shape(1)));
        e.nudge(Point::new(SNAP, 0.0));
        e.nudge(Point::new(0.0, -SNAP));
        assert_eq!(
            e.symbol.graphics[1],
            Shape::Line {
                a: Point::new(2.5, -5.0),
                b: Point::new(2.5, -7.5)
            }
        );
        e.undo();
        assert_eq!(
            e.symbol.graphics[1],
            Shape::Line {
                a: Point::new(2.5, -2.5),
                b: Point::new(2.5, -5.0)
            },
            "each nudge is its own step"
        );
        e.undo();
        assert_eq!(e.symbol, drawn);
        // Nothing selected: nothing moves.
        e.selected = None;
        e.nudge(Point::new(SNAP, 0.0));
        assert_eq!(e.symbol, drawn);
    }

    #[test]
    fn a_drawn_text_and_circle_can_be_changed() -> TestResult {
        let mut e = SymbolEditor::new(Vec::new());
        e.text = "K1".into();
        e.set_tool(Tool::Text);
        e.click(Point::new(5.0, 5.0));
        e.set_tool(Tool::Circle);
        e.click(Point::new(0.0, 0.0));
        e.click(Point::new(5.0, 0.0));
        let drawn = e.symbol.clone();

        // The circle is selected after drawing it: fill it and make it smaller.
        let before = e.symbol.clone();
        if let Some(Shape::Circle { radius, filled, .. }) = e.selected_shape_mut() {
            *radius = 2.5;
            *filled = true;
        } else {
            return Err("the circle should be selected".into());
        }
        e.note_property_edit(before);
        assert_eq!(
            e.symbol.graphics[1],
            Shape::Circle {
                center: Point::new(0.0, 0.0),
                radius: 2.5,
                filled: true
            }
        );

        // Select the text and change its words and height.
        e.set_tool(Tool::Select);
        e.click(Point::new(5.5, 5.0));
        assert_eq!(e.selected, Some(Item::Shape(0)));
        for (words, height) in [("K", 2.5), ("K10", 2.5), ("K10", 3.5)] {
            let before = e.symbol.clone();
            if let Some(Shape::Text { text, height: h, .. }) = e.selected_shape_mut() {
                *text = words.into();
                *h = height;
            }
            e.note_property_edit(before);
        }
        let Shape::Text { text, height, .. } = &e.symbol.graphics[0] else {
            return Err("still a text".into());
        };
        assert_eq!((text.as_str(), *height), ("K10", 3.5));
        e.undo();
        assert!(
            matches!(&e.symbol.graphics[0], Shape::Text { text, .. } if text == "K1"),
            "the typing is one step"
        );
        assert!(
            matches!(e.symbol.graphics[1], Shape::Circle { filled: true, .. }),
            "the fill stays"
        );
        e.undo();
        assert_eq!(e.symbol, drawn);

        // A pin has no shape properties.
        e.selected = Some(Item::Pin(0));
        assert!(e.selected_shape_mut().is_none());
        Ok(())
    }

    #[test]
    fn an_arc_and_a_text_alignment_and_angle_can_be_changed() {
        let mut e = SymbolEditor::new(Vec::new());
        e.set_tool(Tool::Arc);
        e.click(Point::new(0.0, 0.0));
        e.click(Point::new(5.0, 0.0));
        e.click(Point::new(0.0, 5.0));
        e.text = "A".into();
        e.set_tool(Tool::Text);
        e.click(Point::new(10.0, 10.0));
        let drawn = e.symbol.clone();

        e.set_tool(Tool::Select);
        e.click(Point::new(2.5, 2.5));
        assert_eq!(e.selected, Some(Item::Shape(0)));
        let before = e.symbol.clone();
        if let Some(Shape::Arc { radius, start, end, .. }) = e.selected_shape_mut() {
            (*radius, *start, *end) = (7.5, 180.0, 270.0);
        }
        e.note_property_edit(before);
        let arc = e.symbol.graphics[0].clone();
        assert!(matches!(arc, Shape::Arc { radius, start, end, .. } if (radius, start, end) == (7.5, 180.0, 270.0)));

        // Another item: its edit is a step of its own.
        e.click(Point::new(10.5, 10.0));
        assert_eq!(e.selected, Some(Item::Shape(1)));
        let before = e.symbol.clone();
        if let Some(Shape::Text { align, angle, .. }) = e.selected_shape_mut() {
            (*align, *angle) = (Align::Center, 90.0);
        }
        e.note_property_edit(before);
        assert!(matches!(e.symbol.graphics[1], Shape::Text { align: Align::Center, angle, .. } if angle == 90.0));

        e.undo();
        assert_eq!(e.symbol.graphics[1], drawn.graphics[1], "the text is back");
        assert_eq!(e.symbol.graphics[0], arc, "the arc edit stays");
        e.undo();
        assert_eq!(e.symbol, drawn);
    }

    #[test]
    fn showing_the_properties_leaves_odd_values_alone() {
        // Loaded symbols can hold values the boxes would not let you type.
        let shapes = [
            Shape::Text {
                at: Point::new(0.0, 0.0),
                text: "x".into(),
                height: 0.25,
                align: Align::Left,
                angle: 0.0,
            },
            Shape::Circle {
                center: Point::new(0.0, 0.0),
                radius: 0.05,
                filled: false,
            },
            Shape::Arc {
                center: Point::new(0.0, 0.0),
                radius: 2000.0,
                start: -90.0,
                end: 450.0,
            },
        ];
        let ctx = egui::Context::default();
        for shape in shapes {
            let mut shown = shape.clone();
            for _ in 0..2 {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| shape_properties(ui, &mut shown));
                });
            }
            assert_eq!(shown, shape);
        }
    }

    #[test]
    fn undo_during_a_drag_takes_back_the_whole_drag() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        let drawn = e.symbol.clone();
        e.set_tool(Tool::Select);
        e.start_drag(Point::new(-5.0, 0.0));
        e.drag_to(Point::new(0.0, 0.0));
        let moved = e.symbol.clone();
        assert_ne!(moved, drawn);
        // Undo before the mouse is let go.
        e.undo();
        assert!(!e.dragging());
        assert_eq!(e.symbol, drawn);
        e.redo();
        assert_eq!(e.symbol, moved);
        // The mouse moving on after that moves nothing.
        e.drag_to(Point::new(5.0, 0.0));
        assert_eq!(e.symbol, moved);
    }

    #[test]
    fn redo_takes_back_an_undo_until_something_new_is_done() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        let drawn = e.symbol.clone();
        assert!(!e.can_redo());
        e.redo(); // nothing to redo: nothing happens
        assert_eq!(e.symbol, drawn);

        // Back two steps (the tag and the second pin), then forward again one at a time.
        e.undo();
        let one_back = e.symbol.clone();
        e.undo();
        assert_eq!(e.symbol.pins.len(), 1);
        assert!(e.can_redo());
        e.redo();
        assert_eq!(e.symbol, one_back);
        e.redo();
        assert_eq!(e.symbol, drawn);
        assert!(!e.can_redo());
        e.undo();
        e.redo();
        assert_eq!(e.symbol, drawn, "undo and redo again lands on the same symbol");

        // A new change after Undo clears Redo: a drag, a nudge, a property edit, a drawn shape.
        let changes: [fn(&mut SymbolEditor); 4] = [
            |e| {
                e.set_tool(Tool::Select);
                e.start_drag(Point::new(-5.0, 0.0));
                e.drag_to(Point::new(0.0, 0.0));
                e.end_drag();
            },
            |e| {
                e.selected = Some(Item::Shape(0));
                e.nudge(Point::new(SNAP, 0.0));
            },
            |e| {
                let before = e.symbol.clone();
                e.symbol.name = "Other".into();
                e.note_property_edit(before);
            },
            |e| {
                e.set_tool(Tool::Line);
                e.click(Point::new(10.0, 10.0));
                e.click(Point::new(15.0, 10.0));
            },
        ];
        for change in changes {
            e.undo();
            assert!(e.can_redo());
            change(&mut e);
            assert!(!e.can_redo(), "a new change clears redo");
            e.redo();
            e.undo();
            e.redo();
        }

        // Deleting with nothing valid selected changes nothing, so Redo stays.
        e.undo();
        e.selected = Some(Item::Shape(99));
        e.delete_selected();
        assert!(e.can_redo());
    }

    #[test]
    fn property_edits_are_one_undo_step_each() {
        let mut e = SymbolEditor::new(Vec::new());
        draw_coil(&mut e);
        let drawn = e.symbol.clone();
        // Typing the name letter by letter: one step.
        for name in ["R", "Re", "Relay"] {
            let before = e.symbol.clone();
            e.symbol.name = name.into();
            e.note_property_edit(before);
        }
        let before = e.symbol.clone();
        e.note_property_edit(before); // a frame without a change adds nothing
        e.undo();
        assert_eq!(e.symbol, drawn, "the drawing stays, only the typing is undone");
        // A property edit after drawing does not take the drawing with it.
        e.set_tool(Tool::Line);
        e.click(Point::new(-10.0, -10.0));
        e.click(Point::new(-5.0, -10.0));
        let lines = e.symbol.graphics.len();
        let before = e.symbol.clone();
        e.symbol.pins[0].name = "X1".into();
        e.note_property_edit(before);
        e.undo();
        assert_eq!(e.symbol.pins[0].name, "A1");
        assert_eq!(e.symbol.graphics.len(), lines);
        // Editing one pin, then selecting another and editing it: two steps.
        e.set_tool(Tool::Select);
        e.click(Point::new(0.0, -5.0));
        let before = e.symbol.clone();
        e.symbol.pins[0].name = "X1".into();
        e.note_property_edit(before);
        e.click(Point::new(0.0, 5.0));
        assert_eq!(e.selected, Some(Item::Pin(1)));
        let before = e.symbol.clone();
        e.symbol.pins[1].name = "X2".into();
        e.note_property_edit(before);
        e.undo();
        assert_eq!(e.symbol.pins[0].name, "X1", "the first pin's edit stays");
        assert_eq!(e.symbol.pins[1].name, "A2");
    }

    #[test]
    fn a_polyline_closes_on_its_first_point_and_can_be_filled() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let lib = tmp.path().join("lib");
        let mut e = SymbolEditor::new(vec![("mine".into(), lib.clone())]);
        e.set_tool(Tool::Polyline);
        let steps = e.undo.len();
        // A diode triangle: three corners (snapped), the second clicked twice, then back on
        // the first point, which closes it.
        for (x, y) in [(-2.4, -2.6), (2.5, 0.1), (2.5, 0.0), (-2.5, 2.5), (-2.6, -2.4)] {
            e.click(Point::new(x, y));
        }
        let triangle = vec![Point::new(-2.5, -2.5), Point::new(2.5, 0.0), Point::new(-2.5, 2.5)];
        assert_eq!(
            e.symbol.graphics,
            [Shape::Polyline {
                points: triangle.clone(),
                closed: true,
                filled: false
            }]
        );
        assert_eq!(e.undo.len(), steps + 1, "one undo step per polyline");
        assert_eq!(e.selected, Some(Item::Shape(0)));

        // Filled from the properties box, as one more undo step.
        let before = e.symbol.clone();
        if let Some(Shape::Polyline { filled, .. }) = e.selected_shape_mut() {
            *filled = true;
        }
        e.note_property_edit(before);
        assert_eq!(e.undo.len(), steps + 2);

        // Enter (or a double-click) ends an open one; Escape drops it; one point draws nothing.
        e.click(Point::new(5.0, -2.5));
        e.click(Point::new(5.0, 2.5));
        e.finish_polyline();
        assert!(matches!(&e.symbol.graphics[1], Shape::Polyline { points, closed: false, .. } if points.len() == 2));
        e.click(Point::new(10.0, 0.0));
        e.click(Point::new(12.5, 0.0));
        e.cancel_shape();
        e.click(Point::new(15.0, 0.0));
        e.finish_polyline();
        assert_eq!(e.symbol.graphics.len(), 2);

        // Saved as drawn.
        e.symbol.id = "test.diode".into();
        e.symbol.name = "Test diode".into();
        let saved = e.save()?;
        assert_eq!(
            saved.symbol.graphics.first(),
            Some(&Shape::Polyline {
                points: triangle,
                closed: true,
                filled: true
            })
        );
        Ok(())
    }

    fn press(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn opening_a_filled_polyline_clears_filled_too() {
        let ctx = egui::Context::default();
        let (mut closed, mut filled) = (true, true);
        let mut boxed = egui::Rect::NOTHING;
        let mut t = 0.0;
        let mut frame = |events: Vec<egui::Event>, closed: &mut bool, filled: &mut bool| {
            t += 0.05;
            let raw = egui::RawInput {
                events,
                time: Some(t),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    boxed = polyline_properties(ui, 3, closed, filled).rect;
                });
            });
            boxed.center()
        };
        let at = frame(vec![], &mut closed, &mut filled);
        frame(vec![egui::Event::PointerMoved(at)], &mut closed, &mut filled);
        frame(vec![press(at, true)], &mut closed, &mut filled);
        frame(vec![press(at, false)], &mut closed, &mut filled);
        assert!(!closed, "the Closed box was unticked");
        assert!(!filled, "an open polyline is not filled");
    }

    #[test]
    fn two_quick_clicks_on_different_points_do_not_end_a_polyline() {
        let ctx = egui::Context::default();
        let mut e = SymbolEditor::new(Vec::new());
        e.set_tool(Tool::Polyline);
        let t = std::cell::Cell::new(0.0);
        let frame = |e: &mut SymbolEditor, events: Vec<egui::Event>| {
            t.set(t.get() + 0.02);
            let mut xf = Xf {
                origin: Pos2::ZERO,
                zoom: 1.0,
            };
            let raw = egui::RawInput {
                events,
                time: Some(t.get()),
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Where the drawing puts the symbol origin, and its zoom.
                    let r = ui.available_rect_before_wrap();
                    let size = r.size().max(Vec2::new(320.0, 320.0)) - Vec2::new(0.0, 24.0);
                    let rect = egui::Rect::from_min_size(r.min, size);
                    xf = Xf {
                        origin: rect.center(),
                        zoom: (rect.width().min(rect.height()) / 40.0).max(2.0),
                    };
                    e.drawing(ui);
                });
            });
            xf
        };
        let click = |e: &mut SymbolEditor, x: f64, y: f64| {
            let pos = frame(e, vec![]).s(Point::new(x, y));
            frame(e, vec![egui::Event::PointerMoved(pos), press(pos, true)]);
            frame(e, vec![press(pos, false)]);
        };
        // Three corners in quick succession, each within egui's double-click time.
        click(&mut e, 0.0, 0.0);
        click(&mut e, 10.0, 0.0);
        click(&mut e, 10.0, 10.0);
        assert!(e.symbol.graphics.is_empty(), "quick clicks on new points add points");
        assert_eq!(e.clicks.len(), 3);
        // A pause, then a double-click on a new point adds it and ends the polyline there.
        t.set(t.get() + 1.0);
        click(&mut e, 15.0, 10.0);
        assert_eq!(e.clicks.len(), 4);
        click(&mut e, 15.0, 10.0);
        assert!(e.clicks.is_empty());
        assert!(matches!(
            &e.symbol.graphics[..],
            [Shape::Polyline { points, closed: false, .. }] if points.len() == 4
        ));

        // A double-click on the first point of a new one closes it, and starts nothing more.
        t.set(t.get() + 1.0);
        for (x, y) in [(-10.0, 0.0), (-5.0, 0.0), (-5.0, 5.0)] {
            click(&mut e, x, y);
        }
        t.set(t.get() + 1.0);
        click(&mut e, -10.0, 0.0);
        click(&mut e, -10.0, 0.0);
        assert!(e.clicks.is_empty(), "the second click started no new polyline");
        assert!(matches!(
            e.symbol.graphics.last(),
            Some(Shape::Polyline { points, closed: true, .. }) if points.len() == 3
        ));
        // The next click (a new double-click pair is long over) starts one as usual.
        t.set(t.get() + 1.0);
        click(&mut e, -10.0, 0.0);
        assert_eq!(e.clicks, [Point::new(-10.0, 0.0)]);

        // A, B, then B again with no pause: egui calls the last a triple-click; it still ends it.
        let drawn = e.symbol.graphics.len();
        click(&mut e, 0.0, 15.0);
        click(&mut e, 0.0, 15.0);
        assert!(e.clicks.is_empty());
        assert_eq!(e.symbol.graphics.len(), drawn + 1);
        assert!(matches!(
            e.symbol.graphics.last(),
            Some(Shape::Polyline { points, closed: false, .. })
                if points[..] == [Point::new(-10.0, 0.0), Point::new(0.0, 15.0)]
        ));

        // A quick last vertex, then a double-click on the first point with no pause: egui calls
        // the closing click a triple-click, and its pair still starts nothing.
        t.set(t.get() + 1.0);
        for (x, y) in [(20.0, 0.0), (25.0, 0.0), (25.0, 5.0), (20.0, 0.0), (20.0, 0.0)] {
            click(&mut e, x, y);
        }
        assert!(e.clicks.is_empty(), "the last click started no new polyline");
        assert!(matches!(
            e.symbol.graphics.last(),
            Some(Shape::Polyline { points, closed: true, .. }) if points.len() == 3 && points[0] == Point::new(20.0, 0.0)
        ));
    }

    #[test]
    fn a_symbol_with_the_same_id_in_another_folder_is_not_overwritten() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        let targets = vec![("a".to_owned(), a.clone()), ("b".to_owned(), b.clone())];
        let mut first = SymbolEditor::new(targets.clone());
        draw_coil(&mut first);
        first.save()?;
        let mut other = SymbolEditor::new(targets.clone());
        draw_coil(&mut other);
        other.symbol.name = "Folder B's coil".into();
        other.target = 1;
        other.save()?;

        // Opened from A, then pointed at B: B's own symbol stays.
        let (s, _) = (first.symbol.clone(), ());
        let mut e = SymbolEditor::edit(s, "a", targets);
        e.target = 1;
        assert!(e.save().is_err_and(|m| m.contains("already has")));
        let kept: Symbol = wc_core::store::read_json(&b.join("symbols").join("test.coil.json"))?;
        assert_eq!(kept.name, "Folder B's coil");
        // Back to A it saves.
        e.target = 0;
        assert_eq!(e.save()?.source, "a");
        Ok(())
    }
}
