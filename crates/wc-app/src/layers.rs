//! Editor layers: groups of what the canvas shows (free drawings, wire numbers,
//! cross-references, review comments, scanned underlays) that each user can hide or lock on
//! the canvas. They are the same groups the PDF and DXF exports put on their own layers. The
//! choice is a user preference: it is kept with the app's settings, never in project files.

use std::collections::BTreeSet;

use eframe::egui;
use serde::{Deserialize, Deserializer, Serialize};

use crate::state::Selection;

/// One group of what the canvas shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    /// Free drawings and texts on the sheet: notes, construction lines, imported graphics.
    Drawings,
    /// Wire numbers, potential names and cable cores next to wires.
    WireNumbers,
    /// Cross-references between coils and contacts and to other pages.
    CrossReferences,
    /// Review comments pinned on the page.
    Comments,
    /// A scanned sheet under the drawing.
    Underlay,
}

impl Layer {
    pub const ALL: [Layer; 5] = [
        Layer::Drawings,
        Layer::WireNumbers,
        Layer::CrossReferences,
        Layer::Comments,
        Layer::Underlay,
    ];

    /// The name the layer list shows.
    pub fn title(self) -> &'static str {
        match self {
            Layer::Drawings => "Drawings and notes",
            Layer::WireNumbers => "Wire numbers",
            Layer::CrossReferences => "Cross-references",
            Layer::Comments => "Review comments",
            Layer::Underlay => "Scanned sheets",
        }
    }

    /// The name kept in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Layer::Drawings => "drawings",
            Layer::WireNumbers => "wire-numbers",
            Layer::CrossReferences => "cross-references",
            Layer::Comments => "comments",
            Layer::Underlay => "underlay",
        }
    }

    fn from_key(key: &str) -> Option<Layer> {
        Layer::ALL.into_iter().find(|l| l.key() == key)
    }

    /// Whether the canvas has anything on this layer to pick or click, so that locking it
    /// means something. Wire numbers and underlays are only shown.
    pub fn lockable(self) -> bool {
        matches!(self, Layer::Drawings | Layer::CrossReferences | Layer::Comments)
    }
}

/// Which layers are hidden and which are locked. Every layer is shown and unlocked unless
/// listed here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layers {
    #[serde(deserialize_with = "known_layers")]
    hidden: BTreeSet<Layer>,
    #[serde(deserialize_with = "known_layers")]
    locked: BTreeSet<Layer>,
}

/// A list of layer names, skipping names this version does not know (from a newer or older
/// version), so one unknown name does not reset every other setting.
fn known_layers<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeSet<Layer>, D::Error> {
    let names = Vec::<String>::deserialize(d)?;
    Ok(names.iter().filter_map(|n| Layer::from_key(n)).collect())
}

impl Layers {
    pub fn shown(&self, l: Layer) -> bool {
        !self.hidden.contains(&l)
    }

    pub fn locked(&self, l: Layer) -> bool {
        l.lockable() && self.locked.contains(&l)
    }

    /// What is on the layer can be picked, clicked or selected: it is shown and not locked.
    pub fn pickable(&self, l: Layer) -> bool {
        self.shown(l) && !self.locked(l)
    }

    /// Why nothing can be added to `l` now: it is hidden or locked. `None` when it can.
    pub fn refusal(&self, l: Layer) -> Option<String> {
        (!self.pickable(l)).then(|| {
            format!(
                "The layer {} is hidden or locked: show and unlock it in View → Layers to add to it.",
                l.title()
            )
        })
    }

    pub fn set_shown(&mut self, l: Layer, shown: bool) {
        if shown {
            self.hidden.remove(&l);
        } else {
            self.hidden.insert(l);
        }
    }

    pub fn set_locked(&mut self, l: Layer, locked: bool) {
        if locked && l.lockable() {
            self.locked.insert(l);
        } else {
            self.locked.remove(&l);
        }
    }

    /// Whether a selected item may stay selected: free drawings only while their layer can
    /// be picked. Symbols and wires are on no layer.
    pub fn may_select(&self, s: &Selection) -> bool {
        match s {
            Selection::Graphic(_) => self.pickable(Layer::Drawings),
            Selection::Symbol(_) | Selection::Wire(_) => true,
        }
    }

    /// `items` without those on a layer that is hidden or locked.
    pub fn keep(&self, mut items: Vec<Selection>) -> Vec<Selection> {
        items.retain(|s| self.may_select(s));
        items
    }

    /// The layer list: a show and a lock tick box per layer. Returns whether anything changed.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        egui::Grid::new("editor_layers")
            .num_columns(3)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.strong("Layer");
                ui.strong("Show");
                ui.strong("Lock");
                ui.end_row();
                for l in Layer::ALL {
                    ui.label(l.title());
                    let mut shown = self.shown(l);
                    if ui
                        .checkbox(&mut shown, "")
                        .on_hover_text("Show this layer on the canvas")
                        .changed()
                    {
                        self.set_shown(l, shown);
                        changed = true;
                    }
                    if l.lockable() {
                        let mut locked = self.locked(l);
                        if ui
                            .checkbox(&mut locked, "")
                            .on_hover_text("Lock: shown, but nothing on it can be picked or clicked")
                            .changed()
                        {
                            self.set_locked(l, locked);
                            changed = true;
                        }
                    } else {
                        ui.weak("–").on_hover_text("Nothing on this layer can be picked");
                    }
                    ui.end_row();
                }
            });
        ui.add_space(4.0);
        ui.weak("Kept with your settings, not in the project.");
        changed
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn every_layer_starts_shown_and_unlocked() {
        let l = Layers::default();
        for layer in Layer::ALL {
            assert!(l.shown(layer) && !l.locked(layer) && l.pickable(layer), "{layer:?}");
        }
    }

    #[test]
    fn a_hidden_or_locked_layer_cannot_be_picked() {
        let mut l = Layers::default();
        l.set_locked(Layer::Drawings, true);
        assert!(l.shown(Layer::Drawings));
        assert!(!l.pickable(Layer::Drawings));
        l.set_locked(Layer::Drawings, false);
        l.set_shown(Layer::Drawings, false);
        assert!(!l.pickable(Layer::Drawings));
        let sel = vec![
            Selection::Symbol("d1".into()),
            Selection::Graphic(0),
            Selection::Wire("w1".into()),
        ];
        assert_eq!(
            l.keep(sel),
            vec![Selection::Symbol("d1".into()), Selection::Wire("w1".into())]
        );
    }

    #[test]
    fn a_layer_with_nothing_to_pick_is_never_locked() {
        let mut l = Layers::default();
        l.set_locked(Layer::WireNumbers, true);
        l.set_locked(Layer::Underlay, true);
        assert!(!l.locked(Layer::WireNumbers));
        assert!(!l.locked(Layer::Underlay));
        assert_eq!(l, Layers::default());
    }

    #[test]
    fn the_choice_is_kept_by_name_and_unknown_names_are_skipped() {
        let mut l = Layers::default();
        l.set_shown(Layer::WireNumbers, false);
        l.set_locked(Layer::Comments, true);
        let text = toml::to_string(&l).unwrap();
        assert!(text.contains("\"wire-numbers\""), "{text}");
        assert!(text.contains("\"comments\""), "{text}");
        assert_eq!(toml::from_str::<Layers>(&text).unwrap(), l);

        let read: Layers = toml::from_str("hidden = [\"wire-numbers\", \"holograms\"]\n").unwrap();
        assert!(!read.shown(Layer::WireNumbers));
        assert!(read.shown(Layer::Drawings));
        assert!(!read.locked(Layer::Comments));
        // Every key has a layer and back.
        for layer in Layer::ALL {
            assert_eq!(Layer::from_key(layer.key()), Some(layer));
        }
    }
}
