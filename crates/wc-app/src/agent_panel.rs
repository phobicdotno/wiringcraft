//! The assistant panel: the conversation, the input box and the assistant's settings.

use eframe::egui::{self, Key, RichText, Stroke};
use std::sync::Arc;

use wc_agent::{Backend, EFFORTS, Entry, KEY_VAR, MODELS, acp, shown_text};

use crate::App;
use crate::agent::{AppTools, SYSTEM};
use crate::screenshot::{self, MAX_WAITING, Shot};
use crate::theme::Theme;

impl App {
    fn uses_login(&self) -> bool {
        self.prefs.assistant.backend == Backend::ClaudeLogin
    }

    /// A line in the conversation that is showing.
    fn assistant_log(&mut self, e: Entry) {
        if self.uses_login() {
            self.assistant.login_log.push(e);
        } else {
            self.assistant.session.log.push(e);
        }
    }

    /// Send the input box's text, and the screenshots waiting, to the assistant.
    pub(crate) fn assistant_send(&mut self, ctx: &egui::Context) {
        // A turn started now would fold the import's undo step into its own.
        if self.importing.is_some() {
            self.assistant_log(Entry::Error(
                "Not sent: an import is running; send it again when it is done.".into(),
            ));
            return;
        }
        let shots: Vec<Shot> = self.assistant.waiting.iter().map(|(s, _)| s.clone()).collect();
        let images = match shots.iter().map(Shot::to_agent).collect::<Result<Vec<_>, _>>() {
            Ok(i) => i,
            Err(e) => {
                self.assistant_log(Entry::Error(e));
                return;
            }
        };
        let text = std::mem::take(&mut self.assistant.input);
        let context = self.agent_context_to_send();
        let ok = if self.uses_login() {
            self.login_send(ctx, &text, &images, &context)
        } else {
            let settings = self.prefs.assistant.clone();
            let specs = self.assistant.specs().to_vec();
            self.assistant
                .session
                .send(&text, &images, &context, &settings, SYSTEM, &specs)
                .map_err(|e| self.assistant.session.log.push(Entry::Error(e)))
                .is_ok()
        };
        if ok {
            self.assistant.turn = self.open.as_mut().map(|o| (o.dir.clone(), o.start_turn()));
            self.assistant.waiting.clear();
            let a = &mut self.assistant;
            a.shared.extend(shots.iter().cloned());
            let extra = a.shared.len().saturating_sub(MAX_WAITING);
            a.shared.drain(..extra);
            a.sent = shots;
            a.sent_text = text;
        } else {
            self.assistant.input = text;
        }
    }

    /// Add a screenshot to the next message.
    fn add_screenshot(&mut self, shot: Shot) {
        if self.assistant.waiting.len() >= MAX_WAITING {
            self.assistant_log(Entry::Error(format!(
                "At most {MAX_WAITING} screenshots go with one message."
            )));
            return;
        }
        self.assistant.waiting.push((shot, None));
    }

    /// The picture on the clipboard goes with the next message. `quiet` (after Ctrl+V in the
    /// text box) says nothing when there is no picture, or when text was pasted instead.
    fn paste_screenshot(&mut self, quiet: bool) {
        if quiet && self.assistant.clipboard.has_text() {
            return;
        }
        match self.assistant.clipboard.paste() {
            Ok(Some(shot)) => self.add_screenshot(shot),
            Ok(None) if quiet => {}
            Ok(None) => self.assistant_log(Entry::Note(
                "There is no picture on the clipboard. Take a screenshot (on Windows Win+Shift+S, on a Mac Cmd+Ctrl+Shift+4) and try again.".into(),
            )),
            Err(e) => self.assistant_log(Entry::Error(e)),
        }
    }

    /// Put an unanswered message back in the box, saying if screenshots did not fit.
    fn give_back(&mut self, text: Option<String>) {
        let left_out = self.assistant.give_back(text);
        if left_out > 0 {
            self.assistant_log(Entry::Note(format!(
                "{left_out} newer screenshot(s) did not fit next to the message given back (at most {MAX_WAITING})."
            )));
        }
    }

    /// Pictures dropped on the window go to the assistant, once it is not working on a message.
    fn take_dropped_pictures(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .filter(|p| screenshot::is_picture(p))
                .collect()
        });
        if !dropped.is_empty() && self.assistant.busy() {
            self.assistant_log(Entry::Note(
                "The assistant is still working; drop the screenshot again when it has answered.".into(),
            ));
            return;
        }
        for path in dropped {
            match Shot::from_file(&path) {
                Ok(shot) => self.add_screenshot(shot),
                Err(e) => self.assistant_log(Entry::Error(e)),
            }
        }
    }

    /// Claude login: start the bridge and Claude Code when needed, then send.
    fn login_send(&mut self, ctx: &egui::Context, text: &str, images: &[wc_agent::Image], context: &str) -> bool {
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
            // A folder of WiringCraft's own, never the (possibly untrusted) project folder.
            let cwd = match crate::prefs::assistant_dir() {
                Ok(d) => d,
                Err(e) => {
                    a.login_log.push(Entry::Error(e));
                    return false;
                }
            };
            match acp::Acp::start(&argv, &cwd, bridge, SYSTEM, &self.prefs.assistant.login_model, wake) {
                Ok(c) => a.acp = Some(c),
                Err(e) => {
                    a.login_log.push(Entry::Error(e));
                    return false;
                }
            }
        }
        let Some(c) = a.acp.as_mut() else { return false };
        let text = text.trim();
        if text.is_empty() && images.is_empty() {
            return false;
        }
        a.login_log.push(Entry::User(shown_text(text, images.len())));
        c.prompt(&format!("<context>\n{}\n</context>\n\n{text}", context.trim()), images);
        true
    }

    /// Pick up the assistant's answers and run its tools. Called every frame.
    pub(crate) fn assistant_poll(&mut self, ctx: &egui::Context) {
        // Drops are taken before the answer is looked at, so a screenshot dropped in the frame
        // where a message fails is refused rather than joined to the message given back.
        if self.prefs.show_assistant {
            self.take_dropped_pictures(ctx);
        }
        let mut finished = false;
        // Claude login: tool calls from Claude Code, then what it said.
        // Calls outside a running turn (after Stop, or late) are refused, never run.
        if let Some(bridge) = self.assistant.bridge.take() {
            if self.assistant.acp.as_ref().is_some_and(acp::Acp::takes_tools) {
                let entries = bridge.poll(&mut AppTools(self));
                self.assistant.login_log.extend(entries);
            } else {
                // A call already running on a worker thread is still completed.
                let entries = bridge.poll_running(&mut AppTools(self));
                self.assistant.login_log.extend(entries);
                bridge.refuse("Stopped by the user: no turn is running in WiringCraft.");
            }
            self.assistant.bridge = Some(bridge);
        }
        if let Some(c) = self.assistant.acp.as_mut() {
            let p = c.poll(&mut self.assistant.login_log);
            finished |= p.finished;
            if p.unanswered {
                self.give_back(None);
            }
        }
        // API key.
        if self.assistant.session.busy() {
            let settings = self.prefs.assistant.clone();
            let mut session = std::mem::replace(&mut self.assistant.session, wc_agent::Session::new(Arc::new(Idle)));
            let polled = session.poll(&mut AppTools(self), &settings, SYSTEM, false);
            self.assistant.session = session;
            finished |= polled.finished;
            if let Some(text) = self.assistant.session.returned.take() {
                self.give_back(Some(text));
            }
        }
        if finished {
            self.end_turn();
        }
        if self.assistant.busy()
            || self
                .assistant
                .bridge
                .as_ref()
                .is_some_and(wc_agent::bridge::Bridge::working)
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    pub(crate) fn assistant_stop(&mut self) {
        self.assistant.session.stop();
        if let Some(c) = self.assistant.acp.as_mut() {
            c.cancel();
        }
        // Stopped before it began (Claude Code still starting): no finish will end the turn.
        // A turn still running keeps its mark and ends when it finishes.
        if !self.assistant.busy() {
            self.end_turn();
        }
    }

    /// The assistant's turn is over: everything it changed is one undo step, and the project
    /// no longer holds the turn's first step at the front of a full history.
    fn end_turn(&mut self) {
        if let (Some((dir, mark)), Some(open)) = (self.assistant.turn.take(), self.open.as_mut())
            && open.dir == dir
        {
            open.squash_undo(mark);
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
                    self.assistant.waiting.clear();
                    self.assistant.sent.clear();
                    self.assistant.shared.clear();
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
                self.waiting_screenshots(ui, t);
                let edit = egui::TextEdit::multiline(&mut self.assistant.input)
                    .hint_text("Tell the assistant what to draw or change…  (Enter sends, Shift+Enter for a new line)")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY);
                let r = ui.add_enabled(!busy, edit);
                self.assistant.input_id = Some(r.id);
                // Ctrl+V with a picture on the clipboard: the toolkit only pastes text, so the
                // key's release is the sign to look for a picture.
                let paste = r.has_focus() && ui.input(|i| paste_released(&i.events));
                if paste {
                    self.paste_screenshot(true);
                }
                let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
                if enter {
                    // The key press already added a line break.
                    let trimmed = self.assistant.input.trim_end_matches(['\n', '\r']).len();
                    self.assistant.input.truncate(trimmed);
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.backend_label()).small().color(t.faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let words = !self.assistant.input.trim().is_empty();
                        let can = !busy && ready && (words || !self.assistant.waiting.is_empty());
                        if ui.add_enabled(can, egui::Button::new("Send")).clicked() || (enter && can) {
                            let ctx = ui.ctx().clone();
                            self.assistant_send(&ctx);
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("Paste screenshot"))
                            .on_hover_text(
                                "Send a picture from the clipboard with your message. Ctrl+V in the box works too (Windows and Linux), and so does dropping a PNG or JPEG file on the window.",
                            )
                            .clicked()
                        {
                            self.paste_screenshot(false);
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

    /// The screenshots that go with the next message, each with a button to take it off.
    fn waiting_screenshots(&mut self, ui: &mut egui::Ui, t: &Theme) {
        if self.assistant.waiting.is_empty() {
            return;
        }
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (n, (shot, texture)) in self.assistant.waiting.iter_mut().enumerate() {
                let tex = texture.get_or_insert_with(|| {
                    ui.ctx().load_texture(
                        format!("assistant_shot_{n}"),
                        shot.color_image(),
                        egui::TextureOptions::LINEAR,
                    )
                });
                let size = tex.size_vec2();
                let thumb = size * (48.0 / size.y.max(1.0)).min(96.0 / size.x.max(1.0));
                let preview = size * (480.0 / size.x.max(size.y).max(1.0)).min(1.0);
                ui.vertical(|ui| {
                    ui.add(egui::Image::new((tex.id(), thumb))).on_hover_ui(|ui| {
                        ui.add(egui::Image::new((tex.id(), preview)));
                    });
                    if ui
                        .small_button("Remove")
                        .on_hover_text("Do not send this screenshot")
                        .clicked()
                    {
                        remove = Some(n);
                    }
                });
            }
        });
        ui.label(
            RichText::new("Screenshots go with your next message. Leave out anything private.")
                .small()
                .color(t.faint),
        );
        if let Some(n) = remove {
            self.assistant.waiting.remove(n);
        }
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

/// Whether Ctrl+V (Cmd+V) was let go in this frame. The window turns the key press into a text
/// paste and drops it when the clipboard holds only a picture, but the release still arrives.
/// macOS sends no release while Cmd is held, so there the button and dropping a file are the ways in.
fn paste_released(events: &[egui::Event]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, egui::Event::Key { key: Key::V, pressed: false, modifiers, .. } if modifiers.command))
}

/// Stands in for the real transport while the session is lent out for a poll.
struct Idle;

impl wc_agent::Transport for Idle {
    fn send(&self, _key: &str, _body: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("busy".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn ctrl_v_let_go_asks_for_a_picture() {
        let ctrl = egui::Modifiers::COMMAND;
        assert!(paste_released(&[key(Key::V, false, ctrl)]));
        assert!(!paste_released(&[key(Key::V, true, ctrl)]));
        assert!(!paste_released(&[key(Key::V, false, egui::Modifiers::NONE)]));
        assert!(!paste_released(&[key(Key::C, false, ctrl)]));
        assert!(!paste_released(&[]));
    }
}
