//! The project model: pages with placed symbols and wires, plus devices shared by all pages.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rotation, Shape};
use crate::tag::DeviceTag;

/// A library source as listed in `project.toml`. Earlier entries win when ids collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibrarySource {
    pub name: String,
    /// `folder`, `git`.
    pub kind: String,
    /// Folder path (relative to the project or absolute) or git URL.
    pub location: String,
    /// Git branch or tag; ignored for folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// `project.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default)]
    pub format_version: u32,
    /// Title-block fields printed on every page: customer, drawn_by, revision, ...
    #[serde(default)]
    pub title_block: BTreeMap<String, String>,
    #[serde(default, rename = "library")]
    pub libraries: Vec<LibrarySource>,
}

impl ProjectMeta {
    pub const FORMAT_VERSION: u32 = 1;

    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            format_version: Self::FORMAT_VERSION,
            title_block: BTreeMap::new(),
            libraries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PageSize {
    A4,
    #[default]
    A3,
    A2,
}

impl PageSize {
    /// Landscape width and height in millimetres.
    pub fn mm(self) -> (f64, f64) {
        match self {
            PageSize::A4 => (297.0, 210.0),
            PageSize::A3 => (420.0, 297.0),
            PageSize::A2 => (594.0, 420.0),
        }
    }

    /// The reference column (1-based, 1 to [`FRAME_COLUMNS`]) that `x` millimetres falls into.
    pub fn column_of(self, x: f64) -> usize {
        let (w, _) = self.mm();
        let col_w = (w - 2.0 * FRAME_MARGIN) / FRAME_COLUMNS as f64;
        let col = ((x - FRAME_MARGIN) / col_w).floor();
        if col.is_nan() {
            return 1;
        }
        (col as i64).saturating_add(1).clamp(1, FRAME_COLUMNS as i64) as usize
    }

    /// How many reference rows run down the sides of the frame: A to F on A4 and A3, A to H
    /// on A2. Fixed per sheet size, so a reference never moves.
    pub fn frame_rows(self) -> usize {
        match self {
            PageSize::A4 | PageSize::A3 => 6,
            PageSize::A2 => 8,
        }
    }

    /// The reference row (1-based, 1 to [`Self::frame_rows`], counted from the top) that `y`
    /// millimetres falls into.
    pub fn row_of(self, y: f64) -> usize {
        let (_, h) = self.mm();
        let rows = self.frame_rows();
        let row_h = (h - 2.0 * FRAME_MARGIN) / rows as f64;
        let row = ((y - FRAME_MARGIN) / row_h).floor();
        if row.is_nan() {
            return 1;
        }
        (row as i64).saturating_add(1).clamp(1, rows as i64) as usize
    }
}

/// The letter of a reference row: 1 is `A`, 2 is `B`, and so on.
pub fn row_letter(row: usize) -> char {
    let i = row.saturating_sub(1).min(25) as u8;
    char::from(b'A' + i)
}

/// Number of reference columns across a page's drawing area.
pub const FRAME_COLUMNS: usize = 10;
/// Distance from the paper edge to the page frame, in millimetres.
pub const FRAME_MARGIN: f64 = 10.0;

/// A symbol placed on a page as part of a device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedSymbol {
    /// Unique within the page, stable across edits (wires refer to it).
    pub id: String,
    pub symbol: String,
    pub at: Point,
    #[serde(default, skip_serializing_if = "Rotation::is_default")]
    pub rotation: Rotation,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mirror: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<DeviceTag>,
}

/// One end of a wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Endpoint {
    Pin {
        item: String,
        pin: String,
    },
    /// A free point, e.g. a junction or a potential arrow.
    Point {
        at: Point,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wire {
    pub id: String,
    pub from: Endpoint,
    pub to: Endpoint,
    /// Bend points between the two ends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<Point>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    /// Signal name: wires with the same potential name are connected across pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub potential: Option<String>,
    /// The cable this wire runs in, when it is a cable core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable: Option<CableCore>,
}

/// A wire's place in a cable: the cable device (for example `-W1`) and the core's name
/// (a number or a colour such as `BN`). A cable is an ordinary device; its part says the type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CableCore {
    pub cable: DeviceTag,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub core: String,
}

/// A schematic page. Stored as one file each so edits on different pages never conflict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    /// Stable id, also the file stem; never renumbered.
    pub id: String,
    /// Display number, e.g. `1`, `12.3`. Can change freely.
    pub number: String,
    pub title: String,
    #[serde(default)]
    pub size: PageSize,
    #[serde(default)]
    pub symbols: Vec<PlacedSymbol>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    /// Free graphics and imported drawings (for example a traced PDF or DXF).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<Shape>,
    /// A picture drawn under everything else, such as a scanned drawing to trace over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlay: Option<Underlay>,
    /// An overview (line diagram): it shows devices again that schematic pages draw in detail,
    /// so its symbols do not count as the device's drawings in `check`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overview: bool,
}

/// A raster picture under a page: a JPEG or PNG file in the project folder, stretched over a
/// rectangle of the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Underlay {
    /// Path relative to the project folder, with `/` separators, e.g. `underlays/p0003.jpg`.
    pub file: String,
    /// Top-left corner on the page, in millimetres.
    pub min: Point,
    /// Bottom-right corner on the page, in millimetres.
    pub max: Point,
}

impl Underlay {
    /// Folder in a project that holds underlay pictures.
    pub const DIR: &'static str = "underlays";

    /// Whether `file` is a plain relative path inside the project (no `..`, no absolute path,
    /// no backslashes), so reading it can never leave the project folder.
    pub fn file_is_safe(&self) -> bool {
        let f = &self.file;
        !f.is_empty()
            && !f.starts_with('/')
            && !f.contains('\\')
            && !f.contains(':')
            && f.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
    }
}

impl Page {
    pub fn new(id: impl Into<String>, number: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            number: number.into(),
            title: title.into(),
            size: PageSize::default(),
            symbols: Vec::new(),
            wires: Vec::new(),
            graphics: Vec::new(),
            underlay: None,
            overview: false,
        }
    }

    pub fn symbol(&self, id: &str) -> Option<&PlacedSymbol> {
        self.symbols.iter().find(|s| s.id == id)
    }

    /// Next free id with the given prefix (`s1`, `s2`, ...).
    pub fn next_id(&self, prefix: &str) -> String {
        let used = |id: &str| self.symbols.iter().any(|s| s.id == id) || self.wires.iter().any(|w| w.id == id);
        let mut n = 1usize;
        loop {
            let id = format!("{prefix}{n}");
            if !used(&id) {
                return id;
            }
            n += 1;
        }
    }
}

/// A device: one real-world product, possibly drawn with several symbols on several pages.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Device {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

/// A whole project in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub meta: ProjectMeta,
    /// Ordered as in the page navigator (by display number, then id).
    pub pages: Vec<Page>,
    pub devices: BTreeMap<DeviceTag, Device>,
    pub lock: crate::Lockfile,
    /// Cabinet panel layouts, by id order.
    pub panels: Vec<crate::panel::Panel>,
    /// Review comments on pages, by page then number.
    pub comments: Vec<crate::review::Comment>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            meta: ProjectMeta::new(name),
            pages: Vec::new(),
            devices: BTreeMap::new(),
            lock: crate::Lockfile::default(),
            panels: Vec::new(),
            comments: Vec::new(),
        }
    }

    pub fn page(&self, id: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.id == id)
    }

    pub fn page_mut(&mut self, id: &str) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
    }

    /// Sort pages by their display number, comparing numeric parts as numbers.
    pub fn sort_pages(&mut self) {
        self.pages.sort_by(|a, b| {
            natural_key(&a.number)
                .cmp(&natural_key(&b.number))
                .then(a.id.cmp(&b.id))
        });
    }

    /// Every symbol id used on any page.
    pub fn used_symbols(&self) -> std::collections::BTreeSet<String> {
        self.pages
            .iter()
            .flat_map(|p| p.symbols.iter().map(|s| s.symbol.clone()))
            .collect()
    }

    /// Every part id used by any device.
    pub fn used_parts(&self) -> std::collections::BTreeSet<String> {
        self.devices.values().filter_map(|d| d.part.clone()).collect()
    }

    /// Pages that place a given symbol.
    pub fn pages_using_symbol(&self, symbol: &str) -> Vec<&Page> {
        self.pages
            .iter()
            .filter(|p| p.symbols.iter().any(|s| s.symbol == symbol))
            .collect()
    }

    /// Where each tag appears: tag -> [(page id, placed symbol id)].
    pub fn tag_index(&self) -> BTreeMap<DeviceTag, Vec<(String, String)>> {
        let mut idx: BTreeMap<DeviceTag, Vec<(String, String)>> = BTreeMap::new();
        for p in &self.pages {
            for s in &p.symbols {
                if let Some(t) = &s.tag {
                    idx.entry(t.clone()).or_default().push((p.id.clone(), s.id.clone()));
                }
            }
        }
        idx
    }
}

/// Split into alternating text and number chunks so `2` sorts before `10`.
fn natural_key(s: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((std::mem::take(&mut text), num.parse().unwrap_or(u64::MAX)));
                num.clear();
            }
            text.push(c);
        }
    }
    out.push((
        text,
        if num.is_empty() {
            0
        } else {
            num.parse().unwrap_or(u64::MAX)
        },
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cover_the_frame_height_for_each_sheet_size() {
        for (size, rows) in [(PageSize::A4, 6), (PageSize::A3, 6), (PageSize::A2, 8)] {
            let (_, h) = size.mm();
            assert_eq!(size.frame_rows(), rows);
            assert_eq!(size.row_of(0.0), 1, "above the frame is row A");
            assert_eq!(size.row_of(FRAME_MARGIN + 0.1), 1);
            assert_eq!(size.row_of(h - FRAME_MARGIN - 0.1), rows);
            assert_eq!(size.row_of(h), rows, "below the frame is the last row");
            assert_eq!(size.row_of(f64::NAN), 1);
            let row_h = (h - 2.0 * FRAME_MARGIN) / rows as f64;
            assert_eq!(size.row_of(FRAME_MARGIN + 2.5 * row_h), 3, "the middle of row C");
        }
        assert_eq!(row_letter(1), 'A');
        assert_eq!(row_letter(8), 'H');
        assert_eq!(row_letter(0), 'A');
    }

    #[test]
    fn a_page_without_the_overview_field_loads_as_a_normal_page() -> Result<(), serde_json::Error> {
        let page: Page = serde_json::from_str(r#"{"id":"p1","number":"1","title":"Old"}"#)?;
        assert!(!page.overview);
        assert!(!serde_json::to_string(&page)?.contains("overview"));
        Ok(())
    }

    #[test]
    fn pages_sort_naturally() {
        let mut p = Project::new("t");
        for n in ["10", "2", "1.5", "1"] {
            p.pages.push(Page::new(format!("p{n}"), n, ""));
        }
        p.sort_pages();
        let order: Vec<&str> = p.pages.iter().map(|p| p.number.as_str()).collect();
        assert_eq!(order, ["1", "1.5", "2", "10"]);
    }

    #[test]
    fn endpoint_json_is_compact() -> Result<(), serde_json::Error> {
        let e = Endpoint::Pin {
            item: "s1".into(),
            pin: "13".into(),
        };
        assert_eq!(serde_json::to_string(&e)?, r#"{"item":"s1","pin":"13"}"#);
        let back: Endpoint = serde_json::from_str(r#"{"at":{"x":1.0,"y":2.0}}"#)?;
        assert_eq!(
            back,
            Endpoint::Point {
                at: Point::new(1.0, 2.0)
            }
        );
        Ok(())
    }
}
