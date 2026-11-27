//! Schematic pages generated from a table: one copy of a macro per row, each row's cells filling
//! the macro's placeholders (`wiringcraft macro generate`).
//!
//! The table's first row names its columns. A column named like a declared placeholder (case
//! ignored) fills it; a column [`SET_COLUMN`] picks the row's value set, unless the macro
//! declares a placeholder of that name. Any other column is refused, so a typo is not ignored.
//! An empty cell gives no value: the value set's or the default applies.

use std::collections::{BTreeMap, BTreeSet};

use crate::geom::{Point, bounds_of};
use crate::macros::{Macro, key};
use crate::project::{Endpoint, FRAME_MARGIN, Page, PageSize, Project};
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
    let bad = |why: String| Err(Error::Invalid(format!("macro {}: {why}", m.name)));
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
            let known: Vec<&str> = m.placeholders.iter().map(|p| p.name.as_str()).collect();
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

/// The box round everything `m` draws: its symbols (through `symbol`, a missing one as
/// [`Symbol::unknown_bounds`]), its wires' free ends and bends, and its drawings.
pub fn extent(m: &Macro, symbol: impl Fn(&str) -> Option<Symbol>) -> (Point, Point) {
    let mut pts = Vec::new();
    for s in &m.symbols {
        let (a, b) = match symbol(&s.symbol) {
            Some(def) => def.placed_bounds(s.at, s.rotation, s.mirror),
            None => Symbol::unknown_bounds(s.at),
        };
        pts.extend([a, b]);
    }
    for w in &m.wires {
        for end in [&w.from, &w.to] {
            if let Endpoint::Point { at } = end {
                pts.push(*at);
            }
        }
        pts.extend(w.via.iter().copied());
    }
    for g in &m.graphics {
        let (a, b) = g.bounds();
        pts.extend([a, b]);
    }
    if pts.is_empty() {
        pts.push(m.anchor);
    }
    bounds_of(&pts)
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
            "macro {} is {:.0} x {:.0} mm; it does not fit the {:.0} x {:.0} mm drawing area of a {} page (choose a larger size)",
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
    let mut next_number = work
        .pages
        .iter()
        .filter_map(|p| p.number.trim().parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    let mut out = Generated::default();
    let mut page_id = String::new();
    for (i, m) in copies.iter().enumerate() {
        let slot = i % per_page;
        if slot == 0 {
            let mut n = 1u64;
            while work.pages.iter().any(|p| p.id == format!("p{n:04}")) {
                n += 1;
            }
            page_id = format!("p{n:04}");
            next_number += 1;
            while work.pages.iter().any(|p| p.number.trim() == next_number.to_string()) {
                next_number += 1;
            }
            let mut page = Page::new(page_id.clone(), next_number.to_string(), title);
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
        assert!(typo.contains("\"nmae\"") && typo.contains("kW, Name"), "{typo}");
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
        let ext = extent(&copies[0], |_| None);
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
