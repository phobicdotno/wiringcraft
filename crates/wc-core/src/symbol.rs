//! Symbols: the reusable drawing of a device function with named connection points (pins).

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rotation, Shape, bounds_of};
use crate::hash::content_hash;

/// Which way a wire leaves a pin; used by routing and by wire-end snapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinDir {
    Up,
    #[default]
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    /// Connection designation printed next to the pin, e.g. `13`, `A1`, `L1`.
    pub name: String,
    pub at: Point,
    #[serde(default)]
    pub dir: PinDir,
}

/// What a symbol stands for, used for cross-references between coil and contacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolRole {
    /// Drives other symbols with the same tag (relay coil, contactor coil).
    Main,
    /// Driven by a main symbol with the same tag (auxiliary contact).
    Auxiliary,
    #[default]
    Standalone,
    /// A terminal of a terminal strip.
    Terminal,
    /// An interruption point: the wire continues at every other interruption point with the
    /// same tag, usually on another page.
    Interruption,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    /// Stable library id, e.g. `wc.contact.no`. Never reused for a different meaning.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    /// Letter code suggested for new devices (e.g. `K` for relays).
    #[serde(default)]
    pub letter_code: String,
    #[serde(default)]
    pub role: SymbolRole,
    pub graphics: Vec<Shape>,
    pub pins: Vec<Pin>,
    /// Where the device tag is printed, relative to the symbol origin.
    #[serde(default)]
    pub tag_at: Point,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl Symbol {
    pub fn hash(&self) -> String {
        content_hash(self)
    }

    pub fn pin(&self, name: &str) -> Option<&Pin> {
        self.pins.iter().find(|p| p.name == name)
    }

    /// The box around the symbol's drawing when placed at `at`, origin included.
    pub fn placed_bounds(&self, at: Point, rotation: Rotation, mirror: bool) -> (Point, Point) {
        let mut pts = vec![at];
        for g in &self.graphics {
            let (a, b) = g.placed(at, rotation, mirror).bounds();
            pts.extend([a, b]);
        }
        bounds_of(&pts)
    }
}
