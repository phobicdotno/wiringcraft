//! Find and replace across the project (Ctrl+F): a window that lists every hit of the search
//! text in free texts, device tags, wire numbers, potentials and device descriptions, shows a
//! hit on its page when clicked, and replaces the hits that are ticked as one undo step.

use eframe::egui::{self, RichText};
use wc_core::find::{FindKind, FindOptions, FindTarget, Hit, find};
use wc_core::project::Project;

use crate::state::Selection;

const WINDOW_TITLE: &str = "Find and replace";

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
    /// Set by Find (Ctrl+F) while the window is already open: the next frame brings the window
    /// to the front and gives the Find field the keyboard with its text selected, so typing
    /// starts a new search.
    pub focus_query: bool,
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
        if self.focus_query {
            ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, egui::Id::new(WINDOW_TITLE)));
        }
        egui::Window::new(WINDOW_TITLE)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                egui::Grid::new("find-fields").num_columns(2).show(ui, |ui| {
                    ui.label("Find");
                    let mut field = egui::TextEdit::singleline(&mut self.query)
                        .desired_width(280.0)
                        .show(ui);
                    if self.searched.is_none() || self.focus_query {
                        field.response.request_focus();
                    }
                    if std::mem::take(&mut self.focus_query) {
                        let all = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(self.query.chars().count()),
                        );
                        field.state.cursor.set_char_range(Some(all));
                        field.state.store(ui.ctx(), field.response.id);
                    }
                    ui.end_row();
                    ui.label("Replace with");
                    ui.add(egui::TextEdit::singleline(&mut self.with).desired_width(280.0));
                    ui.end_row();
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
                        .on_disabled_hover_text(self.replace_blocked())
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

impl FindState {
    /// Why Replace ticked is greyed out: egui shows no hover text on a disabled button, so
    /// this one says what is missing instead.
    fn replace_blocked(&self) -> &'static str {
        if self.query.is_empty() {
            "Type what to find first."
        } else if self.hits.is_empty() {
            "Nothing found to replace."
        } else {
            "Tick a hit to replace in it."
        }
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

    /// Find (Ctrl+F) with the window already open gives the Find field the keyboard again,
    /// with its text selected so typing replaces it.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn find_again_takes_the_keyboard_back_to_the_find_field() {
        let p = project();
        let ctx = egui::Context::default();
        let mut f = FindState {
            query: "pump".into(),
            ..Default::default()
        };
        let frame = |f: &mut FindState| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                f.show(ctx, &p);
            });
        };
        frame(&mut f);
        let focused = ctx.memory(|m| m.focused());
        assert!(focused.is_some(), "a new window's Find field has the keyboard");
        let field = focused.unwrap();
        // The keyboard goes elsewhere (the Replace field, or the page).
        ctx.memory_mut(|m| m.surrender_focus(field));
        frame(&mut f);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            None,
            "an open window does not take it back by itself"
        );
        f.focus_query = true;
        frame(&mut f);
        assert!(!f.focus_query);
        assert_eq!(ctx.memory(|m| m.focused()), Some(field));
        let range = egui::TextEdit::load_state(&ctx, field)
            .and_then(|s| s.cursor.char_range())
            .unwrap();
        let (a, b) = (
            range.primary.index.min(range.secondary.index),
            range.primary.index.max(range.secondary.index),
        );
        assert_eq!((a, b), (0, 4), "the whole text is selected");
    }

    #[test]
    fn a_greyed_out_replace_says_what_is_missing() {
        let p = project();
        let mut f = FindState::default();
        f.refresh(&p);
        assert_eq!(f.replace_blocked(), "Type what to find first.");
        f.query = "no such text anywhere".into();
        f.refresh(&p);
        assert!(f.hits.is_empty());
        assert_eq!(f.replace_blocked(), "Nothing found to replace.");
        f.query = "pump".into();
        f.refresh(&p);
        assert!(!f.hits.is_empty());
        f.chosen.iter_mut().for_each(|c| *c = false);
        assert_eq!(f.replace_blocked(), "Tick a hit to replace in it.");
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
