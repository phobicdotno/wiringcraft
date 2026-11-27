//! Schematic pages generated from a table: one copy of a macro per row, each row's cells filling
//! the macro's placeholders (`wiringcraft macro generate`).
//!
//! The table's first row names its columns. A column named like a declared placeholder (case
//! ignored) fills it; a column [`SET_COLUMN`] picks the row's value set, unless the macro
//! declares a placeholder of that name. Any other column is refused, so a typo is not ignored.
//! An empty cell gives no value: the value set's or the default applies.

use std::collections::{BTreeMap, BTreeSet};

use crate::geom::{Align, Shape};
use crate::geom::{Point, bounds_of};
use crate::macros::{Macro, key};
use crate::project::{Endpoint, FRAME_MARGIN, Page, PageSize, Project, natural_key};
use crate::symbol::Symbol;
use crate::{Error, Result};

/// The column that names a row's value set.
pub const SET_COLUMN: &str = "set";
/// Distance kept from the page frame, in millimetres.
pub const INSET: f64 = 10.0;
/// Distance kept between two copies, in millimetres.
pub const GAP: f64 = 10.0;
/// Copies are placed with their anchor on this grid, in millimetres.
pub const GRID: f64 = 5.0;

/// One row of the table: the line or worksheet row it came from, its value set and its values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableRow {
    pub line: usize,
    pub set: Option<String>,
    pub values: BTreeMap<String, String>,
}

/// What a column of the table fills.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Column {
    Placeholder(String),
    Set,
    /// A column without a name: its cells must be empty.
    Unnamed,
}

/// Read `cells` (the header row first) as the rows of a table driving `m`. `lines` gives the
/// line or worksheet row of each of `cells` for messages (row `i` is line `i + 1` without it).
pub fn rows_from_table(m: &Macro, cells: &[Vec<String>], lines: &[usize]) -> Result<Vec<TableRow>> {
    let line = |i: usize| lines.get(i).copied().unwrap_or(i + 1);
    // Escaped: a macro's name comes from its file and could hold control characters.
    let bad = |why: String| Err(Error::Invalid(format!("macro {:?}: {why}", m.name)));
    let Some((header, body)) = cells.split_first() else {
        return bad("the table is empty; its first row names the placeholders".into());
    };
    let placeholder = |name: &str| m.placeholders.iter().find(|p| key(&p.name) == key(name));
    let mut columns = Vec::with_capacity(header.len());
    let mut seen = BTreeSet::new();
    for name in header {
        let name = name.trim();
        let column = if name.is_empty() {
            Column::Unnamed
        } else if let Some(p) = placeholder(name) {
            Column::Placeholder(p.name.clone())
        } else if key(name) == SET_COLUMN {
            Column::Set
        } else {
            // Quoted as the column is, so a control character in a name is shown escaped.
            let known: Vec<String> = m.placeholders.iter().map(|p| format!("{:?}", p.name)).collect();
            let has = if known.is_empty() {
                "it has no placeholders".to_owned()
            } else {
                format!("it has {}", known.join(", "))
            };
            return bad(format!(
                "column {name:?} is not one of its placeholders ({has}; a column \"{SET_COLUMN}\" picks the value set)"
            ));
        };
        if column != Column::Unnamed && !seen.insert(key(name)) {
            return bad(format!("the table has two columns {name:?}"));
        }
        columns.push(column);
    }
    if body.is_empty() {
        return bad("the table has no rows below its header".into());
    }
    let mut rows = Vec::with_capacity(body.len());
    for (i, cells) in body.iter().enumerate() {
        let n = line(i + 1);
        let mut row = TableRow {
            line: n,
            ..TableRow::default()
        };
        for (c, cell) in cells.iter().enumerate() {
            let value = cell.trim();
            if value.is_empty() {
                continue;
            }
            match columns.get(c) {
                Some(Column::Placeholder(name)) => {
                    row.values.insert(name.clone(), value.to_owned());
                }
                Some(Column::Set) => row.set = Some(value.to_owned()),
                Some(Column::Unnamed) | None => {
                    return bad(format!(
                        "line {n}: cell {} has a value but its column has no name",
                        c + 1
                    ));
                }
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

/// The macro as each row gives it ([`Macro::apply`]), with the row's value set or else
/// `default_set`. The first row that cannot be applied is an error naming its line.
pub fn apply_rows(m: &Macro, rows: &[TableRow], default_set: Option<&str>) -> Result<Vec<Macro>> {
    rows.iter()
        .map(|row| {
            m.apply(row.set.as_deref().or(default_set), &row.values)
                .map_err(|err| Error::Invalid(format!("line {}: {err}", row.line)))
        })
        .collect()
}

/// The box round everything the copies draw, so one pitch fits each of them: their symbols
/// (through `symbol`, a missing one as [`Symbol::unknown_bounds`]) with their pins, their wires'
/// free ends and bends, and their drawings, texts as wide as they are drawn (their filled-in
/// values can differ from copy to copy).
pub fn extent(copies: &[Macro], symbol: impl Fn(&str) -> Option<Symbol>) -> (Point, Point) {
    let mut pts = Vec::new();
    for m in copies {
        for s in &m.symbols {
            match symbol(&s.symbol) {
                Some(def) => {
                    pts.push(s.at);
                    if let Some(tag) = &s.tag {
                        tag_points(s, &def, tag, &mut pts);
                    }
                    for g in &def.graphics {
                        drawn_points(&g.placed(s.at, s.rotation, s.mirror), &mut pts);
                    }
                    pts.extend(def.pins.iter().map(|pin| s.at + pin.at.transform(s.rotation, s.mirror)));
                }
                None => {
                    let (a, b) = Symbol::unknown_bounds(s.at);
                    pts.extend([a, b]);
                }
            }
        }
        for w in &m.wires {
            for end in [&w.from, &w.to] {
                if let Endpoint::Point { at } = end {
                    pts.push(*at);
                }
            }
            pts.extend(w.via.iter().copied());
            wire_label_points(m, w, &symbol, &mut pts);
        }
        for g in &m.graphics {
            drawn_points(g, &mut pts);
        }
    }
    if pts.is_empty() {
        pts.extend(copies.first().map(|m| m.anchor));
    }
    bounds_of(&pts)
}

/// The height the PDF and the canvas draw a device tag's lines at.
const TAG_HEIGHT: f64 = 2.5;

/// Points round a placed symbol's device tag as [`crate::tag::tag_lines`] lays it out: one line
/// per `=function`, `+location` and `-product`, centred on the tag position, right-aligned (left
/// where the placement turns that side round). Placing a copy renumbers the tag
/// ([`crate::placement::free_tag_like`]: its letter code and a number up to 9999) and the levels
/// under it, so the tag is measured as the macro has it and with the widest numbers placing can
/// give its levels.
fn tag_points(s: &crate::project::PlacedSymbol, def: &Symbol, tag: &crate::tag::DeviceTag, pts: &mut Vec<Point>) {
    // Each product level as it is or, when wider, as its letters with the largest number: a
    // renumbered tag (`-K` placed as `-K1000`) and a level under a renumbered parent (`-K.1`
    // placed as `-K1000.1`) are both within it.
    let mut widest = tag.clone();
    for level in &mut widest.product {
        let letters: String = level.chars().take_while(|c| !c.is_ascii_digit()).collect();
        let numbered = format!("{letters}9999");
        if crate::font::text_width_mm(&numbered, TAG_HEIGHT) > crate::font::text_width_mm(level, TAG_HEIGHT) {
            *level = numbered;
        }
    }
    for t in [tag, &widest] {
        let (align, lines) = crate::tag::tag_lines(s, def, t);
        for (at, line) in &lines {
            pts.extend(text_corners(*at, line, TAG_HEIGHT, align, 0.0));
        }
    }
}

/// Points round a wire's label (its number, potential and cable core) as the PDF draws it: on
/// the middle of its first segment, 2 mm high, right-aligned left of a vertical segment and
/// centred above a horizontal one. A wire without a label, or whose first end is on a pin that
/// is not there, adds nothing.
fn wire_label_points(
    m: &Macro,
    w: &crate::project::Wire,
    symbol: &impl Fn(&str) -> Option<Symbol>,
    pts: &mut Vec<Point>,
) {
    let core = w.cable.as_ref().map(|c| {
        if c.core.is_empty() {
            c.cable.to_string()
        } else {
            format!("{}:{}", c.cable, c.core)
        }
    });
    let label: Vec<&str> = [w.number.as_deref(), w.potential.as_deref(), core.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if label.is_empty() {
        return;
    }
    let end = |e: &Endpoint| match e {
        Endpoint::Point { at } => Some(*at),
        Endpoint::Pin { item, pin } => {
            let s = m.symbols.iter().find(|s| s.id == *item)?;
            let def = symbol(&s.symbol)?;
            let p = def.pins.iter().find(|p| p.name == *pin)?;
            Some(s.at + p.at.transform(s.rotation, s.mirror))
        }
    };
    let Some(p) = end(&w.from) else { return };
    let Some(q) = w.via.first().copied().or_else(|| end(&w.to)) else {
        return;
    };
    let mid = Point::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0);
    let text = label.join(" ");
    if (p.x - q.x).abs() < (p.y - q.y).abs() {
        pts.extend(text_corners(
            Point::new(mid.x - 1.0, mid.y + 0.8),
            &text,
            2.0,
            Align::Right,
            0.0,
        ));
    } else {
        pts.extend(text_corners(
            Point::new(mid.x, mid.y - 1.0),
            &text,
            2.0,
            Align::Center,
            0.0,
        ));
    }
}

/// Points round what a shape draws: a text as wide as it is drawn ([`text_corners`]), and a
/// dimension through its parts, so its measured-length label counts the same way.
fn drawn_points(g: &Shape, pts: &mut Vec<Point>) {
    match g {
        Shape::Text {
            at,
            text,
            height,
            align,
            angle,
        } => pts.extend(text_corners(*at, text, *height, *align, *angle)),
        Shape::Dimension { a, b, .. } => match g.dimension_parts() {
            Some(parts) if !parts.is_empty() => parts.iter().for_each(|p| drawn_points(p, pts)),
            _ => pts.extend([*a, *b]),
        },
        _ => {
            let (a, b) = g.bounds();
            pts.extend([a, b]);
        }
    }
}

/// The corners of the box a text is drawn in: `at` on its baseline, as wide as the PDF draws it
/// ([`crate::font::text_width_mm`]) on the side its alignment gives, `height` above the
/// baseline, turned `angle` degrees anticlockwise (y runs down).
fn text_corners(at: Point, text: &str, height: f64, align: Align, angle: f64) -> [Point; 4] {
    let width = crate::font::text_width_mm(text, height);
    let (from, to) = match align {
        Align::Left => (0.0, width),
        Align::Center => (-width / 2.0, width / 2.0),
        Align::Right => (-width, 0.0),
    };
    let (sin, cos) = angle.to_radians().sin_cos();
    // Along the text, and up from its baseline.
    let along = |u: f64| Point::new(u * cos, -u * sin);
    let rise = Point::new(-height * sin, -height * cos);
    [
        at + along(from),
        at + along(to),
        at + along(from) + rise,
        at + along(to) + rise,
    ]
}

/// What [`generate`] added.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Generated {
    /// The new pages' ids, in order.
    pub pages: Vec<String>,
    /// How many copies were placed.
    pub copies: usize,
}

fn up(v: f64) -> f64 {
    (v / GRID - 1e-9).ceil() * GRID
}

/// Place each of `copies` (macros already [applied](Macro::apply), all drawing the same group,
/// whose box is `extent`) on new pages of `size` titled `title`, added after the last page:
/// left to right, then row below row, inside the frame and above the title block, a new page
/// when one is full. Tags are renumbered as [`crate::macros::place`] does. Nothing changes when
/// one copy cannot be placed. The symbols and parts must be in the project already.
pub fn generate(
    project: &mut Project,
    copies: &[Macro],
    extent: (Point, Point),
    size: PageSize,
    title: &str,
) -> Result<Generated> {
    let Some(first) = copies.first() else {
        return Ok(Generated::default());
    };
    let (min, max) = extent;
    let (w, h) = size.mm();
    let band = project.meta.template.band_height();
    let (x0, y0) = (FRAME_MARGIN + INSET, FRAME_MARGIN + INSET);
    let (x1, y1) = (w - FRAME_MARGIN - INSET, h - FRAME_MARGIN - band - INSET);
    // The anchor goes on the grid, which can move the drawing right or down by up to a step.
    let (dx, dy) = (up(first.anchor.x - min.x), up(first.anchor.y - min.y));
    let slack_x = dx - (first.anchor.x - min.x);
    let slack_y = dy - (first.anchor.y - min.y);
    let (gw, gh) = (max.x - min.x + slack_x, max.y - min.y + slack_y);
    if gw > x1 - x0 || gh > y1 - y0 {
        return Err(Error::Invalid(format!(
            "macro {:?} is {:.0} x {:.0} mm; it does not fit the {:.0} x {:.0} mm drawing area of a {} page (choose a larger size)",
            first.name,
            max.x - min.x,
            max.y - min.y,
            (x1 - x0).max(0.0),
            (y1 - y0).max(0.0),
            size.label()
        )));
    }
    let (step_x, step_y) = (up(gw + GAP), up(gh + GAP));
    let across = 1 + ((x1 - x0 - gw) / step_x).floor() as usize;
    let down = 1 + ((y1 - y0 - gh) / step_y).floor() as usize;
    let per_page = across.saturating_mul(down).max(1);

    let mut work = project.clone();
    // After the last page in page order (see `Project::sort_pages`): its first number, one up.
    // `12.3` gives `13`, `A7` gives `A8`, so every new page sorts after every page there is.
    // Numbers are taken as `sort_pages` takes them, untrimmed: ` 4 ` gives ` 5`.
    let last = work
        .pages
        .iter()
        .map(|p| natural_key(&p.number))
        .max()
        .and_then(|key| key.into_iter().next());
    let (prefix, mut next_number) = last.unwrap_or_default();
    // A new number must not be one there is once spaces are trimmed (` 4 ` and `5` give ` 6`, not
    // ` 5`), as `page add` tells numbers apart.
    let numbers: BTreeSet<String> = work.pages.iter().map(|p| p.number.trim().to_owned()).collect();
    // Ids are told apart without regard to case, as the store does; one cursor walks the free ids.
    let ids: BTreeSet<String> = work.pages.iter().map(|p| p.id.to_ascii_lowercase()).collect();
    let mut id_cursor = 0u64;
    let mut out = Generated::default();
    let mut page_id = String::new();
    for (i, m) in copies.iter().enumerate() {
        let slot = i % per_page;
        if slot == 0 {
            id_cursor += 1;
            while ids.contains(&format!("p{id_cursor:04}")) {
                id_cursor += 1;
            }
            page_id = format!("p{id_cursor:04}");
            loop {
                next_number = next_number
                    .checked_add(1)
                    .ok_or_else(|| Error::Invalid(format!("no page number is left after {prefix}{next_number}")))?;
                if !numbers.contains(format!("{prefix}{next_number}").trim()) {
                    break;
                }
            }
            let mut page = Page::new(page_id.clone(), format!("{prefix}{next_number}"), title);
            page.size = size;
            work.pages.push(page);
            out.pages.push(page_id.clone());
        }
        let (col, row) = (slot % across, slot / across);
        let at = Point::new(x0 + dx + col as f64 * step_x, y0 + dy + row as f64 * step_y);
        crate::macros::place(&mut work, &page_id, m, at)
            .map_err(|err| Error::Invalid(format!("copy {}: {err}", i + 1)))?;
        out.copies += 1;
    }
    work.sort_pages();
    *project = work;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macros::{Placeholder, ValueSet};
    use crate::project::{PlacedSymbol, Wire};
    use crate::{Device, DeviceTag};

    fn starter() -> Result<Macro> {
        let tag = DeviceTag::parse("-Q1")?;
        Ok(Macro {
            name: "Starter".into(),
            symbols: vec![PlacedSymbol {
                id: "s1".into(),
                symbol: "wc.coil".into(),
                at: Point::new(20.0, 30.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(tag.clone()),
            }],
            wires: vec![Wire {
                id: "w1".into(),
                from: Endpoint::Pin {
                    item: "s1".into(),
                    pin: "A1".into(),
                },
                to: Endpoint::Point {
                    at: Point::new(20.0, 10.0),
                },
                via: Vec::new(),
                number: None,
                potential: None,
                cable: None,
                route: Default::default(),
                spec: Default::default(),
            }],
            devices: BTreeMap::from([(
                tag,
                Device {
                    description: "Motor {{kW}} kW".into(),
                    ..Default::default()
                },
            )]),
            anchor: Point::new(10.0, 10.0),
            placeholders: vec![
                Placeholder {
                    name: "kW".into(),
                    ..Default::default()
                },
                Placeholder {
                    name: "Name".into(),
                    default: "pump".into(),
                    ..Default::default()
                },
            ],
            value_sets: vec![
                ValueSet {
                    name: "small".into(),
                    values: BTreeMap::from([("kW".into(), "0.75".into())]),
                    ..Default::default()
                },
                ValueSet {
                    name: "big".into(),
                    values: BTreeMap::from([("kW".into(), "3".into())]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
    }

    fn table(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| r.iter().map(|c| (*c).to_owned()).collect())
            .collect()
    }

    fn err_of<T>(r: Result<T>) -> String {
        r.err().map(|e| e.to_string()).unwrap_or_default()
    }

    #[test]
    fn columns_fill_placeholders_and_the_set_column_picks_the_value_set() -> Result<()> {
        let m = starter()?;
        let rows = rows_from_table(
            &m,
            &table(&[&["KW", "set", ""], &["", "small", ""], &["5.5", " big ", ""]]),
            &[1, 2, 4],
        )?;
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.first().map(|r| (r.line, r.set.clone(), r.values.len())),
            Some((2, Some("small".into()), 0))
        );
        assert_eq!(
            rows.get(1).and_then(|r| r.values.get("kW")).map(String::as_str),
            Some("5.5")
        );
        let applied = apply_rows(&m, &rows, None)?;
        let descriptions: Vec<String> = applied
            .iter()
            .flat_map(|m| m.devices.values().map(|d| d.description.clone()))
            .collect();
        assert_eq!(descriptions, ["Motor 0.75 kW", "Motor 5.5 kW"]);
        Ok(())
    }

    #[test]
    fn a_bad_table_is_refused_with_why() -> Result<()> {
        let m = starter()?;
        let typo = err_of(rows_from_table(&m, &table(&[&["kw", "nmae"], &["1", "x"]]), &[]));
        assert!(typo.contains("\"nmae\"") && typo.contains("\"kW\", \"Name\""), "{typo}");
        let twice = err_of(rows_from_table(&m, &table(&[&["kW", "KW"], &["1", "2"]]), &[]));
        assert!(twice.contains("two columns"), "{twice}");
        let empty = err_of(rows_from_table(&m, &table(&[&["kW"]]), &[]));
        assert!(empty.contains("no rows"), "{empty}");
        let stray = err_of(rows_from_table(&m, &table(&[&["kW", ""], &["1", "x"]]), &[]));
        assert!(stray.contains("line 2") && stray.contains("cell 2"), "{stray}");
        // A row without a value set (and no default given) names its line.
        let rows = rows_from_table(&m, &table(&[&["kW", "set"], &["1", "small"], &["2", ""]]), &[1, 2, 3])?;
        let unset = err_of(apply_rows(&m, &rows, None));
        assert!(
            unset.contains("line 3") && unset.contains("choose a value set"),
            "{unset}"
        );
        assert_eq!(apply_rows(&m, &rows, Some("big"))?.len(), 2);
        Ok(())
    }

    #[test]
    fn a_macro_name_with_control_characters_is_shown_escaped() -> Result<()> {
        let mut m = starter()?;
        m.name = "evil\u{1b}[31m".into();
        if let Some(p) = m.placeholders.first_mut() {
            p.name = "bad\u{7}name".into();
        }
        let typo = err_of(rows_from_table(&m, &table(&[&["nmae"], &["x"]]), &[]));
        assert!(!typo.contains('\u{7}') && typo.contains("\"bad\\u{7}name\""), "{typo}");
        let set = err_of(apply_rows(
            &m,
            &rows_from_table(&m, &table(&[&["set"], &["nope"]]), &[])?,
            None,
        ));
        for err in [typo, set] {
            assert!(!err.contains('\u{1b}') && err.contains("\\u{1b}"), "{err}");
        }
        Ok(())
    }

    #[test]
    fn copies_fill_pages_in_rows_and_get_their_own_tags() -> Result<()> {
        let m = starter()?;
        let mut project = Project::new("x");
        project.pages.push(Page::new("p0001", "1", "Overview"));
        // More copies than one A4 page holds (13 across, 4 down).
        let rows: Vec<TableRow> = (0..60)
            .map(|i| TableRow {
                line: i + 2,
                set: Some("small".into()),
                values: BTreeMap::new(),
            })
            .collect();
        let copies = apply_rows(&m, &rows, None)?;
        // The coil drawn as an unknown symbol: 8 mm round (20, 30), with the wire up to y 10.
        let ext = extent(&copies, |_| None);
        assert_eq!(ext, (Point::new(16.0, 10.0), Point::new(24.0, 34.0)));
        let generated = generate(&mut project, &copies, ext, PageSize::A4, "Motors")?;
        assert_eq!(generated.copies, 60);
        assert_eq!(generated.pages, ["p0002", "p0003"]);
        assert_eq!(generated.pages.first().map(String::as_str), Some("p0002"));
        let page = project.page("p0002").ok_or(Error::Invalid("no page".into()))?;
        assert_eq!((page.number.as_str(), page.title.as_str()), ("2", "Motors"));
        // Every copy has its own tag and lies inside the drawing area.
        let (w, h) = PageSize::A4.mm();
        let band = project.meta.template.band_height();
        let mut tags = BTreeSet::new();
        for p in project.pages.iter().filter(|p| generated.pages.contains(&p.id)) {
            for s in &p.symbols {
                assert!(tags.insert(s.tag.clone()), "tag twice: {:?}", s.tag);
                let (a, b) = Symbol::unknown_bounds(s.at);
                assert!(a.x >= FRAME_MARGIN && b.x <= w - FRAME_MARGIN, "{:?}", s.at);
                assert!(a.y >= FRAME_MARGIN && b.y <= h - FRAME_MARGIN - band, "{:?}", s.at);
                assert_eq!((s.at.x % GRID, s.at.y % GRID), (0.0, 0.0), "{:?}", s.at);
            }
        }
        assert_eq!(tags.len(), 60);
        assert!(project.devices.values().all(|d| d.description == "Motor 0.75 kW"));
        Ok(())
    }

    #[test]
    fn the_extent_holds_every_copy_s_texts_and_the_pins() -> Result<()> {
        let m = starter()?;
        let label = |text: &str| -> Result<Macro> {
            let mut c = m.apply(Some("small"), &BTreeMap::new())?;
            c.graphics.push(Shape::Text {
                at: Point::new(20.0, 40.0),
                text: text.into(),
                height: 5.0,
                align: Align::Left,
                angle: 0.0,
            });
            Ok(c)
        };
        let (short, long) = (label("M1")?, label("Main extract fan, north wing")?);
        let width = crate::font::text_width_mm("Main extract fan, north wing", 5.0);
        let (_, max) = extent(&[short.clone(), long.clone()], |_| None);
        assert!((max.x - (20.0 + width)).abs() < 1e-9, "{max:?}");
        // The copies stay apart on the page: the second starts past the long label.
        let mut project = Project::new("x");
        // A later copy's longer label sets the pitch for all of them.
        let copies = [short.clone(), long, short];
        let ext = extent(&copies, |_| None);
        generate(&mut project, &copies, ext, PageSize::A3, "Fans")?;
        // Every label stays inside the frame.
        let (fx0, fx1) = (FRAME_MARGIN, PageSize::A3.mm().0 - FRAME_MARGIN);
        for page in &project.pages {
            for g in &page.graphics {
                let mut pts = Vec::new();
                drawn_points(g, &mut pts);
                assert!(pts.iter().all(|p| p.x >= fx0 && p.x <= fx1), "{g:?}");
            }
        }
        let xs: Vec<f64> = project
            .pages
            .iter()
            .flat_map(|p| p.symbols.iter().map(|s| s.at.x))
            .collect();
        assert!(xs.windows(2).all(|w| w[1] - w[0] >= width + GAP - 1e-9), "{xs:?}");
        // A pin far out on a symbol that draws nothing counts too.
        let def = Symbol {
            id: "wc.coil".into(),
            pins: vec![crate::symbol::Pin {
                name: "1".into(),
                at: Point::new(400.0, 0.0),
                dir: Default::default(),
            }],
            name: "Coil".into(),
            category: String::new(),
            letter_code: String::new(),
            role: Default::default(),
            graphics: Vec::new(),
            tag_at: Point::default(),
            description: String::new(),
            source: String::new(),
        };
        let one = m.apply(Some("small"), &BTreeMap::new())?;
        let ext = extent(std::slice::from_ref(&one), |_| Some(def.clone()));
        assert_eq!(ext.1.x, 420.0);
        let err = err_of(generate(
            &mut Project::new("x"),
            std::slice::from_ref(&one),
            ext,
            PageSize::A4,
            "Wide",
        ));
        assert!(err.contains("does not fit"), "{err}");
        // A symbol drawn as text alone counts as wide as the text is drawn.
        let words = "Emergency stop, main cabinet";
        let def = Symbol {
            pins: Vec::new(),
            graphics: vec![Shape::Text {
                at: Point::default(),
                text: words.into(),
                height: 5.0,
                align: Align::Left,
                angle: 0.0,
            }],
            ..def
        };
        let at = one.symbols.first().map(|s| s.at).unwrap_or_default();
        let ext = extent(std::slice::from_ref(&one), |_| Some(def.clone()));
        let width = crate::font::text_width_mm(words, 5.0);
        assert!(ext.1.x >= at.x + width - 1e-9, "{ext:?} {at:?}");
        // Copies placed with it keep their texts apart.
        let copies = [one.clone(), one.clone(), one];
        let mut project = Project::new("x");
        generate(&mut project, &copies, ext, PageSize::A3, "Stops")?;
        let mut xs: Vec<f64> = project
            .pages
            .iter()
            .flat_map(|p| p.symbols.iter().filter(|s| s.symbol == "wc.coil").map(|s| s.at.x))
            .collect();
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        assert!(
            xs.len() == 3 && xs.windows(2).all(|w| w[1] - w[0] >= width + GAP - 1e-9),
            "{xs:?}"
        );
        Ok(())
    }

    #[test]
    fn a_symbol_s_device_tag_counts_as_drawn() -> Result<()> {
        let one = starter()?.apply(Some("small"), &BTreeMap::new())?;
        let s = one.symbols.first().ok_or(Error::Invalid("no symbol".into()))?;
        let tag = s.tag.clone().ok_or(Error::Invalid("no tag".into()))?;
        // The tag sits 30 mm left of the symbol, right-aligned: it grows further left.
        let def = Symbol {
            id: "wc.coil".into(),
            pins: Vec::new(),
            name: "Coil".into(),
            category: String::new(),
            letter_code: String::new(),
            role: Default::default(),
            graphics: Vec::new(),
            tag_at: Point::new(-30.0, 0.0),
            description: String::new(),
            source: String::new(),
        };
        let (min, _) = extent(std::slice::from_ref(&one), |_| Some(def.clone()));
        let width = crate::font::text_width_mm(&format!("-{}9999", tag.letter_code()), TAG_HEIGHT);
        assert!(min.x <= s.at.x - 30.0 - width + 1e-9, "{min:?}");
        // A tag with no number yet: once -K1 to -K999 are taken, a copy is placed as -K1000,
        // a digit wider than any three-digit allowance.
        let mut bare = one.clone();
        if let Some(first) = bare.symbols.first_mut() {
            first.tag = Some(crate::tag::DeviceTag::parse("-K")?);
        }
        let (min, _) = extent(std::slice::from_ref(&bare), |_| Some(def.clone()));
        let width = crate::font::text_width_mm("-K1000", TAG_HEIGHT);
        assert!(min.x <= s.at.x - 30.0 - width + 1e-9, "{min:?}");
        // A level under a renumbered parent: -K.1 is placed as -K1000.1 beside -K1000.
        if let Some(first) = bare.symbols.first_mut() {
            first.tag = Some(crate::tag::DeviceTag::parse("-K.1")?);
        }
        let (min, _) = extent(std::slice::from_ref(&bare), |_| Some(def.clone()));
        let width = crate::font::text_width_mm("-K1000.1", TAG_HEIGHT);
        assert!(min.x <= s.at.x - 30.0 - width + 1e-9, "{min:?}");
        Ok(())
    }

    #[test]
    fn a_dimension_s_label_counts_as_wide_as_it_is_drawn() -> Result<()> {
        let mut c = starter()?.apply(Some("small"), &BTreeMap::new())?;
        // A 1 mm dimension whose label is wider than its line.
        let dim = Shape::Dimension {
            a: Point::new(200.0, 40.0),
            b: Point::new(201.0, 40.0),
            offset: 10.0,
            height: 5.0,
        };
        c.graphics.push(dim.clone());
        let (min, max) = extent(std::slice::from_ref(&c), |_| None);
        let parts = dim.dimension_parts().unwrap_or_default();
        let mut label = Vec::new();
        for p in parts.iter().filter(|p| matches!(p, Shape::Text { .. })) {
            drawn_points(p, &mut label);
        }
        assert!(!label.is_empty());
        let (lmin, lmax) = bounds_of(&label);
        assert!(lmax.x > 201.0 && lmin.x < 200.0, "{lmin:?} {lmax:?}");
        assert!(min.x <= lmin.x && min.y <= lmin.y && max.x >= lmax.x && max.y >= lmax.y);
        // Copies placed with it keep their labels apart.
        let mut project = Project::new("x");
        generate(&mut project, &[c.clone(), c], (min, max), PageSize::A3, "Dims")?;
        let mut boxes = Vec::new();
        for g in project.pages.iter().flat_map(|p| &p.graphics) {
            for part in g.dimension_parts().unwrap_or_default() {
                if matches!(part, Shape::Text { .. }) {
                    let mut pts = Vec::new();
                    drawn_points(&part, &mut pts);
                    boxes.push(bounds_of(&pts));
                }
            }
        }
        assert_eq!(boxes.len(), 2);
        let [(a0, a1), (b0, b1)] = [boxes[0], boxes[1]];
        let apart = a1.x <= b0.x || b1.x <= a0.x || a1.y <= b0.y || b1.y <= a0.y;
        assert!(apart, "{boxes:?}");
        Ok(())
    }

    #[test]
    fn a_wire_label_counts_as_wide_as_it_is_drawn() -> Result<()> {
        let mut one = starter()?.apply(Some("small"), &BTreeMap::new())?;
        // A 10 mm horizontal wire whose potential name is far wider than the wire.
        let name = "L1-MAIN-SUPPLY-FROM-THE-INCOMING-FEEDER";
        let wire = crate::project::Wire {
            id: "wl".into(),
            from: Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            to: Endpoint::Point {
                at: Point::new(10.0, 0.0),
            },
            via: vec![],
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        };
        one.wires = vec![wire.clone()];
        let (plain_min, plain_max) = extent(std::slice::from_ref(&one), |_| None);
        one.wires = vec![crate::project::Wire {
            potential: Some(name.into()),
            ..wire
        }];
        let (min, max) = extent(std::slice::from_ref(&one), |_| None);
        let half = crate::font::text_width_mm(name, 2.0) / 2.0;
        assert!(
            min.x <= 5.0 - half + 1e-9 && max.x >= 5.0 + half - 1e-9,
            "{min:?} {max:?}"
        );
        assert!(min.x < plain_min.x && max.x >= plain_max.x);
        Ok(())
    }

    #[test]
    fn new_pages_come_after_every_page_with_free_ids() -> Result<()> {
        let one = starter()?.apply(Some("small"), &BTreeMap::new())?;
        let ext = extent(std::slice::from_ref(&one), |_| None);
        for (numbers, want) in [
            (&["1", "12.3", "4"][..], "13"),
            (&["A7", "3"], "A8"),
            (&[" 4 ", "2"], " 5"),
            // ` 5` would be `5` once trimmed, which is taken.
            (&[" 4 ", "5"], " 6"),
            (&[], "1"),
        ] {
            let mut project = Project::new("x");
            for (i, n) in numbers.iter().enumerate() {
                // Ids differing only in case from the next free one are skipped.
                project.pages.push(Page::new(format!("P{:04}", i + 1), *n, "Old"));
            }
            let done = generate(&mut project, std::slice::from_ref(&one), ext, PageSize::A3, "New")?;
            let id = format!("p{:04}", numbers.len() + 1);
            assert_eq!(done.pages, std::slice::from_ref(&id));
            assert_eq!(project.page(&id).map(|p| p.number.as_str()), Some(want));
            assert_eq!(
                project.pages.last().map(|p| p.id.as_str()),
                Some(id.as_str()),
                "sorted last"
            );
        }
        let mut project = Project::new("x");
        project.pages.push(Page::new("p0001", u64::MAX.to_string(), "Last"));
        let err = err_of(generate(&mut project, &[one], ext, PageSize::A3, "New"));
        assert!(err.contains("no page number is left"), "{err}");
        Ok(())
    }

    #[test]
    fn a_macro_larger_than_the_page_changes_nothing() -> Result<()> {
        let m = starter()?.apply(Some("big"), &BTreeMap::new())?;
        let mut project = Project::new("x");
        let before = project.clone();
        let huge = (Point::new(0.0, 0.0), Point::new(500.0, 50.0));
        let err = err_of(generate(&mut project, &[m], huge, PageSize::A4, "Motors"));
        assert!(err.contains("does not fit") && err.contains("A4 landscape"), "{err}");
        assert_eq!(project, before);
        Ok(())
    }
}
