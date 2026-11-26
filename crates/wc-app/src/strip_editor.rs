//! Terminal strip editor: a window that lists a strip's terminals in rail order, lets the user
//! drag them (or move them up and down) into another order, add and remove spare terminals, set
//! each terminal's level on a multi-level terminal and jumper terminals together or take them out
//! of their jumper. The order and spares are kept in `project.toml`
//! ([`wc_core::project::StripLayout`]); the terminal plan and the WAGO configurator export follow
//! them.

use std::collections::BTreeSet;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use wc_core::DeviceTag;

use crate::state::Open;
use crate::theme::Theme;

/// The editor's fields: which strip is shown and the spare being added.
#[derive(Debug, Default)]
pub struct StripEditor {
    /// The project folder the window was opened for: another project opened since closes it,
    /// so nothing typed here lands in that project.
    pub owner: Option<PathBuf>,
    /// The strip shown; the first strip when none is picked or it is gone.
    pub strip: Option<DeviceTag>,
    /// The strip tag typed for a first spare, when the project has no strips yet.
    pub new_strip: String,
    /// The number typed for that first spare: a draft of its own, so a cancelled new strip
    /// never ends up in the Add spare field of the strip shown.
    pub new_spare: String,
    /// True while the user starts another strip with its first spare ("New strip…").
    pub starting: bool,
    /// The number typed for a new spare terminal.
    pub spare: String,
    /// The terminal a new spare goes after; `None` puts it at the end.
    pub after: Option<String>,
    /// Terminals ticked to be jumpered together, of the strip shown.
    pub ticked: BTreeSet<String>,
    /// Why the last change was refused, if it was.
    pub message: String,
}

/// The levels offered for a terminal: none (no `Level` property of its own), then the bottom
/// level and up.
const LEVELS: &[&str] = &["", "1", "2", "3", "4"];

/// A terminal's row being dragged: its strip and number, so a drop after the rail changed
/// (an undo, another participant's edit) never moves another terminal.
#[derive(Debug, Clone)]
struct Dragged(DeviceTag, String);

impl StripEditor {
    /// An editor for the project open in `open`.
    pub fn for_project(open: &Open) -> Self {
        Self {
            owner: Some(open.dir.clone()),
            ..Self::default()
        }
    }

    /// Show the window; returns false once it is closed.
    pub fn show(&mut self, ctx: &egui::Context, open: &mut Open) -> bool {
        let mut keep = true;
        egui::Window::new("Terminal strip editor")
            .open(&mut keep)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .show(ctx, |ui| self.contents(ui, open));
        keep
    }

    /// Show `strip` from the next frame on.
    pub fn pick(&mut self, strip: DeviceTag) {
        if self.strip.as_ref() != Some(&strip) {
            self.strip = Some(strip);
            self.after = None;
            self.ticked.clear();
            self.message.clear();
        }
    }

    fn contents(&mut self, ui: &mut egui::Ui, open: &mut Open) {
        let strips = open.strip_terminals();
        if self.starting && !strips.is_empty() {
            self.first_spare(ui, open);
            return;
        }
        let Some(strip) = (match &self.strip {
            Some(s) if strips.contains_key(s) => Some(s.clone()),
            _ => strips.keys().next().cloned(),
        }) else {
            self.first_spare(ui, open);
            return;
        };
        // Settled before the picker, so a pick in it is not overwritten in the same frame.
        self.pick(strip.clone());
        ui.horizontal(|ui| {
            ui.label("Strip");
            let mut picked: Option<DeviceTag> = None;
            egui::ComboBox::from_id_salt("strip editor strip")
                .selected_text(strip.to_string())
                .show_ui(ui, |ui| {
                    for s in strips.keys() {
                        if ui.selectable_label(*s == strip, s.to_string()).clicked() {
                            picked = Some(s.clone());
                        }
                    }
                    ui.separator();
                    if ui
                        .selectable_label(false, NEW_STRIP)
                        .on_hover_text("Start another strip with a spare terminal")
                        .clicked()
                    {
                        self.starting = true;
                    }
                });
            if let Some(s) = picked {
                self.pick(s);
            }
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
        let rail = strips.get(&strip).cloned().unwrap_or_default();
        let levels = rail_levels(open, &strip, &rail);
        self.ticked.retain(|t| rail.iter().any(|(n, _)| n == t));
        ui.weak("Drag a terminal by its handle, or use the arrows, to change the order on the rail. Tick terminals to jumper them.");
        ui.separator();

        let mut action: Option<Change> = None;
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            egui::Grid::new("strip editor terminals")
                .striped(true)
                .num_columns(7)
                .show(ui, |ui| {
                    for title in ["", "", "Terminal", "Level", "Jumpered with", "", ""] {
                        ui.label(RichText::new(title).strong());
                    }
                    ui.end_row();
                    for (k, (number, spare)) in rail.iter().enumerate() {
                        let handle = ui
                            .dnd_drag_source(
                                egui::Id::new(("strip editor drag", &strip, number)),
                                Dragged(strip.clone(), number.clone()),
                                |ui| ui.label(RichText::new("≡").monospace()),
                            )
                            .response
                            .on_hover_text("Drag to another place");
                        let mut tick = self.ticked.contains(number);
                        if ui.checkbox(&mut tick, "").on_hover_text("Tick to jumper").changed() {
                            if tick {
                                self.ticked.insert(number.clone());
                            } else {
                                self.ticked.remove(number);
                            }
                        }
                        let label = if *spare {
                            format!("{number} (spare)")
                        } else {
                            number.clone()
                        };
                        let row = ui.label(label);
                        for r in [&handle, &row] {
                            // Only a terminal of this rail moves; a payload from before the rail
                            // changed is dropped.
                            if let Some(from) = r.dnd_release_payload::<Dragged>()
                                && let Some(change) = drop_on(&strip, &rail, &from, k)
                            {
                                action = Some(change);
                            }
                        }
                        if let Some(level) = level_picker(ui, open, &strip, number, levels.get(number).copied()) {
                            action = Some(Change::Level(number.clone(), level));
                        }
                        let with = jumpered_with(open, &strip, number);
                        ui.horizontal(|ui| {
                            ui.label(&with);
                            if !with.is_empty()
                                && ui
                                    .small_button("✕")
                                    .on_hover_text("Take this terminal out of its jumper")
                                    .clicked()
                            {
                                action = Some(Change::Unjumper(number.clone()));
                            }
                        });
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
        let ticked: Vec<String> = rail
            .iter()
            .map(|(t, _)| t.clone())
            .filter(|t| self.ticked.contains(t))
            .collect();
        if ui
            .add_enabled(
                ticked.len() >= 2,
                egui::Button::new(format!("Jumper the {} ticked terminals", ticked.len())),
            )
            .clicked()
        {
            action = Some(Change::Jumper(ticked));
        }
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
            self.apply(open, &strip, change);
        }
        self.show_message(ui);
    }

    /// With no strip in the project yet: a strip tag and a number for its first spare.
    fn first_spare(&mut self, ui: &mut egui::Ui, open: &mut Open) {
        let another = self.starting && !open.strip_terminals().is_empty();
        if another {
            ui.label("Start a new strip with a spare terminal:");
        } else {
            ui.label("No terminal strips yet. Place terminals tagged like -X1.1 on a page, or start a strip with a spare terminal:");
        }
        let mut add = false;
        ui.horizontal(|ui| {
            ui.label("Strip");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_strip)
                    .hint_text("e.g. -X1")
                    .desired_width(80.0),
            );
            ui.label("Spare terminal");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_spare)
                    .hint_text("number, e.g. 1")
                    .desired_width(80.0),
            );
            add = ui
                .add_enabled(
                    !self.new_strip.trim().is_empty() && !self.new_spare.trim().is_empty(),
                    egui::Button::new("Add spare"),
                )
                .clicked();
            if another && ui.button("Cancel").clicked() {
                self.starting = false;
                self.new_strip.clear();
                self.new_spare.clear();
                self.message.clear();
            }
        });
        if add {
            self.add_first_spare(open);
        }
        self.show_message(ui);
    }

    /// What Add spare does in the new-strip form: start the typed strip with the typed spare,
    /// and show that strip from the next frame on. A strip the project has already is refused:
    /// its spares are added on its own rail.
    fn add_first_spare(&mut self, open: &mut Open) {
        match DeviceTag::parse(self.new_strip.trim()) {
            Ok(strip) if open.strip_terminals().contains_key(&strip) => {
                self.message = format!("{strip} is a strip already; pick it in the list to add spares to it");
            }
            Ok(strip) => {
                let spare = self.new_spare.trim().to_owned();
                // apply clears the rail's Add spare field on success; this form has its own.
                let kept = std::mem::take(&mut self.spare);
                self.apply(open, &strip, Change::AddSpare(spare, None));
                self.spare = kept;
                if self.message.is_empty() {
                    self.new_strip.clear();
                    self.new_spare.clear();
                    self.starting = false;
                    self.pick(strip);
                }
            }
            Err(e) => self.message = e.to_string(),
        }
    }

    fn show_message(&self, ui: &mut egui::Ui) {
        if !self.message.is_empty() {
            ui.label(RichText::new(&self.message).color(Theme::of(ui.ctx()).error).small());
        }
    }

    /// Make one change to `strip`, as one undo step; a refusal is shown in the window.
    fn apply(&mut self, open: &mut Open, strip: &DeviceTag, change: Change) {
        let result = match change {
            Change::Move(number, to) => open.move_terminal(strip, &number, to),
            Change::AddSpare(number, after) => {
                let r = open.add_spare(strip, &number, after.as_deref());
                if r.is_ok() {
                    self.spare.clear();
                }
                r
            }
            Change::RemoveSpare(number) => {
                open.remove_spare(strip, &number);
                self.ticked.remove(&number);
                Ok(())
            }
            Change::Level(number, level) => set_level(open, strip, &number, &level),
            Change::Jumper(terminals) => {
                let r = open.add_jumper(strip, &terminals);
                if r.is_ok() {
                    self.ticked.clear();
                }
                r
            }
            Change::Unjumper(number) => {
                open.remove_from_jumper(strip, &number);
                Ok(())
            }
        };
        self.message = result.err().unwrap_or_default();
    }
}

/// What the user asked for in this frame.
#[derive(Debug)]
enum Change {
    /// Move a terminal to a place on the rail.
    Move(String, usize),
    /// Add a spare, after a terminal or at the end.
    AddSpare(String, Option<String>),
    RemoveSpare(String),
    /// Set a terminal's level; empty clears its own `Level` property.
    Level(String, String),
    /// Jumper these terminals together.
    Jumper(Vec<String>),
    /// Take a terminal out of its jumper.
    Unjumper(String),
}

/// What a drag released on place `to` of the rail of `strip` does: move the dragged terminal
/// there, unless it is not on this rail any more (another strip's, or gone after an undo or
/// another participant's edit), when nothing happens.
fn drop_on(strip: &DeviceTag, rail: &[(String, bool)], from: &Dragged, to: usize) -> Option<Change> {
    (from.0 == *strip && rail.iter().any(|(t, _)| *t == from.1)).then(|| Change::Move(from.1.clone(), to))
}

/// The picker entry that starts another strip.
const NEW_STRIP: &str = "New strip…";

/// The level of each terminal on `rail` as the terminal plan shows it, inferred levels included
/// ([`wc_report::strip_levels`]): over the drawn terminals in number order, whatever the rail
/// order.
pub(crate) fn rail_levels(
    open: &Open,
    strip: &DeviceTag,
    rail: &[(String, bool)],
) -> std::collections::BTreeMap<String, u32> {
    let mut drawn: Vec<String> = rail
        .iter()
        .filter(|(_, spare)| !spare)
        .map(|(n, _)| n.clone())
        .collect();
    drawn.sort_by(|a, b| (wc_report::natural_key(a), a).cmp(&(wc_report::natural_key(b), b)));
    drawn.dedup();
    wc_report::strip_levels(&open.project, &strip.to_string(), drawn)
}

/// The device tag of terminal `number` of `strip` (`-X1.3`).
fn terminal_tag(strip: &DeviceTag, number: &str) -> Result<DeviceTag, String> {
    DeviceTag::parse(&format!("{strip}.{number}")).map_err(|e| e.to_string())
}

/// Set the level of terminal `number` of `strip`: its device's `Level` property, as one undo
/// step; an empty level removes the property.
pub fn set_level(open: &mut Open, strip: &DeviceTag, number: &str, level: &str) -> Result<(), String> {
    let tag = terminal_tag(strip, number)?;
    open.set_device_property(&tag, wc_report::LEVEL_PROPERTY, level);
    Ok(())
}

/// The level picker of terminal `number` of `strip`: its device's own `Level` property, from a
/// short list. Returns the level picked in this frame, if any.
fn level_picker(ui: &mut egui::Ui, open: &Open, strip: &DeviceTag, number: &str, shown: Option<u32>) -> Option<String> {
    let Ok(tag) = terminal_tag(strip, number) else {
        ui.label("");
        return None;
    };
    let own = open
        .project
        .devices
        .get(&tag)
        .and_then(|d| d.properties.get(wc_report::LEVEL_PROPERTY))
        .cloned();
    let current = own.clone().unwrap_or_default();
    let mut picked = current.clone();
    egui::ComboBox::from_id_salt(("strip editor level", strip, number))
        .width(90.0)
        .selected_text(level_text(own.as_deref(), shown))
        .show_ui(ui, |ui| {
            for l in LEVELS {
                ui.selectable_value(&mut picked, (*l).to_owned(), if l.is_empty() { "–" } else { l });
            }
        })
        .response
        .on_hover_text(
            "The terminal's own level on a multi-level terminal (1 is the bottom). – clears its own level; \"(inferred)\" is the level the terminal plan still gives it. Levels are inferred in number order, not rail order: a terminal without a level of its own whose next number up has level 2 or more is level 1.",
        );
    (picked != current).then_some(picked)
}

/// What the level picker shows: the terminal's own `Level`, else the level the terminal plan
/// infers for it, marked as such, else – for none.
fn level_text(own: Option<&str>, shown: Option<u32>) -> String {
    match (own, shown) {
        // A value the terminal plan ignores (see wc_report::level_warnings) says so.
        (Some(v), _) if wc_report::parse_level(v).is_err() => {
            format!("{} (ignored)", if v.trim().is_empty() { "empty" } else { v })
        }
        (Some(v), _) => v.to_owned(),
        (None, Some(l)) => format!("{l} (inferred)"),
        (None, None) => "–".to_owned(),
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
        // A level picked in the editor is the device's Level property, one undo step.
        let level = |o: &Open| {
            o.project
                .devices
                .get(&DeviceTag::parse("-X1.S1").unwrap_or_else(|_| x1.clone()))
                .and_then(|d| d.properties.get(wc_report::LEVEL_PROPERTY).cloned())
        };
        let mut ed = StripEditor::for_project(&o);
        ed.apply(&mut o, &x1, Change::Level("S1".into(), "2".into()));
        assert_eq!(level(&o).as_deref(), Some("2"));
        // Each change is its own undo step.
        o.undo();
        assert_eq!(level(&o), None);
        assert_eq!(rail(&o), ["3", "S1*", "1", "2"]);
        o.undo();
        assert_eq!(rail(&o), ["3", "1", "2"]);
        o.redo();
        // Jumper a drawn terminal and the spare from the editor, then take one out again.
        ed.apply(&mut o, &x1, Change::Jumper(vec!["1".into(), "S1".into()]));
        assert_eq!(jumpered_with(&o, &x1, "S1"), "1");
        ed.apply(&mut o, &x1, Change::Unjumper("1".into()));
        assert!(o.project.meta.jumpers.is_empty());
        ed.apply(&mut o, &x1, Change::Jumper(vec!["1".into(), "S1".into()]));
        o.remove_spare(&x1, "S1");
        assert!(o.project.meta.jumpers.is_empty(), "a spare taken off leaves its jumper");
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

        // A strip picked in the list stays picked: the next frame shows its terminals.
        let x7 = DeviceTag::parse("-X7")?;
        o.add_spare(&x7, "70", None)?;
        ed.pick(x7.clone());
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            ed.show(ctx, &mut o);
        });
        assert_eq!(ed.strip.as_ref(), Some(&x7));
        let texts: Vec<String> = out
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "70 (spare)"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "9 (spare)"), "{texts:?}");
        Ok(())
    }

    #[test]
    fn the_first_spare_starts_a_strip_in_a_project_without_strips() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, mut o) = crate::state::tests::project();
        let mut ed = StripEditor::for_project(&o);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        ed.new_strip = "-X5".into();
        ed.new_spare = "1".into();
        let texts = |ed: &mut StripEditor, o: &mut Open| -> Vec<String> {
            ctx.run(egui::RawInput::default(), |ctx| {
                ed.show(ctx, o);
            })
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
        };
        // The empty view offers the strip and spare fields and the button;
        // a window paints its contents from its second frame on.
        texts(&mut ed, &mut o);
        let first = texts(&mut ed, &mut o);
        assert!(first.iter().any(|t| t == "Add spare"), "{first:?}");
        assert!(
            first.iter().any(|t| t.starts_with("No terminal strips yet")),
            "{first:?}"
        );
        // What its Add spare button does; a bad tag is refused and shown.
        ed.new_strip = "X5".into();
        ed.add_first_spare(&mut o);
        assert!(!ed.message.is_empty());
        ed.new_strip = "-X5".into();
        ed.add_first_spare(&mut o);
        assert!(ed.message.is_empty(), "{}", ed.message);
        let strip = DeviceTag::parse("-X5")?;
        assert_eq!(
            o.strip_terminals().get(&strip).cloned().unwrap_or_default(),
            [("1".to_owned(), true)]
        );
        // The next frame shows the new rail.
        let next = texts(&mut ed, &mut o);
        assert!(next.iter().any(|t| t == "1 (spare)"), "{next:?}");
        Ok(())
    }

    #[test]
    fn a_drop_moves_only_a_terminal_still_on_the_rail() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, mut o) = crate::state::tests::project();
        let x1 = DeviceTag::parse("-X1")?;
        o.add_spare(&x1, "1", None)?;
        o.add_spare(&x1, "2", None)?;
        o.add_spare(&x1, "3", None)?;
        let rail = |o: &Open| o.strip_terminals().remove(&x1).unwrap_or_default();
        // A valid drop on the same strip moves the terminal.
        let mut ed = StripEditor::for_project(&o);
        let change = drop_on(&x1, &rail(&o), &Dragged(x1.clone(), "3".into()), 0).ok_or("no move")?;
        ed.apply(&mut o, &x1, change);
        assert_eq!(rail(&o).first().map(|(t, _)| t.as_str()), Some("3"));
        // Dragging spare 3, then an undo takes it off again: the drop on a remaining row does
        // nothing, so neither the rail nor the undo history changes.
        o.undo();
        o.undo();
        let before = (rail(&o), o.undo_mark());
        assert!(drop_on(&x1, &before.0, &Dragged(x1.clone(), "3".into()), 0).is_none());
        // Another strip's terminal is not moved here either.
        let x2 = DeviceTag::parse("-X2")?;
        assert!(drop_on(&x1, &before.0, &Dragged(x2, "1".into()), 0).is_none());
        assert_eq!((rail(&o), o.undo_mark()), before);
        Ok(())
    }

    #[test]
    fn the_editor_shows_inferred_levels_and_starts_another_strip() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, mut o) = crate::state::tests::project();
        let x1 = DeviceTag::parse("-X1")?;
        for (k, tag) in ["-X1.1", "-X1.2"].into_iter().enumerate() {
            let id = o.place(
                1,
                "wc.terminal",
                wc_core::geom::Point::new(20.0 + 20.0 * k as f64, 20.0),
            )?;
            let s = o.project.pages[1]
                .symbols
                .iter_mut()
                .find(|s| s.id == id)
                .ok_or("not placed")?;
            s.tag = Some(DeviceTag::parse(tag)?);
        }
        set_level(&mut o, &x1, "2", "2")?;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut ed = StripEditor::for_project(&o);
        let texts = |ed: &mut StripEditor, o: &mut Open| -> Vec<String> {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                ed.show(ctx, o);
            });
            ctx.run(egui::RawInput::default(), |ctx| {
                ed.show(ctx, o);
            })
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
        };
        // Terminal 1 has no level of its own, but sits right below level 2: the plan says 1.
        let shown = texts(&mut ed, &mut o);
        assert!(shown.iter().any(|t| t == "1 (inferred)"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "2"), "{shown:?}");
        // Moving it on the rail does not change that.
        o.move_terminal(&x1, "2", 0)?;
        let shown = texts(&mut ed, &mut o);
        assert!(shown.iter().any(|t| t == "1 (inferred)"), "{shown:?}");
        // Without the level above, it shows none.
        set_level(&mut o, &x1, "2", "")?;
        let shown = texts(&mut ed, &mut o);
        assert!(!shown.iter().any(|t| t.contains("(inferred)")), "{shown:?}");
        assert_eq!(level_text(None, None), "–");
        assert_eq!(level_text(Some("2"), Some(2)), "2");
        assert_eq!(level_text(Some("02"), Some(2)), "02");
        assert_eq!(level_text(Some("0"), None), "0 (ignored)");
        assert_eq!(level_text(Some(""), None), "empty (ignored)");

        // New strip… in the picker opens the first-spare form beside the existing strip.
        ed.starting = true;
        let shown = texts(&mut ed, &mut o);
        assert!(
            shown.iter().any(|t| t == "Start a new strip with a spare terminal:"),
            "{shown:?}"
        );
        assert!(shown.iter().any(|t| t == "Cancel"), "{shown:?}");
        // An existing strip is refused here; the form stays open.
        ed.new_strip = "-X1".into();
        ed.new_spare = "9".into();
        ed.add_first_spare(&mut o);
        assert!(ed.message.contains("already"), "{}", ed.message);
        assert!(ed.starting);
        assert!(
            !o.strip_terminals()
                .get(&x1)
                .is_some_and(|r| r.iter().any(|(t, _)| t == "9"))
        );
        // The new strip's spare is a draft of its own: the rail's Add spare field keeps its text.
        ed.spare = "7".into();
        ed.new_strip = "-X5".into();
        ed.new_spare = "50".into();
        ed.add_first_spare(&mut o);
        assert_eq!(ed.spare, "7");
        assert!(ed.message.is_empty(), "{}", ed.message);
        assert!(!ed.starting);
        let x5 = DeviceTag::parse("-X5")?;
        assert_eq!(ed.strip.as_ref(), Some(&x5));
        let shown = texts(&mut ed, &mut o);
        assert!(shown.iter().any(|t| t == "50 (spare)"), "{shown:?}");
        assert!(!shown.iter().any(|t| t == "1 (inferred)"), "{shown:?}");
        Ok(())
    }
}
