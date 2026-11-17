//! The project model: pages with placed symbols and wires, plus devices shared by all pages.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rotation, Shape};
use crate::route::Route;
use crate::symbol::SymbolRole;
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
    /// The project's locations (cabinets, rooms, panels), with what they are. A device is in a
    /// location through the `+` part of its tag, so `+L1.L3-K1` is in the location `L1.L3`.
    #[serde(default, rename = "location", skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<Location>,
    /// Jumpers (bridges) between terminals of one strip; jumpered terminals are one potential.
    #[serde(default, rename = "jumper", skip_serializing_if = "Vec::is_empty")]
    pub jumpers: Vec<Jumper>,
}

/// Terminals of one strip bridged together by a jumper: they form one potential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jumper {
    /// The strip, e.g. `-X1`.
    pub strip: DeviceTag,
    /// Terminal numbers on the strip, in strip order, e.g. `["3", "4", "5"]`. At least two.
    pub terminals: Vec<String>,
}

/// The next automatic terminal number after `last`, which it advances; `None` past the largest.
pub fn next_number(last: &mut u64) -> Option<String> {
    *last = last.checked_add(1)?;
    Some(last.to_string())
}

/// Which terminal of which strip each placed terminal is, keyed by (page id, symbol id).
pub type TerminalNumbers = BTreeMap<(String, String), (DeviceTag, String)>;

/// The strip and number of every terminal placed on a schematic page: a tag `-X1.3` is
/// terminal 3 of `-X1`; terminals tagged with only their strip (`-X1`) are numbered after the
/// highest number of that strip, in drawing order (page, then left to right). This is the
/// numbering the terminal plan shows and jumpers refer to. `role(symbol id)` tells terminals
/// apart; when it is not known, a tag with a terminal level (`-X1.3`) is taken as one.
pub fn terminal_numbers(project: &Project, role: impl Fn(&str) -> Option<SymbolRole>) -> TerminalNumbers {
    struct Placed {
        key: (String, String),
        number: Option<String>,
        page: usize,
        x: f64,
    }
    let mut strips: BTreeMap<DeviceTag, Vec<Placed>> = BTreeMap::new();
    for (pi, page) in project.pages.iter().enumerate().filter(|(_, p)| !p.overview) {
        for s in &page.symbols {
            let Some(tag) = &s.tag else { continue };
            let terminal = match role(&s.symbol) {
                Some(r) => r == SymbolRole::Terminal,
                None => tag.product.len() > 1,
            };
            if !terminal || tag.product.is_empty() {
                continue;
            }
            let mut strip = tag.clone();
            let number = if tag.product.len() > 1 {
                strip.product.pop()
            } else {
                None
            };
            strips.entry(strip).or_default().push(Placed {
                key: (page.id.clone(), s.id.clone()),
                number,
                page: pi,
                x: s.at.x,
            });
        }
    }
    let mut out = BTreeMap::new();
    for (strip, mut list) in strips {
        list.sort_by(|a, b| match (&a.number, &b.number) {
            (Some(x), Some(y)) => natural_key(x).cmp(&natural_key(y)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.page.cmp(&b.page).then(a.x.total_cmp(&b.x)),
        });
        let mut next = list
            .iter()
            .filter_map(|t| t.number.as_deref()?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        for t in list {
            // A strip already at the largest number leaves the rest unnumbered.
            let number = match t.number {
                Some(n) => n,
                None => match next_number(&mut next) {
                    Some(n) => n,
                    None => continue,
                },
            };
            out.insert(t.key, (strip.clone(), number));
        }
    }
    out
}

impl Jumper {
    /// Whether the placed symbol `item` on `page` is one of this jumper's terminals: by its
    /// number in `numbers` on a schematic page, by its tag elsewhere (an overview draws
    /// terminals again).
    pub fn has(&self, numbers: &TerminalNumbers, page: &Page, item: &PlacedSymbol, terminal_role: bool) -> bool {
        match numbers.get(&(page.id.clone(), item.id.clone())) {
            Some((strip, n)) => *strip == self.strip && self.terminals.contains(n),
            None => terminal_role && item.tag.as_ref().is_some_and(|t| self.terminal_of(t).is_some()),
        }
    }

    /// The terminal number when `tag` is a terminal of this jumper (`-X1.4` on strip `-X1`).
    pub fn terminal_of<'a>(&self, tag: &'a DeviceTag) -> Option<&'a str> {
        let (last, rest) = tag.product.split_last()?;
        (tag.function == self.strip.function
            && tag.location == self.strip.location
            && rest == self.strip.product.as_slice()
            && self.terminals.contains(last))
        .then_some(last.as_str())
    }
}

/// A location of the project, such as a cabinet or a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// The location part of device tags, levels joined by `.`: `L1.L3` for `+L1.L3`.
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl ProjectMeta {
    /// Fill title-block fields from `fields` (for example an import). A field the project has
    /// with a value is kept; a missing, empty or blank one takes the new value. Returns how many
    /// fields were filled.
    pub fn fill_title_block(&mut self, fields: BTreeMap<String, String>) -> usize {
        let mut filled = 0;
        for (k, v) in fields {
            let have = self.title_block.entry(k).or_default();
            if have.trim().is_empty() && !v.trim().is_empty() {
                *have = v;
                filled += 1;
            }
        }
        filled
    }

    /// Add a location, or fill in the description of one the project already has. A description
    /// already there is kept. Returns whether anything changed.
    pub fn merge_location(&mut self, loc: Location) -> bool {
        match self.locations.iter_mut().find(|l| l.path == loc.path) {
            Some(have) if have.description.is_empty() && !loc.description.is_empty() => {
                have.description = loc.description;
                true
            }
            Some(_) => false,
            None => {
                self.locations.push(loc);
                self.locations.sort_by(|a, b| a.path.cmp(&b.path));
                true
            }
        }
    }
}

impl ProjectMeta {
    pub const FORMAT_VERSION: u32 = 1;

    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            format_version: Self::FORMAT_VERSION,
            title_block: BTreeMap::new(),
            libraries: Vec::new(),
            locations: Vec::new(),
            jumpers: Vec::new(),
        }
    }

    /// Jumper `terminals` of `strip` together. Jumpers of the strip that share a terminal with
    /// them become one. Refused when fewer than two different terminals result, or when a
    /// terminal cannot be the last level of a tag of the strip (`1.2` would be two levels).
    pub fn add_jumper(&mut self, strip: &DeviceTag, terminals: &[String]) -> Result<(), String> {
        let mut set: Vec<String> = Vec::new();
        for t in terminals.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            let mut tag = strip.clone();
            tag.product.push(t.to_owned());
            if DeviceTag::parse(&tag.to_string()).ok().as_ref() != Some(&tag) {
                return Err(format!(
                    "{t:?} cannot be a terminal of {strip}: a terminal is one tag level"
                ));
            }
            if !set.iter().any(|s| s == t) {
                set.push(t.to_owned());
            }
        }
        if set.len() < 2 {
            return Err("a jumper needs at least two different terminals of the strip".into());
        }
        // Take in every jumper of this strip that shares a terminal.
        let mut kept = Vec::new();
        for j in std::mem::take(&mut self.jumpers) {
            if &j.strip == strip && j.terminals.iter().any(|t| set.contains(t)) {
                for t in j.terminals {
                    if !set.contains(&t) {
                        set.push(t);
                    }
                }
            } else {
                kept.push(j);
            }
        }
        // Equal natural keys (`1`, `01`) fall back to the text, so the order never depends on
        // the order given.
        set.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b)));
        kept.push(Jumper {
            strip: strip.clone(),
            terminals: set,
        });
        kept.sort_by(|a, b| {
            a.strip.to_string().cmp(&b.strip.to_string()).then_with(|| {
                let first = |j: &Jumper| j.terminals.first().map(|t| (natural_key(t), t.clone()));
                first(a).cmp(&first(b))
            })
        });
        self.jumpers = kept;
        Ok(())
    }

    /// Take `terminal` of `strip` out of its jumper; a jumper left with one terminal goes.
    /// Returns whether anything changed.
    pub fn remove_from_jumper(&mut self, strip: &DeviceTag, terminal: &str) -> bool {
        let mut changed = false;
        for j in self.jumpers.iter_mut().filter(|j| &j.strip == strip) {
            let before = j.terminals.len();
            j.terminals.retain(|t| t != terminal);
            changed |= j.terminals.len() != before;
        }
        self.jumpers.retain(|j| j.terminals.len() >= 2);
        changed
    }

    /// The jumper a terminal (`-X1.4`) is in, if any.
    pub fn jumper_of(&self, terminal: &DeviceTag) -> Option<&Jumper> {
        self.jumpers.iter().find(|j| j.terminal_of(terminal).is_some())
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
    /// The route style its bend points were made with; older files have none (square).
    #[serde(default, skip_serializing_if = "Route::is_square")]
    pub route: Route,
    /// Cross-section, colour and type, for the wire list. Stored flat in the wire (`section`,
    /// `colour`, `type`); older files have none.
    #[serde(default, flatten)]
    pub spec: WireSpec,
}

/// What a wire is made of, as a wire list for manufacturing needs it. Each part is free text as
/// written in the source or typed in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WireSpec {
    /// Cross-section with its unit, e.g. `1.5 mm²` or `18 AWG`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Colour code, e.g. `BU` or `GNYE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    /// Wire type, e.g. `H07V-K`.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl WireSpec {
    pub fn is_empty(&self) -> bool {
        self.section.is_none() && self.colour.is_none() && self.kind.is_none()
    }

    /// A value as stored: trimmed, and `None` when empty.
    pub fn value(text: &str) -> Option<String> {
        Some(text.trim().to_owned()).filter(|s| !s.is_empty())
    }
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

/// Split into alternating text and number chunks so `2` sorts before `10` and `X2` before `X10`.
/// Every list ordered by number (pages, terminals) uses this.
pub fn natural_key(s: &str) -> Vec<(String, u64)> {
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
    fn locations_are_saved_only_when_there_are_some_and_merge_by_path() -> Result<(), toml::de::Error> {
        let old: ProjectMeta = toml::from_str("name = \"Old\"\nformat_version = 1\n")?;
        assert!(
            old.locations.is_empty(),
            "an older project.toml loads with no locations"
        );
        let mut m = ProjectMeta::new("x");
        assert!(!toml::to_string(&m).is_ok_and(|s| s.contains("location")));
        let loc = |p: &str, d: &str| Location {
            path: p.into(),
            description: d.into(),
        };
        assert!(m.merge_location(loc("L2", "")));
        assert!(m.merge_location(loc("L1.L3", "Engine room")));
        assert!(
            m.merge_location(loc("L2", "Bridge")),
            "an empty description is filled in"
        );
        assert!(
            !m.merge_location(loc("L2", "Other")),
            "a description already there stays"
        );
        let paths: Vec<&str> = m.locations.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["L1.L3", "L2"]);
        let text = toml::to_string(&m).unwrap_or_default();
        assert!(text.contains("[[location]]"), "{text}");
        let back: ProjectMeta = toml::from_str(&text)?;
        assert_eq!(back.locations, m.locations);
        Ok(())
    }

    #[test]
    fn a_page_without_the_overview_field_loads_as_a_normal_page() -> Result<(), serde_json::Error> {
        let page: Page = serde_json::from_str(r#"{"id":"p1","number":"1","title":"Old"}"#)?;
        assert!(!page.overview);
        assert!(!serde_json::to_string(&page)?.contains("overview"));
        Ok(())
    }

    #[test]
    fn imported_title_block_fields_fill_only_what_the_project_has_not_set() {
        let mut m = ProjectMeta::new("t");
        m.title_block.insert("customer".into(), "Typed in".into());
        m.title_block.insert("date".into(), String::new());
        m.title_block.insert("drawn_by".into(), "   ".into());
        let new: BTreeMap<String, String> = [
            ("customer", "Imported"),
            ("date", "2024-01-02"),
            ("drawn_by", "designer"),
            ("project_number", "1042"),
            ("description", "  "),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        assert_eq!(m.fill_title_block(new), 3);
        let tb = &m.title_block;
        assert_eq!(tb["customer"], "Typed in", "a value typed in is kept");
        assert_eq!(tb["date"], "2024-01-02", "an empty field is filled");
        assert_eq!(tb["drawn_by"], "designer", "a blank field is filled");
        assert_eq!(tb["project_number"], "1042", "a missing field is added");
        assert!(
            tb.get("description").is_none_or(|d| d.trim().is_empty()),
            "a blank import fills nothing"
        );
    }

    #[test]
    fn jumpers_merge_split_and_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let x1 = DeviceTag::parse("-X1")?;
        let x2 = DeviceTag::parse("-X2")?;
        let s = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        let mut m = ProjectMeta::new("t");
        assert!(m.add_jumper(&x1, &s(&["3", "3"])).is_err(), "one terminal is no jumper");
        m.add_jumper(&x1, &s(&["10", "9"]))?;
        m.add_jumper(&x1, &s(&["2", "3"]))?;
        m.add_jumper(&x2, &s(&["1", "2"]))?;
        assert_eq!(m.jumpers.len(), 3);
        // Sharing terminal 3 with one and 9 with the other joins all three of X1's.
        m.add_jumper(&x1, &s(&["3", "9"]))?;
        assert_eq!(m.jumpers.len(), 2);
        assert_eq!(m.jumpers[0].terminals, s(&["2", "3", "9", "10"]), "natural order");
        assert_eq!(m.jumper_of(&DeviceTag::parse("-X1.9")?).map(|j| &j.strip), Some(&x1));
        assert!(m.jumper_of(&DeviceTag::parse("-X1.4")?).is_none());
        assert!(m.jumper_of(&DeviceTag::parse("+A-X1.9")?).is_none(), "another location");
        let text = toml::to_string(&m)?;
        assert!(text.contains("[[jumper]]"), "{text}");
        assert_eq!(toml::from_str::<ProjectMeta>(&text)?.jumpers, m.jumpers);
        // Taking terminals out: a jumper left with one terminal goes.
        assert!(m.remove_from_jumper(&x2, "1"));
        assert_eq!(m.jumpers.len(), 1);
        assert!(!m.remove_from_jumper(&x2, "1"));
        let plain = toml::to_string(&ProjectMeta::new("t"))?;
        assert!(!plain.contains("jumper"), "nothing written without jumpers");
        Ok(())
    }

    #[test]
    fn a_wire_keeps_its_section_colour_and_type_and_older_files_load() -> Result<(), serde_json::Error> {
        let old = r#"{"id":"w1","from":{"item":"s1","pin":"1"},"to":{"at":{"x":1.0,"y":2.0}}}"#;
        let w: Wire = serde_json::from_str(old)?;
        assert!(w.spec.is_empty());
        assert_eq!(
            serde_json::to_string(&w)?,
            old,
            "nothing new is written for a plain wire"
        );
        let mut w = w;
        w.spec = WireSpec {
            section: WireSpec::value(" 1.5 mm² "),
            colour: WireSpec::value("BU"),
            kind: WireSpec::value("H07V-K"),
        };
        let text = serde_json::to_string(&w)?;
        assert!(
            text.contains(r#""section":"1.5 mm²","colour":"BU","type":"H07V-K""#),
            "{text}"
        );
        assert_eq!(serde_json::from_str::<Wire>(&text)?, w);
        assert_eq!(WireSpec::value("  "), None);
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

    #[test]
    fn jumper_terminals_are_one_tag_level_and_keep_a_fixed_order() -> Result<(), String> {
        let x1 = DeviceTag::parse("-X1").map_err(|e| e.to_string())?;
        let mut m = ProjectMeta::new("t");
        let refused = m.add_jumper(&x1, &["1.2".into(), "3.4".into()]);
        assert!(
            refused.as_ref().is_err_and(|e| e.contains("one tag level")),
            "{refused:?}"
        );
        assert!(m.jumpers.is_empty(), "nothing saved");
        m.add_jumper(&x1, &["1".into(), "01".into()])?;
        let once = m.clone();
        m.add_jumper(&x1, &["01".into(), "1".into()])?;
        assert_eq!(m, once, "the same terminals in another order change nothing");
        assert_eq!(m.jumpers[0].terminals, ["01", "1"]);
        Ok(())
    }
}
