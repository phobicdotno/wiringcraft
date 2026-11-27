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
    /// Terminal strips arranged in the terminal strip editor: their terminal order and spare
    /// terminals. A strip that is not listed keeps the usual order (by terminal number).
    #[serde(
        default,
        rename = "strip",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "unique_layouts"
    )]
    pub strips: Vec<StripLayout>,
    /// The project's logo, shown on the cover page and in every title block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<Logo>,
    /// The frame's column count and the title block's layout, shared by every page. Only
    /// written once it differs from the built-in layout.
    #[serde(default, skip_serializing_if = "crate::template::PageTemplate::is_default")]
    pub template: crate::template::PageTemplate,
    /// The project's issued revisions, oldest first: what a page's clouds and revision table
    /// refer to.
    #[serde(default, rename = "revision", skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<crate::revision::Revision>,
    /// Report layouts the user designed, one per report kind: columns, headings, order and
    /// sort ([`crate::report_layout`]).
    #[serde(default, rename = "report", skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<crate::report_layout::ReportLayout>,
    /// Check rules the user wrote, run with the built-in project check
    /// ([`crate::check_rule`]).
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<crate::check_rule::CheckRule>,
    /// Library check rules this project turns off, each as `source:number` or `source:name`
    /// (a library source's `rules.toml`, [`crate::check_rule`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules_off: Vec<String>,
    /// PLC racks and the cards in their slots, laid out on the PLC card overview pages
    /// ([`crate::plc_rack`]).
    #[serde(default, rename = "rack", skip_serializing_if = "Vec::is_empty")]
    pub racks: Vec<crate::plc_rack::PlcRack>,
    /// Fieldbus networks (PROFINET, EtherCAT, ...): their devices, addresses and cables
    /// ([`crate::network`]).
    #[serde(default, rename = "network", skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<crate::network::Network>,
}

/// The project's logo: a JPEG or PNG file in the project folder. The title block shows it in
/// its own box on every page; the cover page shows it larger, where the user put it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Logo {
    /// Path relative to the project folder, with `/` separators, e.g. `pictures/logo.png`.
    pub file: String,
    /// The rectangle the logo is fitted into on the cover page (top-left and bottom-right
    /// corners, in millimetres). `None` puts it in its default place above the project name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<(Point, Point)>,
}

impl Logo {
    /// Folder in a project that holds pictures such as the logo.
    pub const DIR: &'static str = "pictures";

    /// Whether `file` is a plain relative path inside the project (see [`relative_file_is_safe`]).
    pub fn file_is_safe(&self) -> bool {
        relative_file_is_safe(&self.file)
    }

    /// The picture kind of `bytes` as a file extension: `png` or `jpg`; `None` for anything
    /// else.
    pub fn picture_kind(bytes: &[u8]) -> Option<&'static str> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("png")
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some("jpg")
        } else {
            None
        }
    }

    /// Copy the PNG or JPEG picture at `src` into the [`Logo::DIR`] folder of the project in
    /// `dir`, for use as the logo; returns its project path (`pictures/logo.png`). A file there
    /// with the same bytes is used as it is; a different file of the same name is kept and the
    /// copy gets a new name (`logo-2.png`), so an undo can still show the old logo. The copy
    /// always lands inside the project, even when the folder is a link.
    pub fn copy_into(dir: &std::path::Path, src: &std::path::Path) -> Result<String, String> {
        use std::path::Path;
        let bytes = std::fs::read(src).map_err(|e| format!("Cannot read {}: {e}", src.display()))?;
        let ext = Logo::picture_kind(&bytes).ok_or("The logo must be a PNG or JPEG picture.")?;
        let stem: String = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("logo")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let stem = if stem.trim_matches('_').is_empty() {
            "logo".to_owned()
        } else {
            stem
        };
        let folder = dir.join(Logo::DIR);
        std::fs::create_dir_all(&folder).map_err(|e| format!("Cannot create {}: {e}", folder.display()))?;
        // The folder may be a link: the copy must still land inside the project.
        let real = |p: &Path| {
            p.canonicalize()
                .map_err(|e| format!("Cannot find {}: {e}", p.display()))
        };
        if !real(&folder)?.starts_with(real(dir)?) {
            return Err(format!(
                "{} leads out of the project folder; the logo is not copied there.",
                folder.display()
            ));
        }
        let mut n = 1;
        let file = loop {
            let name = if n == 1 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem}-{n}.{ext}")
            };
            let path = folder.join(&name);
            match std::fs::read(&path) {
                Ok(have) if have == bytes => break name,
                Ok(_) => n += 1,
                // Only a name nobody uses is written, and never over a file made meanwhile.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let made = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .and_then(|mut f| std::io::Write::write_all(&mut f, &bytes));
                    match made {
                        Ok(()) => break name,
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                        Err(e) => return Err(format!("Cannot write {}: {e}", path.display())),
                    }
                }
                Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
            }
        };
        Ok(format!("{}/{file}", Logo::DIR))
    }
}

/// Whether `file` is a plain relative path inside the project (no `..`, no absolute path,
/// no backslashes or drive letters). This checks the text only: a link inside the project can
/// still lead out of it, so readers also check where the path really goes
/// (`wc_pdf::read_picture`).
pub fn relative_file_is_safe(f: &str) -> bool {
    !f.is_empty()
        && !f.starts_with('/')
        && !f.contains('\\')
        && !f.contains(':')
        && f.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Terminals of one strip bridged together by a jumper: they form one potential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StoredJumper")]
pub struct Jumper {
    /// The strip, e.g. `-X1`.
    pub strip: DeviceTag,
    /// Terminal numbers on the strip, in strip order, e.g. `["3", "4", "5"]`. At least two.
    pub terminals: Vec<String>,
    /// The jumper's part (article), e.g. a WAGO 2002-402 2-way jumper: counted in the parts
    /// list and written in the WAGO configurator export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
}

/// A [`Jumper`] as read from `project.toml`, before its terminals are checked.
#[derive(Deserialize)]
struct StoredJumper {
    strip: DeviceTag,
    terminals: Vec<String>,
    #[serde(default)]
    part: Option<String>,
}

impl TryFrom<StoredJumper> for Jumper {
    type Error = String;

    /// Refuses a jumper naming a number that cannot be a terminal of its strip, or with fewer
    /// than two different terminals (a hand edit or another tool), as
    /// [`ProjectMeta::add_jumper`] does.
    fn try_from(j: StoredJumper) -> Result<Self, String> {
        let mut distinct: Vec<&String> = Vec::new();
        for t in &j.terminals {
            check_terminal_number(&j.strip, t)?;
            if !distinct.contains(&t) {
                distinct.push(t);
            }
        }
        if distinct.len() < 2 {
            return Err(format!(
                "the jumper on {} needs at least two different terminals of the strip",
                j.strip
            ));
        }
        Ok(Jumper {
            strip: j.strip,
            terminals: j.terminals,
            part: j.part,
        })
    }
}

/// How a terminal strip is arranged on its rail: the order of its terminals and the spare
/// terminals it has beyond those drawn on the schematic. Written by the terminal strip editor;
/// the terminal plan and the WAGO configurator export follow it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StoredStripLayout")]
pub struct StripLayout {
    /// The strip, e.g. `-X1`.
    pub strip: DeviceTag,
    /// Terminal numbers in rail order, spares included. Terminals it does not name (drawn
    /// after the order was set) follow at the end in their usual order; numbers that are
    /// neither drawn nor spare are skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// Spare terminals: on the rail but not drawn, e.g. `["S1"]`. A spare whose number is drawn
    /// later is that drawn terminal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spares: Vec<String>,
}

/// Read the `[[strip]]` layouts, refusing two for the same strip: which one counts would
/// depend on their order in the file.
fn unique_layouts<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<StripLayout>, D::Error> {
    let list = Vec::<StripLayout>::deserialize(d)?;
    let mut seen = std::collections::BTreeSet::new();
    for l in &list {
        if !seen.insert(&l.strip) {
            return Err(serde::de::Error::custom(format!(
                "terminal strip {} is arranged twice in [[strip]]",
                l.strip
            )));
        }
    }
    Ok(list)
}

/// A [`StripLayout`] as read from `project.toml`, before its terminal numbers are checked.
#[derive(Deserialize)]
struct StoredStripLayout {
    strip: DeviceTag,
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    spares: Vec<String>,
}

impl TryFrom<StoredStripLayout> for StripLayout {
    type Error = String;

    /// Refuses a layout naming a number that cannot be a terminal of its strip (a hand edit
    /// or another tool), so it never becomes a device tag the project cannot read back.
    fn try_from(s: StoredStripLayout) -> Result<Self, String> {
        for n in s.order.iter().chain(&s.spares) {
            check_terminal_number(&s.strip, n)?;
        }
        Ok(StripLayout {
            strip: s.strip,
            order: s.order,
            spares: s.spares,
        })
    }
}

impl StripLayout {
    /// The strip's terminals in rail order, each once, with whether it is a spare (not drawn):
    /// `drawn` are the drawn terminal numbers in their usual order. Terminals named in
    /// [`StripLayout::order`] come first in that order; the others follow, drawn ones in their
    /// usual order and then spares in natural order.
    pub fn arrange(&self, drawn: &[String]) -> Vec<(String, bool)> {
        let mut all: Vec<(String, bool)> = Vec::new();
        for n in drawn {
            if !all.iter().any(|(t, _)| t == n) {
                all.push((n.clone(), false));
            }
        }
        let mut spares: Vec<&String> = self.spares.iter().filter(|s| !drawn.contains(s)).collect();
        spares.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b)));
        for s in spares {
            if !all.iter().any(|(t, _)| t == s) {
                all.push((s.clone(), true));
            }
        }
        let mut out: Vec<(String, bool)> = Vec::with_capacity(all.len());
        for n in &self.order {
            if let Some(k) = all.iter().position(|(t, _)| t == n) {
                out.push(all.remove(k));
            }
        }
        out.extend(all);
        out
    }

    /// Whether the layout says nothing: no order and no spares.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.spares.is_empty()
    }
}

/// Every terminal strip of the project with its terminals in rail order, each with whether it
/// is a spare: the drawn terminals as [`terminal_numbers`] numbers them, arranged by the
/// strip's [`StripLayout`], plus strips that only have spares.
pub fn strip_terminals(
    project: &Project,
    role: impl Fn(&str) -> Option<SymbolRole>,
) -> BTreeMap<DeviceTag, Vec<(String, bool)>> {
    let numbers = terminal_numbers(project, role);
    let mut drawn: BTreeMap<DeviceTag, Vec<String>> = BTreeMap::new();
    for (strip, n) in numbers.values() {
        let list = drawn.entry(strip.clone()).or_default();
        if !list.contains(n) {
            list.push(n.clone());
        }
    }
    for list in drawn.values_mut() {
        list.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b)));
    }
    for l in &project.meta.strips {
        drawn.entry(l.strip.clone()).or_default();
    }
    drawn
        .into_iter()
        .map(|(strip, list)| {
            let arranged = match project.meta.strip_layout(&strip) {
                Some(l) => l.arrange(&list),
                None => list.into_iter().map(|n| (n, false)).collect(),
            };
            (strip, arranged)
        })
        .filter(|(_, list)| !list.is_empty())
        .collect()
}

/// Whether `number` can be a terminal of `strip`: one more tag level (`1.2` would be two).
fn check_terminal_number(strip: &DeviceTag, number: &str) -> Result<(), String> {
    let mut tag = strip.clone();
    tag.product.push(number.to_owned());
    if number.is_empty() || DeviceTag::parse(&tag.to_string()).ok().as_ref() != Some(&tag) {
        return Err(format!(
            "{number:?} cannot be a terminal of {strip}: a terminal is one tag level"
        ));
    }
    Ok(())
}

/// The next automatic terminal number after `last`, which it advances; `None` past the largest.
pub fn next_number(last: &mut u64) -> Option<String> {
    *last = last.checked_add(1)?;
    Some(last.to_string())
}

/// [`next_number`], passing over the numbers in `taken` (a strip's spares): a terminal tagged
/// with just the strip never takes a spare's number, and adding a spare past the drawn numbers
/// never renumbers it.
pub fn next_free_number(last: &mut u64, taken: &[String]) -> Option<String> {
    loop {
        let n = next_number(last)?;
        if !taken.contains(&n) {
            return Some(n);
        }
    }
}

/// Keep the jumpers of `project` on their terminals after a change: `before` is the
/// [`terminal_numbers`] from before it. A terminal that is still placed but no longer numbered
/// (its page became an overview) counts as unchanged, not deleted. Returns whether a jumper
/// changed.
pub fn follow_terminals_in(
    project: &mut Project,
    before: &TerminalNumbers,
    role: impl Fn(&str) -> Option<SymbolRole>,
) -> bool {
    if !project.meta.follows_terminals() {
        return false;
    }
    let was = project.meta.clone();
    let mut after = numbers_after(project, before, &role);
    // A spare can only go with its strip to a tag no other device is drawn with there, nor has
    // device data of its own: it would take over that device (its part, its panel place), as
    // Project::add_spare refuses, and leave its own data behind. Judged by what is drawn with
    // the tag, not by the numbers terminals tagged with just the strip happen to get, which
    // change as spares move.
    let drawn: std::collections::BTreeSet<DeviceTag> = project
        .pages
        .iter()
        .flat_map(|p| p.symbols.iter())
        .filter_map(|s| s.tag.clone().filter(|t| !is_terminal(t, role(&s.symbol))))
        .collect();
    // A strip's layout taken along to its new name brings its spares, which can change the
    // numbers of terminals tagged with only the strip there: followed again from the start
    // with the numbers those spares give.
    let mut moves = Vec::new();
    for pass in 0..2 {
        let refuse = |to: &DeviceTag, layout: &StripLayout| {
            layout.spares.iter().any(|n| {
                let mut tag = to.clone();
                tag.product.push(n.clone());
                drawn.contains(&tag) || project.devices.contains_key(&tag)
            })
        };
        // Layouts first: a jumper on a spare that went with its strip goes along too.
        moves = project.meta.follow_layouts_refusing(before, &after, refuse);
        let (mut jb, mut ja) = (before.clone(), after.clone());
        for (from, to) in &moves {
            for n in was.strip_layout(from).map(|l| l.spares.clone()).unwrap_or_default() {
                if before.values().any(|(s, m)| s == from && *m == n) {
                    continue;
                }
                let key = (String::new(), format!("spare {from} {n}"));
                jb.insert(key.clone(), (from.clone(), n.clone()));
                ja.insert(key, (to.clone(), n));
            }
        }
        project.meta.follow_terminals(&jb, &ja);
        let again = numbers_after(project, before, &role);
        if again == after || pass == 1 {
            break;
        }
        project.meta = was.clone();
        after = again;
    }
    // The device data of a spare (its part, its level, its panel place) goes with it to the
    // strip's new name, which has none of its own (a layout is not moved onto such a name).
    for (from, to) in &moves {
        let spares = was.strip_layout(from).map(|l| l.spares.clone()).unwrap_or_default();
        for n in spares {
            let (mut old, mut new) = (from.clone(), to.clone());
            old.product.push(n.clone());
            new.product.push(n);
            if !project.devices.contains_key(&new) {
                project.rename_device(&old, &new);
            }
        }
    }
    project.meta != was || !moves.is_empty()
}

/// The [`terminal_numbers`] of `project` after a change, for following it from `before`: a
/// terminal still placed on a page that became an overview, or a registered spare whose
/// drawing went, keeps its old number.
fn numbers_after(
    project: &Project,
    before: &TerminalNumbers,
    role: &impl Fn(&str) -> Option<SymbolRole>,
) -> TerminalNumbers {
    let mut after = terminal_numbers(project, role);
    let taken: std::collections::BTreeSet<(DeviceTag, String)> = after.values().cloned().collect();
    for (key, was) in before {
        // Only a page turned into an overview keeps its terminals' numbers; a symbol that is no
        // longer a terminal leaves its jumper like a deleted one.
        let on_overview = project
            .page(&key.0)
            .is_some_and(|pg| pg.overview && pg.symbols.iter().any(|s| s.id == key.1));
        // Its old number now belongs to another terminal (strip-only terminals renumbered
        // without the overview page): it is no longer that terminal, and leaves its jumper.
        // A registered spare whose drawing went is still on the rail as a spare: it keeps its
        // place in the jumper, like a terminal that is not drawn.
        let spare = project
            .meta
            .strip_layout(&was.0)
            .is_some_and(|l| l.spares.contains(&was.1));
        if (on_overview || spare) && !after.contains_key(key) && !taken.contains(was) {
            after.insert(key.clone(), was.clone());
        }
    }
    after
}

/// Whether a placed symbol with `tag` and symbol `role` counts as a terminal: a terminal symbol,
/// or a symbol of unknown role tagged with a terminal level (`-X1.4`).
pub fn is_terminal(tag: &DeviceTag, role: Option<SymbolRole>) -> bool {
    let terminal = match role {
        Some(r) => r == SymbolRole::Terminal,
        None => tag.product.len() > 1,
    };
    terminal && !tag.product.is_empty()
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
            if !is_terminal(tag, role(&s.symbol)) {
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
        // Terminals tagged with just the strip are numbered after the largest drawn number,
        // passing over the spares' numbers: a spare's number is taken though nothing draws it.
        let spares = project
            .meta
            .strip_layout(&strip)
            .map(|l| l.spares.as_slice())
            .unwrap_or(&[]);
        let mut next = list
            .iter()
            .filter_map(|t| t.number.as_deref()?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        for t in list {
            // A strip already at the largest number leaves the rest unnumbered.
            let number = match t.number {
                Some(n) => n,
                None => match next_free_number(&mut next, spares) {
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

/// The project fields the editor offers by key and label, in the order the project properties
/// form, the cover page and the title block list them. Other keys are kept and shown after.
pub const TITLE_FIELDS: &[(&str, &str)] = &[
    ("customer", "Customer"),
    ("description", "Description"),
    ("project_number", "Project number"),
    ("plant", "Plant"),
    ("location", "Location"),
    ("drawn_by", "Drawn by"),
    ("checked_by", "Checked by"),
    ("revision", "Revision"),
    ("date", "Date"),
];

/// The title-block fields that usually differ from page to page, which the page properties
/// offer first: a page's own value wins over the project's ([`Page::title_field`]).
pub const PAGE_TITLE_FIELDS: &[&str] = &["revision", "date", "drawn_by", "checked_by"];

/// The label for a title-block field key: its [`TITLE_FIELDS`] label, or the key itself.
pub fn title_field_label(key: &str) -> &str {
    TITLE_FIELDS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(key, |(_, label)| label)
}

/// A location path as the project stores it (`L1.L3`), from what was typed (`+L1.L3` or
/// `L1.L3`). Refused when it is not just the `+` part of a tag.
pub fn location_path(s: &str) -> Result<String, String> {
    let typed = s.trim();
    let s = typed.strip_prefix('+').unwrap_or(typed);
    if s.contains(['-', '=']) {
        return Err(format!("{typed:?} is not a location: give only the + part of a tag"));
    }
    // Errors name the location with its `+`, as it would be written in a tag.
    let shown = format!("+{s}");
    // A tag needs a product, so try it with one. The error gives only the reason, since the
    // tag it was found in is made up here.
    let tag = DeviceTag::parse(&format!("+{s}-X")).map_err(|e| match e {
        crate::Error::Tag(_, why) => format!("{shown:?} is not a location: {why}"),
        e => format!("{shown:?} is not a location: {e}"),
    })?;
    Ok(tag.location.join("."))
}

/// The locations of a list of (path, description) rows, as the project keeps them. Rows that
/// are empty are skipped; a row with a description but no path, a bad path, or a path given
/// twice is refused with the reason.
pub fn location_rows(rows: &[(String, String)]) -> Result<Vec<Location>, String> {
    let mut out: Vec<Location> = Vec::new();
    for (path, description) in rows {
        if path.trim().is_empty() {
            if description.trim().is_empty() {
                continue;
            }
            return Err(format!(
                "the location described as {:?} has no path",
                description.trim()
            ));
        }
        let path = location_path(path)?;
        if out.iter().any(|l| l.path == path) {
            return Err(format!("the location +{path} is listed twice"));
        }
        out.push(Location {
            path,
            description: description.trim().to_owned(),
        });
    }
    sort_locations(&mut out);
    Ok(out)
}

/// Sort locations in natural order of their paths, as tags are listed: `+CAB2` before `+CAB10`.
/// Paths alike in natural order (`L01` and `L1`, `+L1` and `L1`) keep a fixed order by their text.
pub fn sort_locations(locations: &mut [Location]) {
    // A path stored as typed (`+CAB10`) sorts by its normal form (`CAB10`).
    let key = |l: &Location| natural_key(&location_path(&l.path).unwrap_or_else(|_| l.path.clone()));
    locations.sort_by_cached_key(|l| (key(l), l.path.clone()));
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

    /// Add a location or change its description (an empty one clears it). The path may be
    /// typed with or without its `+`. Returns whether anything changed.
    pub fn set_location(&mut self, path: &str, description: &str) -> Result<bool, String> {
        let path = location_path(path)?;
        let description = description.trim().to_owned();
        // Stored paths may be as typed (`+L1`), so compare their normal form.
        let found = self
            .locations
            .iter_mut()
            .find(|l| location_path(&l.path).is_ok_and(|p| p == path));
        match found {
            Some(l) if l.path == path && l.description == description => Ok(false),
            Some(l) => {
                l.path = path;
                l.description = description;
                sort_locations(&mut self.locations);
                Ok(true)
            }
            None => {
                self.locations.push(Location { path, description });
                sort_locations(&mut self.locations);
                Ok(true)
            }
        }
    }

    /// Remove a location from the list (its devices keep their tags). A path stored as it was
    /// typed is found too. When nothing goes, the reason: not a location, or not in the list.
    pub fn remove_location(&mut self, path: &str) -> Result<(), String> {
        let parsed = location_path(path);
        let raw = path.trim();
        let before = self.locations.len();
        // The normal form first, then the path as typed (an entry stored as `+L1`).
        if let Ok(key) = &parsed {
            self.locations.retain(|l| l.path != *key);
        }
        if self.locations.len() == before {
            self.locations.retain(|l| l.path != raw);
        }
        if self.locations.len() != before {
            return Ok(());
        }
        match parsed {
            Ok(p) => Err(format!("no location +{p}")),
            Err(why) => Err(why),
        }
    }

    /// Add a location, or fill in the description of one the project already has. A description
    /// already there is kept. Paths match by their normal form, so a location stored as typed
    /// (`+L1`) is found by `L1`. Returns whether anything changed.
    pub fn merge_location(&mut self, mut loc: Location) -> bool {
        let key = location_path(&loc.path).unwrap_or_else(|_| loc.path.clone());
        let found = self
            .locations
            .iter_mut()
            .find(|l| l.path == loc.path || location_path(&l.path).is_ok_and(|p| p == key));
        loc.path = key;
        match found {
            Some(have) if have.description.is_empty() && !loc.description.is_empty() => {
                have.description = loc.description;
                true
            }
            Some(_) => false,
            None => {
                self.locations.push(loc);
                sort_locations(&mut self.locations);
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
            strips: Vec::new(),
            logo: None,
            template: crate::template::PageTemplate::default(),
            revisions: Vec::new(),
            reports: Vec::new(),
            rules: Vec::new(),
            rules_off: Vec::new(),
            racks: Vec::new(),
            networks: Vec::new(),
        }
    }

    /// Jumper `terminals` of `strip` together. Jumpers of the strip that share a terminal with
    /// them become one. Refused when fewer than two different terminals result, or when a
    /// terminal cannot be the last level of a tag of the strip (`1.2` would be two levels).
    pub fn add_jumper(&mut self, strip: &DeviceTag, terminals: &[String]) -> Result<(), String> {
        self.add_jumper_with(strip, terminals, None)
    }

    /// Like [`ProjectMeta::add_jumper`], with the jumper's part. Without one, a jumper it takes
    /// in gives its part (the first that has one, in strip order).
    fn add_jumper_with(
        &mut self,
        strip: &DeviceTag,
        terminals: &[String],
        mut part: Option<String>,
    ) -> Result<(), String> {
        let mut set: Vec<String> = Vec::new();
        for t in terminals.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            check_terminal_number(strip, t)?;
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
                if part.is_none() {
                    part = j.part;
                }
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
            part,
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
        let terminal = terminal.trim();
        let mut changed = false;
        for j in self.jumpers.iter_mut().filter(|j| &j.strip == strip) {
            let before = j.terminals.len();
            j.terminals.retain(|t| t != terminal);
            changed |= j.terminals.len() != before;
        }
        self.jumpers.retain(|j| j.terminals.len() >= 2);
        changed
    }

    /// Keep every jumper on the same placed terminals after the drawing changed: `before` and
    /// `after` are [`terminal_numbers`] of the project before and after the change. A terminal
    /// that got another number (renamed, or a strip-only terminal moved past another) keeps its
    /// place in the jumper under the new number; when all of a jumper's terminals went to one
    /// other strip (the strip was renamed) the jumper goes with them; a terminal that went to
    /// another strip on its own, or was deleted, leaves the jumper. Terminals that were not drawn
    /// stay as they are. Returns whether anything changed.
    pub fn follow_terminals(&mut self, before: &TerminalNumbers, after: &TerminalNumbers) -> bool {
        /// What became of a jumpered terminal.
        enum Went {
            NotDrawn,
            Deleted,
            To(DeviceTag, String),
        }
        let old = std::mem::take(&mut self.jumpers);
        for j in &old {
            let went: Vec<(String, Went)> = j
                .terminals
                .iter()
                .map(|t| {
                    let keys: Vec<&(String, String)> = before
                        .iter()
                        .filter(|(_, (s, n))| *s == j.strip && n == t)
                        .map(|(k, _)| k)
                        .collect();
                    if keys.is_empty() {
                        return (t.clone(), Went::NotDrawn);
                    }
                    // A placement that kept its number means the terminal is still there.
                    let same = (j.strip.clone(), t.clone());
                    let to = if keys.iter().any(|k| after.get(*k) == Some(&same)) {
                        Some(same)
                    } else {
                        keys.iter().find_map(|k| after.get(*k).cloned())
                    };
                    (t.clone(), to.map_or(Went::Deleted, |(s, n)| Went::To(s, n)))
                })
                .collect();
            let mut strips: std::collections::BTreeSet<&DeviceTag> = went
                .iter()
                .filter_map(|(_, w)| match w {
                    Went::To(s, _) => Some(s),
                    _ => None,
                })
                .collect();
            // Terminals that are not drawn stay on their strip, so the jumper only moves when
            // every member went to the same other strip.
            if went.iter().any(|(_, w)| matches!(w, Went::NotDrawn)) {
                strips.insert(&j.strip);
            }
            let strip = match strips.iter().next() {
                Some(only) if strips.len() == 1 => (*only).clone(),
                _ => j.strip.clone(),
            };
            let terminals: Vec<String> = went
                .into_iter()
                .filter_map(|(t, w)| match w {
                    Went::NotDrawn => Some(t),
                    Went::To(s, n) => (s == strip).then_some(n),
                    Went::Deleted => None,
                })
                .collect();
            // One terminal left is no jumper.
            let mut distinct = terminals.clone();
            distinct.sort();
            distinct.dedup();
            if distinct.len() < 2 {
                continue;
            }
            // Jumpers that now share a terminal become one. A number that cannot be a terminal
            // of the strip is refused there; the jumper is then kept as it was, not dropped.
            if self.add_jumper_with(&strip, &terminals, j.part.clone()).is_err() {
                self.jumpers.push(j.clone());
            }
        }
        self.jumpers != old
    }

    /// Give the jumper that holds `terminal` of `strip` a part, or none. Returns whether that
    /// changed anything; `Err` when the terminal is in no jumper.
    pub fn set_jumper_part(&mut self, strip: &DeviceTag, terminal: &str, part: Option<&str>) -> Result<bool, String> {
        let terminal = terminal.trim();
        let j = self
            .jumpers
            .iter_mut()
            .find(|j| &j.strip == strip && j.terminals.iter().any(|t| t == terminal))
            .ok_or_else(|| format!("no jumper of {strip} has terminal {terminal}"))?;
        let part = part.map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned);
        if j.part == part {
            return Ok(false);
        }
        j.part = part;
        Ok(true)
    }

    /// Whether anything names terminals by number (jumpers, or a strip's rail order and spares),
    /// so a change that renames, renumbers or deletes terminals must be followed.
    pub fn follows_terminals(&self) -> bool {
        !self.jumpers.is_empty() || self.strips.iter().any(|l| !l.is_empty())
    }

    /// Keep each strip's rail order on the terminals it named, after a change (`before` to
    /// `after`, as [`ProjectMeta::follow_terminals`]): a renamed or renumbered terminal keeps
    /// its place under its new number, a deleted one leaves the order, and a strip whose drawn
    /// terminals all went to one other strip (the strip renamed) takes its layout along. Spares
    /// and numbers that were not drawn stay. Returns whether a layout changed.
    pub fn follow_layouts(&mut self, before: &TerminalNumbers, after: &TerminalNumbers) -> bool {
        let old = self.strips.clone();
        self.follow_layouts_moving(before, after);
        self.strips != old
    }

    /// [`ProjectMeta::follow_layouts`], returning the layouts taken to another strip, as
    /// (old strip, new strip). Two strips whose terminals all went to the same new one do not
    /// both take their layout there: the first (in strip order) does, the other stays.
    pub fn follow_layouts_moving(
        &mut self,
        before: &TerminalNumbers,
        after: &TerminalNumbers,
    ) -> Vec<(DeviceTag, DeviceTag)> {
        self.follow_layouts_refusing(before, after, |_, _| false)
    }

    /// [`ProjectMeta::follow_layouts_moving`], where a layout `refuse(new strip, layout)` says
    /// may not go to the new strip stays under its old one.
    pub fn follow_layouts_refusing(
        &mut self,
        before: &TerminalNumbers,
        after: &TerminalNumbers,
        refuse: impl Fn(&DeviceTag, &StripLayout) -> bool,
    ) -> Vec<(DeviceTag, DeviceTag)> {
        let old = self.strips.clone();
        let mut moves: Vec<(DeviceTag, DeviceTag)> = Vec::new();
        for layout in std::mem::take(&mut self.strips) {
            let strip = &layout.strip;
            // What became of each drawn terminal of the strip: kept, another number or strip,
            // or deleted (None).
            let went = |n: &str| -> Option<Option<(DeviceTag, String)>> {
                let keys: Vec<&(String, String)> = before
                    .iter()
                    .filter(|(_, (s, m))| s == strip && m == n)
                    .map(|(k, _)| k)
                    .collect();
                if keys.is_empty() {
                    return None;
                }
                let same = (strip.clone(), n.to_owned());
                if keys.iter().any(|k| after.get(*k) == Some(&same)) {
                    return Some(Some(same));
                }
                Some(keys.iter().find_map(|k| after.get(*k).cloned()))
            };
            // The strip renamed: every drawn terminal of it went to one other strip, which has
            // no layout of its own.
            let drawn: Vec<Option<(DeviceTag, String)>> = before
                .iter()
                .filter(|(_, (s, _))| s == strip)
                .map(|(k, _)| after.get(k).cloned())
                .collect();
            let mut targets: Vec<&DeviceTag> = drawn.iter().flatten().map(|(s, _)| s).collect();
            targets.sort();
            targets.dedup();
            let moved_to = match (drawn.iter().all(Option::is_some), targets.as_slice()) {
                (true, [only])
                    if *only != strip
                        && !old.iter().any(|l| &l.strip == *only)
                        && !moves.iter().any(|(_, to)| to == *only)
                        && !refuse(only, &layout) =>
                {
                    Some((*only).clone())
                }
                _ => None,
            };
            if let Some(to) = &moved_to {
                moves.push((strip.clone(), to.clone()));
            }
            let home = moved_to.clone().unwrap_or_else(|| strip.clone());
            let mut order: Vec<String> = Vec::new();
            for n in &layout.order {
                let number = match went(n) {
                    Some(Some((s, m))) if s == home => Some(m),
                    // A spare keeps its place unless its drawing got another number here.
                    _ if layout.spares.contains(n) => Some(n.clone()),
                    None => Some(n.clone()),
                    // Deleted, or gone to another strip.
                    Some(_) => None,
                };
                if let Some(m) = number
                    && !order.contains(&m)
                {
                    order.push(m);
                }
            }
            let layout = StripLayout {
                strip: home.clone(),
                order,
                spares: layout.spares,
            };
            self.with_layout(&home, |l| *l = layout);
        }
        self.drop_empty_layouts();
        moves
    }

    /// How `strip` is arranged in the terminal strip editor, if it was.
    pub fn strip_layout(&self, strip: &DeviceTag) -> Option<&StripLayout> {
        self.strips.iter().find(|l| &l.strip == strip)
    }

    /// Run `f` on the layout of `strip`, made when it has none; layouts stay in strip order.
    fn with_layout<R>(&mut self, strip: &DeviceTag, f: impl FnOnce(&mut StripLayout) -> R) -> R {
        let mut layout = match self.strips.iter().position(|l| &l.strip == strip) {
            Some(k) => self.strips.remove(k),
            None => StripLayout {
                strip: strip.clone(),
                order: Vec::new(),
                spares: Vec::new(),
            },
        };
        let r = f(&mut layout);
        // Kept in strip order: the layout goes back where its strip sorts.
        let key = |l: &StripLayout| {
            let t = l.strip.to_string();
            (natural_key(&t), t)
        };
        let at = self.strips.partition_point(|l| key(l) < key(&layout));
        self.strips.insert(at, layout);
        r
    }

    /// Drop layouts that say nothing any more.
    fn drop_empty_layouts(&mut self) {
        self.strips.retain(|l| !l.is_empty());
    }

    /// Put the terminals of `strip` in rail `order` (terminal numbers, spares included). An
    /// empty order goes back to the usual order. Refused for a number given twice or one that
    /// cannot be a terminal of the strip. Returns whether anything changed.
    pub fn set_terminal_order(&mut self, strip: &DeviceTag, order: &[String]) -> Result<bool, String> {
        let mut list: Vec<String> = Vec::new();
        for n in order.iter().map(|n| n.trim()) {
            check_terminal_number(strip, n)?;
            if list.iter().any(|t| t == n) {
                return Err(format!("terminal {n} of {strip} is given twice"));
            }
            list.push(n.to_owned());
        }
        let before = self.strips.clone();
        self.with_layout(strip, |l| l.order = list);
        self.drop_empty_layouts();
        Ok(self.strips != before)
    }

    /// Add spare terminal `number` to `strip`, placed right after terminal `after` when given
    /// (`current` is the strip's rail order, see [`strip_terminals`]), else at the end. Refused
    /// when the strip already has that terminal or the number cannot be a terminal of it.
    pub fn add_spare(
        &mut self,
        strip: &DeviceTag,
        number: &str,
        after: Option<&str>,
        current: &[(String, bool)],
    ) -> Result<(), String> {
        let number = number.trim();
        check_terminal_number(strip, number)?;
        if current.iter().any(|(t, _)| t == number)
            || self
                .strip_layout(strip)
                .is_some_and(|l| l.spares.iter().any(|s| s == number))
        {
            return Err(format!("{strip} already has terminal {number}"));
        }
        let mut order: Vec<String> = current.iter().map(|(t, _)| t.clone()).collect();
        let at = match after {
            Some(a) => order
                .iter()
                .position(|t| t == a)
                .map(|k| k + 1)
                .ok_or_else(|| format!("{strip} has no terminal {a}"))?,
            None => order.len(),
        };
        order.insert(at, number.to_owned());
        self.with_layout(strip, |l| {
            l.spares.push(number.to_owned());
            l.order = order;
        });
        Ok(())
    }

    /// Take spare terminal `number` off `strip`, and out of its jumper; `current` is the strip's
    /// rail order (see [`strip_terminals`]). A spare that has been drawn since is that drawn
    /// terminal: only its spare entry goes, its place and jumper stay. Returns whether it was
    /// a spare.
    pub fn remove_spare(&mut self, strip: &DeviceTag, number: &str, current: &[(String, bool)]) -> bool {
        let drawn = current.iter().any(|(t, spare)| t == number && !spare);
        let Some(l) = self.strips.iter_mut().find(|l| &l.strip == strip) else {
            return false;
        };
        let before = l.spares.len();
        l.spares.retain(|s| s != number);
        if l.spares.len() == before {
            return false;
        }
        if !drawn {
            l.order.retain(|t| t != number);
        }
        self.drop_empty_layouts();
        if !drawn {
            self.remove_from_jumper(strip, number);
        }
        true
    }

    /// Move terminal `number` of `strip` to `to` (0 is first) in its rail order; `current` is
    /// the strip's rail order (see [`strip_terminals`]). Returns whether anything changed.
    pub fn move_terminal(
        &mut self,
        strip: &DeviceTag,
        number: &str,
        to: usize,
        current: &[(String, bool)],
    ) -> Result<bool, String> {
        let mut order: Vec<String> = current.iter().map(|(t, _)| t.clone()).collect();
        let from = order
            .iter()
            .position(|t| t == number)
            .ok_or_else(|| format!("{strip} has no terminal {number}"))?;
        let n = order.remove(from);
        order.insert(to.min(order.len()), n);
        if order.iter().eq(current.iter().map(|(t, _)| t)) {
            return Ok(false);
        }
        self.set_terminal_order(strip, &order)
    }

    /// The jumper a terminal (`-X1.4`) is in, if any.
    pub fn jumper_of(&self, terminal: &DeviceTag) -> Option<&Jumper> {
        self.jumpers.iter().find(|j| j.terminal_of(terminal).is_some())
    }
}

/// A page's sheet: an ISO A size, landscape or portrait, or a size of its own. Stored by
/// name (`A3`, `A4Portrait`) or as `{ Custom = { width, height } }`. A stored custom size
/// outside [`PageSize::MIN_MM`] to [`PageSize::MAX_MM`] is refused when read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(try_from = "StoredPageSize")]
pub enum PageSize {
    A4,
    #[default]
    A3,
    A2,
    A1,
    A0,
    A4Portrait,
    A3Portrait,
    A2Portrait,
    A1Portrait,
    A0Portrait,
    /// Width and height in whole millimetres, each from [`PageSize::MIN_MM`] to
    /// [`PageSize::MAX_MM`].
    Custom {
        width: u32,
        height: u32,
    },
}

/// [`PageSize`] as stored, before a custom size is checked.
#[derive(Deserialize)]
enum StoredPageSize {
    A4,
    A3,
    A2,
    A1,
    A0,
    A4Portrait,
    A3Portrait,
    A2Portrait,
    A1Portrait,
    A0Portrait,
    Custom { width: u32, height: u32 },
}

impl TryFrom<StoredPageSize> for PageSize {
    type Error = String;

    fn try_from(stored: StoredPageSize) -> Result<Self, String> {
        Ok(match stored {
            StoredPageSize::A4 => PageSize::A4,
            StoredPageSize::A3 => PageSize::A3,
            StoredPageSize::A2 => PageSize::A2,
            StoredPageSize::A1 => PageSize::A1,
            StoredPageSize::A0 => PageSize::A0,
            StoredPageSize::A4Portrait => PageSize::A4Portrait,
            StoredPageSize::A3Portrait => PageSize::A3Portrait,
            StoredPageSize::A2Portrait => PageSize::A2Portrait,
            StoredPageSize::A1Portrait => PageSize::A1Portrait,
            StoredPageSize::A0Portrait => PageSize::A0Portrait,
            StoredPageSize::Custom { width, height } => PageSize::custom(width, height)?,
        })
    }
}

impl PageSize {
    /// The named sizes, in the order lists offer them.
    pub const NAMED: [PageSize; 10] = [
        PageSize::A4,
        PageSize::A4Portrait,
        PageSize::A3,
        PageSize::A3Portrait,
        PageSize::A2,
        PageSize::A2Portrait,
        PageSize::A1,
        PageSize::A1Portrait,
        PageSize::A0,
        PageSize::A0Portrait,
    ];
    /// The smallest side of a custom sheet, in millimetres.
    pub const MIN_MM: u32 = 100;
    /// The largest side of a custom sheet, in millimetres (a little over A0's long side).
    pub const MAX_MM: u32 = 2000;

    /// Width and height in millimetres, as the sheet is drawn.
    pub fn mm(self) -> (f64, f64) {
        let landscape = |(w, h): (f64, f64)| (w, h);
        let portrait = |(w, h): (f64, f64)| (h, w);
        match self {
            PageSize::A4 => landscape((297.0, 210.0)),
            PageSize::A3 => landscape((420.0, 297.0)),
            PageSize::A2 => landscape((594.0, 420.0)),
            PageSize::A1 => landscape((841.0, 594.0)),
            PageSize::A0 => landscape((1189.0, 841.0)),
            PageSize::A4Portrait => portrait((297.0, 210.0)),
            PageSize::A3Portrait => portrait((420.0, 297.0)),
            PageSize::A2Portrait => portrait((594.0, 420.0)),
            PageSize::A1Portrait => portrait((841.0, 594.0)),
            PageSize::A0Portrait => portrait((1189.0, 841.0)),
            PageSize::Custom { width, height } => (f64::from(width), f64::from(height)),
        }
    }

    pub fn is_portrait(self) -> bool {
        let (w, h) = self.mm();
        h > w
    }

    /// The name people read: `A4 landscape`, `A3 portrait`, `500 x 700 mm`.
    pub fn label(self) -> String {
        let iso = |n: &str, portrait: bool| format!("{n} {}", if portrait { "portrait" } else { "landscape" });
        match self {
            PageSize::A4 => iso("A4", false),
            PageSize::A3 => iso("A3", false),
            PageSize::A2 => iso("A2", false),
            PageSize::A1 => iso("A1", false),
            PageSize::A0 => iso("A0", false),
            PageSize::A4Portrait => iso("A4", true),
            PageSize::A3Portrait => iso("A3", true),
            PageSize::A2Portrait => iso("A2", true),
            PageSize::A1Portrait => iso("A1", true),
            PageSize::A0Portrait => iso("A0", true),
            PageSize::Custom { width, height } => format!("{width} x {height} mm"),
        }
    }

    /// The short name commands take: `A4`, `A4-portrait`, `500x700`.
    pub fn code(self) -> String {
        match self {
            PageSize::Custom { width, height } => format!("{width}x{height}"),
            named => {
                let label = named.label();
                let (name, way) = label.split_once(' ').unwrap_or((label.as_str(), ""));
                if way == "portrait" {
                    format!("{name}-portrait")
                } else {
                    name.to_owned()
                }
            }
        }
    }

    /// Read a size as people type it: `A4`, `a4 portrait`, `A4-portrait`, `A4P`, `A3
    /// landscape`, or a custom `500x700` (millimetres, width first). The error says what is
    /// accepted.
    pub fn parse(text: &str) -> Result<PageSize, String> {
        let lower = text.trim().to_ascii_lowercase();
        // Hyphens join the words of a name (`A4-portrait`); a custom size keeps its signs.
        let t = lower.replace(['-', '_'], " ");
        let words: Vec<&str> = t.split_whitespace().collect();
        let (name, portrait) = match words.as_slice() {
            [one] => match one.strip_suffix('p') {
                Some(prefix) if prefix.starts_with('a') => (prefix, true),
                _ => (*one, false),
            },
            [one, "portrait"] => (*one, true),
            [one, "landscape"] => (*one, false),
            _ => ("", false),
        };
        let named = match (name, portrait) {
            ("a4", false) => Some(PageSize::A4),
            ("a3", false) => Some(PageSize::A3),
            ("a2", false) => Some(PageSize::A2),
            ("a1", false) => Some(PageSize::A1),
            ("a0", false) => Some(PageSize::A0),
            ("a4", true) => Some(PageSize::A4Portrait),
            ("a3", true) => Some(PageSize::A3Portrait),
            ("a2", true) => Some(PageSize::A2Portrait),
            ("a1", true) => Some(PageSize::A1Portrait),
            ("a0", true) => Some(PageSize::A0Portrait),
            _ => None,
        };
        if let Some(size) = named {
            return Ok(size);
        }
        let bad = || {
            format!(
                "size {text:?} is not A4, A3, A2, A1 or A0 (add \"portrait\" for an upright sheet) or a custom WIDTHxHEIGHT in millimetres"
            )
        };
        // Spaces around `x` (or `×`) and before `mm` are fine, and each side may carry its own
        // `mm` (`500mm x 700mm`); spaces inside a number are not.
        let dims = lower.replace('×', "x");
        let (w, h) = dims.split_once('x').ok_or_else(bad)?;
        let side = |name: &str, v: &str| {
            let v = v.trim();
            let v = v.strip_suffix("mm").unwrap_or(v);
            v.trim().parse::<u32>().map_err(|e| {
                format!(
                    "size {text:?}: the {name} {:?} is not a whole number of millimetres ({e}); each side is {} to {} mm",
                    v.trim(),
                    Self::MIN_MM,
                    Self::MAX_MM
                )
            })
        };
        let (width, height) = (side("width", w)?, side("height", h)?);
        PageSize::custom(width, height)
    }

    /// A sheet of its own size, in whole millimetres.
    pub fn custom(width: u32, height: u32) -> Result<PageSize, String> {
        for side in [width, height] {
            if !(Self::MIN_MM..=Self::MAX_MM).contains(&side) {
                return Err(format!(
                    "a sheet side of {side} mm is outside {} to {} mm",
                    Self::MIN_MM,
                    Self::MAX_MM
                ));
            }
        }
        Ok(PageSize::Custom { width, height })
    }

    /// The reference column (1-based, 1 to [`FRAME_COLUMNS`]) that `x` millimetres falls into,
    /// with the built-in column count. A project's own count is [`Project::column_of`].
    pub fn column_of(self, x: f64) -> usize {
        self.column_in(FRAME_COLUMNS, x)
    }

    /// The reference column (1-based, 1 to `columns`) that `x` millimetres falls into when the
    /// frame is divided into `columns` columns (at least one).
    pub fn column_in(self, columns: usize, x: f64) -> usize {
        let columns = columns.max(1);
        let (w, _) = self.mm();
        let col_w = (w - 2.0 * FRAME_MARGIN) / columns as f64;
        let col = ((x - FRAME_MARGIN) / col_w).floor();
        if col.is_nan() {
            return 1;
        }
        (col as i64).saturating_add(1).clamp(1, columns as i64) as usize
    }

    /// How many reference rows run down the sides of the frame, by the sheet's height: A to F
    /// up to A3 landscape (297 mm), A to H up to A2 landscape (420 mm), A to K up to A1
    /// landscape (594 mm), A to M above. Fixed per sheet size, so a reference never moves.
    pub fn frame_rows(self) -> usize {
        let (_, h) = self.mm();
        if h <= 297.0 {
            6
        } else if h <= 420.0 {
            8
        } else if h <= 594.0 {
            10
        } else {
            12
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
    /// The project's cover page: its drawing area shows the project's name and fields (from
    /// [`ProjectMeta`]), so editing them once updates the cover and every title block.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cover: bool,
    /// Title-block fields of this page alone (its revision, date, checked by, ...). On this
    /// page's title block they win over the project's fields of the same name
    /// ([`ProjectMeta::title_block`]); fields the page does not set come from the project.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title_block: BTreeMap<String, String>,
    /// Clouds drawn around what a revision changed on this page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clouds: Vec<crate::revision::RevisionCloud>,
    /// Where this page shows the project's revision table (its top-left corner), if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_table: Option<Point>,
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

    /// Whether `file` is a plain relative path inside the project (see [`relative_file_is_safe`]).
    pub fn file_is_safe(&self) -> bool {
        relative_file_is_safe(&self.file)
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
            cover: false,
            title_block: BTreeMap::new(),
            clouds: Vec::new(),
            revision_table: None,
        }
    }

    /// The keys of this page's own title-block fields as a form lists them: the per-page ones
    /// ([`PAGE_TITLE_FIELDS`]) first, then any other key the page has.
    pub fn title_field_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = PAGE_TITLE_FIELDS.iter().map(|k| (*k).to_owned()).collect();
        for k in self.title_block.keys() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        keys
    }

    /// Set this page's own title-block field `key`. A blank value removes it, so the project's
    /// value shows on this page again. Returns whether the page changed.
    pub fn set_title_field(&mut self, key: &str, value: &str) -> bool {
        let value = value.trim();
        if value.is_empty() {
            return self.title_block.remove(key).is_some();
        }
        if self.title_block.get(key).is_some_and(|v| v == value) {
            return false;
        }
        self.title_block.insert(key.to_owned(), value.to_owned());
        true
    }

    /// A title-block field as this page shows it: the page's own value when it has one that
    /// is not blank, else the project's. Empty when neither has it.
    pub fn title_field<'a>(&'a self, meta: &'a ProjectMeta, key: &str) -> &'a str {
        self.title_block
            .get(key)
            .filter(|v| !v.trim().is_empty())
            .or_else(|| meta.title_block.get(key))
            .map_or("", String::as_str)
    }

    pub fn symbol(&self, id: &str) -> Option<&PlacedSymbol> {
        self.symbols.iter().find(|s| s.id == id)
    }

    /// Take the symbols named in `symbols` and the wires named in `wires` off this page, and
    /// with them every wire that ends on a pin of one of those symbols. Returns the removed ids:
    /// symbols first, then wires, each in page order. Ids not on the page are skipped.
    pub fn remove_symbols_and_wires(&mut self, symbols: &[&str], wires: &[&str]) -> Vec<String> {
        let gone_symbols: Vec<String> = self
            .symbols
            .iter()
            .filter(|s| symbols.contains(&s.id.as_str()))
            .map(|s| s.id.clone())
            .collect();
        // Only symbols that are on the page take their wires along: a wire that ends on a
        // missing id stays.
        let on_removed = |end: &Endpoint| matches!(end, Endpoint::Pin { item, .. } if gone_symbols.contains(item));
        // Each wire is judged on its own ends, so a second wire that only shares an id stays.
        let mut gone_wires = Vec::new();
        self.wires.retain(|w| {
            let gone = wires.contains(&w.id.as_str()) || on_removed(&w.from) || on_removed(&w.to);
            if gone {
                gone_wires.push(w.id.clone());
            }
            !gone
        });
        self.symbols.retain(|s| !gone_symbols.contains(&s.id));
        gone_symbols.into_iter().chain(gone_wires).collect()
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
    /// The main part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// More parts that belong to the device: an auxiliary contact block, a marker, an end plate.
    /// Old files without it read as none, and a device without any is saved as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accessories: Vec<DevicePart>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

/// One more part of a device, with how many of it the device needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePart {
    pub part: String,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub qty: u32,
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

impl Device {
    /// Every part of the device with its quantity: the main part (once), then the accessories.
    pub fn all_parts(&self) -> Vec<(&str, u32)> {
        self.part
            .as_deref()
            .map(|p| (p, 1))
            .into_iter()
            .chain(self.accessories.iter().map(|a| (a.part.as_str(), a.qty)))
            .collect()
    }

    /// Whether the device uses part `id`, as its main part or as an accessory.
    pub fn uses_part(&self, id: &str) -> bool {
        self.part.as_deref() == Some(id) || self.accessories.iter().any(|a| a.part == id)
    }
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

/// One wire end on a pin its symbol does not have: see [`Project::dangling_ends`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DanglingEnd {
    pub page: String,
    pub wire: String,
    /// The wire's `to` end (else its `from` end).
    pub to: bool,
    pub item: String,
    pub pin: String,
}

/// Why a page cannot have a number ([`Project::free_page_number`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageNumberError {
    /// The number is empty or blank.
    Empty,
    /// Page `page` (its id) already has `number`.
    Taken { number: String, page: String },
}

impl std::fmt::Display for PageNumberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("the page number cannot be empty"),
            Self::Taken { number, page } => write!(f, "page {page:?} already has number {number:?}"),
        }
    }
}

impl std::error::Error for PageNumberError {}

impl Project {
    /// Add spare terminal `number` to `strip` ([`ProjectMeta::add_spare`]), refused also when
    /// its tag (`-X1.S1`) is drawn as a device that is not a terminal of the strip, or names
    /// the cable of a wire core (`-X1.W1`): the spare would take over that device's part.
    pub fn add_spare(
        &mut self,
        strip: &DeviceTag,
        number: &str,
        after: Option<&str>,
        current: &[(String, bool)],
    ) -> Result<(), String> {
        let number = number.trim();
        if let Ok(tag) = DeviceTag::parse(&format!("{strip}.{number}"))
            && !current.iter().any(|(t, _)| t == number)
            && self.is_drawn(&tag)
        {
            return Err(format!(
                "{tag} is drawn as a device that is not a terminal of {strip}; pick another number"
            ));
        }
        if let Ok(tag) = DeviceTag::parse(&format!("{strip}.{number}"))
            && self.is_cable_of_a_core(&tag)
        {
            return Err(format!("{tag} is the cable of a wire core; pick another number"));
        }
        self.meta.add_spare(strip, number, after, current)
    }

    /// Take spare terminal `number` off `strip` ([`ProjectMeta::remove_spare`]). A spare that is
    /// not drawn anywhere also loses its device entry (its part, level and the like), so the bill
    /// of materials no longer counts it; a drawn one, or one a wire core names as its cable,
    /// keeps it. Returns whether it was a spare.
    pub fn remove_spare(&mut self, strip: &DeviceTag, number: &str, current: &[(String, bool)]) -> bool {
        let number = number.trim();
        if !self.meta.remove_spare(strip, number, current) {
            return false;
        }
        if let Ok(tag) = DeviceTag::parse(&format!("{strip}.{number}"))
            && !self.is_drawn(&tag)
            && !self.is_cable_of_a_core(&tag)
        {
            self.devices.remove(&tag);
        }
        true
    }

    /// The device entry of terminal `number` of `strip` that is not a spare (any more): for
    /// taking a spare off a second time, when its entry is still there (restored by hand, say).
    /// `current` is the strip's rail order (see [`strip_terminals`]), so a terminal drawn with
    /// just the strip's tag and numbered on the rail counts as drawn. An entry nothing uses is
    /// removed (`Ok(true)`); one that is drawn or names the cable of a wire core stays, and the
    /// error says why. `Ok(false)` when there is no such entry. Refused for a number that cannot
    /// be a terminal of the strip (`2.3`), which names another strip's terminal.
    pub fn remove_unused_terminal_entry(
        &mut self,
        strip: &DeviceTag,
        number: &str,
        current: &[(String, bool)],
    ) -> Result<bool, String> {
        let number = number.trim();
        check_terminal_number(strip, number)?;
        let tag = DeviceTag::parse(&format!("{strip}.{number}")).map_err(|e| e.to_string())?;
        if !self.devices.contains_key(&tag) {
            return Ok(false);
        }
        if self.is_drawn(&tag) || current.iter().any(|(t, spare)| t == number && !spare) {
            return Err(format!("{tag} is drawn, so its device entry stays"));
        }
        if self.is_cable_of_a_core(&tag) {
            return Err(format!("{tag} is the cable of a wire core, so its device entry stays"));
        }
        self.devices.remove(&tag);
        Ok(true)
    }

    /// Whether a symbol on a page carries `tag`.
    fn is_drawn(&self, tag: &DeviceTag) -> bool {
        self.pages
            .iter()
            .any(|p| p.symbols.iter().any(|s| s.tag.as_ref() == Some(tag)))
    }

    /// Whether a wire on a page is a core of cable `tag`.
    fn is_cable_of_a_core(&self, tag: &DeviceTag) -> bool {
        self.pages.iter().any(|p| {
            p.wires
                .iter()
                .any(|w| w.cable.as_ref().is_some_and(|c| &c.cable == tag))
        })
    }

    /// Wire ends on pins of symbol `id` that `symbol` does not have: `page 3 -K1:13` each. What
    /// replacing the symbol with that version would leave unconnected.
    pub fn dangling_pins(&self, id: &str, symbol: &crate::Symbol) -> Vec<String> {
        let mut out: Vec<String> = self
            .dangling_ends(id, symbol)
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The wire ends [`Self::dangling_pins`] reports, one by one: keyed by what the end is
    /// (page id, wire id, which end, and the item and pin it names), so the same end is
    /// recognised after a device is renamed and two wires on one missing pin are two ends. With
    /// the end's label for a message. Every end is listed: two wires that share an id (as a
    /// replaced page may have them) give two entries alike.
    pub fn dangling_ends(&self, id: &str, symbol: &crate::Symbol) -> Vec<(DanglingEnd, String)> {
        let mut out = Vec::new();
        for page in &self.pages {
            for w in &page.wires {
                for (to, end) in [(false, &w.from), (true, &w.to)] {
                    let crate::Endpoint::Pin { item, pin } = end else {
                        continue;
                    };
                    let Some(placed) = page.symbol(item) else { continue };
                    if placed.symbol == id && symbol.pin(pin).is_none() {
                        let name = placed.tag.as_ref().map_or_else(|| item.clone(), ToString::to_string);
                        out.push((
                            DanglingEnd {
                                page: page.id.clone(),
                                wire: w.id.clone(),
                                to,
                                item: item.clone(),
                                pin: pin.clone(),
                            },
                            format!("page {} {name}:{pin}", page.number),
                        ));
                    }
                }
            }
        }
        out
    }

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

    /// The reference column `x` millimetres falls into on `page`, with the column count of the
    /// project's page template.
    pub fn column_of(&self, page: &Page, x: f64) -> usize {
        page.size.column_in(self.meta.template.column_count(), x)
    }

    /// The index of the cover page, if the project has one (the first page marked as cover).
    pub fn cover_page(&self) -> Option<usize> {
        self.pages.iter().position(|p| p.cover)
    }

    /// Mark the page at `index` as the project's cover page (`on`), or as a normal page again.
    /// A project has one cover page: marking a page takes the mark off every other page. Returns
    /// the ids of the other pages that lost it; `None` when there is no page at `index`.
    pub fn set_cover(&mut self, index: usize, on: bool) -> Option<Vec<String>> {
        self.pages.get(index)?;
        let mut moved = Vec::new();
        for (i, p) in self.pages.iter_mut().enumerate() {
            if i == index {
                p.cover = on;
            } else if on && p.cover {
                p.cover = false;
                moved.push(p.id.clone());
            }
        }
        Some(moved)
    }

    /// `number` as a page may carry it: trimmed, not empty, and not the number of another page
    /// (blanks around a saved number do not make it another number).
    /// `own` is the id of the page that gets the number, which may keep the number it has;
    /// `None` for a page that is not in the project yet.
    pub fn free_page_number(&self, number: &str, own: Option<&str>) -> Result<String, PageNumberError> {
        let number = number.trim();
        if number.is_empty() {
            return Err(PageNumberError::Empty);
        }
        match self
            .pages
            .iter()
            .find(|p| p.number.trim() == number && Some(p.id.as_str()) != own)
        {
            Some(other) => Err(PageNumberError::Taken {
                number: number.to_owned(),
                page: other.id.clone(),
            }),
            None => Ok(number.to_owned()),
        }
    }

    /// Sort pages by their display number, comparing numeric parts as numbers.
    /// Pages with the same number are ordered by their id.
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

    /// Every part id used by any device or jumper.
    pub fn used_parts(&self) -> std::collections::BTreeSet<String> {
        self.devices
            .values()
            .flat_map(|d| d.all_parts().into_iter().map(|(p, _)| p.to_owned()))
            .chain(self.meta.jumpers.iter().filter_map(|j| j.part.clone()))
            .collect()
    }

    /// Pages that place a given symbol.
    pub fn pages_using_symbol(&self, symbol: &str) -> Vec<&Page> {
        self.pages
            .iter()
            .filter(|p| p.symbols.iter().any(|s| s.symbol == symbol))
            .collect()
    }

    /// Rename device `from` to `to` everywhere it appears: its device entry, the symbols that
    /// carry its tag, the cable cores of a cable, panel items and terminal-strip jumpers.
    /// Returns how many symbols were retagged. It does not check that `to` is free: callers do.
    pub fn rename_device(&mut self, from: &DeviceTag, to: &DeviceTag) -> usize {
        if from == to {
            return 0;
        }
        let mut n = 0;
        if let Some(d) = self.devices.remove(from) {
            self.devices.insert(to.clone(), d);
        }
        for p in &mut self.pages {
            for s in &mut p.symbols {
                if s.tag.as_ref() == Some(from) {
                    s.tag = Some(to.clone());
                    n += 1;
                }
            }
            for w in &mut p.wires {
                if let Some(c) = &mut w.cable
                    && &c.cable == from
                {
                    c.cable = to.clone();
                }
            }
        }
        for panel in &mut self.panels {
            for item in &mut panel.items {
                if let crate::panel::PanelItem::Device { tag, .. } = item
                    && tag == from
                {
                    *tag = to.clone();
                }
            }
        }
        for j in &mut self.meta.jumpers {
            if &j.strip == from {
                j.strip = to.clone();
            }
        }
        for net in &mut self.meta.networks {
            net.rename_device(from, to);
        }
        n
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
    fn removing_symbols_takes_the_wires_on_their_pins_too() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut page = Page::new("p1", "1", "P");
        for id in ["s1", "s2"] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: "k".into(),
                at: Point::default(),
                rotation: Rotation::default(),
                mirror: false,
                tag: None,
            });
        }
        page.wires = serde_json::from_str(
            r#"[{"id":"w1","from":{"item":"s1","pin":"1"},"to":{"at":{"x":0.0,"y":0.0}}},
                {"id":"w2","from":{"at":{"x":0.0,"y":0.0}},"to":{"item":"s2","pin":"1"}},
                {"id":"w3","from":{"at":{"x":0.0,"y":0.0}},"to":{"at":{"x":5.0,"y":0.0}}},
                {"id":"w4","from":{"item":"s1","pin":"2"},"to":{"item":"s2","pin":"2"}}]"#,
        )?;
        let removed = page.remove_symbols_and_wires(&["s2", "nope"], &["w3"]);
        assert_eq!(removed, ["s2", "w2", "w3", "w4"]);
        assert_eq!(page.symbols.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s1"]);
        assert_eq!(page.wires.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["w1"]);
        Ok(())
    }

    #[test]
    fn removing_symbols_judges_each_wire_by_its_own_ends() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut page = Page::new("p1", "1", "P");
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "k".into(),
            at: Point::default(),
            rotation: Rotation::default(),
            mirror: false,
            tag: None,
        });
        // Two wires share the id w1 (a replaced page can hold that); only the first is on s1.
        // w2 ends on `gone`, which is not on the page.
        page.wires = serde_json::from_str(
            r#"[{"id":"w1","from":{"item":"s1","pin":"1"},"to":{"at":{"x":0.0,"y":0.0}}},
                {"id":"w1","from":{"at":{"x":0.0,"y":0.0}},"to":{"at":{"x":5.0,"y":0.0}}},
                {"id":"w2","from":{"item":"gone","pin":"1"},"to":{"at":{"x":5.0,"y":0.0}}}]"#,
        )?;
        let removed = page.remove_symbols_and_wires(&["s1", "gone"], &[]);
        assert_eq!(removed, ["s1", "w1"]);
        assert_eq!(
            page.wires.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            ["w1", "w2"]
        );
        assert!(matches!(page.wires[0].from, Endpoint::Point { .. }));
        Ok(())
    }

    #[test]
    fn a_device_with_one_part_reads_and_writes_as_before() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let old = r#"{"part":"acme.relay","description":"Relay"}"#;
        let d: Device = serde_json::from_str(old)?;
        assert!(d.accessories.is_empty());
        assert_eq!(serde_json::to_string(&d)?, old);
        assert_eq!(d.all_parts(), [("acme.relay", 1)]);

        let mut d = d;
        d.accessories = vec![
            DevicePart {
                part: "acme.block".into(),
                qty: 1,
            },
            DevicePart {
                part: "acme.marker".into(),
                qty: 4,
            },
        ];
        let json = serde_json::to_string(&d)?;
        assert!(json.contains(r#""accessories":[{"part":"acme.block"},{"part":"acme.marker","qty":4}]"#));
        assert_eq!(serde_json::from_str::<Device>(&json)?, d);
        assert!(d.uses_part("acme.marker") && !d.uses_part("acme.other"));
        let mut p = Project::new("x");
        p.devices.insert(DeviceTag::parse("-K1")?, d);
        assert_eq!(p.used_parts().len(), 3);
        Ok(())
    }

    #[test]
    fn one_page_is_the_cover() {
        let mut p = Project::new("x");
        for (id, n) in [("a", "1"), ("b", "2"), ("c", "3")] {
            p.pages.push(Page::new(id, n, ""));
        }
        assert_eq!(p.set_cover(0, true), Some(vec![]));
        assert_eq!(p.cover_page(), Some(0));
        // Marking another page moves the mark.
        assert_eq!(p.set_cover(2, true), Some(vec!["a".to_owned()]));
        assert_eq!(p.pages.iter().filter(|pg| pg.cover).count(), 1);
        assert_eq!(p.cover_page(), Some(2));
        // Off leaves the other pages alone.
        assert_eq!(p.set_cover(2, false), Some(vec![]));
        assert_eq!(p.cover_page(), None);
        assert_eq!(p.set_cover(9, true), None);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn sheet_sizes_read_write_and_measure() {
        assert_eq!(PageSize::A1.mm(), (841.0, 594.0));
        assert_eq!(PageSize::A0.mm(), (1189.0, 841.0));
        assert_eq!(PageSize::A4Portrait.mm(), (210.0, 297.0));
        assert!(PageSize::A4Portrait.is_portrait() && !PageSize::A4.is_portrait());
        for (text, want) in [
            ("A4", PageSize::A4),
            ("a3", PageSize::A3),
            ("A4-portrait", PageSize::A4Portrait),
            ("a4 portrait", PageSize::A4Portrait),
            ("A4P", PageSize::A4Portrait),
            ("A1 landscape", PageSize::A1),
            ("A0_portrait", PageSize::A0Portrait),
            (
                "500x700",
                PageSize::Custom {
                    width: 500,
                    height: 700,
                },
            ),
            (
                "500 x 700 mm",
                PageSize::Custom {
                    width: 500,
                    height: 700,
                },
            ),
            (
                "500mm x 700mm",
                PageSize::Custom {
                    width: 500,
                    height: 700,
                },
            ),
            (
                "500 mm x 700 mm",
                PageSize::Custom {
                    width: 500,
                    height: 700,
                },
            ),
            (
                "500 × 700",
                PageSize::Custom {
                    width: 500,
                    height: 700,
                },
            ),
        ] {
            assert_eq!(PageSize::parse(text), Ok(want), "{text}");
        }
        for bad in [
            "A5",
            "B4",
            "50x700",
            "500x9000",
            "500x",
            "x",
            "",
            "-500x700",
            "500x-700",
            "ap",
            "a4pp",
            "50 0 x 700 mm",
            "500mmmm x 700",
            "500 x 700 mm mm",
        ] {
            assert!(PageSize::parse(bad).is_err(), "{bad}");
        }
        // The error says which side is wrong and why.
        let err = PageSize::parse("5 00x700").unwrap_err();
        assert!(
            err.contains("the width \"5 00\"") && err.contains("100 to 2000 mm"),
            "{err}"
        );
        let err = PageSize::parse("500x9000").unwrap_err();
        assert!(err.contains("9000 mm is outside 100 to 2000 mm"), "{err}");
        for size in PageSize::NAMED.into_iter().chain([PageSize::Custom {
            width: 500,
            height: 700,
        }]) {
            assert_eq!(PageSize::parse(&size.code()), Ok(size), "{}", size.code());
        }
        assert_eq!(PageSize::A3Portrait.label(), "A3 portrait");
        assert_eq!(
            PageSize::Custom {
                width: 500,
                height: 700
            }
            .label(),
            "500 x 700 mm"
        );
        // Pages keep their old file form; the new sizes have their own.
        let page = |size: PageSize| {
            let mut p = Page::new("p1", "1", "One");
            p.size = size;
            p
        };
        let old = serde_json::to_value(page(PageSize::A2)).unwrap();
        assert_eq!(old["size"], "A2");
        assert_eq!(serde_json::from_value::<Page>(old).unwrap().size, PageSize::A2);
        for size in [
            PageSize::A0Portrait,
            PageSize::Custom {
                width: 500,
                height: 700,
            },
        ] {
            let text = serde_json::to_string(&page(size)).unwrap();
            assert_eq!(serde_json::from_str::<Page>(&text).unwrap().size, size, "{text}");
        }
        // A custom size outside the allowed range is refused when read, as when typed.
        let mut text = serde_json::to_value(page(PageSize::Custom {
            width: 500,
            height: 700,
        }))
        .unwrap();
        text["size"]["Custom"]["width"] = 0.into();
        let err = serde_json::from_value::<Page>(text).unwrap_err().to_string();
        assert!(err.contains("outside 100 to 2000 mm"), "{err}");
    }

    #[test]
    fn rows_cover_the_frame_height_for_each_sheet_size() {
        for (size, rows) in [
            (PageSize::A4, 6),
            (PageSize::A3, 6),
            (PageSize::A2, 8),
            (PageSize::A1, 10),
            (PageSize::A0, 12),
            (PageSize::A4Portrait, 6),
            (PageSize::A3Portrait, 8),
            (PageSize::A0Portrait, 12),
            (
                PageSize::Custom {
                    width: 500,
                    height: 300,
                },
                8,
            ),
        ] {
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
        // A location stored as typed is found by its normal form, and the other way round.
        m.locations.push(loc("+L4", ""));
        assert!(m.merge_location(loc("L4", "Deck")), "the stored +L4 is filled in");
        assert!(!m.merge_location(loc("+L2", "Other")));
        assert!(m.merge_location(loc("+L5", "")));
        let paths: Vec<&str> = m.locations.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["L1.L3", "L2", "+L4", "L5"], "+L4 sorts as L4");
        assert_eq!(m.locations[2].description, "Deck");
        m.locations.retain(|l| l.path != "+L4" && l.path != "L5");
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
    fn a_pages_own_title_field_is_set_cleared_and_falls_back() {
        let mut meta = ProjectMeta::new("Demo");
        meta.title_block.insert("revision".into(), "A".into());
        let mut page = Page::new("p1", "1", "Power");
        assert_eq!(page.title_field(&meta, "revision"), "A");
        assert!(page.set_title_field("revision", " B "));
        assert_eq!(page.title_field(&meta, "revision"), "B");
        assert!(!page.set_title_field("revision", "B"), "the same value changes nothing");
        assert!(page.set_title_field("revision", "  "));
        assert!(page.title_block.is_empty(), "a blank value removes the field");
        assert_eq!(page.title_field(&meta, "revision"), "A", "the project's shows again");
        assert!(!page.set_title_field("date", ""));
        page.set_title_field("sheet_note", "x");
        assert_eq!(
            page.title_field_keys(),
            ["revision", "date", "drawn_by", "checked_by", "sheet_note"]
        );
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
    fn jumper_edits_trim_the_terminal_number_like_adding_does() -> Result<(), Box<dyn std::error::Error>> {
        let x1 = DeviceTag::parse("-X1")?;
        let s = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        let mut m = ProjectMeta::new("t");
        m.add_jumper(&x1, &s(&[" 1 ", "2", "3"]))?;
        assert_eq!(m.jumpers[0].terminals, s(&["1", "2", "3"]));
        assert!(m.set_jumper_part(&x1, " 2 ", Some("wago.2002-403"))?);
        assert_eq!(m.jumpers[0].part.as_deref(), Some("wago.2002-403"));
        assert!(m.remove_from_jumper(&x1, " 3 "));
        assert_eq!(m.jumpers[0].terminals, s(&["1", "2"]));
        Ok(())
    }

    #[test]
    fn a_jumper_keeps_its_part_through_merges_renames_and_saving() -> Result<(), Box<dyn std::error::Error>> {
        let x1 = DeviceTag::parse("-X1")?;
        let s = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        let mut m = ProjectMeta::new("t");
        m.add_jumper(&x1, &s(&["1", "2"]))?;
        assert!(
            m.set_jumper_part(&x1, "9", Some("wago.2002-402")).is_err(),
            "no jumper there"
        );
        assert!(m.set_jumper_part(&x1, "2", Some("wago.2002-402"))?);
        assert!(!m.set_jumper_part(&x1, "1", Some("wago.2002-402"))?, "already so");
        // Saved with the part; a file without one still reads.
        let text = toml::to_string(&m)?;
        assert!(text.contains("part = \"wago.2002-402\""), "{text}");
        assert_eq!(toml::from_str::<ProjectMeta>(&text)?.jumpers, m.jumpers);
        let old: ProjectMeta =
            toml::from_str("name = \"t\"\n[[jumper]]\nstrip = \"-X1\"\nterminals = [\"1\", \"2\"]\n")?;
        assert_eq!(old.jumpers[0].part, None);
        // Extended by another jumper: the part stays.
        m.add_jumper(&x1, &s(&["2", "3"]))?;
        assert_eq!(m.jumpers[0].part.as_deref(), Some("wago.2002-402"));
        // Its terminal renumbered: the part stays.
        let key = |id: &str| ("p".to_owned(), id.to_owned());
        let before: TerminalNumbers = [("a", "1"), ("b", "2"), ("c", "3")]
            .iter()
            .map(|(id, n)| (key(id), (x1.clone(), (*n).to_owned())))
            .collect();
        let after: TerminalNumbers = [("a", "7"), ("b", "2"), ("c", "3")]
            .iter()
            .map(|(id, n)| (key(id), (x1.clone(), (*n).to_owned())))
            .collect();
        assert!(m.follow_terminals(&before, &after));
        assert_eq!(m.jumpers[0].terminals, s(&["2", "3", "7"]));
        assert_eq!(m.jumpers[0].part.as_deref(), Some("wago.2002-402"));
        let mut p = Project::new("t");
        p.meta = m.clone();
        assert!(p.used_parts().contains("wago.2002-402"));
        // Cleared with none (or an empty text).
        assert!(m.set_jumper_part(&x1, "3", Some(" "))?);
        assert_eq!(m.jumpers[0].part, None);
        Ok(())
    }

    #[test]
    fn a_logo_is_copied_into_the_pictures_folder_without_overwriting_another()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dir = tmp.path().join("p");
        std::fs::create_dir_all(&dir)?;
        let png = b"\x89PNG\r\n\x1a\nfirst".to_vec();
        let jpg = [0xFF, 0xD8, 0xFF, 0xE0, 1, 2].to_vec();
        let src = tmp.path().join("My logo!.png");
        std::fs::write(&src, &png)?;
        assert_eq!(Logo::copy_into(&dir, &src)?, "pictures/My_logo_.png");
        // The same picture again is used as it is; another one of that name gets a new name.
        assert_eq!(Logo::copy_into(&dir, &src)?, "pictures/My_logo_.png");
        std::fs::write(&src, b"\x89PNG\r\n\x1a\nsecond")?;
        assert_eq!(Logo::copy_into(&dir, &src)?, "pictures/My_logo_-2.png");
        assert_eq!(std::fs::read(dir.join("pictures/My_logo_.png"))?, png);
        let j = tmp.path().join("photo.jpeg");
        std::fs::write(&j, &jpg)?;
        assert_eq!(Logo::copy_into(&dir, &j)?, "pictures/photo.jpg");
        std::fs::write(&j, b"GIF89a")?;
        assert!(
            Logo::copy_into(&dir, &j)
                .err()
                .unwrap_or_default()
                .contains("PNG or JPEG")
        );
        assert!(Logo::copy_into(&dir, &tmp.path().join("none.png")).is_err());
        Ok(())
    }

    #[test]
    fn the_logo_is_saved_with_its_cover_place_and_older_projects_load_without_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let old: ProjectMeta = toml::from_str("name = \"t\"\nformat_version = 1\n")?;
        assert_eq!(old.logo, None);
        assert!(
            !toml::to_string(&old)?.contains("logo"),
            "nothing written without a logo"
        );
        let mut m = ProjectMeta::new("t");
        m.logo = Some(Logo {
            file: "pictures/logo.png".into(),
            cover: Some((Point::new(10.0, 20.0), Point::new(50.0, 40.0))),
        });
        let back: ProjectMeta = toml::from_str(&toml::to_string(&m)?)?;
        assert_eq!(back.logo, m.logo);
        m.logo = Some(Logo {
            file: "pictures/logo.png".into(),
            cover: None,
        });
        let text = toml::to_string(&m)?;
        assert!(!text.contains("cover"), "{text}");
        assert_eq!(toml::from_str::<ProjectMeta>(&text)?.logo, m.logo);
        for bad in [
            "",
            "/etc/logo.png",
            "../logo.png",
            "a/../b.png",
            "C:/logo.png",
            "a\\b.png",
            "a//b.png",
        ] {
            assert!(!relative_file_is_safe(bad), "{bad}");
        }
        assert!(relative_file_is_safe("pictures/logo.png"));
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
    fn a_page_number_is_free_when_no_other_page_has_it() {
        let mut p = Project::new("t");
        p.pages.push(Page::new("a", "1", ""));
        p.pages.push(Page::new("b", "2.1", ""));
        assert_eq!(p.free_page_number(" 3 ", None), Ok("3".to_owned()));
        assert_eq!(p.free_page_number("", None), Err(PageNumberError::Empty));
        assert_eq!(p.free_page_number(" \t", Some("a")), Err(PageNumberError::Empty));
        let taken = PageNumberError::Taken {
            number: "2.1".into(),
            page: "b".into(),
        };
        assert_eq!(p.free_page_number(" 2.1", None), Err(taken.clone()));
        assert_eq!(p.free_page_number("2.1", Some("a")), Err(taken.clone()));
        assert_eq!(taken.to_string(), "page \"b\" already has number \"2.1\"");
        // A page may keep its own number.
        assert_eq!(p.free_page_number("2.1", Some("b")), Ok("2.1".to_owned()));
        // A number saved with blanks around it (an older project file) is taken all the same.
        p.pages.push(Page::new("c", " 4 ", ""));
        let padded = PageNumberError::Taken {
            number: "4".into(),
            page: "c".into(),
        };
        assert_eq!(p.free_page_number("4", None), Err(padded));
        assert_eq!(p.free_page_number("4", Some("c")), Ok("4".to_owned()));
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
    fn a_bad_location_is_named_as_typed_without_the_made_up_tag() {
        let cases = [
            ("+", "\"+\" is not a location: the location aspect (+) has no name"),
            ("", "\"+\" is not a location: the location aspect (+) has no name"),
            (
                "A..B",
                "\"+A..B\" is not a location: the location aspect (+) has an empty level",
            ),
            (
                "+A B",
                "\"+A B\" is not a location: only letters, digits, '.', '_' allowed",
            ),
            ("+A-K1", "\"+A-K1\" is not a location: give only the + part of a tag"),
            ("=F1+A", "\"=F1+A\" is not a location: give only the + part of a tag"),
            ("-K1", "\"-K1\" is not a location: give only the + part of a tag"),
        ];
        for (typed, want) in cases {
            assert_eq!(location_path(typed), Err(want.to_owned()), "{typed:?}");
        }
        assert_eq!(location_path(" +L1.L3 "), Ok("L1.L3".to_owned()));
    }

    #[test]
    fn locations_are_set_and_removed_by_path_with_or_without_the_plus() {
        let mut m = ProjectMeta::new("t");
        assert_eq!(m.set_location("+L1.L3", " Cabinet 3 "), Ok(true));
        assert_eq!(m.set_location("L1.L3", "Cabinet 3"), Ok(false), "same description");
        assert_eq!(m.set_location("L1", ""), Ok(true));
        let paths: Vec<&str> = m.locations.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["L1", "L1.L3"]);
        assert_eq!(m.locations[1].description, "Cabinet 3");
        assert!(m.set_location("+L1-K1", "x").is_err(), "a device tag is no location");
        assert!(m.set_location("", "x").is_err());
        assert_eq!(m.remove_location("+L1"), Ok(()));
        assert_eq!(m.remove_location("L1"), Err("no location +L1".into()));
        // An entry stored with its sign, as an older or hand-edited file may have it.
        m.locations.push(Location {
            path: "+L5".into(),
            description: String::new(),
        });
        assert_eq!(m.remove_location("+L5"), Ok(()));
        assert!(!m.locations.iter().any(|l| l.path == "+L5"));
        let why = m.remove_location("+L1-K1");
        assert!(why.as_ref().is_err_and(|e| e.contains("not a location")), "{why:?}");
        assert_eq!(m.locations.len(), 1);
        // Setting a path stored with its sign updates that entry, in normal form.
        m.locations.push(Location {
            path: "+L7".into(),
            description: "Desk".into(),
        });
        assert_eq!(m.set_location("L7", "Desk 2"), Ok(true));
        let l7: Vec<&Location> = m.locations.iter().filter(|l| l.path.ends_with("L7")).collect();
        assert_eq!(l7.len(), 1, "no second entry");
        assert_eq!((l7[0].path.as_str(), l7[0].description.as_str()), ("L7", "Desk 2"));
    }

    #[test]
    fn locations_are_kept_in_natural_order() -> Result<(), String> {
        let paths = |l: &[Location]| l.iter().map(|l| l.path.clone()).collect::<Vec<_>>();
        let want = ["CAB2", "CAB2.R2", "CAB2.R10", "CAB10"];
        let mut m = ProjectMeta::new("t");
        for p in ["CAB10", "CAB2.R10", "CAB2", "CAB2.R2"] {
            m.set_location(p, "")?;
        }
        assert_eq!(paths(&m.locations), want);
        let mut m = ProjectMeta::new("t");
        for p in ["CAB10", "CAB2.R10", "CAB2", "CAB2.R2"] {
            m.merge_location(Location {
                path: p.into(),
                description: String::new(),
            });
        }
        assert_eq!(paths(&m.locations), want);
        let rows: Vec<(String, String)> = ["+CAB10", "CAB2.R10", "CAB2", "+CAB2.R2"]
            .into_iter()
            .map(|p| (p.to_owned(), String::new()))
            .collect();
        assert_eq!(paths(&location_rows(&rows)?), want);
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

    #[test]
    fn a_hand_edited_jumper_is_checked_on_load() {
        let load = |terminals: &str| {
            let text = format!("name = \"t\"\n[[jumper]]\nstrip = \"-X1\"\nterminals = {terminals}\n");
            toml::from_str::<ProjectMeta>(&text).map_err(|e| e.to_string())
        };
        for bad in [r#"["1.2", "3"]"#, r#"["", "3"]"#, r#"["a b", "3"]"#] {
            let err = load(bad).err().unwrap_or_default();
            assert!(err.contains("cannot be a terminal of -X1"), "{bad}: {err}");
        }
        for single in [r#"["3"]"#, r#"["3", "3"]"#, "[]"] {
            let err = load(single).err().unwrap_or_default();
            assert!(err.contains("at least two different terminals"), "{single}: {err}");
        }
        let ok = load(r#"["3", "4"]"#).map(|m| m.jumpers.len());
        assert_eq!(ok, Ok(1));
    }

    #[test]
    fn strip_layouts_order_terminals_add_spares_and_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let x1 = DeviceTag::parse("-X1")?;
        let s = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        let drawn = s(&["1", "2", "3"]);
        let plain = |v: &[&str]| v.iter().map(|t| ((*t).to_owned(), false)).collect::<Vec<_>>();
        let mut m = ProjectMeta::new("t");
        assert!(m.strip_layout(&x1).is_none());

        // Reorder: 3 to the front.
        assert!(m.move_terminal(&x1, "3", 0, &plain(&["1", "2", "3"]))?);
        let l = m.strip_layout(&x1).ok_or("no layout")?.clone();
        assert_eq!(l.arrange(&drawn), plain(&["3", "1", "2"]));
        // Moving it to where it is changes nothing.
        assert!(!m.move_terminal(&x1, "3", 0, &l.arrange(&drawn))?);
        assert!(m.move_terminal(&x1, "9", 0, &l.arrange(&drawn)).is_err());

        // A spare after terminal 1, and one at the end.
        m.add_spare(&x1, "S1", Some("1"), &l.arrange(&drawn))?;
        let current = m.strip_layout(&x1).ok_or("no layout")?.arrange(&drawn);
        m.add_spare(&x1, "S2", None, &current)?;
        let l = m.strip_layout(&x1).ok_or("no layout")?.clone();
        assert_eq!(
            l.arrange(&drawn),
            vec![
                ("3".to_owned(), false),
                ("1".to_owned(), false),
                ("S1".to_owned(), true),
                ("2".to_owned(), false),
                ("S2".to_owned(), true),
            ]
        );
        let current = l.arrange(&drawn);
        assert!(m.add_spare(&x1, "2", None, &current).is_err(), "drawn already");
        assert!(m.add_spare(&x1, "S1", None, &current).is_err(), "spare already");
        assert!(m.add_spare(&x1, "1.2", None, &current).is_err(), "two levels");
        assert!(m.add_spare(&x1, "S3", Some("7"), &current).is_err(), "no terminal 7");

        // A terminal drawn later follows at the end; a spare that gets drawn is that terminal.
        let later = s(&["1", "2", "3", "4", "S2"]);
        assert_eq!(
            l.arrange(&later)
                .iter()
                .map(|(t, sp)| format!("{t}{}", if *sp { "*" } else { "" }))
                .collect::<Vec<_>>(),
            s(&["3", "1", "S1*", "2", "S2", "4"])
        );

        // Saved and read back.
        let text = toml::to_string(&m)?;
        assert!(text.contains("[[strip]]"), "{text}");
        assert_eq!(toml::from_str::<ProjectMeta>(&text)?.strips, m.strips);
        assert!(!toml::to_string(&ProjectMeta::new("t"))?.contains("strip"));

        // A jumpered spare taken off leaves its jumper; the last spare and the order gone, the
        // layout goes.
        m.add_jumper(&x1, &s(&["S1", "2"]))?;
        let rail = m.strip_layout(&x1).ok_or("no layout")?.arrange(&drawn);
        // Once S1 is drawn, unsparing it keeps its place and its jumper.
        let mut drawn_later = m.clone();
        let with_s1 = s(&["1", "2", "3", "S1"]);
        let now = drawn_later.strip_layout(&x1).ok_or("no layout")?.arrange(&with_s1);
        assert!(drawn_later.remove_spare(&x1, "S1", &now));
        assert_eq!(drawn_later.jumpers.len(), 1);
        assert_eq!(
            drawn_later.strip_layout(&x1).ok_or("no layout")?.arrange(&with_s1),
            plain(&["3", "1", "S1", "2"])
                .into_iter()
                .chain([("S2".to_owned(), true)])
                .collect::<Vec<_>>()
        );
        assert!(m.remove_spare(&x1, "S1", &rail));
        assert!(!m.remove_spare(&x1, "S1", &rail));
        assert!(m.jumpers.is_empty());
        assert!(m.remove_spare(&x1, "S2", &rail));
        assert!(m.set_terminal_order(&x1, &[])?);
        assert!(m.strips.is_empty());
        assert!(m.set_terminal_order(&x1, &s(&["1", "1"])).is_err());

        // A hand-edited layout naming something that cannot be a terminal is refused on load.
        let bad = "name = \"t\"\n[[strip]]\nstrip = \"-X1\"\nspares = [\"=HYPERLINK(1)\"]\n";
        let err = toml::from_str::<ProjectMeta>(bad).err().ok_or("loaded")?.to_string();
        assert!(err.contains("cannot be a terminal of -X1"), "{err}");
        let twice = "name = \"t\"\n[[strip]]\nstrip = \"-X1\"\norder = [\"1\", \"2\"]\n[[strip]]\nstrip = \"-X1\"\norder = [\"2\", \"1\"]\n";
        let err = toml::from_str::<ProjectMeta>(twice).err().ok_or("loaded")?.to_string();
        assert!(err.contains("-X1 is arranged twice"), "{err}");
        assert!(toml::from_str::<ProjectMeta>("name = \"t\"\n")?.strips.is_empty());
        Ok(())
    }

    #[test]
    fn a_strips_rail_order_follows_renamed_renumbered_and_deleted_terminals() -> Result<(), Box<dyn std::error::Error>>
    {
        let (x1, x2) = (DeviceTag::parse("-X1")?, DeviceTag::parse("-X2")?);
        let mut p = Project::new("t");
        let mut page = Page::new("a", "1", "A");
        for (id, tag) in [("t1", "-X1.1"), ("t2", "-X1.2"), ("t3", "-X1.3")] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: "term".into(),
                at: Point::default(),
                rotation: Rotation::default(),
                mirror: false,
                tag: Some(DeviceTag::parse(tag)?),
            });
        }
        p.pages.push(page);
        let s = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        p.meta.set_terminal_order(&x1, &s(&["3", "1", "2"]))?;
        p.meta.add_spare(
            &x1,
            "S1",
            Some("1"),
            &[("3".into(), false), ("1".into(), false), ("2".into(), false)],
        )?;
        assert_eq!(
            p.meta.strip_layout(&x1).ok_or("layout")?.order,
            s(&["3", "1", "S1", "2"])
        );
        let role = |_: &str| None;
        let tag = |p: &mut Project, id: &str, t: &str| -> Result<(), Box<dyn std::error::Error>> {
            let sym = p.pages[0].symbols.iter_mut().find(|s| s.id == id).ok_or("symbol")?;
            sym.tag = Some(DeviceTag::parse(t)?);
            Ok(())
        };

        // -X1.3 renamed to -X1.7: it keeps its place at the front of the rail.
        let before = terminal_numbers(&p, role);
        tag(&mut p, "t3", "-X1.7")?;
        assert!(follow_terminals_in(&mut p, &before, role));
        assert_eq!(
            p.meta.strip_layout(&x1).ok_or("layout")?.order,
            s(&["7", "1", "S1", "2"])
        );

        // -X1.1 deleted: it leaves the order; the spare stays.
        let before = terminal_numbers(&p, role);
        p.pages[0].symbols.retain(|s| s.id != "t1");
        assert!(follow_terminals_in(&mut p, &before, role));
        assert_eq!(p.meta.strip_layout(&x1).ok_or("layout")?.order, s(&["7", "S1", "2"]));

        // The whole strip renamed to -X2: the layout goes with it, spares included.
        let before = terminal_numbers(&p, role);
        tag(&mut p, "t3", "-X2.7")?;
        tag(&mut p, "t2", "-X2.2")?;
        assert!(follow_terminals_in(&mut p, &before, role));
        assert!(p.meta.strip_layout(&x1).is_none());
        let moved = p.meta.strip_layout(&x2).ok_or("layout moved")?;
        assert_eq!(moved.order, s(&["7", "S1", "2"]));
        assert_eq!(moved.spares, s(&["S1"]));
        Ok(())
    }

    /// A project with one page of terminals (symbol id, tag) and the given strip layouts.
    fn strips_project(
        terminals: &[(&str, &str)],
        layouts: &[(&str, &[&str], &[&str])],
    ) -> Result<Project, Box<dyn std::error::Error>> {
        let mut p = Project::new("t");
        let mut page = Page::new("a", "1", "A");
        for (i, (id, tag)) in terminals.iter().enumerate() {
            page.symbols.push(PlacedSymbol {
                id: (*id).into(),
                symbol: "term".into(),
                at: Point::new(i as f64 * 10.0, 0.0),
                rotation: Rotation::default(),
                mirror: false,
                tag: Some(DeviceTag::parse(tag)?),
            });
        }
        p.pages.push(page);
        let s = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        for (strip, order, spares) in layouts {
            p.meta.strips.push(StripLayout {
                strip: DeviceTag::parse(strip)?,
                order: s(order),
                spares: s(spares),
            });
        }
        Ok(p)
    }

    /// Give placed symbol `id` on the first page the tag `t`.
    fn retag(p: &mut Project, id: &str, t: &str) -> Result<(), Box<dyn std::error::Error>> {
        let sym = p.pages[0].symbols.iter_mut().find(|s| s.id == id).ok_or("symbol")?;
        sym.tag = Some(DeviceTag::parse(t)?);
        Ok(())
    }

    #[test]
    fn two_strips_renamed_to_one_keep_both_layouts() -> Result<(), Box<dyn std::error::Error>> {
        let mut p = strips_project(
            &[("a", "-X1.1"), ("b", "-X1.2"), ("c", "-X3.1")],
            &[("-X1", &["2", "1", "S1"], &["S1"]), ("-X3", &["S9", "1"], &["S9"])],
        )?;
        let role = |_: &str| None;
        let before = terminal_numbers(&p, role);
        retag(&mut p, "a", "-X2.1")?;
        retag(&mut p, "b", "-X2.2")?;
        retag(&mut p, "c", "-X2.5")?;
        follow_terminals_in(&mut p, &before, role);
        // The first strip takes its layout to -X2; the other keeps its own, spares included.
        let x2 = p.meta.strip_layout(&DeviceTag::parse("-X2")?).ok_or("x2")?;
        assert_eq!(x2.spares, ["S1"]);
        assert_eq!(x2.order, ["2", "1", "S1"]);
        let x3 = p.meta.strip_layout(&DeviceTag::parse("-X3")?).ok_or("x3 kept")?;
        assert_eq!(x3.spares, ["S9"]);
        assert_eq!(x3.order, ["S9"]);
        Ok(())
    }

    #[test]
    fn a_drawn_spare_renamed_keeps_its_place_on_the_rail() -> Result<(), Box<dyn std::error::Error>> {
        let mut p = strips_project(&[("a", "-X1.1"), ("s", "-X1.S1")], &[("-X1", &["S1", "1"], &["S1"])])?;
        let role = |_: &str| None;
        let before = terminal_numbers(&p, role);
        retag(&mut p, "s", "-X1.S5")?;
        assert!(follow_terminals_in(&mut p, &before, role));
        let x1 = p.meta.strip_layout(&DeviceTag::parse("-X1")?).ok_or("x1")?;
        assert_eq!(x1.order.first().map(String::as_str), Some("S5"));
        Ok(())
    }

    #[test]
    fn a_strip_renamed_with_its_spares_keeps_its_rail_and_their_device_data() -> Result<(), Box<dyn std::error::Error>>
    {
        // Terminal 1, a terminal tagged with just the strip (3: it passes over spare 2), and the
        // spare 2, which has a part.
        let mut p = strips_project(&[("a", "-X1.1"), ("b", "-X1")], &[("-X1", &["3", "2", "1"], &["2"])])?;
        let spare = Device {
            part: Some("WAGO 2002-1201".into()),
            ..Device::default()
        };
        p.devices.insert(DeviceTag::parse("-X1.2")?, spare.clone());
        let role = |id: &str| (id == "term").then_some(crate::symbol::SymbolRole::Terminal);
        let before = terminal_numbers(&p, role);
        assert!(before.values().any(|(_, n)| n == "3"));
        retag(&mut p, "a", "-X2.1")?;
        retag(&mut p, "b", "-X2")?;
        assert!(follow_terminals_in(&mut p, &before, role));
        let x2 = p.meta.strip_layout(&DeviceTag::parse("-X2")?).ok_or("x2")?;
        assert_eq!(x2.order, ["3", "2", "1"]);
        assert_eq!(x2.spares, ["2"]);
        assert!(terminal_numbers(&p, role).values().any(|(_, n)| n == "3"));
        assert_eq!(p.devices.get(&DeviceTag::parse("-X2.2")?), Some(&spare));
        assert!(!p.devices.contains_key(&DeviceTag::parse("-X1.2")?));
        Ok(())
    }

    #[test]
    fn a_jumper_on_a_spare_goes_with_its_strip_when_the_strip_is_renamed() -> Result<(), Box<dyn std::error::Error>> {
        let mut p = strips_project(
            &[("a", "-X1.1"), ("b", "-X1.2")],
            &[("-X1", &["1", "2", "S1"], &["S1"])],
        )?;
        let x1 = DeviceTag::parse("-X1")?;
        p.meta.add_jumper(&x1, &["1".to_owned(), "S1".to_owned()])?;
        let role = |_: &str| None;
        let before = terminal_numbers(&p, role);
        retag(&mut p, "a", "-X2.1")?;
        retag(&mut p, "b", "-X2.2")?;
        assert!(follow_terminals_in(&mut p, &before, role));
        let x2 = DeviceTag::parse("-X2")?;
        assert_eq!(p.meta.jumpers.len(), 1);
        assert_eq!(p.meta.jumpers[0].strip, x2);
        assert_eq!(p.meta.jumpers[0].terminals, ["1", "S1"]);
        assert_eq!(
            p.meta.strip_layout(&x2).map(|l| l.spares.clone()),
            Some(vec!["S1".to_owned()])
        );
        Ok(())
    }

    #[test]
    fn a_strip_renamed_onto_a_drawn_device_named_like_its_spare_keeps_its_layout()
    -> Result<(), Box<dyn std::error::Error>> {
        // -X2.S1 is drawn as something that is no terminal, and has no device entry.
        let mut p = strips_project(&[("a", "-X1.1"), ("k", "-X2.S1")], &[("-X1", &["S1", "1"], &["S1"])])?;
        let spare = Device {
            part: Some("WAGO 2002-1201".into()),
            ..Device::default()
        };
        p.devices.insert(DeviceTag::parse("-X1.S1")?, spare.clone());
        let role = |id: &str| {
            Some(if id == "term" {
                crate::symbol::SymbolRole::Terminal
            } else {
                crate::symbol::SymbolRole::Main
            })
        };
        p.pages[0].symbols[1].symbol = "coil".into();
        let before = terminal_numbers(&p, role);
        retag(&mut p, "a", "-X2.1")?;
        follow_terminals_in(&mut p, &before, role);
        let x1 = p.meta.strip_layout(&DeviceTag::parse("-X1")?).ok_or("x1 kept")?;
        assert_eq!(x1.spares, ["S1"]);
        assert!(p.meta.strip_layout(&DeviceTag::parse("-X2")?).is_none());
        assert_eq!(p.devices.get(&DeviceTag::parse("-X1.S1")?), Some(&spare));
        assert!(!p.devices.contains_key(&DeviceTag::parse("-X2.S1")?));
        Ok(())
    }

    #[test]
    fn a_strip_renamed_onto_a_name_whose_spare_tag_has_its_own_part_keeps_its_layout()
    -> Result<(), Box<dyn std::error::Error>> {
        // -X1.S1 and -X2.S1 (nothing drawn there) each have a part of their own.
        let mut p = strips_project(&[("a", "-X1.1")], &[("-X1", &["S1", "1"], &["S1"])])?;
        let mine = Device {
            part: Some("WAGO 2002-1201".into()),
            ..Device::default()
        };
        let theirs = Device {
            part: Some("Phoenix PT 2,5".into()),
            ..Device::default()
        };
        p.devices.insert(DeviceTag::parse("-X1.S1")?, mine.clone());
        p.devices.insert(DeviceTag::parse("-X2.S1")?, theirs.clone());
        let role = |_: &str| Some(crate::symbol::SymbolRole::Terminal);
        let before = terminal_numbers(&p, role);
        retag(&mut p, "a", "-X2.1")?;
        follow_terminals_in(&mut p, &before, role);
        let x1 = p.meta.strip_layout(&DeviceTag::parse("-X1")?).ok_or("x1 kept")?;
        assert_eq!(x1.spares, ["S1"]);
        assert!(p.meta.strip_layout(&DeviceTag::parse("-X2")?).is_none());
        assert_eq!(p.devices.get(&DeviceTag::parse("-X1.S1")?), Some(&mine));
        assert_eq!(p.devices.get(&DeviceTag::parse("-X2.S1")?), Some(&theirs));
        Ok(())
    }

    #[test]
    fn a_strip_only_terminal_numbered_like_a_blocked_spare_does_not_let_the_layout_move()
    -> Result<(), Box<dyn std::error::Error>> {
        // -X1: terminal 1, a terminal tagged with just the strip (3, past spare 2), and spare 2.
        // -X2.2 is drawn as something that is no terminal.
        let mut p = strips_project(
            &[("a", "-X1.1"), ("b", "-X1"), ("k", "-X2.2")],
            &[("-X1", &["3", "2", "1"], &["2"])],
        )?;
        p.pages[0].symbols[2].symbol = "coil".into();
        let x1 = DeviceTag::parse("-X1")?;
        p.meta.add_jumper(&x1, &["1".to_owned(), "3".to_owned()])?;
        let role = |id: &str| {
            Some(if id == "term" {
                crate::symbol::SymbolRole::Terminal
            } else {
                crate::symbol::SymbolRole::Main
            })
        };
        let before = terminal_numbers(&p, role);
        retag(&mut p, "a", "-X2.1")?;
        retag(&mut p, "b", "-X2")?;
        follow_terminals_in(&mut p, &before, role);
        // The layout stays on -X1; the jumper names only terminals -X2 has.
        assert!(p.meta.strip_layout(&DeviceTag::parse("-X2")?).is_none());
        let now = terminal_numbers(&p, role);
        for j in &p.meta.jumpers {
            for t in &j.terminals {
                assert!(now.values().any(|(s, n)| *s == j.strip && n == t), "{} {t}", j.strip);
            }
        }
        Ok(())
    }

    #[test]
    fn a_spare_moved_with_its_strip_keeps_its_panel_place() -> Result<(), Box<dyn std::error::Error>> {
        let mut p = strips_project(&[("a", "-X1.1")], &[("-X1", &["1", "S1"], &["S1"])])?;
        let mut panel = crate::panel::Panel::new("p", "P", 600.0, 400.0);
        panel.items.push(crate::panel::PanelItem::Device {
            tag: DeviceTag::parse("-X1.S1")?,
            at: Point::default(),
            width: 6.0,
            height: 50.0,
            rotation: crate::geom::Rotation::R0,
        });
        p.panels.push(panel);
        let role = |_: &str| None;
        let before = terminal_numbers(&p, role);
        retag(&mut p, "a", "-X2.1")?;
        assert!(follow_terminals_in(&mut p, &before, role));
        let tags: Vec<String> = p.panels[0]
            .items
            .iter()
            .filter_map(|i| match i {
                crate::panel::PanelItem::Device { tag, .. } => Some(tag.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(tags, ["-X2.S1"]);
        Ok(())
    }

    #[test]
    fn a_jumpered_spare_keeps_its_jumper_when_its_drawing_is_deleted() -> Result<(), Box<dyn std::error::Error>> {
        let x1 = DeviceTag::parse("-X1")?;
        let mut p = Project::new("t");
        let mut page = Page::new("a", "1", "A");
        for (id, tag) in [("t1", "-X1.1"), ("t2", "-X1.2"), ("ts", "-X1.S1")] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: "term".into(),
                at: Point::default(),
                rotation: Rotation::default(),
                mirror: false,
                tag: Some(DeviceTag::parse(tag)?),
            });
        }
        p.pages.push(page);
        p.meta.add_spare(&x1, "S1", None, &[])?;
        p.meta.add_jumper(&x1, &["1".to_owned(), "S1".to_owned()])?;
        p.meta.add_jumper(&x1, &["2".to_owned(), "1".to_owned()])?;
        let role = |_: &str| None;
        // The registered spare's drawing goes: it is a spare again, still jumpered.
        let before = terminal_numbers(&p, role);
        p.pages[0].symbols.retain(|s| s.id != "ts");
        follow_terminals_in(&mut p, &before, role);
        assert_eq!(p.meta.jumpers[0].terminals, ["1", "2", "S1"]);
        // A drawn terminal that is no spare still leaves its jumper when deleted.
        let before = terminal_numbers(&p, role);
        p.pages[0].symbols.retain(|s| s.id != "t2");
        follow_terminals_in(&mut p, &before, role);
        assert_eq!(p.meta.jumpers[0].terminals, ["1", "S1"]);
        Ok(())
    }

    #[test]
    fn a_spare_never_takes_over_another_device_and_leaves_no_part_behind() -> Result<(), Box<dyn std::error::Error>> {
        let x1 = DeviceTag::parse("-X1")?;
        let mut p = Project::new("t");
        let mut page = Page::new("a", "1", "A");
        // -X1.1 is a terminal; -X1.K1 is drawn as another kind of device.
        for (id, tag) in [("t1", "-X1.1"), ("k1", "-X1.K1")] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: id.into(),
                at: Point::default(),
                rotation: Rotation::default(),
                mirror: false,
                tag: Some(DeviceTag::parse(tag)?),
            });
        }
        p.pages.push(page);
        let current = vec![("1".to_owned(), false)];
        let err = p.add_spare(&x1, "K1", None, &current).err().unwrap_or_default();
        assert!(err.contains("-X1.K1"), "{err}");
        assert!(p.meta.strips.is_empty());
        // A spare given a part (WAGO import) and taken off again leaves no device entry.
        p.add_spare(&x1, "S1", None, &current)?;
        let s1 = DeviceTag::parse("-X1.S1")?;
        p.devices.entry(s1.clone()).or_default().part = Some("wago.2002-1201".into());
        let current = vec![("1".to_owned(), false), ("S1".to_owned(), true)];
        assert!(p.remove_spare(&x1, "S1", &current));
        assert!(!p.devices.contains_key(&s1));
        // A spare that is drawn by now keeps its device entry.
        p.add_spare(&x1, "S2", None, &[("1".to_owned(), false)])?;
        let s2 = DeviceTag::parse("-X1.S2")?;
        p.devices.entry(s2.clone()).or_default().part = Some("wago.2002-1201".into());
        p.pages[0].symbols.push(PlacedSymbol {
            id: "t2".into(),
            symbol: "term".into(),
            at: Point::default(),
            rotation: Rotation::default(),
            mirror: false,
            tag: Some(s2.clone()),
        });
        let current = vec![("1".to_owned(), false), ("S2".to_owned(), false)];
        assert!(p.remove_spare(&x1, "S2", &current));
        assert!(p.devices.contains_key(&s2));
        Ok(())
    }

    #[test]
    fn a_spare_never_takes_the_tag_of_a_cable_and_leaves_its_entry() -> Result<(), Box<dyn std::error::Error>> {
        let x1 = DeviceTag::parse("-X1")?;
        let w1 = DeviceTag::parse("-X1.W1")?;
        let mut p = Project::new("t");
        let mut page = Page::new("a", "1", "A");
        // A wire runs as a core of cable -X1.W1; no symbol carries that tag.
        page.wires.push(serde_json::from_str(
            r#"{"id":"w1","from":{"at":{"x":0,"y":0}},"to":{"at":{"x":10,"y":0}},"cable":{"cable":"-X1.W1","core":"1"}}"#,
        )?);
        p.pages.push(page);
        let err = p.add_spare(&x1, "W1", None, &[]).err().unwrap_or_default();
        assert!(err.contains("-X1.W1") && err.contains("cable"), "{err}");
        assert!(p.meta.strips.is_empty());
        // A spare registered before the core named the cable keeps the cable's entry.
        p.meta.add_spare(&x1, "W1", None, &[])?;
        p.devices.entry(w1.clone()).or_default().part = Some("lapp.ölflex".into());
        assert!(p.remove_spare(&x1, "W1", &[("W1".to_owned(), true)]));
        assert!(p.devices.contains_key(&w1));
        // Taken off a second time: the cable's entry stays and the reason is given.
        assert!(!p.remove_spare(&x1, "W1", &[]));
        let err = p.remove_unused_terminal_entry(&x1, "W1", &[]).err().unwrap_or_default();
        assert!(err.contains("cable of a wire core"), "{err}");
        assert!(p.devices.contains_key(&w1));
        // An entry nothing uses (restored by hand after the spare went) is removed.
        let s1 = DeviceTag::parse("-X1.S1")?;
        p.devices.entry(s1.clone()).or_default().part = Some("wago.2002-1201".into());
        assert_eq!(p.remove_unused_terminal_entry(&x1, "S1", &[]), Ok(true));
        assert!(!p.devices.contains_key(&s1));
        assert_eq!(p.remove_unused_terminal_entry(&x1, "S1", &[]), Ok(false));
        // A terminal drawn with just the strip's tag and numbered 1 on the rail keeps its entry.
        let one = DeviceTag::parse("-X1.1")?;
        p.devices.entry(one.clone()).or_default().part = Some("wago.2002-1201".into());
        let err = p
            .remove_unused_terminal_entry(&x1, "1", &[("1".to_owned(), false)])
            .err()
            .unwrap_or_default();
        assert!(err.contains("is drawn"), "{err}");
        assert!(p.devices.contains_key(&one));
        // A number of two levels names another strip's terminal (-X1.2's 3) and is refused.
        let nested = DeviceTag::parse("-X1.2.3")?;
        p.devices.entry(nested.clone()).or_default().part = Some("wago.2002-1201".into());
        let err = p
            .remove_unused_terminal_entry(&x1, "2.3", &[])
            .err()
            .unwrap_or_default();
        assert!(err.contains("one tag level"), "{err}");
        assert!(p.devices.contains_key(&nested));
        Ok(())
    }

    #[test]
    fn a_terminal_tagged_with_just_the_strip_never_takes_a_spares_number() -> Result<(), Box<dyn std::error::Error>> {
        let x1 = DeviceTag::parse("-X1")?;
        let mut p = Project::new("t");
        let mut page = Page::new("a", "1", "A");
        for (id, tag) in [("t1", "-X1.1"), ("t", "-X1")] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: "term".into(),
                at: Point::default(),
                rotation: Rotation::default(),
                mirror: false,
                tag: Some(DeviceTag::parse(tag)?),
            });
        }
        p.pages.push(page);
        p.meta.add_spare(&x1, "2", None, &[("1".to_owned(), false)])?;
        let role = |id: &str| (id == "term").then_some(SymbolRole::Terminal);
        let numbers: Vec<String> = terminal_numbers(&p, role).into_values().map(|(_, n)| n).collect();
        assert!(numbers.contains(&"3".to_owned()), "{numbers:?}");
        assert!(!numbers.contains(&"2".to_owned()), "{numbers:?}");
        // A spare far past the drawn numbers does not renumber it.
        p.meta
            .add_spare(&x1, "10", None, &[("1".to_owned(), false), ("3".to_owned(), false)])?;
        let numbers: Vec<String> = terminal_numbers(&p, role).into_values().map(|(_, n)| n).collect();
        assert!(numbers.contains(&"3".to_owned()), "{numbers:?}");
        Ok(())
    }

    #[test]
    fn jumpers_follow_renamed_moved_and_deleted_terminals() -> Result<(), String> {
        let t = |s: &str| DeviceTag::parse(s).map_err(|e| e.to_string());
        let key = |id: &str| ("a".to_owned(), id.to_owned());
        let nums = |v: &[(&str, &str, &str)]| -> Result<TerminalNumbers, String> {
            v.iter()
                .map(|(id, s, n)| Ok((key(id), (t(s)?, (*n).to_owned()))))
                .collect()
        };
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let x1 = t("-X1")?;
        let before = nums(&[("a", "-X1", "3"), ("b", "-X1", "4"), ("c", "-X1", "5")])?;
        let jumpered = || -> Result<ProjectMeta, String> {
            let mut m = ProjectMeta::new("t");
            m.add_jumper(&x1, &s(&["3", "4"]))?;
            Ok(m)
        };
        // Renamed within the strip: 3 becomes 7.
        let mut m = jumpered()?;
        assert!(m.follow_terminals(
            &before,
            &nums(&[("a", "-X1", "7"), ("b", "-X1", "4"), ("c", "-X1", "5")])?
        ));
        assert_eq!(m.jumpers[0].terminals, s(&["4", "7"]));
        // Strip-only terminals moved: a and c swap their numbers; the jumper stays on a and b.
        let mut m = jumpered()?;
        m.follow_terminals(
            &before,
            &nums(&[("a", "-X1", "5"), ("b", "-X1", "4"), ("c", "-X1", "3")])?,
        );
        assert_eq!(m.jumpers[0].terminals, s(&["4", "5"]));
        // Deleted: one terminal left is no jumper.
        let mut m = jumpered()?;
        m.follow_terminals(&before, &nums(&[("a", "-X1", "3"), ("c", "-X1", "5")])?);
        assert!(m.jumpers.is_empty());
        // The whole strip renamed: the jumper goes with it.
        let mut m = jumpered()?;
        m.follow_terminals(&before, &nums(&[("a", "-X2", "3"), ("b", "-X2", "4")])?);
        assert_eq!(
            (&m.jumpers[0].strip, m.jumpers[0].terminals.clone()),
            (&t("-X2")?, s(&["3", "4"]))
        );
        // One terminal moved to another strip: it leaves the jumper.
        let mut m = ProjectMeta::new("t");
        m.add_jumper(&x1, &s(&["3", "4", "5"]))?;
        m.follow_terminals(
            &before,
            &nums(&[("a", "-X9", "1"), ("b", "-X1", "4"), ("c", "-X1", "5")])?,
        );
        assert_eq!(
            (&m.jumpers[0].strip, m.jumpers[0].terminals.clone()),
            (&x1, s(&["4", "5"]))
        );
        // Only one member drawn, and it moved to another strip: the undrawn ones stay on X1.
        let mut m = ProjectMeta::new("t");
        m.add_jumper(&x1, &s(&["3", "8", "9"]))?;
        m.follow_terminals(&nums(&[("a", "-X1", "3")])?, &nums(&[("a", "-X9", "1")])?);
        assert_eq!(m.jumpers.len(), 1);
        assert_eq!(
            (&m.jumpers[0].strip, m.jumpers[0].terminals.clone()),
            (&x1, s(&["8", "9"]))
        );
        // Nothing changed: nothing to do; a terminal that is not drawn stays.
        let mut m = ProjectMeta::new("t");
        m.add_jumper(&x1, &s(&["4", "8"]))?;
        assert!(!m.follow_terminals(&before, &before));
        Ok(())
    }

    #[test]
    fn a_terminal_drawn_twice_stays_while_one_drawing_keeps_its_number() -> Result<(), String> {
        let t = |s: &str| DeviceTag::parse(s).map_err(|e| e.to_string());
        let key = |id: &str| ("a".to_owned(), id.to_owned());
        let x1 = t("-X1")?;
        let before: TerminalNumbers = [
            (key("a1"), (x1.clone(), "3".to_owned())),
            (key("a2"), (x1.clone(), "3".to_owned())),
            (key("b"), (x1.clone(), "4".to_owned())),
        ]
        .into();
        // The second drawing of 3 is renamed to 7; the first is still 3.
        let after: TerminalNumbers = [
            (key("a1"), (x1.clone(), "3".to_owned())),
            (key("a2"), (x1.clone(), "7".to_owned())),
            (key("b"), (x1.clone(), "4".to_owned())),
        ]
        .into();
        let mut m = ProjectMeta::new("t");
        m.add_jumper(&x1, &["3".into(), "4".into()])?;
        assert!(!m.follow_terminals(&before, &after));
        assert_eq!(m.jumpers[0].terminals, ["3", "4"]);
        Ok(())
    }

    #[test]
    fn a_pages_own_title_field_wins_and_is_kept_out_of_the_file_when_empty()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut meta = Project::new("P").meta;
        meta.title_block.insert("revision".into(), "A".into());
        let mut page = Page::new("p1", "1", "");
        assert_eq!(page.title_field(&meta, "revision"), "A");
        assert_eq!(page.title_field(&meta, "date"), "");
        let text = toml::to_string(&page)?;
        assert!(!text.contains("title_block"), "{text}");
        page.title_block.insert("revision".into(), "B".into());
        page.title_block.insert("date".into(), String::new());
        assert_eq!(page.title_field(&meta, "revision"), "B");
        assert_eq!(page.title_field(&meta, "date"), "", "a blank page value falls back");
        let text = toml::to_string(&page)?;
        let back: Page = toml::from_str(&text)?;
        assert_eq!(back, page);
        Ok(())
    }
}
