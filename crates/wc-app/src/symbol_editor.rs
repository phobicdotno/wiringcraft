//! The symbol editor: draw a symbol's lines, rectangles, circles, arcs and texts, place and name
//! its pins, set where the device tag goes, and save it into one of the project's library
//! folders. [`SymbolEditor`] holds the state and does the work without any UI, so it is
//! tested on its own; [`SymbolEditor::show`] is the window around it.

use std::path::PathBuf;

use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke, Vec2};
use wc_core::Symbol;
use wc_core::geom::{Point, Shape};
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
    /// Centre, then the start of the arc, then its end (clockwise on paper).
    Arc,
    Text,
    Pin,
    /// Where the device tag is printed.
    Tag,
}

impl Tool {
    const ALL: [Tool; 8] = [
        Tool::Select,
        Tool::Line,
        Tool::Rect,
        Tool::Circle,
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
            Tool::Arc => "Arc",
            Tool::Text => "Text",
            Tool::Pin => "Pin",
            Tool::Tag => "Tag position",
        }
    }

    /// What to do next, for the status line.
    fn hint(self, clicks: usize) -> &'static str {
        match (self, clicks) {
            (Tool::Select, _) => "Click a shape or pin to select it.",
            (Tool::Line, 0) => "Click the start of the line.",
            (Tool::Line, _) => "Click the end of the line.",
            (Tool::Rect, 0) => "Click one corner.",
            (Tool::Rect, _) => "Click the opposite corner.",
            (Tool::Circle, 0) => "Click the centre.",
            (Tool::Circle, _) => "Click a point on the circle.",
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

/// A library folder the symbol can be saved into: `(source name, folder)`.
pub type Target = (String, PathBuf);

pub struct SymbolEditor {
    pub symbol: Symbol,
    pub tool: Tool,
    /// The clicks so far of a shape being drawn.
    clicks: Vec<Point>,
    pub selected: Option<Item>,
    /// Folders the symbol can be saved into, and the chosen one.
    pub targets: Vec<Target>,
    pub target: usize,
    /// The id the symbol had when it was opened (`None` for a new symbol).
    pub opened_as: Option<String>,
    /// Text the Text tool places.
    pub text: String,
    /// Name the Pin tool gives the next pin.
    pub pin_name: String,
    pub message: String,
    undo: Vec<Symbol>,
    /// Library symbols that can be opened: `(id, name)`.
    pub choices: Vec<(String, String)>,
}

/// What a save did, for the window around the editor.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub id: String,
    pub path: PathBuf,
}

fn blank() -> Symbol {
    Symbol {
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
    match num.parse::<u64>() {
        Ok(n) => format!("{head}{}", n + 1),
        Err(_) => name.to_owned(),
    }
}

/// The angle (degrees, y down, as [`Shape::Arc`] uses) of `p` seen from `c`.
fn angle(c: Point, p: Point) -> f64 {
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
            selected: None,
            targets,
            target: 0,
            opened_as: None,
            text: String::new(),
            pin_name: "1".into(),
            message: String::new(),
            undo: Vec::new(),
            choices: Vec::new(),
        }
    }

    /// Edit an existing symbol, saving back into the source it came from when that is one of
    /// `targets`.
    pub fn edit(symbol: Symbol, source: &str, targets: Vec<Target>) -> Self {
        let mut e = Self::new(targets);
        e.target = e.targets.iter().position(|(n, _)| n == source).unwrap_or(0);
        e.opened_as = Some(symbol.id.clone());
        e.pin_name = symbol.pins.last().map_or_else(|| "1".into(), |p| next_name(&p.name));
        e.symbol = symbol;
        e.tool = Tool::Select;
        e
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.symbol.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.symbol = s;
            self.selected = None;
            self.clicks.clear();
        }
    }

    /// Change the tool; a shape half drawn is dropped.
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.clicks.clear();
    }

    /// A click on the drawing at `p` (mm, symbol coordinates), snapped to the grid.
    pub fn click(&mut self, p: Point) {
        let p = snap(p);
        self.message.clear();
        match self.tool {
            Tool::Select => self.selected = self.pick(p),
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
            Tool::Arc => {
                self.clicks.push(p);
                if let [c, s, e] = self.clicks[..] {
                    self.clicks.clear();
                    let radius = ((s.x - c.x).powi(2) + (s.y - c.y).powi(2)).sqrt();
                    if radius == 0.0 || e == c {
                        return;
                    }
                    self.add_shape(Shape::Arc {
                        center: c,
                        radius,
                        start: angle(c, s),
                        end: angle(c, e),
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
                if let Some(i) = self.symbol.pins.iter().position(|q| q.at == p) {
                    self.message = format!("Pin {} is already there.", self.symbol.pins[i].name);
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

    fn add_shape(&mut self, shape: Shape) {
        self.checkpoint();
        self.symbol.graphics.push(shape);
        self.selected = Some(Item::Shape(self.symbol.graphics.len() - 1));
    }

    /// The pin at `p`, else the last drawn shape whose box (1 mm wider) holds `p`.
    fn pick(&self, p: Point) -> Option<Item> {
        if let Some(i) = self
            .symbol
            .pins
            .iter()
            .position(|q| (q.at.x - p.x).abs() < 1.3 && (q.at.y - p.y).abs() < 1.3)
        {
            return Some(Item::Pin(i));
        }
        self.symbol.graphics.iter().enumerate().rev().find_map(|(i, g)| {
            let (a, b) = g.bounds();
            (p.x >= a.x - 1.0 && p.x <= b.x + 1.0 && p.y >= a.y - 1.0 && p.y <= b.y + 1.0).then_some(Item::Shape(i))
        })
    }

    pub fn delete_selected(&mut self) {
        let Some(item) = self.selected.take() else { return };
        self.checkpoint();
        match item {
            Item::Shape(i) if i < self.symbol.graphics.len() => {
                self.symbol.graphics.remove(i);
            }
            Item::Pin(i) if i < self.symbol.pins.len() => {
                self.symbol.pins.remove(i);
            }
            _ => {
                self.undo.pop();
            }
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
            if p.name.trim().is_empty() {
                out.push("Every pin needs a name.".into());
            } else if !names.insert(p.name.as_str()) {
                out.push(format!("Two pins are called {}.", p.name));
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
        let id = self.symbol.id.clone();
        let file = root.join("symbols").join(format!("{id}.json"));
        if self.opened_as.as_deref() != Some(id.as_str()) && file.exists() {
            return Err(format!(
                "Library {source} already has a symbol {id}; choose another id, or open that symbol to change it."
            ));
        }
        let path = wc_library::write_symbol(&root, &self.symbol).map_err(|e| e.to_string())?;
        self.opened_as = Some(id.clone());
        self.message = format!("Saved {id} into library {source}.");
        Ok(Saved { id, path })
    }

    /// The editor window. Returns what was saved this frame, and false once it was closed.
    /// `open_symbol` is asked for a library symbol picked in "Open", by id.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        open_symbol: &mut dyn FnMut(&str) -> Option<(Symbol, String)>,
    ) -> (Option<Saved>, bool) {
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
                    if ui
                        .add_enabled(self.selected.is_some(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        self.delete_selected();
                    }
                });
                ui.separator();
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

    fn properties(&mut self, ui: &mut egui::Ui, open_symbol: &mut dyn FnMut(&str) -> Option<(Symbol, String)>) {
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
                    Some((s, source)) => {
                        let choices = std::mem::take(&mut self.choices);
                        *self = Self::edit(s, &source, std::mem::take(&mut self.targets));
                        self.choices = choices;
                    }
                    None => self.message = format!("Symbol {id} could not be read."),
                }
            }
        }
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
    }

    fn drawing(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size().max(Vec2::new(320.0, 320.0)) - Vec2::new(0.0, 24.0);
        let (resp, painter) = ui.allocate_painter(size, Sense::click());
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
        if let Some(h) = resp.hover_pos() {
            let at = snap(xf.mm(h));
            painter.circle_stroke(xf.s(at), 3.0, Stroke::new(1.0_f32, pick));
            if let Some(a) = self.clicks.first() {
                painter.line_segment([xf.s(*a), xf.s(at)], Stroke::new(1.0_f32, pick));
            }
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            self.click(xf.mm(pos));
        }
        if resp.secondary_clicked() {
            self.clicks.clear();
        }
    }
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
        assert!(!e.message.is_empty());
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
}
