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
    /// What `hits` were found for: the query, the case option and the project's change count.
    searched: Option<(String, bool, u64)>,
    pub message: String,
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
    pub fn refresh(&mut self, project: &Project, version: u64) {
        let key = (self.query.clone(), self.match_case, version);
        if self.searched.as_ref() == Some(&key) {
            return;
        }
        self.hits = find(project, &self.query, self.options());
        self.chosen = vec![true; self.hits.len()];
        self.searched = Some(key);
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

    /// Draw the window. `version` is the project's change count, so the hits follow edits.
    pub fn show(&mut self, ctx: &egui::Context, project: &Project, version: u64) -> FindOutcome {
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
                    if self.searched.is_none() {
                        field.request_focus();
                    }
                    ui.end_row();
                    ui.label("Replace with");
                    ui.add(egui::TextEdit::singleline(&mut self.with).desired_width(280.0));
                    ui.end_row();
                });
                ui.checkbox(&mut self.match_case, "Match case");
                self.refresh(project, version);
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
        f.refresh(&p, 1);
        assert_eq!(f.hits.len(), 1);
        f.chosen[0] = false;
        f.refresh(&p, 1);
        assert!(!f.chosen[0], "the same search keeps the ticks");
        p.pages[0].graphics.clear();
        f.refresh(&p, 2);
        assert!(f.hits.is_empty(), "a changed project is searched again");
        f.match_case = true;
        f.query = "K1".into();
        f.refresh(&p, 2);
        assert_eq!(f.chosen_hits().len(), 1);
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
