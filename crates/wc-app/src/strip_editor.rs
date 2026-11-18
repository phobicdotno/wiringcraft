//! Terminal strip editor: a window that lists a strip's terminals in rail order, lets the user
//! drag them (or move them up and down) into another order, add and remove spare terminals, set
//! each terminal's level on a multi-level terminal and see what it is jumpered with. The order
//! and spares are kept in `project.toml` ([`wc_core::project::StripLayout`]); the terminal plan
//! and the WAGO configurator export follow them.

use eframe::egui::{self, RichText};
use wc_core::DeviceTag;

use crate::state::Open;
use crate::theme::Theme;

/// The editor's fields: which strip is shown and the spare being added.
#[derive(Debug, Default)]
pub struct StripEditor {
    /// The strip shown; the first strip when none is picked or it is gone.
    pub strip: Option<DeviceTag>,
    /// The number typed for a new spare terminal.
    pub spare: String,
    /// The terminal a new spare goes after; `None` puts it at the end.
    pub after: Option<String>,
    /// Why the last change was refused, if it was.
    pub message: String,
}

/// The levels offered for a terminal: none (single-level), then the bottom level and up.
const LEVELS: &[&str] = &["", "1", "2", "3", "4"];

/// A terminal's row being dragged: its place on the rail.
#[derive(Debug, Clone, Copy)]
struct Dragged(usize);

impl StripEditor {
    /// Show the window; returns false once it is closed.
    pub fn show(&mut self, ctx: &egui::Context, open: &mut Open) -> bool {
        let mut keep = true;
        egui::Window::new("Terminal strip editor")
            .open(&mut keep)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| self.contents(ui, open));
        keep
    }

    fn contents(&mut self, ui: &mut egui::Ui, open: &mut Open) {
        let strips = open.strip_terminals();
        if strips.is_empty() {
            ui.label("No terminal strips yet. Place terminals tagged like -X1.1 on a page first.");
            return;
        }
        let strip = match &self.strip {
            Some(s) if strips.contains_key(s) => s.clone(),
            _ => match strips.keys().next() {
                Some(s) => s.clone(),
                None => return,
            },
        };
        ui.horizontal(|ui| {
            ui.label("Strip");
            egui::ComboBox::from_id_salt("strip editor strip")
                .selected_text(strip.to_string())
                .show_ui(ui, |ui| {
                    for s in strips.keys() {
                        if ui.selectable_label(*s == strip, s.to_string()).clicked() && *s != strip {
                            self.strip = Some(s.clone());
                            self.after = None;
                            self.message.clear();
                        }
                    }
                });
            let arranged = open
                .project
                .meta
                .strip_layout(&strip)
                .is_some_and(|l| !l.order.is_empty());
            if ui
                .add_enabled(arranged, egui::Button::new("Order by number"))
                .on_hover_text("Put the terminals back in the order of their numbers; spares go at the end")
                .clicked()
            {
                open.reset_terminal_order(&strip);
            }
        });
        if self.strip.as_ref() != Some(&strip) {
            self.strip = Some(strip.clone());
        }
        let rail = strips.get(&strip).cloned().unwrap_or_default();
        ui.weak("Drag a terminal by its handle, or use the arrows, to change the order on the rail.");
        ui.separator();

        let mut action: Option<Change> = None;
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            egui::Grid::new("strip editor terminals")
                .striped(true)
                .num_columns(6)
                .show(ui, |ui| {
                    ui.label("");
                    ui.label(RichText::new("Terminal").strong());
                    ui.label(RichText::new("Level").strong());
                    ui.label(RichText::new("Jumpered with").strong());
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                    for (k, (number, spare)) in rail.iter().enumerate() {
                        let handle = ui
                            .dnd_drag_source(egui::Id::new(("strip editor drag", &strip, number)), Dragged(k), |ui| {
                                ui.label(RichText::new("≡").monospace())
                            })
                            .response
                            .on_hover_text("Drag to another place");
                        let label = if *spare {
                            format!("{number} (spare)")
                        } else {
                            number.clone()
                        };
                        let row = ui.label(label);
                        for r in [&handle, &row] {
                            if let Some(from) = r.dnd_release_payload::<Dragged>() {
                                action = Some(Change::Move(rail[from.0].0.clone(), k));
                            }
                        }
                        level_picker(ui, open, &strip, number);
                        ui.label(jumpered_with(open, &strip, number));
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(k > 0, egui::Button::new("↑"))
                                .on_hover_text("Move up")
                                .clicked()
                            {
                                action = Some(Change::Move(number.clone(), k - 1));
                            }
                            if ui
                                .add_enabled(k + 1 < rail.len(), egui::Button::new("↓"))
                                .on_hover_text("Move down")
                                .clicked()
                            {
                                action = Some(Change::Move(number.clone(), k + 1));
                            }
                        });
                        if *spare {
                            if ui
                                .button("Remove")
                                .on_hover_text("Take this spare off the strip")
                                .clicked()
                            {
                                action = Some(Change::RemoveSpare(number.clone()));
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Spare terminal");
            ui.add(
                egui::TextEdit::singleline(&mut self.spare)
                    .hint_text("number, e.g. 9")
                    .desired_width(90.0),
            );
            if self.after.as_ref().is_some_and(|a| !rail.iter().any(|(t, _)| t == a)) {
                self.after = None;
            }
            egui::ComboBox::from_id_salt("strip editor spare after")
                .selected_text(match &self.after {
                    Some(a) => format!("after {a}"),
                    None => "at the end".into(),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.after, None, "at the end");
                    for (t, _) in &rail {
                        ui.selectable_value(&mut self.after, Some(t.clone()), format!("after {t}"));
                    }
                });
            if ui
                .add_enabled(!self.spare.trim().is_empty(), egui::Button::new("Add spare"))
                .clicked()
            {
                action = Some(Change::AddSpare(self.spare.trim().to_owned(), self.after.clone()));
            }
        });
        if let Some(change) = action {
            let result = match change {
                Change::Move(number, to) => open.move_terminal(&strip, &number, to),
                Change::AddSpare(number, after) => {
                    let r = open.add_spare(&strip, &number, after.as_deref());
                    if r.is_ok() {
                        self.spare.clear();
                    }
                    r
                }
                Change::RemoveSpare(number) => {
                    open.remove_spare(&strip, &number);
                    Ok(())
                }
            };
            self.message = result.err().unwrap_or_default();
        }
        if !self.message.is_empty() {
            ui.label(RichText::new(&self.message).color(Theme::of(ui.ctx()).error).small());
        }
    }
}

/// What the user asked for in this frame.
enum Change {
    /// Move a terminal to a place on the rail.
    Move(String, usize),
    /// Add a spare, after a terminal or at the end.
    AddSpare(String, Option<String>),
    RemoveSpare(String),
}

/// The level of terminal `number` of `strip` on a multi-level terminal: its device's `Level`
/// property, picked from a short list.
fn level_picker(ui: &mut egui::Ui, open: &mut Open, strip: &DeviceTag, number: &str) {
    let Ok(tag) = DeviceTag::parse(&format!("{strip}.{number}")) else {
        ui.label("");
        return;
    };
    let current = open
        .project
        .devices
        .get(&tag)
        .and_then(|d| d.properties.get(wc_report::LEVEL_PROPERTY))
        .cloned()
        .unwrap_or_default();
    let mut picked = current.clone();
    egui::ComboBox::from_id_salt(("strip editor level", strip, number))
        .width(50.0)
        .selected_text(if current.is_empty() { "–" } else { current.as_str() })
        .show_ui(ui, |ui| {
            for l in LEVELS {
                ui.selectable_value(&mut picked, (*l).to_owned(), if l.is_empty() { "–" } else { l });
            }
        })
        .response
        .on_hover_text("Level on a multi-level terminal: 1 is the bottom; – is a single-level terminal");
    if picked != current {
        open.set_device_property(&tag, wc_report::LEVEL_PROPERTY, &picked);
    }
}

/// The other terminals of the jumper that holds terminal `number` of `strip`, e.g. `3, 5`.
fn jumpered_with(open: &Open, strip: &DeviceTag, number: &str) -> String {
    open.project
        .meta
        .jumpers
        .iter()
        .find(|j| j.strip == *strip && j.terminals.iter().any(|t| t == number))
        .map(|j| {
            j.terminals
                .iter()
                .filter(|t| *t != number)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_moves_terminals_adds_spares_and_sets_levels_as_undo_steps() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_tmp, mut o) = crate::state::tests::project();
        let x1 = DeviceTag::parse("-X1")?;
        let page = 1;
        for (k, tag) in ["-X1.1", "-X1.2", "-X1.3"].into_iter().enumerate() {
            let id = o.place(
                page,
                "wc.terminal",
                wc_core::geom::Point::new(20.0 + 20.0 * k as f64, 20.0),
            )?;
            let s = o.project.pages[page]
                .symbols
                .iter_mut()
                .find(|s| s.id == id)
                .ok_or("not placed")?;
            s.tag = Some(DeviceTag::parse(tag)?);
        }
        let rail = |o: &Open| -> Vec<String> {
            o.strip_terminals()
                .remove(&x1)
                .unwrap_or_default()
                .into_iter()
                .map(|(t, spare)| if spare { format!("{t}*") } else { t })
                .collect()
        };
        assert_eq!(rail(&o), ["1", "2", "3"]);
        o.move_terminal(&x1, "3", 0)?;
        assert_eq!(rail(&o), ["3", "1", "2"]);
        o.add_spare(&x1, "S1", Some("3"))?;
        assert_eq!(rail(&o), ["3", "S1*", "1", "2"]);
        assert!(o.add_spare(&x1, "1", None).is_err());
        o.set_device_property(&DeviceTag::parse("-X1.S1")?, wc_report::LEVEL_PROPERTY, "2");
        // Each change is its own undo step.
        o.undo();
        assert_eq!(rail(&o), ["3", "S1*", "1", "2"]);
        o.undo();
        assert_eq!(rail(&o), ["3", "1", "2"]);
        o.redo();
        o.remove_spare(&x1, "S1");
        assert_eq!(rail(&o), ["3", "1", "2"]);
        o.reset_terminal_order(&x1);
        assert_eq!(rail(&o), ["1", "2", "3"]);
        assert!(o.project.meta.strips.is_empty());

        // The window draws every terminal of the strip.
        let mut ed = StripEditor::default();
        o.add_spare(&x1, "9", None)?;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut texts: Vec<String> = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            assert!(ed.show(ctx, &mut o));
        });
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            ed.show(ctx, &mut o);
        });
        for shape in out.shapes {
            if let egui::epaint::Shape::Text(t) = shape.shape {
                texts.push(t.galley.text().to_owned());
            }
        }
        for want in ["1", "2", "3", "9 (spare)", "Add spare"] {
            assert!(texts.iter().any(|t| t == want), "{want} missing in {texts:?}");
        }
        Ok(())
    }
}
