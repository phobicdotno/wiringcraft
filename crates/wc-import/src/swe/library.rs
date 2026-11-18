//! The libraries a SolidWorks Electrical archive carries: manufacturer parts with their
//! terminals, cable types with their cores, and symbol definitions with their connection points.
//!
//! Parts live in `tew_catalog.sqlite`: `tew_catreference` (one row per orderable part: reference,
//! manufacturer, description), `tew_catelement` (the part's circuits, e.g. a coil and its
//! contacts) and `tew_catterminal` (each circuit's terminal marks). Cable types and symbol
//! definitions live in `tew_app_data.sqlite`: `tew_cablereference` with its cores in
//! `tew_cablereferencewire`, and `tew_block` with its connection points in `tew_blockpt`.
//!
//! Every SolidWorks table names its columns `<prefix>_<field>`, with one prefix per table, and a
//! column pointing at another table's row is `<prefix>_<other prefix>_id` (`wir_cab_id`,
//! `caw_cab_id` in the project database). The prefixes of these library tables are not fixed
//! here: each table's prefix is the one most of its columns share, and fields are found by their
//! name after the prefix (`reference`, `manufacturer`, `txt`, ...). A table without the fields
//! this needs is skipped with a warning, so one odd table never stops the rest.
//!
//! Parts become WiringCraft parts (`<manufacturer>.<reference>` ids, as [`Part::make_id`]
//! builds them), with their terminals as properties; cable types become parts with their cores;
//! blocks become symbols (`swe.lib.<block>`) with their connection points as pins, drawn from the
//! block's DWG file in the archive (`Block/<name>.dwg`, read like the project import does, #230),
//! or as a box around the points when the archive has no readable drawing for the block. The
//! archive is treated as hostile like the project database: sizes are capped and unreadable rows
//! are counted, never a panic.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use rusqlite::Connection;
use wc_core::geom::Point;
use wc_core::{Device, DevicePart, Part, Symbol};

use super::{MAX_DATABASE_BYTES, SweDrawings, box_symbol, open, readable, slug, sql};
use crate::aml::Built;
use crate::{Error, Result};

/// The catalog database's file name in an archive.
pub const CATALOG_DB: &str = "tew_catalog.sqlite";
/// The application data database's file name in an archive (cable types, symbol blocks).
pub const APP_DATA_DB: &str = "tew_app_data.sqlite";

/// Property that tells a cable type apart from other parts.
pub const PROP_KIND: &str = "Kind";
/// Property listing a part's terminals, circuit by circuit: `Coil: A1 A2; NO: 13 14`.
pub const PROP_TERMINALS: &str = "Terminals";
/// Property listing a cable type's cores: `1 BN, 2 BU, 3 GNYE`.
pub const PROP_CORE_COLOURS: &str = "Core colours";
pub const PROP_CORES: &str = "Cores";

/// Optional reference columns kept as part properties: (field after the prefix, property name).
const EXTRA_FIELDS: &[(&str, &str)] = &[
    ("typedesignation", "Type designation"),
    ("family", "Family"),
    ("class", "Class"),
    ("classname", "Class"),
    ("voltage", "Voltage"),
    ("current", "Current"),
    ("power", "Power"),
    ("frequency", "Frequency"),
    ("width", "Width"),
    ("height", "Height"),
    ("depth", "Depth"),
    ("weight", "Weight"),
    ("eancode", "GTIN"),
    ("ean", "GTIN"),
    ("supplier", "Supplier"),
    ("suppliercode", "Supplier order number"),
    ("url", "URL"),
    ("comment", "Comment"),
];

/// What an archive's libraries gave, and what was left out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SweLibrary {
    /// Manufacturer parts, then cable types, sorted by id; one per id.
    pub parts: Vec<Part>,
    /// Box symbols with the blocks' connection points, sorted by id.
    pub symbols: Vec<Symbol>,
    pub report: LibraryReport,
    pub warnings: Vec<String>,
}

/// Counts for the import report: rows found in the source and items made from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LibraryReport {
    pub parts_in: usize,
    pub parts_out: usize,
    pub terminals: usize,
    pub cables_in: usize,
    pub cables_out: usize,
    pub blocks_in: usize,
    pub blocks_out: usize,
    /// Of the blocks imported, how many are drawn from their DWG file; the rest are boxes.
    pub blocks_drawn: usize,
}

impl LibraryReport {
    /// One line per kind: what, rows in the source, items imported.
    pub fn lines(&self) -> Vec<String> {
        vec![
            format!(
                "parts: {} in the catalog, {} imported, with {} terminal(s)",
                self.parts_in, self.parts_out, self.terminals
            ),
            format!(
                "cable types: {} in the source, {} imported",
                self.cables_in, self.cables_out
            ),
            format!(
                "symbol blocks: {} in the source, {} imported with their connection points: {} drawn from their DWG file, {} as boxes",
                self.blocks_in,
                self.blocks_out,
                self.blocks_drawn,
                self.blocks_out.saturating_sub(self.blocks_drawn)
            ),
        ]
    }
}

/// A table's columns, found by their field name after the table's prefix.
struct Table {
    name: &'static str,
    prefix: String,
    columns: Vec<String>,
}

impl Table {
    /// `Ok(None)` when the database has no such table.
    fn open(conn: &Connection, name: &'static str) -> Result<Option<Self>> {
        let mut st = conn.prepare("SELECT name FROM pragma_table_info(?1)").map_err(sql)?;
        let columns: Vec<String> = st
            .query_map([name], |r| r.get::<_, String>(0))
            .map_err(sql)?
            .collect::<rusqlite::Result<_>>()
            .map_err(sql)?;
        if columns.is_empty() {
            return Ok(None);
        }
        // The prefix most columns share; ties go to the first column's.
        let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (i, c) in columns.iter().enumerate() {
            if let Some((p, _)) = c.split_once('_') {
                let e = counts.entry(p).or_insert((0, i));
                e.0 += 1;
            }
        }
        let prefix = counts
            .iter()
            .max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.1.1.cmp(&a.1.1)))
            .map(|(p, _)| (*p).to_owned())
            .unwrap_or_default();
        Ok(Some(Self { name, prefix, columns }))
    }

    /// The column for the first of `fields` the table has (case-insensitive).
    fn field(&self, fields: &[&str]) -> Option<&str> {
        fields.iter().find_map(|f| {
            let want = format!("{}_{f}", self.prefix);
            self.columns
                .iter()
                .find(|c| c.eq_ignore_ascii_case(&want))
                .map(String::as_str)
        })
    }

    /// The column pointing at a row of `other`: `<prefix>_<other prefix>_id`.
    fn link(&self, other: &Table) -> Option<&str> {
        self.field(&[&format!("{}_id", other.prefix)])
    }

    /// The column for `fields`, or an error naming what is missing.
    fn need(&self, fields: &[&str]) -> std::result::Result<&str, String> {
        self.field(fields).ok_or_else(|| {
            format!(
                "{} has no {}_{} column",
                self.name,
                self.prefix,
                fields.first().copied().unwrap_or_default()
            )
        })
    }
}

/// `"col"` quoted for SQL; column names come from the database itself.
fn quoted(c: &str) -> String {
    format!("\"{}\"", c.replace('"', "\"\""))
}

/// A column as trimmed text, empty when NULL.
fn text(c: &str) -> String {
    format!("TRIM(COALESCE(CAST({} AS TEXT), ''))", quoted(c))
}

/// Parts with their terminals from a catalog database. Missing tables or fields are warnings.
pub fn read_catalog(conn: &Connection, report: &mut LibraryReport, warnings: &mut Vec<String>) -> Result<Vec<Part>> {
    let Some(refs) = Table::open(conn, "tew_catreference")? else {
        warnings.push("the catalog has no tew_catreference table; no parts were read".into());
        return Ok(Vec::new());
    };
    let (id, reference, maker) = match (
        refs.need(&["id"]),
        refs.need(&["reference", "partnumber", "ordernumber"]),
        refs.need(&["manufacturer", "manufacturername"]),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a.to_owned(), b.to_owned(), c.to_owned()),
        (a, b, c) => {
            let why = [a.err(), b.err(), c.err()]
                .into_iter()
                .flatten()
                .next()
                .unwrap_or_default();
            warnings.push(format!("no parts were read: {why}"));
            return Ok(Vec::new());
        }
    };
    let description = refs.field(&["description", "designation", "label"]).map(text);
    let extras: Vec<(&str, String)> = EXTRA_FIELDS
        .iter()
        .filter_map(|(f, prop)| refs.field(&[f]).map(|c| (*prop, text(c))))
        .collect();
    let mut select = vec![
        quoted(&id),
        text(&reference),
        text(&maker),
        description.unwrap_or_else(|| "''".into()),
    ];
    select.extend(extras.iter().map(|(_, c)| c.clone()));
    let q = format!(
        "SELECT {} FROM tew_catreference ORDER BY {}",
        select.join(", "),
        quoted(&id)
    );
    let mut st = conn.prepare(&q).map_err(sql)?;
    let n_extra = extras.len();
    let rows = st
        .query_map([], |r| {
            let mut props = Vec::with_capacity(n_extra);
            for i in 0..n_extra {
                props.push(r.get::<_, String>(4 + i)?);
            }
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                props,
            ))
        })
        .map_err(sql)?;
    // Every row the source has counts, also one that cannot be read.
    let rows: Vec<rusqlite::Result<_>> = rows.collect();
    report.parts_in = rows.len();
    let rows = readable(rows.into_iter(), "catalog part", warnings);

    let terminals = catalog_terminals(conn, &refs, report, warnings)?;
    let mut by_id: BTreeMap<String, Part> = BTreeMap::new();
    let mut no_reference = 0usize;
    let mut repeated = 0usize;
    for (row, reference, maker, description, props) in rows {
        let pid = Part::make_id(&maker, &reference);
        if reference.is_empty() || pid.is_empty() {
            no_reference += 1;
            continue;
        }
        if by_id.contains_key(&pid) {
            repeated += 1;
            continue;
        }
        let mut properties = BTreeMap::new();
        for ((prop, _), value) in extras.iter().zip(props) {
            if !value.is_empty() {
                properties.entry((*prop).to_owned()).or_insert(value);
            }
        }
        if let Some(t) = terminals.get(&row).filter(|t| !t.is_empty()) {
            properties.insert(PROP_TERMINALS.to_owned(), t.clone());
        }
        by_id.insert(
            pid.clone(),
            Part {
                id: pid,
                manufacturer: maker,
                order_number: reference,
                description,
                symbol: None,
                properties,
                contacts: None,
            },
        );
    }
    if no_reference > 0 {
        warnings.push(format!(
            "{no_reference} catalog part(s) without a reference were left out"
        ));
    }
    if repeated > 0 {
        warnings.push(format!(
            "{repeated} catalog part(s) repeat a manufacturer and reference already read; the first one was kept"
        ));
    }
    report.parts_out = by_id.len();
    Ok(by_id.into_values().collect())
}

/// Each catalog part's terminals as `Coil: A1 A2; NO: 13 14`, by part row id. Circuits without
/// a name list their marks only.
fn catalog_terminals(
    conn: &Connection,
    refs: &Table,
    report: &mut LibraryReport,
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<i64, String>> {
    let (Some(elements), Some(terms)) = (
        Table::open(conn, "tew_catelement")?,
        Table::open(conn, "tew_catterminal")?,
    ) else {
        warnings
            .push("the catalog has no tew_catelement or tew_catterminal table; parts come without terminals".into());
        return Ok(BTreeMap::new());
    };
    let found = (|| -> std::result::Result<_, String> {
        let el_id = elements.need(&["id"])?.to_owned();
        let el_ref = elements
            .link(refs)
            .ok_or_else(|| format!("tew_catelement has no {}_{}_id column", elements.prefix, refs.prefix))?
            .to_owned();
        let t_el = terms
            .link(&elements)
            .ok_or_else(|| format!("tew_catterminal has no {}_{}_id column", terms.prefix, elements.prefix))?
            .to_owned();
        let t_mark = terms.need(&["txt", "name", "label", "tag", "mark"])?.to_owned();
        Ok((el_id, el_ref, t_el, t_mark))
    })();
    let (el_id, el_ref, t_el, t_mark) = match found {
        Ok(f) => f,
        Err(why) => {
            warnings.push(format!("parts come without terminals: {why}"));
            return Ok(BTreeMap::new());
        }
    };
    let el_name = elements
        .field(&["name", "label", "tag", "type", "description"])
        .map(text)
        .unwrap_or_else(|| "''".into());
    let el_order = elements
        .field(&["orderno", "no"])
        .map(quoted)
        .unwrap_or_else(|| quoted(&el_id));
    let t_order = terms
        .field(&["no", "orderno"])
        .map(quoted)
        .unwrap_or_else(|| "rowid".into());

    let q = format!(
        "SELECT {}, {}, {el_name} FROM tew_catelement ORDER BY {}, {el_order}, {}",
        quoted(&el_id),
        quoted(&el_ref),
        quoted(&el_ref),
        quoted(&el_id)
    );
    let mut st = conn.prepare(&q).map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        })
        .map_err(sql)?;
    let elements_read = readable(rows, "catalog circuit", warnings);

    let q = format!(
        "SELECT {}, {} FROM tew_catterminal ORDER BY {}, {t_order}",
        quoted(&t_el),
        text(&t_mark),
        quoted(&t_el)
    );
    let mut st = conn.prepare(&q).map_err(sql)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(sql)?;
    let mut marks: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for (el, mark) in readable(rows, "catalog terminal", warnings) {
        if !mark.is_empty() {
            marks.entry(el).or_default().push(mark);
        }
    }

    let mut out: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for (el, part, name) in elements_read {
        let Some(m) = marks.remove(&el) else { continue };
        report.terminals += m.len();
        let list = m.join(" ");
        out.entry(part).or_default().push(if name.is_empty() {
            list
        } else {
            format!("{name}: {list}")
        });
    }
    let orphans: usize = marks.values().map(Vec::len).sum();
    if orphans > 0 {
        warnings.push(format!(
            "{orphans} catalog terminal(s) belong to no circuit of the catalog and were left out"
        ));
    }
    Ok(out.into_iter().map(|(k, v)| (k, v.join("; "))).collect())
}

/// Cable types with their cores, as parts. Missing tables or fields are warnings.
pub fn read_cable_types(
    conn: &Connection,
    report: &mut LibraryReport,
    warnings: &mut Vec<String>,
) -> Result<Vec<Part>> {
    let Some(cables) = Table::open(conn, "tew_cablereference")? else {
        warnings.push("no tew_cablereference table; no cable types were read".into());
        return Ok(Vec::new());
    };
    let (id, reference, maker) = match (
        cables.need(&["id"]),
        cables.need(&["reference", "partnumber"]),
        cables.need(&["manufacturer", "manufacturername"]),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a.to_owned(), b.to_owned(), c.to_owned()),
        (a, b, c) => {
            let why = [a.err(), b.err(), c.err()]
                .into_iter()
                .flatten()
                .next()
                .unwrap_or_default();
            warnings.push(format!("no cable types were read: {why}"));
            return Ok(Vec::new());
        }
    };
    let description = cables
        .field(&["description", "designation", "label"])
        .map(text)
        .unwrap_or_else(|| "''".into());
    let q = format!(
        "SELECT {}, {}, {}, {description} FROM tew_cablereference ORDER BY {}",
        quoted(&id),
        text(&reference),
        text(&maker),
        quoted(&id)
    );
    let mut st = conn.prepare(&q).map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(sql)?;
    // Every row the source has counts, also one that cannot be read.
    let rows: Vec<rusqlite::Result<_>> = rows.collect();
    report.cables_in = rows.len();
    let rows = readable(rows.into_iter(), "cable type", warnings);

    // Cores: (number, colour, cross-section) by cable row.
    let mut cores: BTreeMap<i64, Vec<(String, String, String)>> = BTreeMap::new();
    match Table::open(conn, "tew_cablereferencewire")? {
        None => warnings.push("no tew_cablereferencewire table; cable types come without cores".into()),
        Some(wires) => match wires.link(&cables) {
            None => warnings.push(format!(
                "cable types come without cores: tew_cablereferencewire has no {}_{}_id column",
                wires.prefix, cables.prefix
            )),
            Some(link) => {
                let no = wires.field(&["no", "number", "orderno"]);
                let colour = wires
                    .field(&["color", "colour"])
                    .map(text)
                    .unwrap_or_else(|| "''".into());
                let section = wires
                    .field(&["section", "sectionorgauge", "gauge"])
                    .map(text)
                    .unwrap_or_else(|| "''".into());
                let order = no.map(quoted).unwrap_or_else(|| "rowid".into());
                let q = format!(
                    "SELECT {}, {}, {colour}, {section} FROM tew_cablereferencewire ORDER BY {}, {order}",
                    quoted(link),
                    no.map(text).unwrap_or_else(|| "''".into()),
                    quoted(link)
                );
                let mut st = conn.prepare(&q).map_err(sql)?;
                let rows = st
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    })
                    .map_err(sql)?;
                for (cable, n, c, s) in readable(rows, "cable core", warnings) {
                    cores.entry(cable).or_default().push((n, c, s));
                }
            }
        },
    }

    let mut by_id: BTreeMap<String, Part> = BTreeMap::new();
    let mut skipped = 0usize;
    for (row, reference, maker, description) in rows {
        let pid = Part::make_id(&maker, &reference);
        if reference.is_empty() || pid.is_empty() || by_id.contains_key(&pid) {
            skipped += 1;
            continue;
        }
        let mut properties = BTreeMap::from([(PROP_KIND.to_owned(), "Cable".to_owned())]);
        let list = cores.remove(&row).unwrap_or_default();
        properties.insert(PROP_CORES.to_owned(), list.len().to_string());
        let colours: Vec<String> = list
            .iter()
            .enumerate()
            .map(|(i, (n, c, _))| {
                let n = if n.is_empty() { (i + 1).to_string() } else { n.clone() };
                if c.is_empty() { n } else { format!("{n} {c}") }
            })
            .collect();
        if list.iter().any(|(_, c, _)| !c.is_empty()) {
            properties.insert(PROP_CORE_COLOURS.to_owned(), colours.join(", "));
        }
        let sections: BTreeSet<&str> = list
            .iter()
            .map(|(_, _, s)| s.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if sections.len() == 1 {
            properties.insert(
                "Core cross-section".to_owned(),
                sections.into_iter().next().unwrap_or_default().to_owned(),
            );
        }
        by_id.insert(
            pid.clone(),
            Part {
                id: pid,
                manufacturer: maker,
                order_number: reference,
                description: if description.is_empty() {
                    "Cable".to_owned()
                } else {
                    description
                },
                symbol: None,
                properties,
                contacts: None,
            },
        );
    }
    if skipped > 0 {
        warnings.push(format!(
            "{skipped} cable type(s) without a reference, or repeating one already read, were left out"
        ));
    }
    report.cables_out = by_id.len();
    Ok(by_id.into_values().collect())
}

/// Symbol blocks with their connection points. A block `drawings` has a drawing for is drawn from
/// it (y down, the insertion point at the origin); the others are boxes around their points.
/// Missing tables or fields are warnings.
pub fn read_blocks(
    conn: &Connection,
    drawings: Option<&SweDrawings>,
    report: &mut LibraryReport,
    warnings: &mut Vec<String>,
) -> Result<Vec<Symbol>> {
    // The counts are for this call's result, also when the report was used before.
    (report.blocks_in, report.blocks_out, report.blocks_drawn) = (0, 0, 0);
    let Some(blocks) = Table::open(conn, "tew_block")? else {
        warnings.push("no tew_block table; no symbol blocks were read".into());
        return Ok(Vec::new());
    };
    let Some(points) = Table::open(conn, "tew_blockpt")? else {
        warnings.push("no tew_blockpt table; no symbol blocks were read".into());
        return Ok(Vec::new());
    };
    let found = (|| -> std::result::Result<_, String> {
        let id = blocks.need(&["id"])?.to_owned();
        let name = blocks.need(&["name", "blockname"])?.to_owned();
        let link = points
            .link(&blocks)
            .ok_or_else(|| format!("tew_blockpt has no {}_{}_id column", points.prefix, blocks.prefix))?
            .to_owned();
        let x = points.need(&["dx", "x", "posx"])?.to_owned();
        let y = points.need(&["dy", "y", "posy"])?.to_owned();
        Ok((id, name, link, x, y))
    })();
    let (id, name, link, x, y) = match found {
        Ok(f) => f,
        Err(why) => {
            warnings.push(format!("no symbol blocks were read: {why}"));
            return Ok(Vec::new());
        }
    };
    let description = blocks
        .field(&["description", "label"])
        .map(text)
        .unwrap_or_else(|| "''".into());
    let q = format!(
        "SELECT {}, {}, {description} FROM tew_block ORDER BY {}",
        quoted(&id),
        text(&name),
        quoted(&id)
    );
    let mut st = conn.prepare(&q).map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })
        .map_err(sql)?;
    // Every row the source has counts, also one that cannot be read.
    let rows: Vec<rusqlite::Result<_>> = rows.collect();
    report.blocks_in = rows.len();
    let rows = readable(rows.into_iter(), "symbol block", warnings);

    let no = points.field(&["no", "orderno"]);
    let pin_name = points.field(&["txt", "name", "label", "tag"]);
    let q = format!(
        "SELECT {}, {}, {}, {}, {} FROM tew_blockpt ORDER BY {}, {}",
        quoted(&link),
        quoted(&x),
        quoted(&y),
        pin_name.map(text).unwrap_or_else(|| "''".into()),
        no.map(text).unwrap_or_else(|| "''".into()),
        quoted(&link),
        no.map(quoted).unwrap_or_else(|| "rowid".into())
    );
    let mut st = conn.prepare(&q).map_err(sql)?;
    let rows_pt = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(sql)?;
    let mut pts: BTreeMap<i64, Vec<(String, Point)>> = BTreeMap::new();
    let mut bad = 0usize;
    for (block, x, y, label, n) in readable(rows_pt, "block connection point", warnings) {
        if !x.is_finite() || !y.is_finite() || x.abs() > 1e5 || y.abs() > 1e5 {
            bad += 1;
            continue;
        }
        let list = pts.entry(block).or_default();
        let pin = if !label.is_empty() {
            label
        } else if !n.is_empty() {
            n
        } else {
            (list.len() + 1).to_string()
        };
        // y up in the source, y down in WiringCraft symbols.
        list.push((pin, Point::new(x, -y)));
    }
    if bad > 0 {
        warnings.push(format!(
            "{bad} block connection point(s) without a usable position were left out"
        ));
    }

    let mut by_id: BTreeMap<String, Symbol> = BTreeMap::new();
    let mut skipped = 0usize;
    // One entity budget for every block drawing, like the project import's.
    let mut budget = drawings.map_or(0, SweDrawings::draw_limit);
    let mut not_drawn: BTreeMap<String, usize> = BTreeMap::new();
    // Blocks boxed, and blocks drawn only in part, because that budget ran out.
    let (mut over_budget, mut cut_short) = (0usize, 0usize);
    let mut drawn_blocks = 0usize;
    for (row, name, description) in rows {
        let Some(points) = pts.remove(&row).filter(|p| !p.is_empty()) else {
            skipped += 1;
            continue;
        };
        // Blocks whose names clean up alike get a number.
        let stem = format!("swe.lib.{}", slug(&name));
        let mut sid = stem.clone();
        let mut k = 1;
        while by_id.contains_key(&sid) {
            k += 1;
            sid = format!("{stem}.{k}");
        }
        let (boxed, pins) = box_symbol(&name, Point::new(0.0, 0.0), None, &points);
        let over = |n: &BTreeMap<String, usize>| n.get(crate::dxf::OVER_LIMIT).copied().unwrap_or(0);
        let over_before = over(&not_drawn);
        let drawn = drawings.and_then(|d| d.block_graphics(&name, &mut budget, &mut not_drawn));
        let hit_limit = over(&not_drawn) > over_before;
        let graphics = match drawn {
            Some(drawn) => {
                drawn_blocks += 1;
                cut_short += usize::from(hit_limit);
                drawn
            }
            None => {
                over_budget += usize::from(hit_limit);
                boxed
            }
        };
        by_id.insert(
            sid.clone(),
            Symbol {
                id: sid,
                name: if name.is_empty() {
                    "SolidWorks Electrical block".into()
                } else {
                    name
                },
                category: "SolidWorks Electrical library".to_owned(),
                letter_code: String::new(),
                role: Default::default(),
                graphics,
                pins,
                tag_at: Point::new(0.0, -super::MARGIN - 1.0),
                description,
            },
        );
    }
    if skipped > 0 {
        warnings.push(format!(
            "{skipped} symbol block(s) without connection points were left out (frames, texts, graphics)"
        ));
    }
    if over_budget > 0 {
        warnings.push(format!(
            "{over_budget} symbol block(s) are boxes because the drawings' entity limit was reached"
        ));
    }
    if cut_short > 0 {
        warnings.push(format!(
            "{cut_short} symbol block(s) are drawn only in part because the drawings' entity limit was reached"
        ));
    }
    not_drawn.remove(crate::dxf::OVER_LIMIT);
    if !not_drawn.is_empty() {
        let kinds: Vec<String> = not_drawn.iter().map(|(k, n)| format!("{k}: {n}")).collect();
        warnings.push(format!("block drawings: entities not drawn ({})", kinds.join(", ")));
    }
    report.blocks_out = by_id.len();
    report.blocks_drawn = drawn_blocks;
    Ok(by_id.into_values().collect())
}

/// Read every library a catalog and an application data database hold. Either may be missing.
/// Symbol blocks are boxes; [`read_with`] draws them from their DWG files.
pub fn read(catalog: Option<&Connection>, app_data: Option<&Connection>) -> Result<SweLibrary> {
    read_with(catalog, app_data, None)
}

/// [`read`], drawing each symbol block `drawings` has a drawing for from it.
pub fn read_with(
    catalog: Option<&Connection>,
    app_data: Option<&Connection>,
    drawings: Option<&SweDrawings>,
) -> Result<SweLibrary> {
    let mut lib = SweLibrary::default();
    if let Some(c) = catalog {
        lib.parts = read_catalog(c, &mut lib.report, &mut lib.warnings)?;
    } else {
        lib.warnings
            .push(format!("no {CATALOG_DB}; no manufacturer parts were read"));
    }
    if let Some(a) = app_data {
        let cables = read_cable_types(a, &mut lib.report, &mut lib.warnings)?;
        let before = lib.parts.len();
        let have: BTreeSet<String> = lib.parts.iter().map(|p| p.id.clone()).collect();
        lib.parts.extend(cables.into_iter().filter(|c| !have.contains(&c.id)));
        let clashes = lib.report.cables_out - (lib.parts.len() - before);
        if clashes > 0 {
            lib.report.cables_out -= clashes;
            lib.warnings.push(format!(
                "{clashes} cable type(s) have the id of a catalog part and were left out"
            ));
        }
        lib.parts.sort_by(|a, b| a.id.cmp(&b.id));
        lib.symbols = read_blocks(a, drawings, &mut lib.report, &mut lib.warnings)?;
    } else {
        lib.warnings
            .push(format!("no {APP_DATA_DB}; no cable types or symbol blocks were read"));
    }
    Ok(lib)
}

/// The archive entry named `file`: the one outside `Projects/` (the shared libraries), else the
/// shortest path, so a project's own copy is used only when the archive has nothing else.
fn find_entry<R: Read + std::io::Seek>(zip: &zip::ZipArchive<R>, file: &str) -> Option<String> {
    let mut found: Vec<&str> = zip
        .file_names()
        .filter(|n| n.rsplit('/').next().is_some_and(|f| f.eq_ignore_ascii_case(file)))
        .collect();
    found.sort_by_key(|n| (n.starts_with("Projects/"), n.matches('/').count(), n.to_string()));
    found.first().map(|s| (*s).to_owned())
}

fn read_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let f = zip.by_name(name).map_err(|e| Error::Format(format!("{name}: {e}")))?;
    let mut bytes = Vec::new();
    f.take(MAX_DATABASE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Format(format!("{name}: {e}")))?;
    if bytes.len() as u64 > MAX_DATABASE_BYTES {
        return Err(Error::Format(format!(
            "{name} is larger than {} MB",
            MAX_DATABASE_BYTES >> 20
        )));
    }
    Ok(bytes)
}

/// Read the libraries out of a project archive (`.tewzip`), drawing symbol blocks from the
/// archive's block files (`Block/<name>.dwg` outside `Projects/`). An archive with neither library
/// database is an error; one with only one of them gives what that one holds. A drawing that
/// cannot be read leaves its block a box and is a warning.
pub fn from_archive<R: Read + std::io::Seek>(reader: R) -> Result<SweLibrary> {
    from_archive_with(reader, true)
}

/// [`from_archive`] without reading any drawing: for callers that need only the parts.
pub fn catalog_from_archive<R: Read + std::io::Seek>(reader: R) -> Result<SweLibrary> {
    from_archive_with(reader, false)
}

/// What reading the block drawings left out: files that could not be read, and entities the DWG
/// reader does not read, by type (damaged objects are already in the files' warnings).
fn read_warnings(d: &SweDrawings) -> Vec<String> {
    // The warnings name archive entries, whose names the archive's author chose.
    let mut out: Vec<String> = d
        .warnings
        .iter()
        .map(|w| crate::printable(&format!("block drawings: {w}")))
        .collect();
    let kinds: Vec<String> = d
        .skipped
        .iter()
        .filter(|(k, _)| k.as_str() != crate::dwg::objects::DAMAGED)
        .map(|(k, n)| format!("{k}: {n}"))
        .collect();
    if !kinds.is_empty() {
        out.push(crate::printable(&format!(
            "block drawings: entities not read ({})",
            kinds.join(", ")
        )));
    }
    out
}

fn from_archive_with<R: Read + std::io::Seek>(reader: R, draw: bool) -> Result<SweLibrary> {
    let mut zip =
        zip::ZipArchive::new(reader).map_err(|e| Error::Format(format!("not a SolidWorks Electrical archive: {e}")))?;
    let catalog = find_entry(&zip, CATALOG_DB);
    let app_data = find_entry(&zip, APP_DATA_DB);
    if catalog.is_none() && app_data.is_none() {
        return Err(Error::Format(format!(
            "the archive holds neither {CATALOG_DB} nor {APP_DATA_DB}"
        )));
    }
    let catalog = match catalog {
        Some(n) => Some(open(&read_entry(&mut zip, &n)?)?),
        None => None,
    };
    let app_data = match app_data {
        Some(n) => Some(open(&read_entry(&mut zip, &n)?)?),
        None => None,
    };
    // Block drawings only matter when there are blocks to draw.
    let mut drawing_warnings = Vec::new();
    let drawings = if draw && app_data.is_some() {
        let mut reader = zip.into_inner();
        // No project folder: only the shared library's block files are read. A rewind that
        // fails leaves the blocks boxes, like a drawing read that fails.
        let read = reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| Error::Format(format!("archive: {e}")))
            .and_then(|_| super::drawings_from_archive(reader, ""));
        match read {
            Ok(d) => {
                drawing_warnings = read_warnings(&d);
                Some(d)
            }
            Err(e) => {
                drawing_warnings.push(crate::printable(&format!(
                    "block drawings were not read, blocks are boxes: {e}"
                )));
                None
            }
        }
    } else {
        None
    };
    let mut lib = read_with(catalog.as_ref(), app_data.as_ref(), drawings.as_ref())?;
    lib.warnings.extend(drawing_warnings);
    Ok(lib)
}

/// Give each imported device and cable the library part its `Manufacturer` and `PartNumber`
/// name, when `parts` has it, and add those parts to the import so they are vendored with it.
/// A part matches on its manufacturer and order number as written (case and outer spaces aside),
/// not on its id, which `RLY-24` and `RLY/24` share. The text properties stay, so reports that
/// read them are unchanged. Returns how many devices got a part.
pub fn link_parts(built: &mut Built, parts: &[Part]) -> usize {
    let key = |maker: &str, number: &str| (maker.trim().to_lowercase(), number.trim().to_lowercase());
    let by_id: BTreeMap<&str, &Part> = parts.iter().map(|p| (p.id.as_str(), p)).collect();
    let by_name: BTreeMap<(String, String), &Part> = parts
        .iter()
        .map(|p| (key(&p.manufacturer, &p.order_number), p))
        .collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut linked = 0;
    for d in built.devices.values_mut() {
        let find = |d: &Device, number: &str, maker: &str| {
            let number = d.properties.get(number).filter(|n| !n.trim().is_empty())?;
            let maker = d.properties.get(maker).map(String::as_str).unwrap_or_default();
            by_name.get(&key(maker, number)).copied()
        };
        if d.part.is_none()
            && let Some(p) = find(d, "PartNumber", "Manufacturer")
        {
            d.part = Some(p.id.clone());
            used.insert(p.id.clone());
            linked += 1;
        }
        // Further parts ("PartNumber 2", ...) become accessories; the same part twice counts 2.
        if d.accessories.is_empty() {
            let mut accessories: Vec<DevicePart> = Vec::new();
            for n in 2.. {
                let number = format!("PartNumber {n}");
                if !d.properties.contains_key(&number) {
                    break;
                }
                let Some(p) = find(d, &number, &format!("Manufacturer {n}")) else {
                    continue;
                };
                match accessories.iter_mut().find(|a| a.part == p.id) {
                    Some(a) => a.qty += 1,
                    None => accessories.push(DevicePart {
                        part: p.id.clone(),
                        qty: 1,
                    }),
                }
                used.insert(p.id.clone());
            }
            d.accessories = accessories;
        }
    }
    for id in used {
        if !built.parts.iter().any(|p| p.id == id)
            && let Some(p) = by_id.get(id.as_str())
        {
            built.parts.push((*p).clone());
        }
    }
    linked
}

#[cfg(test)]
mod tests;
