//! The assistant panel: the conversation, the input box and the assistant's settings.

use eframe::egui::{self, Key, RichText, Stroke};
use wc_agent::{EFFORTS, Entry, KEY_VAR, MODELS};

use crate::App;
use crate::agent::{AppTools, SYSTEM};
use crate::theme::Theme;

impl App {
    /// Send the input box's text to the assistant.
    pub(crate) fn assistant_send(&mut self) {
        let text = std::mem::take(&mut self.assistant.input);
        let context = self.agent_context();
        let settings = self.prefs.assistant.clone();
        let specs = self.assistant.specs().to_vec();
        match self.assistant.session.send(&text, &context, &settings, SYSTEM, &specs) {
            Ok(()) => {
                self.assistant.turn = self.open.as_ref().map(|o| (o.dir.clone(), o.undo_mark()));
            }
            Err(e) => {
                self.assistant.input = text;
                self.assistant.session.log.push(Entry::Error(e));
            }
        }
    }

    /// Pick up the assistant's answers and run its tools. Called every frame.
    pub(crate) fn assistant_poll(&mut self, ctx: &egui::Context) {
        if !self.assistant.session.busy() {
            return;
        }
        let settings = self.prefs.assistant.clone();
        let mut session = std::mem::replace(
            &mut self.assistant.session,
            wc_agent::Session::new(std::sync::Arc::new(Idle)),
        );
        let polled = session.poll(&mut AppTools(self), &settings, SYSTEM, false);
        self.assistant.session = session;
        if polled.finished {
            // Everything the assistant changed in this turn is one undo step.
            if let (Some((dir, mark)), Some(open)) = (self.assistant.turn.take(), self.open.as_mut())
                && open.dir == dir
            {
                open.squash_undo(mark);
            }
            if let Some(text) = self.assistant.session.returned.take()
                && self.assistant.input.is_empty()
            {
                self.assistant.input = text;
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(80));
    }

    /// The panel on the right of the window.
    pub(crate) fn assistant_panel(&mut self, ctx: &egui::Context) {
        if !self.prefs.show_assistant {
            return;
        }
        let t = Theme::of(ctx);
        egui::SidePanel::right("assistant")
            .frame(
                egui::Frame::new()
                    .fill(t.panel)
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show_separator_line(false)
            .resizable(true)
            .default_width(340.0)
            .min_width(260.0)
            .max_width(620.0)
            .show(ctx, |ui| self.assistant_ui(ui, &t));
    }

    fn assistant_ui(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let busy = self.assistant.session.busy();
        ui.horizontal(|ui| {
            crate::panels::section(ui, "Assistant", None);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("Hide the assistant (Ctrl+J)")
                    .clicked()
                {
                    self.prefs.show_assistant = false;
                    self.prefs.save();
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("New").small())
                    .on_hover_text("Start a new conversation")
                    .clicked()
                {
                    self.assistant.session.clear();
                }
                if ui.selectable_label(self.assistant.show_settings, "Settings").clicked() {
                    self.assistant.show_settings = !self.assistant.show_settings;
                }
            });
        });

        let key = self.prefs.assistant.key();
        if self.assistant.show_settings || key.is_none() {
            self.assistant_settings(ui, t, key.is_some());
            ui.separator();
        }

        // The input sits at the bottom; the conversation fills the space above it.
        egui::TopBottomPanel::bottom("assistant_input")
            .frame(egui::Frame::new())
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                ui.add_space(6.0);
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Working in your project…").color(t.muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Stop").clicked() {
                                self.assistant.session.stop();
                            }
                        });
                    });
                }
                let edit = egui::TextEdit::multiline(&mut self.assistant.input)
                    .hint_text("Tell the assistant what to draw or change…  (Enter sends, Shift+Enter for a new line)")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY);
                let r = ui.add_enabled(!busy, edit);
                let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
                if enter {
                    // The key press already added a line break.
                    let trimmed = self.assistant.input.trim_end_matches(['\n', '\r']).len();
                    self.assistant.input.truncate(trimmed);
                }
                ui.horizontal(|ui| {
                    let model = MODELS
                        .iter()
                        .find(|m| m.id == self.prefs.assistant.model)
                        .map(|m| m.label)
                        .unwrap_or(self.prefs.assistant.model.as_str());
                    ui.label(RichText::new(model).small().color(t.faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can = !busy && key.is_some() && !self.assistant.input.trim().is_empty();
                        if ui.add_enabled(can, egui::Button::new("Send")).clicked() || (enter && can) {
                            self.assistant_send();
                        }
                    });
                });
            });

        egui::ScrollArea::vertical()
            .id_salt("assistant_log")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                if self.assistant.session.log.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Ask for drawings and changes in plain words, for example:\n\n\
                             • \"Draw a DOL motor starter for -M1 on a new page: breaker, contactor, overload and start/stop buttons.\"\n\
                             • \"Give every relay on this page a Phoenix part.\"\n\
                             • \"Check the project and fix the open pins.\"\n\
                             • \"Number the wires and export a PDF.\"\n\n\
                             It works in the open project as you watch. Ctrl+Z takes back everything it did in one turn.",
                        )
                        .color(t.muted),
                    );
                }
                for e in &self.assistant.session.log {
                    entry(ui, t, e);
                }
            });
    }

    fn assistant_settings(&mut self, ui: &mut egui::Ui, t: &Theme, has_key: bool) {
        let mut changed = false;
        let from_env = std::env::var(KEY_VAR).is_ok_and(|k| !k.trim().is_empty());
        if from_env {
            ui.label(
                RichText::new(format!("API key: from {KEY_VAR}."))
                    .small()
                    .color(t.muted),
            );
        } else {
            if !has_key {
                ui.label(
                    RichText::new(format!(
                        "The assistant needs an Anthropic API key (console.anthropic.com). Paste it here or set {KEY_VAR}."
                    ))
                    .color(t.text),
                );
            }
            ui.horizontal(|ui| {
                ui.label("API key");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.prefs.assistant.api_key)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
                changed |= r.lost_focus() || r.changed();
            });
            ui.label(
                RichText::new("Kept in your WiringCraft preferences on this computer, never in a project.")
                    .small()
                    .color(t.faint),
            );
        }
        egui::Grid::new("assistant_settings").num_columns(2).show(ui, |ui| {
            ui.label("Model");
            let current = MODELS
                .iter()
                .find(|m| m.id == self.prefs.assistant.model)
                .map(|m| m.label)
                .unwrap_or("Other");
            egui::ComboBox::from_id_salt("assistant_model")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for m in MODELS {
                        changed |= ui
                            .selectable_value(&mut self.prefs.assistant.model, m.id.to_owned(), m.label)
                            .changed();
                    }
                });
            ui.end_row();
            ui.label("Effort").on_hover_text("Higher is more thorough and slower");
            egui::ComboBox::from_id_salt("assistant_effort")
                .selected_text(self.prefs.assistant.effort.clone())
                .show_ui(ui, |ui| {
                    for e in EFFORTS {
                        changed |= ui
                            .selectable_value(&mut self.prefs.assistant.effort, (*e).to_owned(), *e)
                            .changed();
                    }
                });
            ui.end_row();
        });
        if changed {
            self.prefs.save();
        }
    }
}

fn entry(ui: &mut egui::Ui, t: &Theme, e: &Entry) {
    ui.add_space(6.0);
    match e {
        Entry::User(text) => {
            egui::Frame::new()
                .fill(t.accent_soft)
                .corner_radius(8.0)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(text).color(t.text));
                });
        }
        Entry::Assistant(text) => {
            ui.label(RichText::new(text).color(t.text));
        }
        Entry::Tool { what, ok, detail } => {
            let (mark, color) = if *ok { ("✔", t.success) } else { ("✖", t.error) };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(mark).small().color(color));
                ui.label(RichText::new(what).small().color(t.muted))
                    .on_hover_text(detail);
            });
        }
        Entry::Note(text) => {
            ui.label(RichText::new(text).small().italics().color(t.muted));
        }
        Entry::Error(text) => {
            ui.label(RichText::new(text).color(t.error));
        }
    }
}

/// Stands in for the real transport while the session is lent out for a poll.
struct Idle;

impl wc_agent::Transport for Idle {
    fn send(&self, _key: &str, _body: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("busy".into())
    }
}
