//! Parts: orderable articles with commercial and technical data.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hash::content_hash;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Part {
    /// Stable library id, usually `<manufacturer>.<order number>` normalised.
    pub id: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub order_number: String,
    #[serde(default)]
    pub description: String,
    /// Symbol id this part is usually drawn with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// Free technical properties: voltage, current, width, ...
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
    /// The contacts the part has, when known: what the design check compares a device's drawn
    /// contacts with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contacts: Option<Contacts>,
    /// The maker's drawing of the part as seen on the mounting plate (usually the front view of
    /// its CAE data), drawn on layout pages instead of a plain box (#312).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprint: Option<Footprint>,
}

/// A part's outline drawing for cabinet layouts, in millimetres at 1:1 with y pointing down and
/// the drawing's top-left corner at (0, 0), so it can be placed at a device's `at`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Footprint {
    /// Extents of the drawing: the part's width and height on the plate.
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub shapes: Vec<crate::Shape>,
}

impl Footprint {
    /// A footprint from shapes in any position: moved so their extents start at (0, 0).
    /// `None` when there is nothing to draw or the extents are not a usable size.
    pub fn from_shapes(shapes: Vec<crate::Shape>) -> Option<Self> {
        let pts: Vec<crate::Point> = shapes.iter().flat_map(extent_points).collect();
        if pts.is_empty() {
            return None;
        }
        let (min, max) = crate::geom::bounds_of(&pts);
        let (width, height) = (max.x - min.x, max.y - min.y);
        let usable = |v: f64| v.is_finite() && v > 0.0 && v <= 5000.0;
        if !(usable(width) && usable(height)) {
            return None;
        }
        let shift = crate::Point::new(-min.x, -min.y);
        Some(Self {
            width,
            height,
            shapes: shapes.iter().map(|s| s.translate(shift)).collect(),
        })
    }

    /// The shapes for a device box of `width` x `height` with its top-left corner at `at`: at
    /// real size when the drawing fits the box, else shrunk evenly until it does, and centred
    /// in the box either way.
    pub fn placed(&self, at: crate::Point, width: f64, height: f64) -> Vec<crate::Shape> {
        let k = (width / self.width).min(height / self.height).min(1.0);
        let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
        let shift = crate::Point::new(
            at.x + (width - self.width * k) / 2.0,
            at.y + (height - self.height * k) / 2.0,
        );
        self.shapes.iter().map(|s| s.scaled(k).translate(shift)).collect()
    }
}

/// Points that span a shape's extents. An arc counts by its end points and the quarter points
/// its sweep passes, not its whole circle, so a quarter arc of radius 10 spans 10 x 10 mm.
fn extent_points(s: &crate::Shape) -> Vec<crate::Point> {
    let crate::Shape::Arc {
        center,
        radius,
        start,
        end,
    } = s
    else {
        let (a, b) = s.bounds();
        return vec![a, b];
    };
    // Angles run as the editor and the PDF draw them: from `start` to `end`, a point at `a`
    // being centre + r (cos a, sin a) on the y-down page.
    // Both into [0, 360), so angles far out of range cannot overflow the quarter count.
    let mut sweep = (end - start).rem_euclid(360.0);
    if sweep <= 0.0 {
        sweep += 360.0;
    }
    if !(sweep.is_finite() && start.is_finite()) || sweep >= 360.0 {
        let (a, b) = s.bounds();
        return vec![a, b];
    }
    let start = &start.rem_euclid(360.0);
    let at = |deg: f64| {
        let t = deg.to_radians();
        crate::Point::new(center.x + radius * t.cos(), center.y + radius * t.sin())
    };
    let mut out = vec![at(*start), at(start + sweep)];
    let first = (start / 90.0).floor() as i64 + 1;
    let mut k = first;
    while (k as f64) * 90.0 < start + sweep && k - first < 4 {
        out.push(at(k as f64 * 90.0));
        k += 1;
    }
    out
}

/// How many contacts of each kind a part has, or a device draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Contacts {
    /// Main (power) contacts: one per pole, pins 1-2, 3-4, 5-6, 7-8.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub main: u32,
    /// Normally open auxiliary contacts (pins x3-x4, x7-x8).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub no: u32,
    /// Normally closed auxiliary contacts (pins x1-x2, x5-x6).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub nc: u32,
    /// Changeover contacts (pins x1-x2-x4): each can serve as one NO or one NC.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub co: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The kind of one contact element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContactKind {
    Main,
    No,
    Nc,
    Co,
}

impl Contacts {
    /// Count one more contact of `kind`.
    pub fn add(&mut self, kind: ContactKind) {
        let n = match kind {
            ContactKind::Main => &mut self.main,
            ContactKind::No => &mut self.no,
            ContactKind::Nc => &mut self.nc,
            ContactKind::Co => &mut self.co,
        };
        *n = n.saturating_add(1);
    }

    /// Add `other` taken `times` times (an accessory fitted more than once).
    pub fn add_all(&mut self, other: &Contacts, times: u32) {
        self.main = self.main.saturating_add(other.main.saturating_mul(times));
        self.no = self.no.saturating_add(other.no.saturating_mul(times));
        self.nc = self.nc.saturating_add(other.nc.saturating_mul(times));
        self.co = self.co.saturating_add(other.co.saturating_mul(times));
    }

    pub fn is_empty(&self) -> bool {
        *self == Contacts::default()
    }

    /// Short form, such as `3 main, 1 NO, 1 NC`; `none` when empty.
    pub fn describe(&self) -> String {
        let parts: Vec<String> = [(self.main, "main"), (self.no, "NO"), (self.nc, "NC"), (self.co, "CO")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, k)| format!("{n} {k}"))
            .collect();
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join(", ")
        }
    }

    /// What `drawn` uses beyond these contacts, or `None` when they are enough. A changeover
    /// contact the drawing does not use as one can stand in for a missing NO or NC.
    pub fn overflow(&self, drawn: &Contacts) -> Option<Contacts> {
        let more = |d: u32, h: u32| d.saturating_sub(h);
        let mut over = Contacts {
            main: more(drawn.main, self.main),
            no: more(drawn.no, self.no),
            nc: more(drawn.nc, self.nc),
            co: more(drawn.co, self.co),
        };
        let mut spare_co = self.co.saturating_sub(drawn.co);
        for n in [&mut over.no, &mut over.nc] {
            let used = (*n).min(spare_co);
            *n -= used;
            spare_co -= used;
        }
        (!over.is_empty()).then_some(over)
    }
}

/// The contact elements a contact symbol with these pin names draws, by IEC 60947 terminal
/// numbers: single-digit pairs 1-2, 3-4, 5-6, 7-8 are main contacts; two-digit numbers group by
/// their first digit, where x1-x2 and x5-x6 are NC, x3-x4 and x7-x8 are NO, and x1-x2-x4 or
/// x5-x6-x8 are changeovers when x3 or x7 is not there. Pins that fit none of these are left out.
pub fn contact_kinds<'a>(pins: impl IntoIterator<Item = &'a str>) -> Vec<ContactKind> {
    let mut singles = std::collections::BTreeSet::new();
    let mut groups: BTreeMap<u32, std::collections::BTreeSet<u32>> = BTreeMap::new();
    for p in pins {
        let p = p.trim();
        if p.is_empty() || p.len() > 2 || !p.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(n) = p.parse::<u32>() else { continue };
        match p.len() {
            1 => {
                singles.insert(n);
            }
            _ if n >= 10 => {
                groups.entry(n / 10).or_default().insert(n % 10);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (a, b) in [(1, 2), (3, 4), (5, 6), (7, 8)] {
        if singles.contains(&a) && singles.contains(&b) {
            out.push(ContactKind::Main);
        }
    }
    for units in groups.values() {
        let has = |d: &[u32]| d.iter().all(|u| units.contains(u));
        // x1-x2-x4 is a changeover only when x3 is not there: with it, x1-x2 and x3-x4 are a
        // separate NC and NO (as on an overload relay's 95-96 and 97-98).
        let co_low = has(&[1, 2, 4]) && !units.contains(&3);
        let co_high = has(&[5, 6, 8]) && !units.contains(&7);
        if co_low {
            out.push(ContactKind::Co);
        } else if has(&[1, 2]) {
            out.push(ContactKind::Nc);
        }
        if has(&[3, 4]) {
            out.push(ContactKind::No);
        }
        if co_high {
            out.push(ContactKind::Co);
        } else if has(&[5, 6]) {
            out.push(ContactKind::Nc);
        }
        if has(&[7, 8]) {
            out.push(ContactKind::No);
        }
    }
    out.sort();
    out
}

impl Part {
    pub fn hash(&self) -> String {
        content_hash(self)
    }

    /// A file- and id-safe form of `manufacturer.order_number`.
    pub fn make_id(manufacturer: &str, order_number: &str) -> String {
        let norm = |s: &str| -> String {
            s.trim()
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>()
                .trim_matches('-')
                .to_owned()
        };
        let m = norm(manufacturer);
        let o = norm(order_number);
        if m.is_empty() { o } else { format!("{m}.{o}") }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn contact_kinds_follow_the_terminal_numbers() {
        use ContactKind::*;
        assert_eq!(contact_kinds(["13", "14"]), [No]);
        assert_eq!(contact_kinds(["21", "22"]), [Nc]);
        assert_eq!(contact_kinds(["11", "12", "14"]), [Co]);
        assert_eq!(contact_kinds(["95", "96"]), [Nc]);
        assert_eq!(contact_kinds(["97", "98"]), [No]);
        assert_eq!(contact_kinds(["95", "96", "98"]), [Co]);
        assert_eq!(
            contact_kinds(["95", "96", "97", "98"]),
            [No, Nc],
            "an NC and an NO, no changeover"
        );
        assert_eq!(contact_kinds(["11", "12", "13", "14"]), [No, Nc]);
        assert_eq!(contact_kinds(["1", "2", "3", "4", "5", "6"]), [Main, Main, Main]);
        assert_eq!(contact_kinds(["13", "14", "21", "22"]), [No, Nc]);
        assert!(contact_kinds(["A1", "A2"]).is_empty());
        assert!(contact_kinds(["13"]).is_empty(), "half a contact");
        assert!(contact_kinds(["L", "N", "PE", "113", "07"]).is_empty());
    }

    #[test]
    fn a_spare_changeover_covers_a_missing_no_or_nc() {
        let has = Contacts {
            main: 3,
            no: 1,
            co: 1,
            ..Default::default()
        };
        let drawn = |main, no, nc, co| Contacts { main, no, nc, co };
        assert_eq!(has.overflow(&drawn(3, 1, 0, 0)), None);
        assert_eq!(has.overflow(&drawn(3, 2, 0, 0)), None, "the changeover as a second NO");
        assert_eq!(has.overflow(&drawn(3, 1, 1, 0)), None, "the changeover as an NC");
        assert_eq!(has.overflow(&drawn(3, 1, 1, 1)), Some(drawn(0, 0, 1, 0)));
        assert_eq!(has.overflow(&drawn(4, 1, 0, 0)), Some(drawn(1, 0, 0, 0)));
        assert_eq!(has.overflow(&drawn(3, 2, 1, 0)), Some(drawn(0, 0, 1, 0)));
        assert_eq!(drawn(3, 1, 0, 2).describe(), "3 main, 1 NO, 2 CO");
        assert_eq!(Contacts::default().describe(), "none");
    }

    #[test]
    fn contacts_are_read_from_a_part_file_and_left_out_when_unknown() {
        let p: Part = serde_json::from_str(r#"{"id": "x", "contacts": {"main": 3, "no": 1}}"#).unwrap();
        assert_eq!(
            p.contacts,
            Some(Contacts {
                main: 3,
                no: 1,
                ..Default::default()
            })
        );
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains(r#""contacts":{"main":3,"no":1}"#), "{text}");
        let plain: Part = serde_json::from_str(r#"{"id": "y"}"#).unwrap();
        assert!(!serde_json::to_string(&plain).unwrap().contains("contacts"));
    }

    #[test]
    fn ids_are_safe() {
        assert_eq!(Part::make_id("Acme Corp", "AB 12/34"), "acme-corp.ab-12-34");
        assert_eq!(Part::make_id("", "X1"), "x1");
    }
}
