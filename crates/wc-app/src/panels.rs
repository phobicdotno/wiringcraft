//! Side panels: page navigator and devices on the left, symbol library picker, properties on
//! the right, and the status bar.

use eframe::egui::{self, RichText};
use wc_core::DeviceTag;
use wc_core::geom::{Rotation, Shape};

use crate::actions::Action;
use crate::canvas::Editor;
use crate::state::{Open, Selection, Tool};
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
    tag_error: Option<String>,
}

pub fn navigator(ui: &mut egui::Ui, open: &Open, ed: &mut Editor, out: &mut Vec<Action>) {
    ui.heading("Pages");
    egui::ScrollArea::vertical()
        .id_salt("pages")
        .max_height(ui.available_height() * 0.5)
        .show(ui, |ui| {
            for (i, p) in open.project.pages.iter().enumerate() {
                let label = format!("{:>3}  {}", p.number, p.title);
                let r = ui.selectable_label(ed.page == i, RichText::new(label).monospace());
                let pick = |ed: &mut Editor| {
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
                if r.double_clicked() {
                    out.push(Action::PageProps);
                }
                r.context_menu(|ui| {
                    pick(ed);
                    for a in [
                        Action::PageProps,
                        Action::DuplicatePage,
                        Action::PageUp,
                        Action::PageDown,
                        Action::DeletePage,
                    ] {
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
        });
    ui.separator();
    ui.heading("Devices");
    egui::ScrollArea::vertical().id_salt("devices").show(ui, |ui| {
        for (tag, uses) in open.project.tag_index() {
            let pages: Vec<String> = uses
                .iter()
                .filter_map(|(page_id, _)| open.project.page(page_id).map(|p| p.number.clone()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let r = ui
                .selectable_label(false, RichText::new(tag.to_string()).monospace())
                .on_hover_text(format!("On page {}", pages.join(", ")));
            if r.clicked()
                && let Some((page_id, item)) = uses.first()
                && let Some(i) = open.project.pages.iter().position(|p| &p.id == page_id)
            {
                ed.page = i;
                ed.select_only(Selection::Symbol(item.clone()));
                ed.view.fit = true;
            }
        }
    });
}

pub fn library(ui: &mut egui::Ui, open: &Open, ed: &mut Editor, ps: &mut PanelState) {
    ui.heading("Symbols");
    ui.add(egui::TextEdit::singleline(&mut ps.search).hint_text("Search symbols"));
    let q = ps.search.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("library")
        .max_height(ui.available_height() * 0.45)
        .show(ui, |ui| {
            for (id, name) in &open.library {
                if !q.is_empty() && !id.to_lowercase().contains(&q) && !name.to_lowercase().contains(&q) {
                    continue;
                }
                let picked = ed.place_symbol.as_deref() == Some(id.as_str()) && ed.tool == Tool::Place;
                let vendored = open.symbols.contains_key(id);
                let text = RichText::new(name);
                let r = ui
                    .selectable_label(picked, if vendored { text } else { text.italics() })
                    .on_hover_text(if vendored {
                        format!("{id}\nIn this project")
                    } else {
                        format!("{id}\nFrom the library; copied into the project when placed")
                    });
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
            ps.description = tag
                .and_then(|t| open.project.devices.get(&t).map(|d| d.description.clone()))
                .unwrap_or_default();
        }
        Some(Selection::Wire(id)) => {
            let w = page.wires.iter().find(|w| &w.id == id);
            ps.number = w.and_then(|w| w.number.clone()).unwrap_or_default();
            ps.potential = w.and_then(|w| w.potential.clone()).unwrap_or_default();
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
    ui.heading("Properties");
    load_buffers(open, ed, ps);
    let page_idx = ed.page;
    if ed.selection.len() > 1 {
        let n = ed.selection.len();
        ui.label(format!("{n} items selected"));
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
            if let Some(sym) = open.symbols.get(&placed.symbol) {
                ui.add_space(6.0);
                ui.label(RichText::new("Pins").strong());
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
                    ui.label("Bends");
                    ui.label(wire.via.len().to_string());
                    ui.end_row();
                });
        }
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
    ui.horizontal(|ui| {
        let tool = match ed.tool {
            Tool::Select => "Select",
            Tool::Place => "Place symbol",
            Tool::Wire => "Draw wire",
            Tool::Text => "Text",
            Tool::Line => "Line",
            Tool::Rect => "Rectangle",
        };
        ui.label(RichText::new(tool).strong());
        ui.separator();
        match ed.cursor {
            Some(c) => ui.label(RichText::new(format!("X {:>6.1}  Y {:>6.1} mm", c.x, c.y)).monospace()),
            None => ui.label(RichText::new("X      –  Y      – mm").monospace()),
        };
        ui.separator();
        ui.label(format!("{:.0} %", ed.view.zoom / 3.78 * 100.0));
        ui.separator();
        ui.label(RichText::new(&ed.message).color(t.faint));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(o) = open {
                if o.dirty {
                    ui.label(RichText::new("unsaved changes").color(t.selection));
                }
                if !o.branch.is_empty() {
                    ui.label(format!("branch {}", o.branch));
                    ui.separator();
                }
            }
        });
    });
}
