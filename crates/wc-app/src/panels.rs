//! Side panels: page navigator and devices on the left, symbol library picker, properties on
//! the right, and the status bar.

use eframe::egui::{self, RichText};
use wc_core::DeviceTag;
use wc_core::geom::{Rotation, Shape};

use crate::actions::Action;
use crate::canvas::Editor;
use crate::state::{Open, Selection, SpecPart, Tool};
use crate::theme::Theme;

#[derive(Default)]
pub struct PanelState {
    pub search: String,
    /// Edit buffers for the properties panel and the selection they belong to.
    /// The page and item the fields were loaded from: the same item id on another page is
    /// another item.
    loaded_for: Option<(usize, Selection)>,
    tag: String,
    description: String,
    number: String,
    potential: String,
    text: String,
    text_height: f64,
    /// Edit buffer for a free drawing's own values (a circle's centre and radius, an arc's
    /// radius and angles, a polyline closed or open).
    shape: Option<Shape>,
    /// Which of `shape`'s fields (as [`shape_fields`] lists them) are being typed in or dragged:
    /// those are not read again from the drawing meanwhile. The others are, each frame, so a
    /// drawing moved on the canvas is not put back by the next value set here.
    shape_in_use: Vec<bool>,
    /// The drawing's values when each field was read: a field is applied only when it differs
    /// from these, to the drawing as it is then (moved meanwhile, it stays where it went).
    shape_base: Option<Shape>,
    /// Why values set for the drawing were refused, as shown: those of `shape_refused`.
    shape_error: Option<String>,
    /// The fields whose values were refused, each with its reason: a reason stays until a
    /// value is entered in its own field again, and is taken (or changes nothing).
    shape_refused: std::collections::BTreeMap<usize, String>,
    /// Which of `shape`'s fields have been typed in or dragged since they took the focus: a
    /// field only tabbed through enters nothing.
    shape_touched: Vec<bool>,
    /// The page, index and [`PanelState::drawing_gen`] of the drawing `shape` was read for.
    shape_slot: Option<(usize, usize, u64)>,
    /// The drawing whose properties are shown: its page and index, a hash of the drawings
    /// before it, how many the page had, and its kind. Another drawing at that index is told by these.
    drawing_seen: Option<(usize, usize, u64, usize, std::mem::Discriminant<Shape>)>,
    /// Counts the drawings shown: one more each time another drawing takes the place.
    drawing_gen: u64,
    cable: String,
    core: String,
    cable_error: Option<String>,
    part_search: String,
    part_error: Option<String>,
    accessory_search: String,
    new_key: String,
    new_value: String,
    /// Edit buffers for the device's own properties, by key.
    props: Vec<(String, String)>,
    tag_error: Option<String>,
    /// A reply being written to the comment shown.
    reply: String,
    /// Edit state for the selected wires' section, colour and type, in `SpecPart::ALL` order, and
    /// the wires it belongs to.
    spec: [SpecField; 3],
    spec_for: (usize, Vec<String>),
}

impl PanelState {
    /// A fresh state for a newly opened project: nothing typed in it carries over. The device
    /// search stays.
    pub fn reset(&mut self) {
        *self = Self {
            search: std::mem::take(&mut self.search),
            ..Self::default()
        };
    }
}

/// The edit state of one wire spec field.
#[derive(Default)]
struct SpecField {
    /// What the field shows.
    buffer: String,
    /// The value it was loaded with (empty when the wires differ).
    loaded: String,
    /// Whether the wires differ in it.
    mixed: bool,
    /// Whether the user typed in it since it was loaded or committed.
    dirty: bool,
}

/// Keep the spec buffers in step with the given wires of a page: refilled when the selection
/// changes, and a field the user is not editing follows changes made elsewhere (undo, the agent).
fn load_spec(open: &Open, page: usize, ids: &[String], ps: &mut PanelState) {
    let fresh = ps.spec_for.0 != page || ps.spec_for.1 != ids;
    if fresh {
        ps.spec_for = (page, ids.to_vec());
        for f in &mut ps.spec {
            f.dirty = false;
        }
    }
    let Some(p) = open.project.pages.get(page) else {
        return;
    };
    let wires: Vec<&wc_core::Wire> = p.wires.iter().filter(|w| ids.contains(&w.id)).collect();
    for (part, f) in SpecPart::ALL.into_iter().zip(&mut ps.spec) {
        let values: std::collections::BTreeSet<Option<&String>> =
            wires.iter().map(|w| part.of(&w.spec).as_ref()).collect();
        let mixed = values.len() > 1;
        let common = if mixed {
            String::new()
        } else {
            values.into_iter().next().flatten().cloned().unwrap_or_default()
        };
        if fresh || (!f.dirty && (common != f.loaded || mixed != f.mixed)) {
            f.buffer = common.clone();
            f.loaded = common;
            f.mixed = mixed;
        }
    }
}

/// Forget the wire spec drafts when this frame shows no wire rows (a panel, a symbol, a comment,
/// the page): coming back to the same wires then reloads their values instead of an old draft.
fn forget_spec_unless_wires(ed: &Editor, ps: &mut PanelState) {
    let wires =
        ed.panel.is_none() && !ed.selection.is_empty() && ed.selection.iter().all(|s| matches!(s, Selection::Wire(_)));
    if !wires {
        ps.spec_for = Default::default();
    }
}

/// Rows for cross-section, colour and type of the given wires, inside a two-column grid. A value
/// typed in is set on all of them; emptying a field clears it on all of them. A field nobody typed
/// in changes nothing, so tabbing through a field the wires differ in keeps their own values.
fn spec_rows(ui: &mut egui::Ui, open: &mut Open, page: usize, ids: &[String], ps: &mut PanelState) {
    load_spec(open, page, ids, ps);
    for (part, f) in SpecPart::ALL.into_iter().zip(&mut ps.spec) {
        ui.label(part.label());
        let hint = if f.mixed { "several values" } else { part.hint() };
        let r = ui.add(egui::TextEdit::singleline(&mut f.buffer).hint_text(hint));
        if r.changed() {
            f.dirty = true;
        }
        if committed(&r) && f.dirty {
            f.dirty = false;
            let value = wc_core::WireSpec::value(&f.buffer);
            open.set_wire_spec(page, ids, part, value.clone());
            f.buffer = value.unwrap_or_default();
            f.loaded = f.buffer.clone();
            f.mixed = false;
        }
        ui.end_row();
    }
}

/// Jumper controls for selected terminal symbols: several terminals of one strip can be jumpered
/// together; a jumpered terminal shows what it is jumpered with and can leave its jumper.
fn jumper_controls(ui: &mut egui::Ui, open: &mut Open, page: usize, ids: &[String]) {
    let Some(p) = open.project.pages.get(page) else {
        return;
    };
    // Each terminal's strip and number as the terminal plan has them, so a terminal tagged
    // with only its strip (numbered automatically) is offered the same as a numbered one.
    let numbers = open.terminal_numbers();
    let mut terms: Vec<(DeviceTag, String)> = Vec::new();
    for id in ids {
        if p.symbol(id).is_none() {
            return;
        }
        let Some(term) = numbers.get(&(p.id.clone(), id.clone())) else {
            return;
        };
        terms.push(term.clone());
    }
    let Some((strip, number)) = terms.first().cloned() else {
        return;
    };
    if terms.iter().any(|(s, _)| *s != strip) {
        return;
    }
    if terms.len() == 1 {
        let others = jumper_with(open, &strip, &number).map(|j| {
            j.terminals
                .iter()
                .filter(|t| **t != number)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        });
        if let Some(others) = others {
            ui.horizontal(|ui| {
                ui.label(format!("Jumpered with {others}"));
                if ui.button("Remove from jumper").clicked() {
                    open.remove_from_jumper(&strip, &number);
                }
            });
            jumper_part(ui, open, &strip, &number);
        }
    } else {
        let numbers: Vec<String> = terms.into_iter().map(|(_, n)| n).collect();
        // Only offered when it changes something; when it cannot be done, say why.
        match open.jumper_offer(&strip, &numbers) {
            Ok(true) => {
                if ui
                    .button(format!("Jumper these {} terminals of {strip}", numbers.len()))
                    .clicked()
                    && let Err(why) = open.add_jumper(&strip, &numbers)
                {
                    ui.label(why);
                }
            }
            Ok(false) => {}
            Err(why) => {
                ui.weak(format!("No jumper: {why}"));
            }
        }
    }
}

/// The jumper that holds terminal `number` of `strip`.
fn jumper_with<'a>(open: &'a Open, strip: &DeviceTag, number: &str) -> Option<&'a wc_core::project::Jumper> {
    open.project
        .meta
        .jumpers
        .iter()
        .find(|j| j.strip == *strip && j.terminals.iter().any(|t| t == number))
}

/// The part of the jumper that holds terminal `number` of `strip`: pick one from the project's
/// and the libraries' parts, or none.
fn jumper_part(ui: &mut egui::Ui, open: &mut Open, strip: &DeviceTag, number: &str) {
    let current = jumper_with(open, strip, number).and_then(|j| j.part.clone());
    let search_id = ui.id().with("jumper part search");
    let mut search: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let pick = ui
        .horizontal(|ui| {
            ui.label("Jumper part");
            part_picker(
                ui,
                "jumper part",
                200.0,
                &open.part_library,
                current.as_deref(),
                &mut search,
            )
        })
        .inner;
    ui.data_mut(|d| d.insert_temp(search_id, search));
    let error_id = ui.id().with("jumper part error");
    if let Some(choice) = pick {
        let result = open.set_jumper_part(strip, number, choice.as_deref()).err();
        ui.data_mut(|d| d.insert_temp(error_id, result));
    }
    if let Some(Some(e)) = ui.data(|d| d.get_temp::<Option<String>>(error_id)) {
        ui.label(RichText::new(e).color(Theme::of(ui.ctx()).error).small());
    }
}

/// A panel section title: small capitals in the muted colour, with an optional count on the
/// right.
pub fn section(ui: &mut egui::Ui, title: &str, count: Option<usize>) {
    let t = Theme::of(ui.ctx());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title.to_uppercase())
                .font(crate::theme::strong(10.5))
                .extra_letter_spacing(0.8)
                .color(t.muted),
        );
        if let Some(n) = count {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                badge(ui, &n.to_string(), &t);
            });
        }
    });
    ui.add_space(2.0);
}

/// A sub-heading inside the properties panel.
fn subsection(ui: &mut egui::Ui, title: &str) {
    let t = Theme::of(ui.ctx());
    ui.add_space(8.0);
    ui.label(RichText::new(title).font(crate::theme::strong(12.0)).color(t.text));
}

/// A small rounded count.
fn badge(ui: &mut egui::Ui, text: &str, t: &Theme) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), egui::FontId::proportional(10.5), t.muted);
    let size = egui::vec2((galley.size().x + 10.0).max(18.0), 16.0);
    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(r, 8.0, t.hover);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, t.muted);
}

/// What sits at the start of a list row.
pub enum Lead<'a> {
    /// A page number in a small box.
    Number(&'a str),
    /// A coloured dot.
    Dot(egui::Color32),
}

/// A full-width list row with hover and selection states. Returns the click response.
pub fn row(ui: &mut egui::Ui, selected: bool, lead: Lead, text: RichText, mono: bool) -> egui::Response {
    let t = Theme::of(ui.ctx());
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(egui::vec2(w, 26.0), egui::Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(r, 6.0, t.accent_soft);
        p.rect_filled(
            egui::Rect::from_min_size(r.min + egui::vec2(0.0, 6.0), egui::vec2(3.0, r.height() - 12.0)),
            1.5,
            t.accent,
        );
    } else if resp.hovered() {
        p.rect_filled(r, 6.0, t.hover);
    }
    let mut x = r.left() + 10.0;
    match lead {
        Lead::Number(n) => {
            let g = p.layout_no_wrap(n.to_string(), egui::FontId::monospace(11.0), t.muted);
            let bw = (g.size().x + 10.0).max(24.0);
            let b = egui::Rect::from_min_size(egui::pos2(x, r.center().y - 9.0), egui::vec2(bw, 18.0));
            let (fill, fg) = if selected {
                (t.accent, egui::Color32::WHITE)
            } else {
                (t.hover, t.muted)
            };
            p.rect_filled(b, 4.0, fill);
            let g = p.layout_no_wrap(n.to_string(), egui::FontId::monospace(11.0), fg);
            p.galley(b.center() - g.size() / 2.0, g, fg);
            x = b.right() + 8.0;
        }
        Lead::Dot(c) => {
            p.circle_filled(egui::pos2(x + 3.0, r.center().y), 3.0, c);
            x += 14.0;
        }
    }
    let color = if selected { t.text } else { t.text.gamma_multiply(0.92) };
    let text = if mono { text.monospace() } else { text };
    let galley = egui::WidgetText::from(text.color(color)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        r.right() - x - 6.0,
        egui::TextStyle::Body,
    );
    ui.painter()
        .galley(egui::pos2(x, r.center().y - galley.size().y / 2.0), galley, color);
    resp
}

pub fn navigator(ui: &mut egui::Ui, open: &Open, ed: &mut Editor, out: &mut Vec<Action>) {
    section(ui, "Pages", Some(open.project.pages.len()));
    egui::ScrollArea::vertical()
        .id_salt("pages")
        .max_height(ui.available_height() * 0.5)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for (i, p) in open.project.pages.iter().enumerate() {
                let title = match ed.held_by.get(&p.id) {
                    Some(name) => RichText::new(format!("{}  · {name}", p.title)).italics(),
                    None => RichText::new(&p.title),
                };
                let r = row(
                    ui,
                    ed.page == i && ed.panel.is_none(),
                    Lead::Number(&p.number),
                    title,
                    false,
                );
                let r = match ed.held_by.get(&p.id) {
                    Some(name) => r.on_hover_text(format!("Open by {name}: read-only for you")),
                    None => r,
                };
                let pick = |ed: &mut Editor| {
                    if ed.panel.take().is_some() {
                        ed.view.fit = true;
                    }
                    if ed.page != i {
                        ed.page = i;
                        ed.selection.clear();
                        ed.wire = None;
                        ed.view.fit = true;
                    }
                };
                if r.clicked() {
                    pick(ed);
                }
                // The cover page opens its form; other pages their properties.
                let r = if p.cover {
                    r.on_hover_text("Cover page: double-click to edit the project's name and fields")
                } else {
                    r
                };
                if r.double_clicked() {
                    out.push(if p.cover { Action::CoverPage } else { Action::PageProps });
                }
                r.context_menu(|ui| {
                    pick(ed);
                    for a in [
                        Action::CoverPage,
                        Action::PageProps,
                        Action::DuplicatePage,
                        Action::PageUp,
                        Action::PageDown,
                        Action::DeletePage,
                    ] {
                        if a == Action::CoverPage && !p.cover {
                            continue;
                        }
                        let enabled = match a {
                            Action::PageUp => i > 0,
                            Action::PageDown => i + 1 < open.project.pages.len(),
                            Action::DeletePage => open.project.pages.len() > 1,
                            _ => true,
                        };
                        if ui.add_enabled(enabled, egui::Button::new(a.info().label)).clicked() {
                            out.push(a);
                            ui.close_menu();
                        }
                    }
                });
            }
            if open.project.cover_page().is_none() {
                ui.add_space(4.0);
                if ui
                    .small_button("+ Cover page")
                    .on_hover_text(Action::CoverPage.info().tip)
                    .clicked()
                {
                    out.push(Action::CoverPage);
                }
            }
        });
    if !open.project.panels.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        section(ui, "Panels", Some(open.project.panels.len()));
        ui.spacing_mut().item_spacing.y = 1.0;
        let t = Theme::of(ui.ctx());
        for (i, p) in open.project.panels.iter().enumerate() {
            let r = row(
                ui,
                ed.panel == Some(i),
                Lead::Dot(t.accent),
                RichText::new(&p.name),
                false,
            )
            .on_hover_text(format!(
                "{} x {} mm, {} device(s)",
                p.width,
                p.height,
                p.device_tags().len()
            ));
            if r.clicked() && ed.panel != Some(i) {
                ed.panel = Some(i);
                ed.panel_pick = None;
                ed.selection.clear();
                ed.view.fit = true;
            }
            r.context_menu(|ui| {
                for a in [Action::ArrangePanel, Action::DeletePanel] {
                    if ui.button(a.info().label).clicked() {
                        ed.panel = Some(i);
                        out.push(a);
                        ui.close_menu();
                    }
                }
            });
        }
    }
    ui.add_space(6.0);
    ui.separator();
    let index = open.project.tag_index();
    section(ui, "Devices", Some(index.len()));
    let t = Theme::of(ui.ctx());
    egui::ScrollArea::vertical().id_salt("devices").show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for (tag, uses) in index {
            let pages: Vec<String> = uses
                .iter()
                .filter_map(|(page_id, _)| open.project.page(page_id).map(|p| p.number.clone()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let selected = ed.selection.iter().any(|sel| match sel {
                Selection::Symbol(id) => uses.iter().any(|(_, item)| item == id),
                _ => false,
            }) && uses
                .iter()
                .any(|(pid, _)| open.project.pages.get(ed.page).is_some_and(|p| &p.id == pid));
            let r = row(ui, selected, Lead::Dot(t.muted), RichText::new(tag.to_string()), true)
                .on_hover_text(format!("On page {}", pages.join(", ")));
            if r.clicked()
                && let Some((page_id, item)) = uses.first()
                && let Some(i) = open.project.pages.iter().position(|p| &p.id == page_id)
            {
                ed.page = i;
                ed.select_only(Selection::Symbol(item.clone()));
                ed.view.fit = true;
            }
            // A terminal's strip can have its markers exported on their own.
            if let Some(strip) = terminal_strip_of(open, &tag, &uses) {
                r.context_menu(|ui| {
                    if ui.button(format!("Export markers of {strip}...")).clicked() {
                        ed.marking_strip = Some(strip.clone());
                        out.push(Action::ExportWagoMarking);
                        ui.close_menu();
                    }
                });
            }
        }
    });
}

/// The strip a terminal tag belongs to, named as the terminal plan names it (`-X1` of
/// `-X1.3`; a terminal tagged with only its strip is that strip), or `None` when no use of the
/// tag is a terminal.
pub fn terminal_strip_of(open: &Open, tag: &DeviceTag, uses: &[(String, String)]) -> Option<String> {
    let terminal = uses.iter().any(|(page, item)| {
        open.project
            .page(page)
            .and_then(|p| p.symbol(item))
            .and_then(|s| open.symbols.get(&s.symbol))
            .is_some_and(|y| y.role == wc_core::symbol::SymbolRole::Terminal)
    });
    if !terminal {
        return None;
    }
    Some(tag.terminal_parts().0.to_string())
}

pub fn library(ui: &mut egui::Ui, open: &Open, ed: &mut Editor, ps: &mut PanelState) {
    section(ui, "Symbols", Some(open.library.len()));
    ui.add(
        egui::TextEdit::singleline(&mut ps.search)
            .hint_text("🔍  Search symbols")
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 5)),
    );
    ui.add_space(2.0);
    let q = ps.search.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("library")
        .max_height(ui.available_height() * 0.45)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let t = Theme::of(ui.ctx());
            for (id, name) in &open.library {
                if !q.is_empty() && !id.to_lowercase().contains(&q) && !name.to_lowercase().contains(&q) {
                    continue;
                }
                let picked = ed.place_symbol.as_deref() == Some(id.as_str()) && ed.tool == Tool::Place;
                let vendored = open.symbols.contains_key(id);
                let text = RichText::new(name);
                let lead = Lead::Dot(if vendored { t.accent } else { t.border });
                let r = row(ui, picked, lead, if vendored { text } else { text.italics() }, false).on_hover_text(
                    if vendored {
                        format!("{id}\nIn this project")
                    } else {
                        format!("{id}\nFrom the library; copied into the project when placed")
                    },
                );
                if r.clicked() {
                    ed.place_symbol = Some(id.clone());
                    ed.tool = Tool::Place;
                    ed.wire = None;
                }
            }
        });
}

fn load_buffers(open: &Open, ed: &Editor, ps: &mut PanelState) {
    let one = ed.one().cloned();
    let key = one.clone().map(|s| (ed.page, s));
    if ps.loaded_for == key {
        return;
    }
    ps.loaded_for = key;
    ps.tag_error = None;
    let Some(page) = open.project.pages.get(ed.page) else {
        return;
    };
    match &one {
        Some(Selection::Symbol(id)) => {
            let tag = page.symbol(id).and_then(|s| s.tag.clone());
            ps.tag = tag.as_ref().map(ToString::to_string).unwrap_or_default();
            let device = tag
                .and_then(|t| open.project.devices.get(&t).cloned())
                .unwrap_or_default();
            ps.description = device.description;
            ps.props = device.properties.into_iter().collect();
            ps.part_error = None;
        }
        Some(Selection::Wire(id)) => {
            let w = page.wires.iter().find(|w| &w.id == id);
            ps.number = w.and_then(|w| w.number.clone()).unwrap_or_default();
            ps.potential = w.and_then(|w| w.potential.clone()).unwrap_or_default();
            let c = w.and_then(|w| w.cable.clone());
            ps.cable = c.as_ref().map(|c| c.cable.to_string()).unwrap_or_default();
            ps.core = c.map(|c| c.core).unwrap_or_default();
            ps.cable_error = None;
        }
        Some(Selection::Graphic(i)) => {
            if let Some(Shape::Text { text, height, .. }) = page.graphics.get(*i) {
                ps.text = text.clone();
                ps.text_height = *height;
            }
            ps.shape_in_use.clear();
            ps.shape_touched.clear();
            ps.shape_error = None;
            ps.shape_refused.clear();
        }
        None => {}
    }
}

/// What [`shape_rows`] commits: each field whose edit ended with another value, with that
/// value (each is set on its own), and the fields a value was entered in (typed or dragged,
/// changed or not).
struct ShapeCommit {
    changes: Vec<(usize, f64)>,
    entered: Vec<(usize, f64)>,
}

/// A number typed in a drawing's field, read as egui reads one: spaces ignored, and the minus
/// sign taken for a hyphen.
fn parse_number(text: &str) -> Option<f64> {
    text.split_whitespace()
        .collect::<String>()
        .replace('\u{2212}', "-")
        .parse()
        .ok()
}

/// The fields of a circle, an arc or a polyline (`drawing`, read into the edit buffer except
/// for a field in use). When values are committed (a drag ended, a typed value left its field,
/// the closed box was ticked), the fields that ended, with the drawing as it is now with those
/// of them that changed set. A field only tabbed through, or typed in with text that is not a
/// number, is no change, and a field still in use is left to its own commit.
fn shape_rows(
    ui: &mut egui::Ui,
    ps: &mut PanelState,
    drawing: &Shape,
    slot: (usize, usize, u64),
) -> Option<ShapeCommit> {
    let now = shape_fields(drawing);
    // A draft is for the drawing in its slot: another drawing taking its place drops it.
    if ps.shape_slot != Some(slot) || ps.shape_in_use.len() != now.len() {
        ps.shape_in_use = vec![false; now.len()];
        ps.shape_slot = Some(slot);
    }
    if ps.shape_touched.len() != now.len() {
        ps.shape_touched = vec![false; now.len()];
    }
    let mut draft = ps.shape.as_ref().map(shape_fields).unwrap_or_default();
    let mut base = ps.shape_base.as_ref().map(shape_fields).unwrap_or_default();
    if draft.len() != now.len() || base.len() != now.len() {
        (draft, base) = (now.clone(), now.clone());
    }
    for (((d, b), n), used) in draft.iter_mut().zip(base.iter_mut()).zip(&now).zip(&ps.shape_in_use) {
        if !used {
            (*d, *b) = (*n, *n);
        }
    }
    let mut shape = drawing.clone();
    set_shape_fields(&mut shape, &draft);
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    // Text typed, pasted or deleted, or a value stepped with an arrow key, this frame (it goes
    // to the field with focus; egui steps a value only for an arrow with no modifier). A field
    // left without any is no entry, even when egui reads its rounded text back as another number.
    let typing = ui.input(|i| {
        i.events.iter().any(|e| match e {
            egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Cut => true,
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => match key {
                egui::Key::Backspace | egui::Key::Delete => true,
                egui::Key::ArrowUp | egui::Key::ArrowDown => modifiers.is_none(),
                _ => false,
            },
            _ => false,
        })
    });
    let mut ended = vec![false; now.len()];
    let mut in_use = vec![false; now.len()];
    let mut entered = vec![false; now.len()];
    let touched = &mut ps.shape_touched;
    let mut field = 0;
    let mut num = |ui: &mut egui::Ui, label: &str, v: &mut f64, suffix: &str| {
        ui.label(label);
        // Typed text is read when it leaves the field, not as it is typed: a number typed and
        // then spoiled ("7x") is no value, rather than its last good prefix.
        let read = std::cell::Cell::new(None);
        let r = ui.add(
            egui::DragValue::new(v)
                .speed(0.1)
                .max_decimals(SHAPE_DECIMALS)
                .update_while_editing(false)
                .custom_parser(|text| {
                    let v = parse_number(text);
                    read.set(Some(v.is_some()));
                    v
                })
                .suffix(suffix),
        );
        ui.end_row();
        let done = r.drag_stopped() || r.lost_focus();
        if let Some(e) = ended.get_mut(field) {
            *e = done;
        }
        // A value is entered when the field was typed in or dragged and what it holds is a
        // number; tabbing through it, or text that is no number, enters none.
        if let Some(t) = touched.get_mut(field) {
            // Text typed and entered in one frame is applied as the field loses focus.
            if r.changed() && (r.has_focus() || r.dragged() || (r.lost_focus() && !escape && typing)) {
                *t = true;
            }
            if done {
                if let Some(en) = entered.get_mut(field) {
                    *en = *t && read.get() != Some(false);
                }
                *t = false;
            }
        }
        // A field still in use, also one focus just moved to as another one ended.
        if let Some(u) = in_use.get_mut(field) {
            *u = (r.has_focus() || r.dragged()) && !done;
        }
        field += 1;
    };
    match &mut shape {
        Shape::Circle { center, radius, .. } => {
            num(ui, "Centre X", &mut center.x, " mm");
            num(ui, "Centre Y", &mut center.y, " mm");
            num(ui, "Radius", radius, " mm");
        }
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => {
            num(ui, "Centre X", &mut center.x, " mm");
            num(ui, "Centre Y", &mut center.y, " mm");
            num(ui, "Radius", radius, " mm");
            num(ui, "Start angle", start, "°");
            num(ui, "End angle", end, "°");
        }
        Shape::Polyline { points, closed, .. } => {
            ui.label("Points");
            ui.label(points.len().to_string());
            ui.end_row();
            ui.label("Closed");
            let can = *closed || points.len() >= 3;
            let r = ui
                .add_enabled(can, egui::Checkbox::without_text(closed))
                .on_disabled_hover_text("A polyline needs three points or more to close.");
            ui.end_row();
            if let Some(e) = ended.first_mut() {
                *e = r.changed();
            }
            if let Some(en) = entered.first_mut() {
                *en = r.changed();
            }
        }
        _ => {}
    }
    let mut draft = shape_fields(&shape);
    ps.shape_in_use = in_use;
    let fields: Vec<usize> = ended.iter().enumerate().filter_map(|(k, &e)| e.then_some(k)).collect();
    if fields.is_empty() {
        ps.shape = Some(shape);
        ps.shape_base = Some(with_fields(drawing, &base));
        return None;
    }
    // Escape drops what was entered in the fields it ended (an arrow key may already have
    // changed one).
    if escape {
        for (k, (d, b)) in draft.iter_mut().zip(&base).enumerate() {
            if ended.get(k).copied().unwrap_or(false) {
                *d = *b;
            }
        }
        ps.shape = Some(with_fields(drawing, &draft));
        ps.shape_base = Some(with_fields(drawing, &base));
        return None;
    }
    let mut changes = Vec::new();
    for (k, ((d, b), e)) in draft.iter().zip(base.iter_mut()).zip(&ended).enumerate() {
        if !e {
            continue;
        }
        // Not within reach of the shown value (a number that is not finite included).
        let shown = (d - *b).abs() <= SAME_VALUE;
        if !shown {
            changes.push((k, *d));
        }
        // What was entered is the baseline now.
        *b = *d;
    }
    ps.shape = Some(with_fields(drawing, &draft));
    ps.shape_base = Some(with_fields(drawing, &base));
    let entered = fields
        .iter()
        .copied()
        .filter(|&k| entered.get(k).copied().unwrap_or(false))
        .filter_map(|k| Some((k, *draft.get(k)?)))
        .collect();
    Some(ShapeCommit { changes, entered })
}

/// Decimals shown in a drawing's fields (egui may show fewer for a large value): the text
/// read back from a field left untouched is the value it showed to within [`SAME_VALUE`].
const SHAPE_DECIMALS: usize = 6;
/// A field whose value moved less than this (a ten-thousandth of a millimetre or a degree) from
/// what it showed was not changed.
const SAME_VALUE: f64 = 1e-4;

/// The values [`shape_rows`] shows for a drawing, in its order: a circle's centre x, centre y
/// and radius; an arc's, then its start and end angle; a polyline's closed box (1 when ticked).
fn shape_fields(s: &Shape) -> Vec<f64> {
    match s {
        Shape::Circle { center, radius, .. } => vec![center.x, center.y, *radius],
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => vec![center.x, center.y, *radius, *start, *end],
        Shape::Polyline { closed, .. } => vec![if *closed { 1.0 } else { 0.0 }],
        _ => Vec::new(),
    }
}

/// Set `s`'s fields from `v`, in [`shape_fields`]' order.
fn set_shape_fields(s: &mut Shape, v: &[f64]) {
    match (s, v) {
        (Shape::Circle { center, radius, .. }, [x, y, r]) => {
            (center.x, center.y, *radius) = (*x, *y, *r);
        }
        (
            Shape::Arc {
                center,
                radius,
                start,
                end,
            },
            [x, y, r, a, b],
        ) => {
            (center.x, center.y, *radius, *start, *end) = (*x, *y, *r, *a, *b);
        }
        (Shape::Polyline { closed, .. }, [c]) => *closed = *c != 0.0,
        _ => {}
    }
}

/// `s` with its fields set from `v`.
fn with_fields(s: &Shape, v: &[f64]) -> Shape {
    let mut out = s.clone();
    set_shape_fields(&mut out, v);
    out
}

fn committed(r: &egui::Response) -> bool {
    r.lost_focus() || (r.has_focus() && r.ctx.input(|i| i.key_pressed(egui::Key::Enter)))
}

pub fn properties(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor, ps: &mut PanelState) {
    let t = Theme::of(ui.ctx());
    section(ui, "Properties", None);
    forget_spec_unless_wires(ed, ps);
    if let Some(pi) = ed.panel {
        panel_properties(ui, open, ed, pi);
        return;
    }
    if ed.selection.is_empty()
        && let Some(id) = ed.comment.clone()
    {
        comment_properties(ui, open, ed, ps, &id, &t);
        return;
    }
    if ed.selection.is_empty() {
        // The page and its comments: comments are not part of the page, so they stay usable on
        // a page someone else holds.
        if let Some(p) = open.project.pages.get(ed.page) {
            ui.label(format!("Page {} · {}", p.number, p.title));
            ui.label(RichText::new(format!("{} symbols, {} wires", p.symbols.len(), p.wires.len())).color(t.faint));
        }
        ui.label(RichText::new("Select an item on the sheet to edit it.").color(t.faint));
        page_comments(ui, open, ed, &t);
        return;
    }
    // On a page someone else holds the values are shown, not edited. A value half typed when
    // the page became held is dropped (the fields are read from the project again each frame),
    // and if a field still commits as it loses focus, the change and its undo step are taken back.
    let held = open
        .project
        .pages
        .get(ed.page)
        .is_some_and(|p| ed.held_by.contains_key(&p.id));
    if held {
        ps.loaded_for = None;
        ps.spec_for = Default::default();
        open.without_changes(|open| {
            ui.add_enabled_ui(false, |ui| selection_properties(ui, open, ed, ps, &t));
        });
    } else {
        selection_properties(ui, open, ed, ps, &t);
    }
}

/// The properties of the selected items (not a comment, not nothing).
fn selection_properties(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor, ps: &mut PanelState, t: &Theme) {
    load_buffers(open, ed, ps);
    let page_idx = ed.page;
    if ed.selection.len() > 1 {
        let n = ed.selection.len();
        let wires: Vec<String> = ed
            .selection
            .iter()
            .filter_map(|s| match s {
                Selection::Wire(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let symbols: Vec<String> = ed
            .selection
            .iter()
            .filter_map(|s| match s {
                Selection::Symbol(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        if wires.len() == n {
            // Only wires: their section, colour and type can be set together.
            ui.label(format!("{n} wires selected"));
            egui::Grid::new("wires-props")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| spec_rows(ui, open, page_idx, &wires, ps));
            ui.label(RichText::new("A value typed here is set on all of them.").color(t.faint));
        } else {
            ui.label(format!("{n} items selected"));
        }
        if symbols.len() == n {
            jumper_controls(ui, open, page_idx, &symbols);
        }
        ui.label(RichText::new("Move, rotate, mirror, copy or delete them together.").color(t.faint));
        return;
    }
    match ed.one().cloned() {
        None => {
            if let Some(p) = open.project.pages.get(page_idx) {
                ui.label(format!("Page {} · {}", p.number, p.title));
                ui.label(RichText::new(format!("{} symbols, {} wires", p.symbols.len(), p.wires.len())).color(t.faint));
            }
            ui.label(RichText::new("Select an item on the sheet to edit it.").color(t.faint));
            page_comments(ui, open, ed, t);
        }
        Some(Selection::Symbol(id)) => {
            let Some(placed) = open.project.pages.get(page_idx).and_then(|p| p.symbol(&id)).cloned() else {
                ed.selection.clear();
                return;
            };
            let sym_name = open
                .symbols
                .get(&placed.symbol)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "missing symbol".into());
            jumper_controls(ui, open, page_idx, std::slice::from_ref(&id));
            egui::Grid::new("sym-props")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Symbol");
                    ui.label(sym_name).on_hover_text(&placed.symbol);
                    ui.end_row();
                    ui.label("Device tag");
                    let r = ui.text_edit_singleline(&mut ps.tag);
                    if committed(&r) {
                        let new = if ps.tag.trim().is_empty() {
                            Ok(None)
                        } else {
                            DeviceTag::parse(ps.tag.trim()).map(Some)
                        };
                        match new {
                            Ok(tag) if tag != placed.tag => {
                                open.checkpoint();
                                if let Some(tg) = &tag {
                                    open.project.devices.entry(tg.clone()).or_default();
                                }
                                if let Some(s) = open
                                    .project
                                    .pages
                                    .get_mut(page_idx)
                                    .and_then(|p| p.symbols.iter_mut().find(|s| s.id == id))
                                {
                                    s.tag = tag;
                                }
                                ps.tag_error = None;
                                ps.loaded_for = None;
                            }
                            Ok(_) => ps.tag_error = None,
                            Err(e) => ps.tag_error = Some(e.to_string()),
                        }
                    }
                    ui.end_row();
                    if let Some(err) = &ps.tag_error {
                        ui.label("");
                        ui.label(RichText::new(err).color(t.error).small());
                        ui.end_row();
                    }
                    if let Some(tag) = placed.tag.clone() {
                        ui.label("Description");
                        let r = ui.text_edit_singleline(&mut ps.description);
                        if committed(&r)
                            && open.project.devices.get(&tag).map(|d| d.description.as_str())
                                != Some(ps.description.as_str())
                        {
                            open.checkpoint();
                            open.project.devices.entry(tag).or_default().description = ps.description.clone();
                        }
                        ui.end_row();
                    }
                    ui.label("Rotation");
                    let mut rot = placed.rotation;
                    egui::ComboBox::from_id_salt("rot")
                        .selected_text(rotation_label(rot))
                        .show_ui(ui, |ui| {
                            for r in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
                                ui.selectable_value(&mut rot, r, rotation_label(r));
                            }
                        });
                    if rot != placed.rotation {
                        open.checkpoint();
                        if let Some(s) = open
                            .project
                            .pages
                            .get_mut(page_idx)
                            .and_then(|p| p.symbols.iter_mut().find(|s| s.id == id))
                        {
                            s.rotation = rot;
                        }
                    }
                    ui.end_row();
                    ui.label("Position");
                    ui.label(format!("{:.1}, {:.1} mm", placed.at.x, placed.at.y));
                    ui.end_row();
                });
            if let Some(tag) = placed.tag.clone() {
                device_section(ui, open, ps, &tag, t);
            }
            if let Some(sym) = open.symbols.get(&placed.symbol) {
                ui.add_space(6.0);
                subsection(ui, "Pins");
                let names: Vec<&str> = sym.pins.iter().map(|p| p.name.as_str()).collect();
                ui.label(names.join("  "));
            }
        }
        Some(Selection::Graphic(i)) => {
            let Some(g) = open
                .project
                .pages
                .get(page_idx)
                .and_then(|p| p.graphics.get(i))
                .cloned()
            else {
                ed.selection.clear();
                return;
            };
            let kind = match &g {
                Shape::Line { .. } => "Line",
                Shape::Polyline { .. } => "Polyline",
                Shape::Rect { .. } => "Rectangle",
                Shape::Circle { .. } => "Circle",
                Shape::Arc { .. } => "Arc",
                Shape::Text { .. } => "Text",
                Shape::Dimension { .. } => "Dimension",
            };
            // The fields are this drawing's: one being typed in when another is selected goes
            // with its drawing, not into the same field of the next one. Drawings carry no id, so
            // the one at this index is taken for another when a drawing before it changes, or
            // when the page has fewer drawings (one deleted, this one maybe): the fields then
            // read it afresh and lose what was being typed. One appended after it leaves them be.
            let graphics = open
                .project
                .pages
                .get(page_idx)
                .map(|p| p.graphics.as_slice())
                .unwrap_or_default();
            let count = graphics.len();
            let before = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                format!("{:?}", graphics.get(..i).unwrap_or_default()).hash(&mut h);
                h.finish()
            };
            // A drawing of another kind in its place is another drawing too.
            let kind_now = std::mem::discriminant(&g);
            let same = matches!(ps.drawing_seen, Some((p, j, h, n, k))
                if p == page_idx && j == i && h == before && count >= n && k == kind_now);
            if !same {
                ps.drawing_gen += 1;
                if let Shape::Text { text, height, .. } = &g {
                    ps.text = text.clone();
                    ps.text_height = *height;
                }
                ps.shape = None;
                ps.shape_base = None;
                ps.shape_in_use.clear();
                ps.shape_touched.clear();
                ps.shape_error = None;
                ps.shape_refused.clear();
            }
            ps.drawing_seen = Some((page_idx, i, before, count, kind_now));
            let shown = ps.drawing_gen;
            ui.push_id(("gfx-props", page_idx, i, shown), |ui| {
                egui::Grid::new("gfx-props")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Drawing");
                        ui.label(kind);
                        ui.end_row();
                        if let Shape::Text { at, .. } = &g {
                            ui.label("Text");
                            let r = ui.text_edit_singleline(&mut ps.text);
                            ui.end_row();
                            ui.label("Height");
                            let h = ui.add(
                                egui::DragValue::new(&mut ps.text_height)
                                    .range(0.5..=50.0)
                                    .speed(0.1)
                                    .suffix(" mm"),
                            );
                            ui.end_row();
                            if committed(&r) || h.drag_stopped() || h.lost_focus() {
                                if ps.text.trim().is_empty() {
                                    // An empty text would vanish; put the old one back.
                                    ps.loaded_for = None;
                                } else {
                                    open.set_text(page_idx, i, ps.text.trim(), ps.text_height);
                                }
                            }
                            ui.label("Position");
                            ui.label(format!("{:.1}, {:.1} mm", at.x, at.y));
                            ui.end_row();
                        } else if matches!(g, Shape::Circle { .. } | Shape::Arc { .. } | Shape::Polyline { .. }) {
                            if let Some(c) = shape_rows(ui, ps, &g, (page_idx, i, shown)) {
                                // Each field is set on its own (one undo step each): a field
                                // refused (no radius, an arc of no angle) keeps its reason and
                                // reads the drawing's own value again, and does not take a good
                                // value entered in another field with it.
                                let mut cur = g.clone();
                                let mut refused = Vec::new();
                                for (k, v) in c.changes {
                                    let mut values = shape_fields(&cur);
                                    if let Some(f) = values.get_mut(k) {
                                        *f = v;
                                    }
                                    let next = with_fields(&cur, &values);
                                    match open.set_shape(page_idx, i, next.clone()) {
                                        Ok(_) => cur = next,
                                        Err(why) => {
                                            ps.shape_refused.insert(k, why.to_owned());
                                            refused.push(k);
                                        }
                                    }
                                }
                                // A refused field entered again with a value that is taken (or
                                // changes nothing and would be taken) takes its reason back;
                                // another field doesn't. A value too near the stored one to be a
                                // change is still checked: 0 for a radius of 0.00005 is no radius.
                                for (k, v) in c.entered.iter().filter(|(k, _)| !refused.contains(k)) {
                                    let mut values = shape_fields(&cur);
                                    if let Some(f) = values.get_mut(*k) {
                                        *f = *v;
                                    }
                                    match open.shape_checked(page_idx, i, with_fields(&cur, &values)) {
                                        Ok(_) => {
                                            ps.shape_refused.remove(k);
                                        }
                                        Err(why) => {
                                            ps.shape_refused.insert(*k, why.to_owned());
                                        }
                                    }
                                }
                                let mut why: Vec<&str> = Vec::new();
                                for w in ps.shape_refused.values() {
                                    if !why.contains(&w.as_str()) {
                                        why.push(w);
                                    }
                                }
                                ps.shape_error = (!why.is_empty()).then(|| why.join(" "));
                            }
                            if let Some(err) = &ps.shape_error {
                                ui.label("");
                                ui.label(RichText::new(err).color(t.error).small());
                                ui.end_row();
                            }
                        } else {
                            for (label, value) in drawing_rows(&g) {
                                ui.label(label);
                                ui.label(value);
                                ui.end_row();
                            }
                        }
                    });
            });
        }
        Some(Selection::Wire(id)) => {
            let Some(wire) = open
                .project
                .pages
                .get(page_idx)
                .and_then(|p| p.wires.iter().find(|w| w.id == id))
                .cloned()
            else {
                ed.selection.clear();
                return;
            };
            egui::Grid::new("wire-props")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Wire number");
                    let r = ui.text_edit_singleline(&mut ps.number);
                    let number = Some(ps.number.trim().to_owned()).filter(|s| !s.is_empty());
                    if committed(&r) && number != wire.number {
                        open.checkpoint();
                        if let Some(w) = open
                            .project
                            .pages
                            .get_mut(page_idx)
                            .and_then(|p| p.wires.iter_mut().find(|w| w.id == id))
                        {
                            w.number = number;
                        }
                    }
                    ui.end_row();
                    ui.label("Potential");
                    let r = ui.text_edit_singleline(&mut ps.potential);
                    let potential = Some(ps.potential.trim().to_owned()).filter(|s| !s.is_empty());
                    if committed(&r) && potential != wire.potential {
                        open.checkpoint();
                        if let Some(w) = open
                            .project
                            .pages
                            .get_mut(page_idx)
                            .and_then(|p| p.wires.iter_mut().find(|w| w.id == id))
                        {
                            w.potential = potential;
                        }
                    }
                    ui.end_row();
                    ui.label("Cable");
                    let cables = open.cables();
                    let r = ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut ps.cable)
                                .hint_text("e.g. -W1")
                                .desired_width(110.0),
                        );
                        let mut picked = false;
                        if !cables.is_empty() {
                            egui::ComboBox::from_id_salt("cables")
                                .selected_text("")
                                .width(18.0)
                                .show_ui(ui, |ui| {
                                    for c in &cables {
                                        if ui.selectable_label(false, c.to_string()).clicked() {
                                            ps.cable = c.to_string();
                                            picked = true;
                                        }
                                    }
                                });
                        }
                        committed(&r) || picked
                    });
                    ui.end_row();
                    ui.label("Core");
                    let rc = ui.add(
                        egui::TextEdit::singleline(&mut ps.core)
                            .hint_text("e.g. 1 or BN")
                            .desired_width(110.0),
                    );
                    ui.end_row();
                    if r.inner || committed(&rc) {
                        let cable = ps.cable.trim();
                        if cable.is_empty() {
                            ps.cable_error = None;
                            open.set_wire_cable(page_idx, &id, None);
                        } else {
                            match DeviceTag::parse(cable) {
                                Ok(tag) => {
                                    ps.cable_error = None;
                                    open.set_wire_cable(
                                        page_idx,
                                        &id,
                                        Some(wc_core::CableCore {
                                            cable: tag,
                                            core: ps.core.trim().to_owned(),
                                        }),
                                    );
                                }
                                Err(e) => ps.cable_error = Some(e.to_string()),
                            }
                        }
                    }
                    if let Some(e) = &ps.cable_error {
                        ui.label("");
                        ui.label(RichText::new(e).color(t.error).small());
                        ui.end_row();
                    }
                    spec_rows(ui, open, page_idx, std::slice::from_ref(&id), ps);
                    ui.label("Bends");
                    ui.label(wire.via.len().to_string());
                    ui.end_row();
                });
        }
    }
}

/// More parts of the device (auxiliary contact block, marker, end plate), each with a quantity.
fn accessories_section(ui: &mut egui::Ui, open: &mut Open, ps: &mut PanelState, tag: &DeviceTag, t: &Theme) {
    subsection(ui, "Accessories");
    let current: Vec<wc_core::DevicePart> = open
        .project
        .devices
        .get(tag)
        .map(|d| d.accessories.clone())
        .unwrap_or_default();
    let label = |id: &str| {
        open.part_library
            .iter()
            .find(|(p, _)| p == id)
            .map_or_else(|| id.to_owned(), |(_, l)| l.clone())
    };
    // Row edits are noted while drawing and applied after, so no index is shared between lists.
    let mut edit: Option<(usize, i8)> = None;
    for (i, a) in current.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.small_button("\u{2212}").on_hover_text("One less").clicked() {
                edit = Some((i, -1));
            }
            ui.label(format!("{} x", a.qty));
            if ui.small_button("+").on_hover_text("One more").clicked() {
                edit = Some((i, 1));
            }
            ui.add(egui::Label::new(label(&a.part)).truncate())
                .on_hover_text(&a.part);
        });
    }
    let mut next = current.clone();
    if let Some((i, step)) = edit
        && let Some(a) = next.get_mut(i)
    {
        if step > 0 {
            a.qty = a.qty.saturating_add(1);
        } else if a.qty > 1 {
            a.qty -= 1;
        } else {
            next.remove(i);
        }
    }
    let mut add = None;
    egui::ComboBox::from_id_salt("accessory")
        .width(ui.available_width().min(240.0))
        .selected_text("Add a part\u{2026}")
        .height(320.0)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut ps.accessory_search).hint_text("Search parts"));
            let q = ps.accessory_search.to_lowercase();
            for (id, l) in &open.part_library {
                if !q.is_empty() && !id.to_lowercase().contains(&q) && !l.to_lowercase().contains(&q) {
                    continue;
                }
                if ui.selectable_label(false, l).on_hover_text(id).clicked() {
                    add = Some(id.clone());
                }
            }
        });
    if let Some(id) = add {
        match next.iter_mut().find(|a| a.part == id) {
            Some(a) => a.qty = a.qty.saturating_add(1),
            None => next.push(wc_core::DevicePart { part: id, qty: 1 }),
        }
    }
    if next != current {
        ps.part_error = open.set_device_accessories(tag, next).err();
    }
    if current.is_empty() {
        ui.label(
            RichText::new("None. Add parts that come with the main part.")
                .color(t.faint)
                .small(),
        );
    }
}

/// A drop-down to pick a part from `library` (id, label), or none, with a search field
/// (`search`) at its top. Shows `current` by its label. Returns the pick, if one was made:
/// `Some(None)` for none.
fn part_picker(
    ui: &mut egui::Ui,
    salt: &str,
    max_width: f32,
    library: &[(String, String)],
    current: Option<&str>,
    search: &mut String,
) -> Option<Option<String>> {
    let label = current.map_or_else(
        || "none".to_owned(),
        |id| {
            library
                .iter()
                .find(|(p, _)| p == id)
                .map_or(id.to_owned(), |(_, l)| l.clone())
        },
    );
    let short: String = if label.chars().count() > 34 {
        format!("{}\u{2026}", label.chars().take(33).collect::<String>())
    } else {
        label
    };
    let mut pick = None;
    egui::ComboBox::from_id_salt(salt)
        .width(ui.available_width().min(max_width))
        .selected_text(short)
        .height(320.0)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(search).hint_text("Search parts"));
            if ui
                .selectable_label(current.is_none(), RichText::new("none").italics())
                .clicked()
            {
                pick = Some(None);
            }
            let q = search.to_lowercase();
            for (id, l) in library {
                if !q.is_empty() && !id.to_lowercase().contains(&q) && !l.to_lowercase().contains(&q) {
                    continue;
                }
                if ui
                    .selectable_label(current == Some(id.as_str()), l)
                    .on_hover_text(id)
                    .clicked()
                {
                    pick = Some(Some(id.clone()));
                }
            }
        });
    pick
}

/// Part and own properties of the selected symbol's device.
fn device_section(ui: &mut egui::Ui, open: &mut Open, ps: &mut PanelState, tag: &DeviceTag, t: &Theme) {
    subsection(ui, "Part");
    let current = open.project.devices.get(tag).and_then(|d| d.part.clone());
    let pick = part_picker(
        ui,
        "part",
        240.0,
        &open.part_library,
        current.as_deref(),
        &mut ps.part_search,
    );
    if let Some(choice) = pick {
        ps.part_error = open.set_device_part(tag, choice.as_deref()).err();
    }
    if let Some(e) = &ps.part_error {
        ui.label(RichText::new(e).color(t.error).small());
    }
    if let Some(part) = current.as_ref().and_then(|id| open.parts.get(id)) {
        let rows = [
            ("Manufacturer", part.manufacturer.as_str()),
            ("Order number", part.order_number.as_str()),
            ("Description", part.description.as_str()),
        ];
        for (k, v) in rows
            .into_iter()
            .chain(part.properties.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        {
            if !v.is_empty() {
                let mut job = egui::text::LayoutJob::default();
                let font = egui::TextStyle::Body.resolve(ui.style());
                job.append(&format!("{k}  "), 0.0, egui::TextFormat::simple(font.clone(), t.faint));
                job.append(v, 0.0, egui::TextFormat::simple(font, ui.visuals().text_color()));
                ui.add(egui::Label::new(job).wrap());
            }
        }
    }

    accessories_section(ui, open, ps, tag, t);

    subsection(ui, "Device properties");
    egui::Grid::new("dev-props")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for (k, v) in &mut ps.props {
                ui.label(k.as_str());
                let r = ui.add(egui::TextEdit::singleline(v).desired_width(140.0));
                if committed(&r) {
                    open.set_device_property(tag, k, v);
                }
                ui.end_row();
            }
            ui.add(
                egui::TextEdit::singleline(&mut ps.new_key)
                    .hint_text("new property")
                    .desired_width(90.0),
            );
            let r = ui.add(
                egui::TextEdit::singleline(&mut ps.new_value)
                    .hint_text("value")
                    .desired_width(140.0),
            );
            if committed(&r) && !ps.new_key.trim().is_empty() && !ps.new_value.trim().is_empty() {
                open.set_device_property(tag, &ps.new_key, &ps.new_value);
                ps.props
                    .push((ps.new_key.trim().to_owned(), ps.new_value.trim().to_owned()));
                ps.new_key.clear();
                ps.new_value.clear();
            }
            ui.end_row();
        });
    ui.label(
        RichText::new("Shared by every symbol with this tag. Clear a value to remove it.")
            .color(t.faint)
            .small(),
    );
}

/// The review comments on the current page, open ones first; clicking one shows it.
fn page_comments(ui: &mut egui::Ui, open: &Open, ed: &mut Editor, t: &Theme) {
    let Some(page) = open.project.pages.get(ed.page) else {
        return;
    };
    let mut list: Vec<&wc_core::Comment> = open.project.page_comments(&page.id).collect();
    if list.is_empty() {
        return;
    }
    list.sort_by_key(|c| c.resolved);
    ui.add_space(10.0);
    let open_count = list.iter().filter(|c| !c.resolved).count();
    section(ui, "Comments", Some(open_count));
    for c in list {
        let first: String = c.text.lines().next().unwrap_or_default().chars().take(40).collect();
        let text = RichText::new(format!("{}  {first}", c.id));
        let text = if c.resolved { text.color(t.faint) } else { text };
        if row(
            ui,
            false,
            Lead::Dot(if c.resolved { t.faint } else { t.accent }),
            text,
            false,
        )
        .on_hover_text(format!("{} · {}", c.author, wc_core::review::utc_date(c.time)))
        .clicked()
        {
            ed.comment = Some(c.id.clone());
        }
    }
}

/// One review comment: who wrote it and when, the text, the replies, and what can be done
/// with it.
fn comment_properties(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor, ps: &mut PanelState, id: &str, t: &Theme) {
    let Some(c) = open.project.comments.iter().find(|c| c.id == id).cloned() else {
        ed.comment = None;
        return;
    };
    let number = open.project.page(&c.page).map(|p| p.number.clone()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Comment {}", c.id)).strong());
        badge(ui, if c.resolved { "Resolved" } else { "Open" }, t);
    });
    ui.label(RichText::new(format!("Page {number}")).color(t.faint).small());
    ui.add_space(6.0);
    let entry = |ui: &mut egui::Ui, author: &str, time: i64, text: &str| {
        ui.label(
            RichText::new(format!("{author}  ·  {}", wc_core::review::utc_date(time)))
                .color(t.muted)
                .small(),
        );
        ui.add(egui::Label::new(text).wrap());
        ui.add_space(6.0);
    };
    entry(ui, &c.author, c.time, &c.text);
    for r in &c.replies {
        entry(ui, &r.author, r.time, &r.text);
    }
    ui.add(
        egui::TextEdit::multiline(&mut ps.reply)
            .hint_text("Reply")
            .desired_rows(2)
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!ps.reply.trim().is_empty(), egui::Button::new("Reply"))
            .clicked()
        {
            let mut next = open.project.clone();
            if next
                .reply_to_comment(id, &crate::state::author_name(), crate::state::now_secs(), &ps.reply)
                .is_ok()
            {
                open.checkpoint();
                open.project = next;
                ps.reply.clear();
            }
        }
        let (label, to) = if c.resolved {
            ("Reopen", false)
        } else {
            ("Resolve", true)
        };
        if ui.button(label).clicked() {
            open.checkpoint();
            let _ = open.project.resolve_comment(id, to);
        }
        if ui.button("Delete").clicked() {
            open.checkpoint();
            let _ = open.project.remove_comment(id);
            ed.comment = None;
        }
    });
    ui.add_space(4.0);
    if ui.small_button("Back to the page").clicked() {
        ed.comment = None;
    }
}

fn rotation_label(r: Rotation) -> &'static str {
    match r {
        Rotation::R0 => "0°",
        Rotation::R90 => "90°",
        Rotation::R180 => "180°",
        Rotation::R270 => "270°",
    }
}

/// The status bar. `live` is the check while drawing's text, whether a check is pending and
/// whether the last one failed, or `None` when that is off; it returns true when that text was
/// clicked.
pub fn status_bar(ui: &mut egui::Ui, open: Option<&Open>, ed: &Editor, live: Option<(String, bool, bool)>) -> bool {
    let mut show_check = false;
    let t = Theme::of(ui.ctx());
    let sep = |ui: &mut egui::Ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(1.0, 14.0), egui::Sense::hover());
        ui.painter().line_segment(
            [r.center_top(), r.center_bottom()],
            egui::Stroke::new(1.0_f32, t.border),
        );
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if open.is_none() {
            ui.label(RichText::new("No project open").color(t.muted));
            return;
        }
        let tool = match ed.tool {
            Tool::Select => "Select",
            Tool::Place => "Place symbol",
            Tool::Wire => "Draw wire",
            Tool::Text => "Text",
            Tool::Comment => "Comment",
            Tool::Line => "Line",
            Tool::Rect => "Rectangle",
            Tool::Circle => "Circle",
            Tool::Arc => "Arc",
            Tool::Polyline => "Polyline",
            Tool::Dimension => "Dimension",
        };
        // The current tool as an accent pill.
        let g = ui
            .painter()
            .layout_no_wrap(tool.to_string(), crate::theme::strong(11.5), t.accent);
        let (r, _) = ui.allocate_exact_size(egui::vec2(g.size().x + 16.0, 18.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 9.0, t.accent_soft);
        ui.painter().galley(r.center() - g.size() / 2.0, g, t.accent);
        sep(ui);
        let coords = match ed.cursor {
            Some(c) => format!("X {:>6.1}   Y {:>6.1}  mm", c.x, c.y),
            None => "X      –   Y      –  mm".to_string(),
        };
        ui.label(RichText::new(coords).monospace().color(t.muted));
        sep(ui);
        ui.label(RichText::new(format!("{:.0} %", ed.view.zoom / 3.78 * 100.0)).color(t.muted));
        sep(ui);
        ui.label(RichText::new(&ed.message).color(t.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(o) = open {
                if let Some((mut text, pending, failed)) = live {
                    let errors = ed.live.iter().any(|f| f.severity == wc_report::Severity::Error);
                    let warnings = ed.live.iter().any(|f| f.severity == wc_report::Severity::Warning);
                    let c = if errors || failed {
                        t.error
                    } else if warnings {
                        t.warning
                    } else {
                        t.muted
                    };
                    if pending {
                        text.push_str(" …");
                    }
                    let r = ui
                        .add(egui::Label::new(RichText::new(text).color(c)).sense(egui::Sense::click()))
                        .on_hover_text("Problems the check found after the last edit: click to list them");
                    show_check = r.clicked();
                    if !o.branch.is_empty() {
                        sep(ui);
                    }
                }
                if !o.branch.is_empty() {
                    ui.label(RichText::new(&o.branch).monospace().color(t.text));
                    let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    crate::icons::paint(ui.painter(), r, crate::icons::Icon::Branch, t.muted, t.accent);
                }
                if o.dirty {
                    sep(ui);
                    ui.label(RichText::new("Unsaved changes").color(t.warning));
                    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, t.warning);
                } else {
                    sep(ui);
                    ui.label(RichText::new("Saved").color(t.muted));
                    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, t.success);
                }
            }
        });
    });
    show_check
}

/// The properties of the panel layout shown: plate size, the picked device, and the devices
/// with a part that are not on the plate yet.
fn panel_properties(ui: &mut egui::Ui, open: &Open, ed: &Editor, pi: usize) {
    use wc_core::panel::{PanelItem, on_door, part_size};
    let t = Theme::of(ui.ctx());
    let Some(panel) = open.project.panels.get(pi) else {
        return;
    };
    ui.label(RichText::new(&panel.name).strong());
    ui.label(RichText::new(format!("Mounting plate {} x {} mm", panel.width, panel.height)).color(t.faint));
    ui.add_space(6.0);
    if let Some(PanelItem::Device { tag, at, width, height }) = ed.panel_pick.and_then(|i| panel.items.get(i)) {
        let dev = open.project.devices.get(tag);
        egui::Grid::new("panel-dev")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Device").color(t.muted));
                ui.label(tag.to_string());
                ui.end_row();
                ui.label(RichText::new("Description").color(t.muted));
                ui.label(dev.map(|d| d.description.as_str()).unwrap_or(""));
                ui.end_row();
                ui.label(RichText::new("Part").color(t.muted));
                ui.label(dev.and_then(|d| d.part.as_deref()).unwrap_or("none"));
                ui.end_row();
                ui.label(RichText::new("Size").color(t.muted));
                ui.label(format!("{width} x {height} mm"));
                ui.end_row();
                ui.label(RichText::new("Position").color(t.muted));
                ui.label(format!("x {:.0}, y {:.0} mm", at.x, at.y));
                ui.end_row();
            });
        ui.add_space(6.0);
    } else {
        ui.label(RichText::new("Click a device on the plate to see it; drag it to move it.").color(t.faint));
        ui.add_space(6.0);
    }
    // Devices with a part that could go on the plate but are not on it.
    let on_plate = panel.device_tags();
    let missing: Vec<String> = open
        .project
        .devices
        .iter()
        .filter(|(tag, _)| !on_plate.contains(*tag))
        .filter_map(|(tag, d)| {
            let part = open.parts.get(d.part.as_ref()?)?;
            (!on_door(part)).then(|| {
                let size = part_size(part).map_or_else(|| "no size".to_owned(), |(w, h)| format!("{w} x {h}"));
                format!("{tag}  ({size})")
            })
        })
        .collect();
    if missing.is_empty() {
        ui.label(RichText::new("Every device with a part is on the plate.").color(t.faint));
    } else {
        ui.label(RichText::new(format!("Not on the plate yet ({})", missing.len())).color(t.muted));
        for m in missing {
            ui.label(RichText::new(m).monospace());
        }
        ui.label(RichText::new("Page menu → Arrange devices puts them on the rails.").color(t.faint));
    }
}

/// The Properties rows of a drawing without fields of its own: a dimension shows its length
/// and the two points it measures, other drawings the corners of their bounds.
fn drawing_rows(g: &Shape) -> Vec<(&'static str, String)> {
    let at = |p: wc_core::Point| format!("{:.1}, {:.1} mm", p.x, p.y);
    match g {
        Shape::Dimension { a, b, .. } => vec![
            ("Length", format!("{} mm", wc_core::geom::format_length(a.distance(*b)))),
            ("From", at(*a)),
            ("To", at(*b)),
        ],
        _ => {
            let (a, b) = g.bounds();
            vec![("From", at(a)), ("To", at(b))]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts drawn by one frame of `jumper_controls` for `ids` on page `page`.
    fn jumper_texts(o: &mut Open, page: usize, ids: &[String]) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| jumper_controls(ui, o, page, ids));
        });
        let mut texts = Vec::new();
        for clipped in out.shapes {
            if let egui::Shape::Text(t) = clipped.shape {
                texts.push(t.galley.text().to_owned());
            }
        }
        texts
    }

    #[test]
    fn a_terminal_numbered_automatically_has_its_jumper_and_part_controls() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, mut o) = crate::state::tests::project();
        let page = o
            .project
            .pages
            .iter()
            .position(|p| p.id == "p0002")
            .ok_or("no page p0002")?;
        let x1 = DeviceTag::parse("-X1")?;
        let mut ids = Vec::new();
        for at in [10.0, 20.0] {
            let id = o.place(page, "wc.terminal", wc_core::geom::Point::new(at, 20.0))?;
            let s = o.project.pages[page]
                .symbols
                .iter_mut()
                .find(|s| s.id == id)
                .ok_or("not placed")?;
            s.tag = Some(x1.clone());
            ids.push(id);
        }
        // Both tagged with only the strip: numbered 1 and 2 by the terminal plan.
        let texts = jumper_texts(&mut o, page, &ids);
        assert!(
            texts.iter().any(|t| t.contains("Jumper these 2 terminals")),
            "{texts:?}"
        );
        o.add_jumper(&x1, &["1".into(), "2".into()])?;
        o.set_jumper_part(&x1, "2", Some("example.ex-tb-2-5"))?;
        let texts = jumper_texts(&mut o, page, &ids[..1]);
        assert!(texts.iter().any(|t| t == "Jumpered with 2"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Jumper part"), "{texts:?}");
        assert!(
            texts.iter().all(|t| t != "none"),
            "the jumper's part is shown: {texts:?}"
        );
        Ok(())
    }

    #[test]
    fn a_half_typed_value_is_dropped_when_the_page_becomes_held() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, mut o) = crate::state::tests::project();
        let tag = DeviceTag::parse("-K1")?;
        o.project.pages[0].symbols.push(wc_core::PlacedSymbol {
            id: "s1".into(),
            symbol: "iec.coil".into(),
            at: wc_core::geom::Point::new(50.0, 50.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(tag),
        });
        let mut ed = Editor::default();
        ed.selection = vec![Selection::Symbol("s1".into())];
        let mut ps = PanelState::default();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let frame = |o: &mut Open, ed: &mut Editor, ps: &mut PanelState| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| properties(ui, o, ed, ps));
            });
        };
        frame(&mut o, &mut ed, &mut ps);
        assert_eq!(ps.tag, "-K1");
        // A new tag half typed, then Ola opens the page.
        ps.tag = "-K9".into();
        let before = o.project.clone();
        let page = o.project.pages[0].id.clone();
        ed.held_by.insert(page, "Ola".into());
        frame(&mut o, &mut ed, &mut ps);
        assert_eq!(ps.tag, "-K1", "the field shows the project's value again");
        assert_eq!(o.project, before, "nothing was changed");
        Ok(())
    }

    #[test]
    fn a_field_being_typed_in_changes_nothing_when_the_page_becomes_held() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, mut o) = crate::state::tests::project();
        o.project.pages[0].symbols.push(wc_core::PlacedSymbol {
            id: "s1".into(),
            symbol: "iec.coil".into(),
            at: wc_core::geom::Point::new(50.0, 50.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(DeviceTag::parse("-K1")?),
        });
        let mut ed = Editor::default();
        ed.selection = vec![Selection::Symbol("s1".into())];
        let mut ps = PanelState::default();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let frame = |o: &mut Open, ed: &mut Editor, ps: &mut PanelState, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| properties(ui, o, ed, ps));
            });
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(&mut o, &mut ed, &mut ps, Vec::new());
        // Tab to the tag field and type in it.
        let mut typing = false;
        for _ in 0..12 {
            frame(&mut o, &mut ed, &mut ps, vec![key(egui::Key::Tab)]);
            frame(&mut o, &mut ed, &mut ps, vec![egui::Event::Text("9".into())]);
            if ps.tag.ends_with('9') {
                typing = true;
                break;
            }
        }
        assert!(typing, "the tag field never took the typing");
        let (before, mark, dirty) = (o.project.clone(), o.undo_mark(), o.dirty);

        // Ola opens the page: the field is disabled and loses focus.
        let page = o.project.pages[0].id.clone();
        ed.held_by.insert(page, "Ola".into());
        frame(&mut o, &mut ed, &mut ps, Vec::new());
        frame(&mut o, &mut ed, &mut ps, vec![key(egui::Key::Enter)]);
        assert_eq!(o.project, before, "nothing was changed");
        assert_eq!(o.undo_mark(), mark, "no undo step");
        assert_eq!(o.dirty, dirty);
        assert_eq!(ps.tag, "-K1");
        Ok(())
    }

    #[test]
    fn a_comment_on_a_held_page_can_still_be_replied_to() -> Result<(), Box<dyn std::error::Error>> {
        // Comments are not part of the page: their controls stay enabled while someone else
        // has the page open. A disabled reply field would not take the typing.
        let (_tmp, mut o) = crate::state::tests::project();
        let page = o.project.pages[0].id.clone();
        let id = o.project.add_comment(&page, None, "Kari", 1, "Check this")?;
        let mut ed = Editor::default();
        ed.comment = Some(id);
        ed.held_by.insert(page, "Ola".into());
        let mut ps = PanelState::default();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let frame = |o: &mut Open, ed: &mut Editor, ps: &mut PanelState, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| properties(ui, o, ed, ps));
            });
        };
        let tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(&mut o, &mut ed, &mut ps, Vec::new());
        for _ in 0..8 {
            frame(&mut o, &mut ed, &mut ps, vec![tab.clone()]);
            frame(&mut o, &mut ed, &mut ps, vec![egui::Event::Text("x".into())]);
            if ps.reply.ends_with('x') {
                break;
            }
        }
        assert!(ps.reply.ends_with('x'), "the reply field took no typing");
        Ok(())
    }

    #[test]
    fn a_wire_colour_draft_is_dropped_when_the_page_becomes_held() {
        use wc_core::geom::Point;
        let (_tmp, mut o) = crate::state::tests::project();
        let at = |x: f64| wc_core::Endpoint::Point {
            at: Point::new(x, 10.0),
        };
        o.add_wire(0, at(10.0), at(20.0), Vec::new());
        o.add_wire(0, at(30.0), at(40.0), Vec::new());
        let ids: Vec<String> = o.project.pages[0].wires.iter().map(|w| w.id.clone()).collect();
        o.set_wire_spec(0, &ids, SpecPart::Colour, Some("BU".into()));
        let mut ed = Editor::default();
        ed.selection = ids.iter().map(|id| Selection::Wire(id.clone())).collect();
        let mut ps = PanelState::default();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let frame = |o: &mut Open, ed: &mut Editor, ps: &mut PanelState| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| properties(ui, o, ed, ps));
            });
        };
        frame(&mut o, &mut ed, &mut ps);
        assert_eq!(ps.spec[1].buffer, "BU");
        // RD half typed for both wires: kept while the page is yours.
        ps.spec[1].buffer = "RD".into();
        ps.spec[1].dirty = true;
        frame(&mut o, &mut ed, &mut ps);
        assert_eq!(ps.spec[1].buffer, "RD", "a draft stays on your own page");
        // Then Ola opens the page: the field shows the project's colour again.
        let before = o.project.clone();
        let page = o.project.pages[0].id.clone();
        ed.held_by.insert(page, "Ola".into());
        frame(&mut o, &mut ed, &mut ps);
        assert_eq!(ps.spec[1].buffer, "BU");
        assert!(!ps.spec[1].dirty);
        assert_eq!(o.project, before);
    }

    #[test]
    fn the_same_item_id_on_another_page_loads_that_pages_values() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, mut o) = crate::state::tests::project();
        for (page, tag) in [(0, "-K1"), (1, "-K2")] {
            o.project.pages[page].symbols.push(wc_core::PlacedSymbol {
                id: "s1".into(),
                symbol: "iec.coil".into(),
                at: wc_core::geom::Point::new(50.0, 50.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(DeviceTag::parse(tag)?),
            });
        }
        let mut ed = Editor::default();
        let mut ps = PanelState::default();
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let frame = |o: &mut Open, ed: &mut Editor, ps: &mut PanelState| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| properties(ui, o, ed, ps));
            });
        };
        ed.selection = vec![Selection::Symbol("s1".into())];
        frame(&mut o, &mut ed, &mut ps);
        assert_eq!(ps.tag, "-K1");
        // Nothing selected for a frame, then s1 on the next page.
        ed.selection.clear();
        frame(&mut o, &mut ed, &mut ps);
        ed.page = 1;
        ed.selection = vec![Selection::Symbol("s1".into())];
        frame(&mut o, &mut ed, &mut ps);
        assert_eq!(ps.tag, "-K2");
        Ok(())
    }

    #[test]
    fn spec_drafts_are_forgotten_when_no_wire_rows_show() {
        let mut ps = PanelState::default();
        let mut wire = Editor::default();
        wire.selection = vec![Selection::Wire("w1".into())];
        ps.spec_for = (0, vec!["w1".into()]);
        forget_spec_unless_wires(&wire, &mut ps);
        assert_eq!(ps.spec_for, (0, vec!["w1".into()]), "the wire rows show: kept");
        let mut symbol = Editor::default();
        symbol.selection = vec![Selection::Symbol("s1".into())];
        forget_spec_unless_wires(&symbol, &mut ps);
        assert_eq!(ps.spec_for, (0, Vec::new()), "a symbol shows: forgotten");
    }

    /// A properties panel showing free drawings on the first page, driven a frame at a time.
    struct ShapePanel {
        _tmp: tempfile::TempDir,
        o: Open,
        ed: Editor,
        ps: PanelState,
        ctx: egui::Context,
        /// The drawings' indices on the page.
        at: Vec<usize>,
    }

    impl ShapePanel {
        fn new(shapes: &[Shape]) -> Self {
            let (_tmp, mut o) = crate::state::tests::project();
            let mut at = Vec::new();
            for g in shapes {
                at.push(o.project.pages[0].graphics.len());
                o.add_graphic(0, g.clone());
            }
            let mut ed = Editor::default();
            ed.selection = vec![Selection::Graphic(at[0])];
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            let mut me = Self {
                _tmp,
                o,
                ed,
                ps: PanelState::default(),
                ctx,
                at,
            };
            // Grids lay out over a frame before they show.
            me.frame(Vec::new());
            me.frame(Vec::new());
            me
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let (o, ed, ps) = (&mut self.o, &mut self.ed, &mut self.ps);
            let _ = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| properties(ui, o, ed, ps));
            });
        }

        fn key(&mut self, key: egui::Key) {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }]);
        }

        /// Tab `n` times from nothing focused.
        fn tab(&mut self, n: usize) {
            for _ in 0..n {
                self.key(egui::Key::Tab);
            }
        }

        /// Replace the focused field's text with `text`.
        fn typed(&mut self, text: &str) {
            self.frame(vec![
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                },
                egui::Event::Text(text.into()),
            ]);
        }

        /// Replace the focused field's text with `text` and press Enter, all in one frame.
        fn typed_and_entered(&mut self, text: &str) {
            self.frame(vec![
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                },
                egui::Event::Text(text.into()),
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                },
            ]);
        }

        fn drawing(&self, k: usize) -> Shape {
            self.o.project.pages[0].graphics[self.at[k]].clone()
        }

        /// A frame in which `f` runs first, before the panel is drawn.
        fn frame_with(&mut self, f: impl FnOnce(&egui::Context)) {
            self.frame_events_with(Vec::new(), f);
        }

        /// A frame of `events` in which `f` runs first, before the panel is drawn.
        fn frame_events_with(&mut self, events: Vec<egui::Event>, f: impl FnOnce(&egui::Context)) {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let (o, ed, ps) = (&mut self.o, &mut self.ed, &mut self.ps);
            let mut f = Some(f);
            let _ = self.ctx.run(input, |ctx| {
                if let Some(f) = f.take() {
                    f(ctx);
                }
                egui::CentralPanel::default().show(ctx, |ui| properties(ui, o, ed, ps));
            });
        }
    }

    fn text(s: &str) -> Shape {
        Shape::Text {
            at: wc_core::geom::Point::new(20.0, 20.0),
            text: s.into(),
            height: 3.5,
            align: Default::default(),
            angle: 0.0,
        }
    }

    /// The tabs from nothing focused to a text drawing's text field.
    fn tabs_to_text() -> Result<usize, &'static str> {
        for n in 1..30 {
            let mut p = ShapePanel::new(&[text("a")]);
            p.tab(n);
            p.typed("b");
            p.key(egui::Key::Enter);
            if p.drawing(0) == text("b") {
                return Ok(n);
            }
        }
        Err("no text field reached by tabbing")
    }

    fn circle(x: f64, r: f64) -> Shape {
        Shape::Circle {
            center: wc_core::geom::Point::new(x, 50.0),
            radius: r,
            filled: false,
        }
    }

    /// The tabs from nothing focused to a circle's radius field: found by typing a new radius
    /// after each count until one lands there.
    fn tabs_to_radius() -> Result<usize, &'static str> {
        for n in 1..30 {
            let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
            p.tab(n);
            p.typed("7");
            p.key(egui::Key::Enter);
            if p.drawing(0) == circle(100.0, 7.0) {
                return Ok(n);
            }
        }
        Err("no radius field reached by tabbing")
    }

    #[test]
    fn a_radius_typed_in_the_panel_is_one_undo_step() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        let mark = p.o.undo_mark();
        p.tab(n);
        p.typed("7");
        p.key(egui::Key::Enter);
        assert_eq!(p.drawing(0), circle(100.0, 7.0));
        assert_eq!(p.o.undo_mark(), mark + 1, "one undo step");
        assert!(p.o.undo());
        assert_eq!(p.drawing(0), circle(100.0, 5.0));
        Ok(())
    }

    #[test]
    fn a_radius_field_tabbed_through_or_spoiled_changes_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        // Shown to six decimals: read back as it was.
        for typed in [None, Some("7x")] {
            let mut p = ShapePanel::new(&[circle(100.0, 7.123_456_789)]);
            let (before, mark) = (p.o.project.clone(), p.o.undo_mark());
            p.tab(n);
            if let Some(t) = typed {
                // "7" first, a number of its own, then spoiled.
                p.typed(&t[..1]);
                p.frame(vec![egui::Event::Text(t[1..].into())]);
            }
            p.key(egui::Key::Enter);
            p.key(egui::Key::Tab);
            p.frame(Vec::new());
            assert_eq!(p.o.project, before, "{typed:?}: nothing changed");
            assert_eq!(p.o.undo_mark(), mark, "{typed:?}: no undo step");
        }
        Ok(())
    }

    #[test]
    fn a_radius_typed_for_one_circle_does_not_go_to_the_next_one_selected() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0), circle(150.0, 5.0)]);
        p.tab(n);
        p.typed("7");
        // Another circle selected while the field has the typing, then Enter.
        p.ed.selection = vec![Selection::Graphic(p.at[1])];
        p.frame(Vec::new());
        p.key(egui::Key::Enter);
        p.frame(Vec::new());
        assert_eq!(
            p.drawing(1),
            circle(150.0, 5.0),
            "the circle selected after keeps its radius"
        );
        assert_eq!(p.drawing(0), circle(100.0, 5.0), "the draft was dropped with its field");
        Ok(())
    }

    #[test]
    fn a_radius_set_while_the_circle_was_moved_leaves_it_where_it_went() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("7");
        // Moved meanwhile (the assistant, a guest's change).
        let i = p.at[0];
        p.o.project.pages[0].graphics[i] = circle(120.0, 5.0);
        p.key(egui::Key::Enter);
        assert_eq!(p.drawing(0), circle(120.0, 7.0));
        Ok(())
    }

    #[test]
    fn a_radius_of_zero_is_refused_with_its_reason() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert_eq!(p.drawing(0), circle(100.0, 5.0));
        assert_eq!(p.ps.shape_error.as_deref(), Some("The radius must be above zero."));
        Ok(())
    }

    #[test]
    fn a_radius_being_typed_changes_nothing_when_the_page_becomes_held() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("7");
        let (before, mark) = (p.o.project.clone(), p.o.undo_mark());
        let page = p.o.project.pages[0].id.clone();
        p.ed.held_by.insert(page, "Ola".into());
        p.frame(Vec::new());
        p.key(egui::Key::Enter);
        assert_eq!(p.o.project, before, "nothing was changed");
        assert_eq!(p.o.undo_mark(), mark, "no undo step");
        Ok(())
    }

    #[test]
    fn escape_drops_a_radius_changed_with_an_arrow_key() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        let (before, mark) = (p.o.project.clone(), p.o.undo_mark());
        p.tab(n);
        p.key(egui::Key::ArrowUp);
        p.key(egui::Key::Escape);
        p.frame(Vec::new());
        assert_eq!(p.o.project, before);
        assert_eq!(p.o.undo_mark(), mark);
        Ok(())
    }

    #[test]
    fn a_refusal_shown_goes_when_a_good_value_is_entered() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        // The radius it has, entered again: no change, and no refusal shown any more.
        p.tab(n);
        p.typed("5");
        p.key(egui::Key::Enter);
        assert_eq!(p.ps.shape_error, None);
        assert_eq!(p.drawing(0), circle(100.0, 5.0));
        Ok(())
    }

    #[test]
    fn a_radius_typed_for_a_circle_deleted_meanwhile_does_not_go_to_the_next_one()
    -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0), circle(150.0, 5.0)]);
        p.tab(n);
        p.typed("7");
        // The circle is deleted (by the assistant); the next one takes its place in the list.
        let i = p.at[0];
        p.o.project.pages[0].graphics.remove(i);
        p.key(egui::Key::Enter);
        p.frame(Vec::new());
        assert_eq!(p.o.project.pages[0].graphics[i], circle(150.0, 5.0));
        Ok(())
    }

    #[test]
    fn a_field_tabbed_into_as_another_commits_keeps_a_change_made_meanwhile() -> Result<(), Box<dyn std::error::Error>>
    {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        // Centre Y, then Tab: Y is committed and the radius field takes the focus.
        p.tab(n - 1);
        p.typed("60");
        p.key(egui::Key::Tab);
        let i = p.at[0];
        assert_eq!(p.o.project.pages[0].graphics[i], circle(100.0, 5.0).moved_y(60.0));
        // The radius changes elsewhere while its field has the focus, then the field is left
        // untouched: the change stays.
        p.o.project.pages[0].graphics[i] = circle(100.0, 9.0).moved_y(60.0);
        p.frame(Vec::new());
        p.key(egui::Key::Tab);
        p.frame(Vec::new());
        assert_eq!(p.o.project.pages[0].graphics[i], circle(100.0, 9.0).moved_y(60.0));
        Ok(())
    }

    #[test]
    fn a_refusal_stays_while_another_field_is_only_tabbed_through() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        // Centre X, then on to Centre Y: the radius was not entered again.
        p.tab(n - 2);
        p.key(egui::Key::Tab);
        p.frame(Vec::new());
        assert!(p.ps.shape_error.is_some(), "the radius's refusal is still shown");
        Ok(())
    }

    #[test]
    fn a_field_focused_in_the_frame_another_commits_keeps_a_change_made_meanwhile()
    -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let radius = {
            let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
            p.tab(n);
            p.ctx.memory(|m| m.focused()).ok_or("the radius field has the focus")?
        };
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n - 1);
        p.typed("60");
        // In one frame Centre Y ends and the radius field takes the focus.
        p.frame_with(|ctx| ctx.memory_mut(|m| m.request_focus(radius)));
        let i = p.at[0];
        assert_eq!(p.o.project.pages[0].graphics[i], circle(100.0, 5.0).moved_y(60.0));
        assert_eq!(p.ctx.memory(|m| m.focused()), Some(radius));
        let mark = p.o.undo_mark();
        // The radius changes elsewhere while its field has the focus, then the field is left
        // untouched: the change stays, with no step of its own.
        p.o.project.pages[0].graphics[i] = circle(100.0, 9.0).moved_y(60.0);
        p.frame(Vec::new());
        p.key(egui::Key::Tab);
        p.frame(Vec::new());
        assert_eq!(p.o.project.pages[0].graphics[i], circle(100.0, 9.0).moved_y(60.0));
        assert_eq!(p.o.undo_mark(), mark);
        Ok(())
    }

    #[test]
    fn a_radius_dragged_as_another_field_commits_is_one_step_of_its_own() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let radius = {
            let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
            p.tab(n);
            p.ctx.memory(|m| m.focused()).ok_or("the radius field has the focus")?
        };
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        let rect = p.ctx.read_response(radius).ok_or("the radius field is shown")?.rect;
        p.tab(n - 1);
        let y = p.ctx.memory(|m| m.focused()).ok_or("Centre Y has the focus")?;
        p.typed("60");
        let mark = p.o.undo_mark();
        let at = rect.center();
        let pointer = |x: f32, pressed: Option<bool>| {
            let mut ev = vec![egui::Event::PointerMoved(egui::pos2(at.x + x, at.y))];
            if let Some(pressed) = pressed {
                ev.push(egui::Event::PointerButton {
                    pos: egui::pos2(at.x + x, at.y),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
            ev
        };
        // Pressing on the radius ends Centre Y ("60"); as the radius is dragged, Centre Y
        // takes the focus (from the keyboard) and ends again.
        p.frame(pointer(0.0, Some(true)));
        p.frame(pointer(20.0, None));
        p.frame_with(|ctx| ctx.memory_mut(|m| m.request_focus(y)));
        p.frame(pointer(30.0, None));
        p.frame_with(|ctx| ctx.memory_mut(|m| m.surrender_focus(y)));
        p.frame(pointer(40.0, None));
        p.frame(pointer(40.0, Some(false)));
        p.frame(Vec::new());
        let i = p.at[0];
        let Shape::Circle { center, radius: r, .. } = p.o.project.pages[0].graphics[i] else {
            return Err("still a circle".into());
        };
        assert_eq!(center.y, 60.0);
        assert!(r > 5.0, "dragged: {r}");
        // One step for Centre Y, one for the whole drag.
        p.o.undo();
        let Shape::Circle { center, radius: r, .. } = p.o.project.pages[0].graphics[i] else {
            return Err("still a circle".into());
        };
        assert_eq!((center.y, r), (60.0, 5.0), "the drag is undone in one step");
        p.o.undo();
        assert_eq!(p.o.undo_mark(), mark);
        Ok(())
    }

    #[test]
    fn a_good_value_ended_with_a_refused_one_is_kept() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let radius = {
            let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
            p.tab(n);
            p.ctx.memory(|m| m.focused()).ok_or("the radius field has the focus")?
        };
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        let rect = p.ctx.read_response(radius).ok_or("the radius field is shown")?.rect;
        let y = {
            p.tab(n - 1);
            let y = p.ctx.memory(|m| m.focused()).ok_or("Centre Y has the focus")?;
            p.key(egui::Key::Escape);
            p.frame(Vec::new());
            y
        };
        let at = rect.center();
        let pointer = |x: f32, pressed: Option<bool>| {
            let mut ev = vec![egui::Event::PointerMoved(egui::pos2(at.x + x, at.y))];
            if let Some(pressed) = pressed {
                ev.push(egui::Event::PointerButton {
                    pos: egui::pos2(at.x + x, at.y),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
            ev
        };
        // The radius is dragged below nothing while Centre Y is typed in; both end in one frame.
        p.frame(pointer(0.0, Some(true)));
        p.frame(pointer(-20.0, None));
        p.frame_with(|ctx| ctx.memory_mut(|m| m.request_focus(y)));
        p.typed("60");
        p.frame(pointer(-90.0, None));
        p.frame_events_with(pointer(-90.0, Some(false)), |ctx| {
            ctx.memory_mut(|m| m.surrender_focus(y))
        });
        p.frame(Vec::new());
        let i = p.at[0];
        assert_eq!(
            p.o.project.pages[0].graphics[i],
            circle(100.0, 5.0).moved_y(60.0),
            "Centre Y is kept"
        );
        assert!(p.ps.shape_error.is_some(), "the radius's refusal is shown");
        assert_eq!(p.ps.shape_refused.keys().copied().collect::<Vec<_>>(), [2]);
        Ok(())
    }

    fn arc(radius: f64, start: f64, end: f64) -> Shape {
        Shape::Arc {
            center: wc_core::geom::Point::new(100.0, 50.0),
            radius,
            start,
            end,
        }
    }

    #[test]
    fn each_refused_field_keeps_its_reason_until_it_is_entered_again() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[arc(5.0, 0.0, 90.0)]);
        // Radius 0, then an end angle at the start angle: both refused.
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        p.tab(n + 2);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert_eq!(p.drawing(0), arc(5.0, 0.0, 90.0));
        assert_eq!(p.ps.shape_refused.keys().copied().collect::<Vec<_>>(), [2, 4]);
        // The end angle entered again: its reason goes, the radius's stays.
        p.tab(n + 2);
        p.typed("90");
        p.key(egui::Key::Enter);
        assert_eq!(p.ps.shape_refused.keys().copied().collect::<Vec<_>>(), [2]);
        assert!(p.ps.shape_error.is_some(), "the radius's refusal is still shown");
        Ok(())
    }

    #[test]
    fn a_value_typed_and_entered_in_one_frame_takes_the_refusal_back() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        p.tab(n);
        p.typed_and_entered("4");
        assert_eq!(p.drawing(0), circle(100.0, 4.0));
        assert!(p.ps.shape_error.is_none(), "{:?}", p.ps.shape_error);
        Ok(())
    }

    #[test]
    fn a_refused_value_entered_again_too_near_the_stored_one_to_change_it_is_still_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 0.00005)]);
        p.tab(n);
        p.typed("-1");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        let mark = p.o.undo_mark();
        // 0 is within reach of the stored 0.00005, so nothing is set, but it is no radius.
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some(), "0 is no radius: the refusal stays");
        assert_eq!(p.drawing(0), circle(100.0, 0.00005));
        assert_eq!(p.o.undo_mark(), mark);
        Ok(())
    }

    #[test]
    fn a_refusal_stays_when_its_field_is_spoiled_or_tabbed_through() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        let mark = p.o.undo_mark();
        // Text that is no number.
        p.tab(n);
        p.typed("7");
        p.frame(vec![egui::Event::Text("x".into())]);
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some(), "spoiled: still shown");
        // Tabbed through.
        p.tab(n);
        p.key(egui::Key::Tab);
        p.frame(Vec::new());
        assert!(p.ps.shape_error.is_some(), "tabbed through: still shown");
        assert_eq!(p.drawing(0), circle(100.0, 5.0));
        assert_eq!(p.o.undo_mark(), mark);
        Ok(())
    }

    #[test]
    fn a_value_stepped_and_entered_in_one_frame_takes_the_refusal_back() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        p.tab(n);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        p.frame(vec![key(egui::Key::ArrowUp), key(egui::Key::Enter)]);
        assert_ne!(p.drawing(0), circle(100.0, 5.0), "the arrow key stepped the radius");
        assert!(p.ps.shape_error.is_none(), "{:?}", p.ps.shape_error);
        Ok(())
    }

    #[test]
    fn a_modified_arrow_key_as_a_field_is_left_is_no_entry() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 7.123456789)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        p.tab(n);
        // Ctrl+ArrowUp steps nothing; egui reads the shown 7.123457 back as Enter ends the field.
        p.frame(vec![
            egui::Event::Key {
                key: egui::Key::ArrowUp,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            },
        ]);
        assert!(p.ps.shape_error.is_some(), "no value was entered: still shown");
        assert_eq!(p.drawing(0), circle(100.0, 7.123456789));
        Ok(())
    }

    #[test]
    fn a_refusal_stays_when_a_field_with_more_decimals_than_shown_is_tabbed_through()
    -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 7.123456789)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        let mark = p.o.undo_mark();
        // egui reads the shown 7.123457 back as the field loses focus: no text was entered.
        p.tab(n);
        p.key(egui::Key::Tab);
        p.frame(Vec::new());
        assert!(p.ps.shape_error.is_some(), "tabbed through: still shown");
        assert_eq!(p.drawing(0), circle(100.0, 7.123456789));
        assert_eq!(p.o.undo_mark(), mark);
        Ok(())
    }

    #[test]
    fn a_text_being_typed_keeps_its_field_when_a_drawing_is_added_after_it() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_text()?;
        let mut p = ShapePanel::new(&[text("a")]);
        p.tab(n);
        p.typed("b");
        p.o.project.pages[0].graphics.push(circle(100.0, 5.0));
        p.frame(Vec::new());
        p.key(egui::Key::Enter);
        assert_eq!(p.drawing(0), text("b"));
        Ok(())
    }

    #[test]
    fn a_text_being_typed_does_not_go_to_a_drawing_moved_into_its_place() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_text()?;
        // Two texts alike: the second takes the first's place when it is deleted.
        let mut p = ShapePanel::new(&[text("a"), text("a")]);
        p.tab(n);
        p.typed("b");
        let i = p.at[0];
        p.o.project.pages[0].graphics.remove(i);
        p.frame(Vec::new());
        p.key(egui::Key::Enter);
        p.frame(Vec::new());
        assert_eq!(p.o.project.pages[0].graphics[i], text("a"), "Enter");
        // Nor through the height field: it writes the text the drawing has.
        p.tab(n + 1);
        p.key(egui::Key::ArrowUp);
        p.key(egui::Key::Tab);
        p.frame(Vec::new());
        let Shape::Text { text: now, height, .. } = &p.o.project.pages[0].graphics[i] else {
            return Err("still a text".into());
        };
        assert_eq!(now, "a", "Height");
        assert!(*height > 3.5, "the height was set: {height}");
        Ok(())
    }

    #[test]
    fn a_refusal_goes_when_a_drawing_of_another_kind_takes_its_place() -> Result<(), Box<dyn std::error::Error>> {
        let n = tabs_to_radius()?;
        let mut p = ShapePanel::new(&[circle(100.0, 5.0)]);
        p.tab(n);
        p.typed("0");
        p.key(egui::Key::Enter);
        assert!(p.ps.shape_error.is_some());
        // A polyline in the circle's place has no radius: the refusal is not its own.
        let i = p.at[0];
        p.o.project.pages[0].graphics[i] = Shape::Polyline {
            points: vec![
                wc_core::Point::new(10.0, 10.0),
                wc_core::Point::new(20.0, 10.0),
                wc_core::Point::new(20.0, 20.0),
            ],
            closed: false,
            filled: false,
        };
        p.frame(Vec::new());
        assert_eq!(p.ps.shape_error, None);
        assert!(p.ps.shape_refused.is_empty());
        Ok(())
    }

    trait MovedY {
        fn moved_y(self, y: f64) -> Shape;
    }

    impl MovedY for Shape {
        fn moved_y(mut self, y: f64) -> Shape {
            if let Shape::Circle { center, .. } = &mut self {
                center.y = y;
            }
            self
        }
    }

    #[test]
    fn a_dimension_shows_the_points_it_measures_not_its_bounds() {
        let d = Shape::Dimension {
            a: wc_core::Point::new(10.0, 20.0),
            b: wc_core::Point::new(40.0, 60.0),
            offset: 100.0,
            height: 2.5,
        };
        assert_eq!(
            drawing_rows(&d),
            vec![
                ("Length", "50 mm".to_string()),
                ("From", "10.0, 20.0 mm".to_string()),
                ("To", "40.0, 60.0 mm".to_string()),
            ]
        );
        let line = Shape::Line {
            a: wc_core::Point::new(5.0, 1.0),
            b: wc_core::Point::new(2.0, 3.0),
        };
        assert_eq!(
            drawing_rows(&line),
            vec![("From", "2.0, 1.0 mm".to_string()), ("To", "5.0, 3.0 mm".to_string())]
        );
    }
}
