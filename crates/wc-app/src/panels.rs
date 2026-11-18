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
    loaded_for: Option<Selection>,
    tag: String,
    description: String,
    number: String,
    potential: String,
    text: String,
    text_height: f64,
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
    let mut terms: Vec<(DeviceTag, String, DeviceTag)> = Vec::new();
    for id in ids {
        let Some(s) = p.symbol(id) else { return };
        let is_terminal = open
            .symbols
            .get(&s.symbol)
            .is_some_and(|y| y.role == wc_core::symbol::SymbolRole::Terminal);
        let Some(tag) = s.tag.clone().filter(|_| is_terminal) else {
            return;
        };
        let (strip, Some(n)) = tag.terminal_parts() else { return };
        terms.push((strip, n, tag));
    }
    let Some((strip, number, tag)) = terms.first().cloned() else {
        return;
    };
    if terms.iter().any(|(s, _, _)| *s != strip) {
        return;
    }
    if terms.len() == 1 {
        let others = open.project.meta.jumper_of(&tag).map(|j| {
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
        }
    } else {
        let numbers: Vec<String> = terms.into_iter().map(|(_, n, _)| n).collect();
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
    if ps.loaded_for == one {
        return;
    }
    ps.loaded_for = one.clone();
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
        }
        None => {}
    }
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
            };
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
                    } else {
                        let (a, b) = g.bounds();
                        ui.label("From");
                        ui.label(format!("{:.1}, {:.1} mm", a.x, a.y));
                        ui.end_row();
                        ui.label("To");
                        ui.label(format!("{:.1}, {:.1} mm", b.x, b.y));
                        ui.end_row();
                    }
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

/// Part and own properties of the selected symbol's device.
fn device_section(ui: &mut egui::Ui, open: &mut Open, ps: &mut PanelState, tag: &DeviceTag, t: &Theme) {
    subsection(ui, "Part");
    let current = open.project.devices.get(tag).and_then(|d| d.part.clone());
    let label = current
        .as_ref()
        .map(|id| {
            open.part_library
                .iter()
                .find(|(p, _)| p == id)
                .map_or(id.clone(), |(_, l)| l.clone())
        })
        .unwrap_or_else(|| "none".into());
    let mut pick: Option<Option<String>> = None;
    let short: String = if label.chars().count() > 34 {
        format!("{}\u{2026}", label.chars().take(33).collect::<String>())
    } else {
        label.clone()
    };
    egui::ComboBox::from_id_salt("part")
        .width(ui.available_width().min(240.0))
        .selected_text(short)
        .height(320.0)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut ps.part_search).hint_text("Search parts"));
            if ui
                .selectable_label(current.is_none(), RichText::new("none").italics())
                .clicked()
            {
                pick = Some(None);
            }
            let q = ps.part_search.to_lowercase();
            for (id, l) in &open.part_library {
                if !q.is_empty() && !id.to_lowercase().contains(&q) && !l.to_lowercase().contains(&q) {
                    continue;
                }
                if ui
                    .selectable_label(current.as_deref() == Some(id.as_str()), l)
                    .on_hover_text(id)
                    .clicked()
                {
                    pick = Some(Some(id.clone()));
                }
            }
        });
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

pub fn status_bar(ui: &mut egui::Ui, open: Option<&Open>, ed: &Editor) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
