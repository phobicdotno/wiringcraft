//! 2D cabinet layout pages (`fil_filetype` [`KIND_CABINET`]) as WiringCraft panel layouts.
//!
//! On a layout page every item is a placed symbol (`tew_symbol`): the mounting plate, DIN rails,
//! wire ducts and the devices' footprints. The database has no item kind of its own for them, so
//! an item is sorted by its block name first (`Rail 35`, `Duct 40x60`, `Mounting plate`), then by
//! whether its component is an imported device, then by its component's description and part
//! number. A footprint's size is the symbol's box (`sym_posend*`); without one a device gets the
//! [`DEFAULT_SIZE`] footprint at its insertion point and is counted as estimated, and a rail or
//! duct, which has no length then, is left out. WiringCraft rails run left to right, so an
//! upright rail is left out too. Everything left out is counted with the reason.
//!
//! The plate is the plate symbol's box when the page has one, else the box round every item.
//! Coordinates turn from the page's y up to the panel's y down from the plate's top-left corner.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::geom::Point;
use wc_core::panel::{DEFAULT_SIZE, Panel, PanelItem};
use wc_core::{Device, DeviceTag};

use super::{SweProject, SweSymbol, round, slug};

/// `tew_file.fil_filetype` of a 2D cabinet layout page.
pub const KIND_CABINET: i64 = 9;

/// Words that name an item when a token of the name starts with them (`Rail`, `Rail35`, `rails`).
const RAIL_WORDS: &[&str] = &["rail"];
const DUCT_WORDS: &[&str] = &["duct", "trunking", "channel", "goulotte", "wireway"];
const PLATE_WORDS: &[&str] = &["plate", "platine", "backplate"];
/// German compounds end with the word: `Tragschiene`, `Kabelkanal`, `Montageplatte`.
const RAIL_ENDINGS: &[&str] = &["schiene", "schienen"];
const DUCT_ENDINGS: &[&str] = &["kanal", "kanaele", "kanäle"];
const PLATE_ENDINGS: &[&str] = &["platte"];

/// What the layout pages gave, for the import report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layouts {
    pub panels: Vec<Panel>,
    /// Symbols on layout pages.
    pub items_in: usize,
    pub rails: usize,
    pub ducts: usize,
    pub devices: usize,
    /// Devices drawn with the default footprint: their symbol has no box.
    pub estimated: usize,
    /// Rails and ducts without a box, so without a length.
    pub no_size: usize,
    /// Rails that run up and down.
    pub upright: usize,
    /// Plates whose box gives a panel its size (one per panel at most).
    pub plates: usize,
    /// Items that are neither plate, rail, duct nor an imported device (texts, frames, graphics),
    /// and plates after the first on a page.
    pub other: usize,
    /// Layout pages with no item to place.
    pub empty_pages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Plate,
    Rail,
    Duct,
    Device,
    Other,
}

/// Whether a token of `text` (split at anything but letters and digits) is one of `words`, or
/// one of them followed by digits, `x` or `s` (`Rail35`, `Duct40x60`, `rails`), or ends with one
/// of `endings`. A word inside another word does not count: `INDUCTOR` is no duct.
fn has_word(text: &str, (words, endings): (&[&str], &[&str])) -> bool {
    let t = text.to_lowercase();
    t.split(|c: char| !c.is_alphanumeric()).any(|token| {
        words.iter().any(|w| {
            token
                .strip_prefix(w)
                .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == 'x' || c == 's'))
        }) || endings.iter().any(|e| token.ends_with(e))
    })
}

const RAIL: (&[&str], &[&str]) = (RAIL_WORDS, RAIL_ENDINGS);
const DUCT: (&[&str], &[&str]) = (DUCT_WORDS, DUCT_ENDINGS);
const PLATE: (&[&str], &[&str]) = (PLATE_WORDS, PLATE_ENDINGS);

fn kind_of(s: &SweSymbol, p: &SweProject, tag: Option<&DeviceTag>) -> Kind {
    // The block name decides first: a plate, rail or duct may carry a tag of its own.
    if has_word(&s.block, RAIL) {
        return Kind::Rail;
    }
    if has_word(&s.block, DUCT) {
        return Kind::Duct;
    }
    if has_word(&s.block, PLATE) {
        return Kind::Plate;
    }
    if tag.is_some() {
        return Kind::Device;
    }
    let about = s
        .component
        .and_then(|c| p.components.get(&c))
        .map(|c| format!("{} {} {}", c.name, c.description, c.part_number))
        .unwrap_or_default();
    if has_word(&about, RAIL) {
        Kind::Rail
    } else if has_word(&about, DUCT) {
        Kind::Duct
    } else if has_word(&about, PLATE) {
        Kind::Plate
    } else {
        Kind::Other
    }
}

/// A box, y up: (min, max).
fn boxed(a: Point, b: Point) -> (Point, Point) {
    (
        Point::new(a.x.min(b.x), a.y.min(b.y)),
        Point::new(a.x.max(b.x), a.y.max(b.y)),
    )
}

/// One panel per cabinet layout page that has something to place. `tags` are the components'
/// device tags; only components that became devices (`devices`) are placed as devices.
pub fn layouts(p: &SweProject, tags: &BTreeMap<i64, DeviceTag>, devices: &BTreeMap<DeviceTag, Device>) -> Layouts {
    let mut out = Layouts::default();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for page in p.pages.iter().filter(|pg| pg.kind == KIND_CABINET) {
        let mut plate: Option<(Point, Point)> = None;
        // Items in page coordinates (y up), placed on the plate once its box is known.
        let mut rails: Vec<(Point, Point)> = Vec::new();
        let mut ducts: Vec<(Point, Point)> = Vec::new();
        let mut placed: Vec<(DeviceTag, Point, Point)> = Vec::new();
        for s in p.symbols.iter().filter(|s| s.page == page.id) {
            out.items_in += 1;
            let tag = s
                .component
                .and_then(|c| tags.get(&c))
                .filter(|t| devices.contains_key(*t));
            let size = s.corner.map(|c| boxed(s.at, c));
            match (kind_of(s, p, tag), size) {
                (Kind::Plate, Some(b)) if plate.is_none() => {
                    plate = Some(b);
                    out.plates += 1;
                }
                (Kind::Rail, Some((min, max))) if max.x - min.x >= max.y - min.y => rails.push((min, max)),
                (Kind::Rail, Some(_)) => out.upright += 1,
                (Kind::Duct, Some(b)) => ducts.push(b),
                (Kind::Rail | Kind::Duct, None) => out.no_size += 1,
                (Kind::Device, b) => {
                    let Some(tag) = tag else { continue };
                    let (min, max) = b.unwrap_or_else(|| {
                        out.estimated += 1;
                        // The insertion point as the top-left corner.
                        (
                            Point::new(s.at.x, s.at.y - DEFAULT_SIZE.1),
                            Point::new(s.at.x + DEFAULT_SIZE.0, s.at.y),
                        )
                    });
                    placed.push((tag.clone(), min, max));
                }
                _ => out.other += 1,
            }
        }
        // A plate alone still makes a panel; a page of graphics only does not.
        if plate.is_none() && rails.is_empty() && ducts.is_empty() && placed.is_empty() {
            out.empty_pages += 1;
            continue;
        }
        let (min, max) = plate.unwrap_or_else(|| {
            let corners: Vec<Point> = rails
                .iter()
                .chain(&ducts)
                .flat_map(|(a, b)| [*a, *b])
                .chain(placed.iter().flat_map(|(_, a, b)| [*a, *b]))
                .collect();
            wc_core::geom::bounds_of(&corners)
        });
        // Page (y up) to plate (y down from the top-left corner).
        let at = |q: Point| Point::new(round(q.x - min.x), round(max.y - q.y));
        let stem = format!(
            "swe-layout-{}",
            slug(if page.label.is_empty() {
                &page.title
            } else {
                &page.label
            })
        );
        let mut id = stem.clone();
        let mut k = 1;
        while !ids.insert(id.clone()) {
            k += 1;
            id = format!("{stem}-{k}");
        }
        let name = match (page.folder.is_empty(), page.title.is_empty()) {
            (false, false) => format!("{}: {}", page.folder, page.title),
            (true, false) => page.title.clone(),
            _ => format!("Cabinet layout {}", page.label),
        };
        let mut panel = Panel::new(id, name, round(max.x - min.x), round(max.y - min.y));
        for (a, b) in &ducts {
            panel.items.push(PanelItem::Duct {
                min: at(Point::new(a.x, b.y)),
                max: at(Point::new(b.x, a.y)),
            });
        }
        for (a, b) in &rails {
            panel.items.push(PanelItem::Rail {
                at: at(Point::new(a.x, (a.y + b.y) / 2.0)),
                length: round(b.x - a.x),
            });
        }
        for (tag, a, b) in placed {
            panel.items.push(PanelItem::Device {
                tag,
                at: at(Point::new(a.x, b.y)),
                width: round(b.x - a.x),
                height: round(b.y - a.y),
            });
        }
        out.rails += rails.len();
        out.ducts += ducts.len();
        out.devices += panel.items.len() - rails.len() - ducts.len();
        out.panels.push(panel);
    }
    out
}

#[cfg(test)]
mod tests;
