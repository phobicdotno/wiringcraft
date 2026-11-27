//! Find and replace across the project (Ctrl+F): a window that lists every hit of the search
//! text in free texts, device tags, wire numbers, potentials and device descriptions, shows a
//! hit on its page when clicked, and replaces the hits that are ticked as one undo step.

use eframe::egui::{self, RichText};
use wc_core::find::{FindKind, FindOptions, FindTarget, Hit, find};
use wc_core::project::Project;

use crate::state::Selection;

/// The find window's fields and its last search.
#[derive(Debug, Default)]
pub struct FindState {
    pub query: String,
    pub with: String,
    pub match_case: bool,
    pub hits: Vec<Hit>,
    /// Which hits Replace changes, in step with `hits`.
    pub chosen: Vec<bool>,
    /// What `hits` were found for: the query and the case option, and the project as it was
    /// then. Compared by content, so another participant's edits or another project opened in
    /// its place search again too, not only this window's own edits.
    searched: Option<(String, bool)>,
    searched_in: Option<Project>,
    pub message: String,
    /// Put the keyboard in the Find field on the next frame: set when Ctrl+F is pressed
    /// while the window is already open, so the shortcut brings you back to typing.
    pub focus: bool,
}

/// What the window asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum FindOutcome {
    None,
    /// Show this page and select the item.
    Go {
        page: String,
        select: Option<Selection>,
    },
    /// Replace the chosen hits.
    Replace,
    Close,
}

impl FindState {
    /// Search again when the text, the case option or the project changed since the last search.
    /// A new text or case option ticks every hit; a changed project keeps the ticks of the hits
    /// that are still there unchanged, so an untick survives others' edits.
    pub fn refresh(&mut self, project: &Project) {
        let key = (self.query.clone(), self.match_case);
        let same_search = self.searched.as_ref() == Some(&key);
        if same_search && self.searched_in.as_ref() == Some(project) {
            return;
        }
        let hits = find(project, &self.query, self.options());
        self.chosen = hits
            .iter()
            .map(|h| {
                !same_search
                    || self
                        .hits
                        .iter()
                        .zip(&self.chosen)
                        .find(|(old, _)| *old == h)
                        .is_none_or(|(_, c)| *c)
            })
            .collect();
        self.hits = hits;
        self.searched = Some(key);
        self.searched_in = Some(project.clone());
    }

    pub fn options(&self) -> FindOptions {
        FindOptions {
            match_case: self.match_case,
        }
    }

    /// The hits that are ticked.
    pub fn chosen_hits(&self) -> Vec<Hit> {
        self.hits
            .iter()
            .zip(&self.chosen)
            .filter(|(_, c)| **c)
            .map(|(h, _)| h.clone())
            .collect()
    }

    /// Draw the window. The hits follow every change to `project`.
    pub fn show(&mut self, ctx: &egui::Context, project: &Project) -> FindOutcome {
        let mut out = FindOutcome::None;
        let mut open = true;
        egui::Window::new("Find and replace")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                egui::Grid::new("find-fields").num_columns(2).show(ui, |ui| {
                    ui.label("Find");
                    let field = ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(280.0));
                    if std::mem::take(&mut self.focus) || self.searched.is_none() {
                        field.request_focus();
                    }
                    ui.end_row();
                    ui.label("Replace with");
                    let with = ui.add(egui::TextEdit::singleline(&mut self.with).desired_width(280.0));
                    ui.end_row();
                    // Escape in either field closes the window, as Cancel does in a dialog. The
                    // field had the keyboard, so the canvas did not take the Escape as well.
                    if (field.lost_focus() || with.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        out = FindOutcome::Close;
                    }
                });
                ui.checkbox(&mut self.match_case, "Match case");
                self.refresh(project);
                ui.separator();
                if self.query.is_empty() {
                    ui.label("Searches free texts, device tags, wire numbers, potentials and device descriptions on every page.");
                } else if self.hits.is_empty() {
                    ui.label("Nothing found.");
                } else {
                    ui.label(format!("{} found. Click one to show it; untick those to leave alone.", self.hits.len()));
                }
                let number = |id: &str| project.page(id).map(|p| p.number.clone()).unwrap_or_default();
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    egui::Grid::new("find-hits").striped(true).num_columns(4).show(ui, |ui| {
                        for (hit, chosen) in self.hits.iter().zip(self.chosen.iter_mut()) {
                            ui.checkbox(chosen, "");
                            ui.label(hit.target.page().map(number).unwrap_or_else(|| "-".into()));
                            ui.label(RichText::new(hit.kind.label()).weak());
                            match go_to(hit) {
                                Some(go) => {
                                    if ui.link(&hit.value).on_hover_text("Show it").clicked() {
                                        out = go;
                                    }
                                }
                                None => {
                                    ui.label(&hit.value).on_hover_text("Not drawn on any page");
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    let any = self.chosen.iter().any(|c| *c);
                    if ui
                        .add_enabled(any, egui::Button::new("Replace ticked"))
                        .on_hover_text("Replace in the ticked hits, as one undo step. A device tag is renamed everywhere it appears.")
                        .clicked()
                    {
                        out = FindOutcome::Replace;
                    }
                    if ui.button("Close").clicked() {
                        out = FindOutcome::Close;
                    }
                });
                if !self.message.is_empty() {
                    ui.label(&self.message);
                }
            });
        if !open {
            out = FindOutcome::Close;
        }
        out
    }
}

/// Where clicking a hit takes you: its page and the item to select.
pub fn go_to(hit: &Hit) -> Option<FindOutcome> {
    let (page, select) = match &hit.target {
        FindTarget::Symbol { page, id } => (page, Some(Selection::Symbol(id.clone()))),
        FindTarget::Wire { page, id } => (page, Some(Selection::Wire(id.clone()))),
        FindTarget::Graphic { page, index } => (page, Some(Selection::Graphic(*index))),
        FindTarget::Device { shown_at, .. } => {
            let (page, id) = shown_at.as_ref()?;
            (page, Some(Selection::Symbol(id.clone())))
        }
    };
    Some(FindOutcome::Go {
        page: page.clone(),
        select,
    })
}

/// A short note on how a replace went.
pub fn replaced_note(n: usize, hits: &[Hit]) -> String {
    let tags = hits.iter().any(|h| h.kind == FindKind::Tag);
    match n {
        0 => "Nothing to replace in the ticked hits.".into(),
        1 => "Replaced 1 value.".into(),
        n if tags => format!("Replaced {n} values; renamed devices changed everywhere they appear."),
        n => format!("Replaced {n} values."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wc_core::geom::{Point, Shape};
    use wc_core::{Page, PlacedSymbol};

    fn project() -> Project {
        let mut p = Project::new("Find");
        let mut page = Page::new("p1", "1", "");
        page.graphics.push(Shape::Text {
            at: Point::new(0.0, 0.0),
            text: "Pump".into(),
            height: 3.5,
            align: Default::default(),
            angle: 0.0,
        });
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "wc.coil".into(),
            at: Point::new(0.0, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: wc_core::DeviceTag::parse("-K1").ok(),
        });
        p.pages.push(page);
        p
    }

    #[test]
    fn searches_again_only_when_something_changed() {
        let mut p = project();
        let mut f = FindState {
            query: "pump".into(),
            ..Default::default()
        };
        f.refresh(&p);
        assert_eq!(f.hits.len(), 1);
        f.chosen[0] = false;
        f.refresh(&p);
        assert!(!f.chosen[0], "the same search keeps the ticks");
        p.pages[0].graphics.clear();
        f.refresh(&p);
        assert!(f.hits.is_empty(), "a changed project is searched again");
        f.match_case = true;
        f.query = "K1".into();
        f.refresh(&p);
        assert_eq!(f.chosen_hits().len(), 1);
    }

    #[test]
    fn an_untick_survives_an_unrelated_change() {
        let mut p = project();
        p.pages[0].graphics.push(Shape::Text {
            at: Point::new(0.0, 10.0),
            text: "K1 coil".into(),
            height: 3.5,
            align: Default::default(),
            angle: 0.0,
        });
        let mut f = FindState {
            query: "K1".into(),
            ..Default::default()
        };
        f.refresh(&p);
        assert!(f.hits.len() >= 2, "{:?}", f.hits);
        f.chosen[0] = false;
        p.pages[0].title = "Renamed".into();
        f.refresh(&p);
        assert!(!f.chosen[0], "still left alone after another edit");
        assert!(f.chosen[1..].iter().all(|c| *c));
        // A new search ticks everything again.
        f.match_case = true;
        f.refresh(&p);
        assert!(f.chosen.iter().all(|c| *c));
    }

    #[test]
    fn another_project_is_searched_again() {
        let p = project();
        let mut f = FindState {
            query: "pump".into(),
            ..Default::default()
        };
        f.refresh(&p);
        assert_eq!(f.hits.len(), 1);
        // Another project in its place (opened, joined or changed by others): no stale hits.
        let mut q = project();
        q.pages[0].graphics.clear();
        f.refresh(&q);
        assert!(f.hits.is_empty());
    }

    fn frame(ctx: &egui::Context, f: &mut FindState, p: &Project, events: Vec<egui::Event>) -> FindOutcome {
        let mut out = FindOutcome::None;
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| out = f.show(ctx, p));
        out
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn escape_in_the_find_field_closes_the_window() {
        let p = project();
        let ctx = egui::Context::default();
        let mut f = FindState::default();
        assert_eq!(frame(&ctx, &mut f, &p, vec![]), FindOutcome::None);
        assert_eq!(
            frame(&ctx, &mut f, &p, vec![egui::Event::Text("pump".into())]),
            FindOutcome::None
        );
        assert_eq!(f.query, "pump", "the Find field has the keyboard when the window opens");
        assert_eq!(
            frame(&ctx, &mut f, &p, vec![key(egui::Key::Escape)]),
            FindOutcome::Close
        );
    }

    #[test]
    fn escape_right_after_opening_closes_the_window() {
        // Opened by Find, which asks for the keyboard: that request is used up on the first
        // pass, so it cannot put the keyboard back after Escape on the next one.
        let p = project();
        let ctx = egui::Context::default();
        let mut f = FindState {
            focus: true,
            ..Default::default()
        };
        assert_eq!(frame(&ctx, &mut f, &p, vec![]), FindOutcome::None);
        assert!(!f.focus);
        assert_eq!(
            frame(&ctx, &mut f, &p, vec![key(egui::Key::Escape)]),
            FindOutcome::Close
        );
    }

    #[test]
    fn ctrl_f_again_puts_the_keyboard_back_in_the_find_field() {
        let p = project();
        let ctx = egui::Context::default();
        let mut f = FindState::default();
        frame(&ctx, &mut f, &p, vec![]);
        ctx.memory_mut(|m| m.stop_text_input());
        frame(&ctx, &mut f, &p, vec![]);
        frame(&ctx, &mut f, &p, vec![egui::Event::Text("x".into())]);
        assert_eq!(f.query, "", "nothing has the keyboard");
        f.focus = true;
        frame(&ctx, &mut f, &p, vec![]);
        frame(&ctx, &mut f, &p, vec![egui::Event::Text("pump".into())]);
        assert_eq!(f.query, "pump");
        assert!(!f.focus, "asked once");
    }

    #[test]
    fn a_hit_leads_to_its_page_and_item() {
        let p = project();
        let hits = find(&p, "", FindOptions::default());
        assert!(hits.is_empty());
        let hits = find(&p, "pump", FindOptions::default());
        assert_eq!(
            go_to(&hits[0]),
            Some(FindOutcome::Go {
                page: "p1".into(),
                select: Some(Selection::Graphic(0))
            })
        );
        let hits = find(&p, "K1", FindOptions::default());
        assert_eq!(
            go_to(&hits[0]),
            Some(FindOutcome::Go {
                page: "p1".into(),
                select: Some(Selection::Symbol("s1".into()))
            })
        );
    }
}
