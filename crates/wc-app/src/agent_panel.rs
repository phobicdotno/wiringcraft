//! The assistant panel: the conversation, the input box and the assistant's settings.

use eframe::egui::{self, Key, RichText, Stroke};
use std::sync::Arc;

use wc_agent::{Backend, EFFORTS, Entry, KEY_VAR, MODELS, acp};

use crate::App;
use crate::agent::{AppTools, SYSTEM};
use crate::theme::Theme;

impl App {
    fn uses_login(&self) -> bool {
        self.prefs.assistant.backend == Backend::ClaudeLogin
    }

    /// Send the input box's text to the assistant.
    pub(crate) fn assistant_send(&mut self, ctx: &egui::Context) {
        let text = std::mem::take(&mut self.assistant.input);
        let context = self.agent_context();
        let ok = if self.uses_login() {
            self.login_send(ctx, &text, &context)
        } else {
            let settings = self.prefs.assistant.clone();
            let specs = self.assistant.specs().to_vec();
            self.assistant
                .session
                .send(&text, &context, &settings, SYSTEM, &specs)
                .map_err(|e| self.assistant.session.log.push(Entry::Error(e)))
                .is_ok()
        };
        if ok {
            self.assistant.turn = self.open.as_ref().map(|o| (o.dir.clone(), o.undo_mark()));
        } else {
            self.assistant.input = text;
        }
    }

    /// Claude login: start the bridge and Claude Code when needed, then send.
    fn login_send(&mut self, ctx: &egui::Context, text: &str, context: &str) -> bool {
        let wake: Arc<dyn Fn() + Send + Sync> = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };
        let a = &mut self.assistant;
        if a.bridge.is_none() {
            match wc_agent::bridge::Bridge::start(a.specs().to_vec(), SYSTEM.to_owned(), Arc::clone(&wake)) {
                Ok(b) => a.bridge = Some(b),
                Err(e) => {
                    a.login_log
                        .push(Entry::Error(format!("Could not open WiringCraft's tool port: {e}")));
                    return false;
                }
            }
        }
        if !a.acp.as_ref().is_some_and(acp::Acp::alive) {
            let Some(argv) = acp::agent_command(&self.prefs.assistant.agent_command) else {
                a.login_log.push(Entry::Error(format!(
                    "Claude Code's connector was not found. Install Claude Code and its connector in a terminal:\n\n  {}\n\nthen log in once with `claude` and /login. Or switch Settings to an API key.",
                    acp::INSTALL
                )));
                return false;
            };
            let program = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    a.login_log
                        .push(Entry::Error(format!("Could not find WiringCraft itself: {e}")));
                    return false;
                }
            };
            let Some(bridge) = a.bridge.as_ref().map(|b| b.server_entry(&program)) else {
                return false;
            };
            let cwd = self
                .open
                .as_ref()
                .map(|o| o.dir.clone())
                .or_else(|| {
                    std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .map(Into::into)
                })
                .unwrap_or_else(|| ".".into());
            // Claude Code needs an absolute folder, and it is also the folder it starts in.
            let cwd = cwd.canonicalize().or_else(|_| std::path::absolute(&cwd)).unwrap_or(cwd);
            match acp::Acp::start(&argv, &cwd, bridge, SYSTEM, &self.prefs.assistant.login_model, wake) {
                Ok(c) => a.acp = Some(c),
                Err(e) => {
                    a.login_log.push(Entry::Error(e));
                    return false;
                }
            }
        }
        let Some(c) = a.acp.as_mut() else { return false };
        a.login_log.push(Entry::User(text.to_owned()));
        c.prompt(&format!("<context>\n{}\n</context>\n\n{text}", context.trim()));
        true
    }

    /// Pick up the assistant's answers and run its tools. Called every frame.
    pub(crate) fn assistant_poll(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        // Claude login: tool calls from Claude Code, then what it said.
        if let Some(bridge) = self.assistant.bridge.take() {
            let entries = bridge.poll(&mut AppTools(self));
            self.assistant.login_log.extend(entries);
            self.assistant.bridge = Some(bridge);
        }
        if let Some(c) = self.assistant.acp.as_mut() {
            let p = c.poll(&mut self.assistant.login_log);
            finished |= p.finished;
        }
        // API key.
        if self.assistant.session.busy() {
            let settings = self.prefs.assistant.clone();
            let mut session = std::mem::replace(&mut self.assistant.session, wc_agent::Session::new(Arc::new(Idle)));
            let polled = session.poll(&mut AppTools(self), &settings, SYSTEM, false);
            self.assistant.session = session;
            finished |= polled.finished;
            if let Some(text) = self.assistant.session.returned.take()
                && self.assistant.input.is_empty()
            {
                self.assistant.input = text;
            }
        }
        if finished {
            // Everything the assistant changed in this turn is one undo step.
            if let (Some((dir, mark)), Some(open)) = (self.assistant.turn.take(), self.open.as_mut())
                && open.dir == dir
            {
                open.squash_undo(mark);
            }
        }
        if self.assistant.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn assistant_stop(&mut self) {
        self.assistant.session.stop();
        if let Some(c) = self.assistant.acp.as_mut() {
            c.cancel();
        }
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
        let busy = self.assistant.busy();
        let login = self.uses_login();
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
                    self.assistant.login_log.clear();
                    self.assistant.acp = None;
                }
                if ui.selectable_label(self.assistant.show_settings, "Settings").clicked() {
                    self.assistant.show_settings = !self.assistant.show_settings;
                }
            });
        });

        let ready = login || self.prefs.assistant.key().is_some();
        if self.assistant.show_settings || !ready {
            self.assistant_settings(ui, t, busy);
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
                        let starting = self
                            .assistant
                            .acp
                            .as_ref()
                            .is_some_and(|c| c.state == acp::State::Starting);
                        let what = if starting {
                            "Starting Claude Code…"
                        } else {
                            "Working in your project…"
                        };
                        ui.label(RichText::new(what).color(t.muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Stop").clicked() {
                                self.assistant_stop();
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
                    ui.label(RichText::new(self.backend_label()).small().color(t.faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can = !busy && ready && !self.assistant.input.trim().is_empty();
                        if ui.add_enabled(can, egui::Button::new("Send")).clicked() || (enter && can) {
                            let ctx = ui.ctx().clone();
                            self.assistant_send(&ctx);
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
                let log = if login { &self.assistant.login_log } else { &self.assistant.session.log };
                if log.is_empty() {
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
                for e in log {
                    entry(ui, t, e);
                }
            });
    }

    /// The line under the input: who answers.
    fn backend_label(&self) -> String {
        if self.uses_login() {
            match &self.assistant.acp {
                Some(c) if c.alive() => {
                    let model = c
                        .models
                        .iter()
                        .find(|(id, _)| *id == c.model)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| c.model.clone());
                    if model.is_empty() {
                        "Claude Code (your Claude login)".into()
                    } else {
                        format!("Claude Code (your login) · {model}")
                    }
                }
                _ => "Claude Code (your Claude login)".into(),
            }
        } else {
            MODELS
                .iter()
                .find(|m| m.id == self.prefs.assistant.model)
                .map(|m| format!("API key · {}", m.label))
                .unwrap_or_else(|| format!("API key · {}", self.prefs.assistant.model))
        }
    }

    fn assistant_settings(&mut self, ui: &mut egui::Ui, t: &Theme, busy: bool) {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Connect with");
            ui.add_enabled_ui(!busy, |ui| {
                changed |= ui
                    .radio_value(&mut self.prefs.assistant.backend, Backend::ClaudeLogin, "My Claude login")
                    .on_hover_text("Claude Code on this computer, signed in with your Claude account (Pro, Max, Team). No key needed.")
                    .changed();
                changed |= ui
                    .radio_value(&mut self.prefs.assistant.backend, Backend::ApiKey, "API key")
                    .on_hover_text("An Anthropic API key, billed per use")
                    .changed();
            });
        });
        if self.uses_login() {
            self.login_settings(ui, t, &mut changed);
        } else {
            self.key_settings(ui, t, &mut changed);
        }
        if changed {
            self.prefs.save();
        }
    }

    fn login_settings(&mut self, ui: &mut egui::Ui, t: &Theme, changed: &mut bool) {
        let found = acp::agent_command(&self.prefs.assistant.agent_command);
        match &found {
            Some(argv) => {
                ui.label(
                    RichText::new(format!("Claude Code connector: {}", argv.join(" ")))
                        .small()
                        .color(t.muted),
                );
                if !acp::looks_logged_in() {
                    ui.label(
                        RichText::new(
                            "If Claude Code is not logged in yet: run `claude` in a terminal and type /login.",
                        )
                        .small()
                        .color(t.muted),
                    );
                }
            }
            None => {
                ui.label(
                    RichText::new(format!(
                        "Claude Code's connector is not installed. In a terminal run:\n  {}\nthen `claude` and /login once.",
                        acp::INSTALL
                    ))
                    .color(t.text),
                );
            }
        }
        ui.horizontal(|ui| {
            ui.label("Command");
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.prefs.assistant.agent_command)
                    .hint_text("found automatically")
                    .desired_width(f32::INFINITY),
            );
            *changed |= r.lost_focus();
        });
        // The models Claude Code offers, once it is running.
        let models = self.assistant.acp.as_ref().map(|c| (c.models.clone(), c.model.clone()));
        if let Some((models, current)) = models
            && !models.is_empty()
        {
            let label = models
                .iter()
                .find(|(id, _)| *id == current)
                .map(|(_, n)| n.clone())
                .unwrap_or(current.clone());
            let mut pick = None;
            ui.horizontal(|ui| {
                ui.label("Model");
                egui::ComboBox::from_id_salt("login_model")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for (id, name) in &models {
                            if ui.selectable_label(*id == current, name).clicked() {
                                pick = Some(id.clone());
                            }
                        }
                    });
            });
            if let Some(id) = pick {
                self.prefs.assistant.login_model = id.clone();
                *changed = true;
                if let Some(c) = self.assistant.acp.as_mut() {
                    c.set_model(&id);
                }
            }
        }
    }

    fn key_settings(&mut self, ui: &mut egui::Ui, t: &Theme, changed: &mut bool) {
        let from_env = std::env::var(KEY_VAR).is_ok_and(|k| !k.trim().is_empty());
        if from_env {
            ui.label(
                RichText::new(format!("API key: from {KEY_VAR}."))
                    .small()
                    .color(t.muted),
            );
        } else {
            if self.prefs.assistant.key().is_none() {
                ui.label(
                    RichText::new(format!(
                        "Paste an Anthropic API key (console.anthropic.com) or set {KEY_VAR}."
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
                *changed |= r.lost_focus() || r.changed();
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
                        *changed |= ui
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
                        *changed |= ui
                            .selectable_value(&mut self.prefs.assistant.effort, (*e).to_owned(), *e)
                            .changed();
                    }
                });
            ui.end_row();
        });
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
