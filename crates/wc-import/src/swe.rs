//! SolidWorks Electrical projects.
//!
//! A project archive (`.tewzip`) is a zip. Each project in it has a SQLite database,
//! `Projects/<n>/Database/tew_project_data.sqlite`, which holds the electrical model: books
//! (`tew_bundle`), folders (`tew_folder`) and pages (`tew_file`), placed symbols (`tew_symbol`)
//! and their connection points (`tew_symbolpt`), components (`tew_component`) with their
//! terminals, wire lines (`tew_line`) with their potentials (`tew_equipotential`), the wires they
//! carry (`tew_linewirepair`, `tew_wire`), cables and their cores (`tew_cable`, `tew_cablewire`),
//! parts (`tew_buildofmaterial`) and translated texts (`tew_translatedtext`). A terminal strip is
//! a component of type 3; its terminals are components of type 4 whose `com_com_id` is the strip,
//! numbered by `com_tag` in `com_tagorderno` order. Each wire record names the component terminal
//! at both of its ends (`wir_cel_idfrom` + `wir_cte_nofrom`, and the same for `to`). Page drawings and
//! symbol graphics are DWG files and are not read: each placed symbol becomes a box with its
//! connection points where the database puts them.
//!
//! Database coordinates are millimetres with y up. A connection point is an offset in its
//! symbol's own frame: scaled by the symbol's scale (a negative scale mirrors), then rotated by
//! the symbol's angle, then moved to the symbol's position.
//!
//! Input is treated as hostile: missing tables or columns are errors; rows that cannot be read,
//! and everything left out, are counted in the warnings and the report; nothing panics.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use rusqlite::Connection;
use wc_core::connectivity::{PinRef, nets_linked};
use wc_core::geom::{Point, Shape};
use wc_core::symbol::{Pin, PinDir, SymbolRole};
use wc_core::{CableCore, Device, DeviceTag, Endpoint, Page, PageSize, PlacedSymbol, Symbol, Wire};

use crate::aml::Built;
use crate::{Error, Result};

/// `tew_file.fil_filetype` values (the product's `EwFileType`). Drawn as schematics: schematic
/// sheet, line diagram, mixed schematic, P&ID, fluid. The others are folders, reports (bill of
/// materials, terminal tables), cover pages, cabinet layouts and appendices.
pub const KIND_FOLIO: i64 = 0;
pub const KIND_LINE_DIAGRAM: i64 = 1;
pub const KIND_COVER: i64 = 5;
pub const KIND_MIXED: i64 = 12;
pub const KIND_PID: i64 = 13;
pub const KIND_FLUID: i64 = 15;
pub const DRAWN_KINDS: [i64; 5] = [KIND_FOLIO, KIND_LINE_DIAGRAM, KIND_MIXED, KIND_PID, KIND_FLUID];

/// `tew_component.com_type` of origin-destination arrows (`EwComponentType`): the two arrows
/// that carry a wire from one page to another. Imported as interruption points.
pub const COMPONENT_ARROW: i64 = 5;
/// `tew_component.com_type` of a terminal strip. Its terminals are [`COMPONENT_TERMINAL`]s.
pub const COMPONENT_TERMINAL_STRIP: i64 = 3;
/// `tew_component.com_type` of one terminal; `com_com_id` is its strip.
pub const COMPONENT_TERMINAL: i64 = 4;

/// The largest project database read out of an archive.
pub const MAX_DATABASE_BYTES: u64 = 1 << 30;

/// Pin and wire ends closer than this (mm) are the same point.
const SNAP: f64 = 0.05;
/// Wire ends this close (mm) to a pin, to another wire end or to another wire are joined to it:
/// the source's drawings are not always exact.
const JOIN: f64 = 0.5;
/// Box margin around a symbol's connection points (mm).
const MARGIN: f64 = 2.5;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SwePage {
    pub id: i64,
    pub order: i64,
    pub kind: i64,
    /// The page's own label in its book, e.g. `03`.
    pub label: String,
    pub title: String,
    pub folder: String,
    /// The book's number (`tew_bundle.bun_tag`), e.g. `1`.
    pub book: String,
    /// The title block (drawing frame) name; it names the sheet size, e.g. `..._A2_...`.
    pub frame: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwePin {
    pub name: String,
    /// Page position in millimetres, y up.
    pub at: Point,
    /// The component terminal this point stands for: (`cel_id`, `cte_no`), as wire records name
    /// their ends.
    pub terminal: Option<(i64, i64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SweSymbol {
    pub id: i64,
    pub page: i64,
    pub component: Option<i64>,
    pub block: String,
    /// Insertion point, y up.
    pub at: Point,
    /// The opposite corner of a box symbol (black box), y up.
    pub corner: Option<Point>,
    pub pins: Vec<SwePin>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SweLine {
    pub id: i64,
    pub page: i64,
    pub a: Point,
    pub b: Point,
    pub potential: Option<String>,
    /// The line's equipotential (`lin_equ_id`): lines with the same one are one net, also where
    /// the drawing leaves an end open or the net continues on another page.
    pub net: Option<i64>,
    /// The wires this line carries (`tew_wire.wir_id`), in order.
    pub wires: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SweWire {
    pub id: i64,
    pub cable: Option<i64>,
    /// The core's number in its cable.
    pub core: i64,
    pub tag: String,
    /// The component terminal at each end, (`cel_id`, `cte_no`), when the source names it.
    pub from: Option<(i64, i64)>,
    pub to: Option<(i64, i64)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SweCable {
    pub id: i64,
    pub tag: String,
    pub part_number: String,
    pub manufacturer: String,
    /// Core colour by core number (empty when the core has none).
    pub cores: BTreeMap<i64, String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SweComponent {
    pub id: i64,
    /// Full tag path as written in the database, e.g. `=F1+L1-K3`, or just `K3`.
    pub tag: String,
    /// `com_type`, see [`COMPONENT_ARROW`].
    pub kind: i64,
    pub description: String,
    pub part_number: String,
    pub manufacturer: String,
    /// The component this one belongs to (`com_com_id`): a terminal's strip.
    pub parent: Option<i64>,
    /// Place among its parent's children (`com_tagorderno`): a terminal's place on its strip.
    pub order: i64,
    /// Its own tag without the path (`com_tag`): a terminal's number.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SweProject {
    pub pages: Vec<SwePage>,
    /// How many books the project has.
    pub books: usize,
    pub symbols: Vec<SweSymbol>,
    pub lines: Vec<SweLine>,
    pub wires: BTreeMap<i64, SweWire>,
    pub cables: BTreeMap<i64, SweCable>,
    pub components: BTreeMap<i64, SweComponent>,
    pub locations: Vec<SweLocation>,
    pub warnings: Vec<String>,
}

/// A location (`tew_location`): a cabinet, room or panel.
#[derive(Debug, Clone, PartialEq)]
pub struct SweLocation {
    pub id: i64,
    /// Full tag path as written in the database, e.g. `+L1+L3`.
    pub tag_path: String,
    pub description: String,
}

/// A location tag path as WiringCraft writes the location part of a tag: `+L1+L3` is `L1.L3`.
/// Built with [`device_tag`], the same cleanup a device's tag path gets, so a device in this
/// location always carries exactly this path (`+Room 1` is `Room_1` on both sides). Empty when
/// that cleanup gives no valid location.
fn location_path(tag_path: &str) -> String {
    device_tag(&format!("{}-X", tag_path.trim()))
        .map(|t| t.location.join("."))
        .unwrap_or_default()
}

/// One row of the import report: how many of a kind the source has and how many came in.
#[derive(Debug, Clone, PartialEq)]
pub struct Count {
    pub what: &'static str,
    pub source: usize,
    pub imported: usize,
    /// Why the two differ, when they do.
    pub note: String,
}

fn sql(e: rusqlite::Error) -> Error {
    Error::Format(format!("SolidWorks Electrical database: {e}"))
}

/// The rows that could be read; the others are counted under `what`.
fn readable<T>(rows: impl Iterator<Item = rusqlite::Result<T>>, what: &str, warnings: &mut Vec<String>) -> Vec<T> {
    let mut bad = 0usize;
    let mut first: Option<String> = None;
    let out: Vec<T> = rows
        .filter_map(|r| match r {
            Ok(v) => Some(v),
            Err(e) => {
                bad += 1;
                first.get_or_insert_with(|| e.to_string());
                None
            }
        })
        .collect();
    if bad > 0 {
        // The first reason names the column and the problem, so the source can be fixed.
        let why = first.unwrap_or_default();
        warnings.push(format!(
            "{bad} {what} row(s) could not be read and were left out (first: {why})"
        ));
    }
    out
}

/// A project database taken out of an archive.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveDatabase {
    /// The project's folder in the archive, e.g. `Projects/4`.
    pub folder: String,
    /// How many projects the archive holds.
    pub count: usize,
    pub bytes: Vec<u8>,
}

/// Take project number `pick` (counted from 1, in project folder order) out of an archive. Only
/// that database is unpacked, so a large archive costs little memory when read from a file.
/// Snapshots stored inside the archive are skipped.
pub fn database_from_archive<R: Read + std::io::Seek>(reader: R, pick: usize) -> Result<ArchiveDatabase> {
    let mut zip =
        zip::ZipArchive::new(reader).map_err(|e| Error::Format(format!("not a SolidWorks Electrical archive: {e}")))?;
    let names = project_databases(&zip);
    let count = names.len();
    if count == 0 {
        return Err(Error::Format(
            "the archive holds no SolidWorks Electrical project database".into(),
        ));
    }
    let Some(name) = pick.checked_sub(1).and_then(|i| names.get(i)) else {
        return Err(Error::Format(format!(
            "the archive holds {count} project(s); pick one from 1 to {count}"
        )));
    };
    let f = zip.by_name(name).map_err(|e| Error::Format(format!("{name}: {e}")))?;
    let mut bytes = Vec::new();
    // Read at most one byte past the limit, whatever the zip header claims.
    f.take(MAX_DATABASE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Format(format!("{name}: {e}")))?;
    if bytes.len() as u64 > MAX_DATABASE_BYTES {
        return Err(Error::Format(format!(
            "{name} is larger than {} MB",
            MAX_DATABASE_BYTES >> 20
        )));
    }
    let folder = name.rsplitn(3, '/').nth(2).unwrap_or_default().to_owned();
    Ok(ArchiveDatabase { folder, count, bytes })
}

/// The project database paths in an archive, in project folder order.
fn project_databases<R: Read + std::io::Seek>(zip: &zip::ZipArchive<R>) -> Vec<String> {
    let mut names: Vec<String> = zip
        .file_names()
        .filter(|n| {
            let parts: Vec<&str> = n.split('/').collect();
            parts.len() == 4
                && parts[0] == "Projects"
                && parts[2] == "Database"
                && parts[3] == "tew_project_data.sqlite"
        })
        .map(str::to_owned)
        .collect();
    names.sort_by_key(|n| {
        let num = n
            .split('/')
            .nth(1)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(u64::MAX);
        (num, n.clone())
    });
    names
}

/// Open a project database held in memory.
pub fn open(bytes: &[u8]) -> Result<Connection> {
    let mut conn = Connection::open_in_memory().map_err(sql)?;
    conn.deserialize_read_exact(rusqlite::MAIN_DB, std::io::Cursor::new(bytes), bytes.len(), true)
        .map_err(sql)?;
    Ok(conn)
}

/// Texts by object id, English first, then any language.
fn texts(conn: &Connection, warnings: &mut Vec<String>) -> Result<BTreeMap<i64, String>> {
    let mut out: BTreeMap<i64, String> = BTreeMap::new();
    let mut st = conn
        .prepare(
            "SELECT tra_objectid, tra_0 FROM tew_translatedtext WHERE tra_objectno = -1 AND TRIM(tra_0) <> '' \
             ORDER BY tra_lan_strid = 'en'",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))
        .map_err(sql)?;
    // English sorts last, so it wins.
    for (id, text) in readable(rows, "text", warnings) {
        if let Some(t) = text {
            out.insert(id, t);
        }
    }
    Ok(out)
}

/// A connection point's page position: offset scaled, rotated by `angle` degrees, moved to `at`.
pub fn place(at: Point, angle: f64, scale: (f64, f64), offset: Point) -> Point {
    let (x, y) = (offset.x * scale.0, offset.y * scale.1);
    let r = angle.to_radians();
    let (s, c) = r.sin_cos();
    Point::new(at.x + x * c - y * s, at.y + x * s + y * c)
}

/// Read the drawable model of one project database.
pub fn read(conn: &Connection) -> Result<SweProject> {
    let mut p = SweProject::default();
    let mut warnings = Vec::new();
    let text = texts(conn, &mut warnings)?;

    // Books and pages.
    let mut st = conn
        .prepare("SELECT bun_id, COALESCE(bun_tag, '') FROM tew_bundle")
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(sql)?;
    let books: BTreeMap<i64, String> = readable(rows, "book", &mut warnings).into_iter().collect();
    p.books = books.len();

    let mut st = conn
        .prepare(
            "SELECT f.fil_id, COALESCE(f.fil_orderno, 0), COALESCE(f.fil_filetype, 0), COALESCE(f.fil_title, ''), \
             f.fil_fol_id, COALESCE(d.fol_name, ''), COALESCE(f.fil_titleblock, ''), f.fil_bun_id \
             FROM tew_file f LEFT JOIN tew_folder d ON d.fol_id = f.fil_fol_id \
             ORDER BY f.fil_orderno, f.fil_id",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<i64>>(7)?,
            ))
        })
        .map_err(sql)?;
    for (id, order, kind, label, folder_id, folder_name, frame, book) in readable(rows, "page", &mut warnings) {
        let folder = folder_id.and_then(|f| text.get(&f).cloned()).unwrap_or(folder_name);
        p.pages.push(SwePage {
            id,
            order,
            kind,
            label,
            title: text.get(&id).cloned().unwrap_or_default(),
            folder,
            book: book.and_then(|b| books.get(&b).cloned()).unwrap_or_default(),
            frame,
        });
    }

    // Components with their tag, description and first part.
    let mut st = conn
        .prepare(
            "SELECT com_id, COALESCE(NULLIF(com_tagpath, ''), com_tag, ''), COALESCE(com_type, -1), com_com_id, \
             COALESCE(com_tagorderno, 0), COALESCE(CAST(com_tag AS TEXT), '') FROM tew_component",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok(SweComponent {
                id: r.get(0)?,
                tag: r.get(1)?,
                kind: r.get(2)?,
                parent: r.get::<_, Option<i64>>(3)?.filter(|c| *c > 0),
                order: r.get(4)?,
                name: r.get(5)?,
                ..Default::default()
            })
        })
        .map_err(sql)?;
    for mut c in readable(rows, "component", &mut warnings) {
        c.description = text.get(&c.id).cloned().unwrap_or_default();
        p.components.insert(c.id, c);
    }
    let mut st = conn
        .prepare(
            "SELECT bom_objectid, COALESCE(bom_reference, ''), COALESCE(bom_manufacturer, '') FROM tew_buildofmaterial \
             WHERE bom_reference <> '' ORDER BY bom_objectid, bom_orderno, bom_id",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })
        .map_err(sql)?;
    let mut extra_parts = 0;
    for (id, part, maker) in readable(rows, "part", &mut warnings) {
        if let Some(c) = p.components.get_mut(&id) {
            if c.part_number.is_empty() {
                c.part_number = part;
                c.manufacturer = maker;
            } else {
                extra_parts += 1;
            }
        }
    }
    if extra_parts > 0 {
        warnings.push(format!(
            "{extra_parts} extra part(s) left out: a component keeps only its first part"
        ));
    }

    // Cables, their cores, and the wires.
    let mut st = conn
        .prepare("SELECT cab_id, COALESCE(cab_tag, ''), COALESCE(cab_reference, ''), COALESCE(cab_manufacturer, '') FROM tew_cable")
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok(SweCable {
                id: r.get(0)?,
                tag: r.get(1)?,
                part_number: r.get(2)?,
                manufacturer: r.get(3)?,
                cores: BTreeMap::new(),
            })
        })
        .map_err(sql)?;
    for c in readable(rows, "cable", &mut warnings) {
        p.cables.insert(c.id, c);
    }
    let mut st = conn
        .prepare("SELECT caw_cab_id, caw_no, COALESCE(caw_color, '') FROM tew_cablewire")
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        })
        .map_err(sql)?;
    for (cable, no, colour) in readable(rows, "cable core", &mut warnings) {
        if let Some(c) = p.cables.get_mut(&cable) {
            c.cores.insert(no, colour);
        }
    }
    let mut st = conn
        .prepare(
            "SELECT wir_id, wir_cab_id, COALESCE(wir_caw_no, -1), COALESCE(wir_tag, ''), \
             wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto FROM tew_wire",
        )
        .map_err(sql)?;
    // An end is named by its element and terminal number; an element of 0 or less is none.
    let end = |el: Option<i64>, no: Option<i64>| el.filter(|e| *e > 0).zip(no);
    let rows = st
        .query_map([], |r| {
            Ok(SweWire {
                id: r.get(0)?,
                cable: r.get::<_, Option<i64>>(1)?.filter(|c| *c > 0),
                core: r.get(2)?,
                tag: r.get(3)?,
                from: end(r.get(4)?, r.get(5)?),
                to: end(r.get(6)?, r.get(7)?),
            })
        })
        .map_err(sql)?;
    for w in readable(rows, "wire", &mut warnings) {
        p.wires.insert(w.id, w);
    }
    let mut line_wires: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut st = conn
        .prepare("SELECT lwp_lin_id, lwp_wir_id FROM tew_linewirepair ORDER BY lwp_lin_id, lwp_wir_orderno, lwp_wir_id")
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(sql)?;
    for (line, wire) in readable(rows, "line-wire link", &mut warnings) {
        line_wires.entry(line).or_default().push(wire);
    }

    // Connection points by symbol, named after the component terminal they stand for.
    struct RawPin {
        circuit: i64,
        no: i64,
        offset: Point,
        terminal: Option<String>,
        /// The element's order in its component, when the component is drawn as several
        /// symbols (terminal strips, connectors drawn pin by pin).
        element: Option<i64>,
        /// (`cel_id`, `cte_no`) of the component terminal, when there is one.
        stands_for: Option<(i64, i64)>,
    }
    let mut pins: BTreeMap<i64, Vec<RawPin>> = BTreeMap::new();
    let mut st = conn
        .prepare(
            "SELECT p.spt_sym_id, COALESCE(p.spt_sct_no, 0), COALESCE(p.spt_no, 0), p.spt_dx, p.spt_dy, t.cte_txt, \
             CASE WHEN (SELECT COUNT(*) FROM tew_componentelement x WHERE x.cel_com_id = e.cel_com_id AND x.cel_sym_id > 0) > 1 \
             THEN e.cel_orderno END, e.cel_id, t.cte_no \
             FROM tew_symbolpt p \
             LEFT JOIN tew_componentelement e ON e.cel_sym_id = p.spt_sym_id AND e.cel_sct_no = p.spt_sct_no \
             LEFT JOIN tew_componentterminal t ON t.cte_cel_id = e.cel_id AND t.cte_no = p.spt_no \
             ORDER BY p.spt_sym_id, p.spt_sct_no, p.spt_no",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                RawPin {
                    circuit: r.get(1)?,
                    no: r.get(2)?,
                    offset: Point::new(r.get(3)?, r.get(4)?),
                    terminal: r.get(5)?,
                    element: r.get(6)?,
                    stands_for: r.get::<_, Option<i64>>(7)?.zip(r.get::<_, Option<i64>>(8)?),
                },
            ))
        })
        .map_err(sql)?;
    for (sym, pin) in readable(rows, "connection point", &mut warnings) {
        let list = pins.entry(sym).or_default();
        // A joined terminal can repeat a point; keep the first.
        if !list.iter().any(|q| q.circuit == pin.circuit && q.no == pin.no) {
            list.push(pin);
        }
    }

    let mut st = conn
        .prepare(
            "SELECT sym_id, sym_fil_id, sym_com_id, COALESCE(sym_blockname, ''), sym_posx, sym_posy, \
             COALESCE(sym_angle, 0), COALESCE(sym_scalex, 1), COALESCE(sym_scaley, 1), sym_posendx, sym_posendy \
             FROM tew_symbol ORDER BY sym_fil_id, sym_id",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, Option<f64>>(5)?,
                r.get::<_, f64>(6)?,
                r.get::<_, f64>(7)?,
                r.get::<_, f64>(8)?,
                r.get::<_, Option<f64>>(9)?,
                r.get::<_, Option<f64>>(10)?,
            ))
        })
        .map_err(sql)?;
    let mut unplaced = 0;
    let mut bad_points = 0;
    for (id, page, component, block, x, y, angle, sx, sy, ex, ey) in readable(rows, "symbol", &mut warnings) {
        let (Some(x), Some(y)) = (x, y) else {
            unplaced += 1;
            continue;
        };
        if !(x.is_finite() && y.is_finite() && angle.is_finite() && sx.is_finite() && sy.is_finite()) {
            unplaced += 1;
            continue;
        }
        let at = Point::new(x, y);
        let scale = (if sx == 0.0 { 1.0 } else { sx }, if sy == 0.0 { 1.0 } else { sy });
        let mut out_pins: Vec<SwePin> = Vec::new();
        for raw in pins.remove(&id).unwrap_or_default() {
            // Finite inputs can still overflow in the transform (a huge scale).
            let placed = place(at, angle, scale, raw.offset);
            if !(placed.x.is_finite() && placed.y.is_finite()) {
                bad_points += 1;
                continue;
            }
            // Unnamed terminals: numbered by element (terminal strips, connectors drawn pin by
            // pin, so each symbol of the device has its own names), else by circuit, else 1, 2...
            let n = raw.no.saturating_add(1);
            let name = raw
                .terminal
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| match raw.element {
                    Some(el) => format!("{}.{n}", el.saturating_add(1)),
                    None if raw.circuit > 0 => format!("{}.{n}", raw.circuit),
                    None => n.to_string(),
                });
            out_pins.push(SwePin {
                name,
                at: placed,
                terminal: raw.stands_for,
            });
        }
        // `sym_posend*` is a box size (width, minus height) in the symbol's frame; 0, 0 is none.
        let corner = match (ex, ey) {
            (Some(ex), Some(ey)) if ex.is_finite() && ey.is_finite() && (ex != 0.0 || ey != 0.0) => {
                Some(place(at, angle, scale, Point::new(ex, ey))).filter(|c| c.x.is_finite() && c.y.is_finite())
            }
            _ => None,
        };
        p.symbols.push(SweSymbol {
            id,
            page,
            component: component.filter(|c| *c > 0),
            block,
            at,
            corner,
            pins: out_pins,
        });
    }
    if unplaced > 0 {
        warnings.push(format!("{unplaced} symbol(s) without a usable position were left out"));
    }
    if bad_points > 0 {
        warnings.push(format!(
            "{bad_points} connection point(s) whose position cannot be computed were left out"
        ));
    }

    let mut st = conn
        .prepare(
            "SELECT l.lin_id, l.lin_fil_id, l.lin_ptstartx, l.lin_ptstarty, l.lin_ptendx, l.lin_ptendy, q.equ_text, \
             q.equ_id FROM tew_line l LEFT JOIN tew_equipotential q ON q.equ_id = l.lin_equ_id \
             ORDER BY l.lin_fil_id, l.lin_id",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<f64>>(2)?,
                r.get::<_, Option<f64>>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, Option<f64>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<i64>>(7)?,
            ))
        })
        .map_err(sql)?;
    let mut unusable = 0;
    for (id, page, ax, ay, bx, by, potential, net) in readable(rows, "line", &mut warnings) {
        let (Some(ax), Some(ay), Some(bx), Some(by)) = (ax, ay, bx, by) else {
            unusable += 1;
            continue;
        };
        if ![ax, ay, bx, by].iter().all(|v| v.is_finite()) {
            unusable += 1;
            continue;
        }
        p.lines.push(SweLine {
            id,
            page,
            a: Point::new(ax, ay),
            b: Point::new(bx, by),
            potential: potential.filter(|s| !s.trim().is_empty()),
            net,
            wires: line_wires.remove(&id).unwrap_or_default(),
        });
    }
    if unusable > 0 {
        warnings.push(format!("{unusable} line(s) without usable end points were left out"));
    }
    // Locations, when the database has them (older or trimmed databases may not).
    let has_locations = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'tew_location'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(sql)?
        > 0;
    if has_locations {
        let mut st = conn
            .prepare("SELECT loc_id, COALESCE(loc_tagpath, '') FROM tew_location ORDER BY loc_tagpath")
            .map_err(sql)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(sql)?;
        for (id, tag_path) in readable(rows, "location", &mut warnings) {
            p.locations.push(SweLocation {
                id,
                tag_path,
                description: text.get(&id).cloned().unwrap_or_default(),
            });
        }
    }
    p.warnings = warnings;
    Ok(p)
}

/// A tag WiringCraft accepts, from a SolidWorks tag path (`=F1+L1+L3-K3`, or a bare `K3`):
/// repeated aspects become levels (`+L1.L3`), other characters become `_`.
pub fn device_tag(path: &str) -> Option<DeviceTag> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let path = if path.starts_with(['=', '+', '-']) {
        path.to_owned()
    } else {
        format!("-{path}")
    };
    let mut aspects: BTreeMap<char, Vec<String>> = BTreeMap::new();
    let mut current: Option<char> = None;
    let mut buf = String::new();
    let mut flush = |aspect: Option<char>, buf: &mut String| {
        if let Some(a) = aspect
            && !buf.is_empty()
        {
            aspects.entry(a).or_default().push(std::mem::take(buf));
        }
        buf.clear();
    };
    for c in path.chars() {
        if matches!(c, '=' | '+' | '-') {
            flush(current, &mut buf);
            current = Some(c);
        } else if c.is_alphanumeric() || c == '_' || c == '.' {
            buf.push(c);
        } else {
            buf.push('_');
        }
    }
    flush(current, &mut buf);
    let mut s = String::new();
    for a in ['=', '+', '-'] {
        if let Some(levels) = aspects.get(&a) {
            s.push(a);
            s.push_str(&levels.join("."));
        }
    }
    DeviceTag::parse(&s).ok()
}

/// The sheet size a title block name gives (`..._A2_...`, `...SHEETA3...`), if any.
pub fn frame_size(frame: &str) -> Option<PageSize> {
    let up = frame.to_ascii_uppercase();
    let has = |s: &str| {
        up.match_indices(s).any(|(i, _)| {
            // Not part of a longer number (A20, A3000).
            !up[i + s.len()..].starts_with(|c: char| c.is_ascii_digit())
        })
    };
    if has("A2") {
        Some(PageSize::A2)
    } else if has("A3") {
        Some(PageSize::A3)
    } else if has("A4") {
        Some(PageSize::A4)
    } else {
        None
    }
}

/// The page size a page's content fits on, and whether it fits at all.
fn page_size(extent: Point) -> (PageSize, bool) {
    for size in [PageSize::A3, PageSize::A2] {
        let (w, h) = size.mm();
        if extent.x <= w + SNAP && extent.y <= h + SNAP {
            return (size, true);
        }
    }
    (PageSize::A2, false)
}

fn near(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() <= SNAP && (a.y - b.y).abs() <= SNAP
}

fn dist(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Where `q` lies along the segment `a`-`b` (0 at `a`, 1 at `b`), when it lies on it strictly
/// between the ends.
fn on_segment(a: Point, b: Point, q: Point) -> Option<f64> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= SNAP * SNAP {
        return None;
    }
    let t = ((q.x - a.x) * dx + (q.y - a.y) * dy) / len2;
    let foot = Point::new(a.x + t * dx, a.y + t * dy);
    if dist(q, foot) > JOIN || dist(q, a) <= JOIN || dist(q, b) <= JOIN || t <= 0.0 || t >= 1.0 {
        return None;
    }
    Some(t)
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn slug(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-');
    let out: String = out.chars().take(48).collect();
    if out.is_empty() { "symbol".to_owned() } else { out }
}

/// The box symbol for one placed symbol, in page orientation (y down), origin at the insertion
/// point. Pins point away from the box side they are nearest to.
fn box_symbol(block: &str, origin: Point, corner: Option<Point>, pins: &[(String, Point)]) -> (Vec<Shape>, Vec<Pin>) {
    let mut pts: Vec<Point> = pins
        .iter()
        .map(|(_, at)| Point::new(at.x - origin.x, at.y - origin.y))
        .collect();
    if let Some(c) = corner {
        pts.push(Point::new(0.0, 0.0));
        pts.push(Point::new(c.x - origin.x, c.y - origin.y));
    }
    let (mut min, mut max) = wc_core::geom::bounds_of(&pts);
    if corner.is_none() {
        min = Point::new(min.x - MARGIN, min.y - MARGIN);
        max = Point::new(max.x + MARGIN, max.y + MARGIN);
    }
    if max.x - min.x < 2.0 * MARGIN {
        let mid = (min.x + max.x) / 2.0;
        min.x = mid - MARGIN;
        max.x = mid + MARGIN;
    }
    if max.y - min.y < 2.0 * MARGIN {
        let mid = (min.y + max.y) / 2.0;
        min.y = mid - MARGIN;
        max.y = mid + MARGIN;
    }
    let min = Point::new(round(min.x), round(min.y));
    let max = Point::new(round(max.x), round(max.y));
    // A pin points out of the box on the side it lies towards, seen from the box centre and
    // measured in half box sizes; on a tie the longer side of the box decides.
    let centre = Point::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
    let (half_w, half_h) = ((max.x - min.x) / 2.0, (max.y - min.y) / 2.0);
    let mut used = BTreeSet::new();
    let mut out = Vec::new();
    for (i, (name, at)) in pins.iter().enumerate() {
        let local = Point::new(round(at.x - origin.x), round(at.y - origin.y));
        let (nx, ny) = ((local.x - centre.x) / half_w, (local.y - centre.y) / half_h);
        let vertical = if (ny.abs() - nx.abs()).abs() < 1e-9 {
            half_h >= half_w
        } else {
            ny.abs() > nx.abs()
        };
        let dir = match (vertical, ny < 0.0, nx < 0.0) {
            (true, true, _) => PinDir::Up,
            (true, false, _) => PinDir::Down,
            (false, _, true) => PinDir::Left,
            (false, _, false) => PinDir::Right,
        };
        // A repeated name gets `#k`, with k counted up until the name is free (a later pin may
        // already be called `A#3`).
        let mut n = name.clone();
        let mut k = i + 1;
        while !used.insert(n.clone()) {
            n = format!("{name}#{k}");
            k += 1;
        }
        out.push(Pin {
            name: n,
            at: local,
            dir,
        });
    }
    let mut graphics = vec![Shape::Rect { min, max }];
    if pins.is_empty() && !block.is_empty() {
        graphics.push(Shape::Text {
            at: Point::new(min.x + 0.5, min.y + 3.0),
            text: block.to_owned(),
            height: 2.0,
            align: Default::default(),
            angle: 0.0,
        });
    }
    (graphics, out)
}

/// The name of an equipotential without a number: `net<id>`, or `net<id>_2`, ... when the source
/// already uses that as a potential name.
fn fallback_net(id: i64, taken: &BTreeSet<String>) -> String {
    let base = format!("net{id}");
    if !taken.contains(&base) {
        return base;
    }
    (2usize..)
        .map(|k| format!("{base}_{k}"))
        .find(|n| !taken.contains(n))
        .unwrap_or(base)
}

/// Tags made unique: the second `-K1` becomes `-K1_2`. `renamed` counts the changed ones.
fn unique(tag: DeviceTag, taken: &mut BTreeSet<DeviceTag>, renamed: &mut usize) -> DeviceTag {
    if taken.insert(tag.clone()) {
        return tag;
    }
    *renamed += 1;
    for n in 2usize.. {
        let mut t = tag.clone();
        if let Some(last) = t.product.last_mut() {
            *last = format!("{last}_{n}");
        }
        if taken.insert(t.clone()) {
            return t;
        }
    }
    tag
}

/// The page numbers: the source's own page labels, with the book number in front when the
/// project has several books (`2.05`), made unique; or, with `first_page_number`, plain numbers
/// from there on.
fn page_numbers(
    pages: &[&SwePage],
    books: usize,
    first_page_number: Option<usize>,
    renumbered: &mut usize,
) -> Vec<String> {
    let mut used = BTreeSet::new();
    pages
        .iter()
        .enumerate()
        .map(|(i, pg)| {
            let base = match first_page_number {
                Some(first) => first.saturating_add(i).to_string(),
                None => {
                    let label = if pg.label.trim().is_empty() {
                        (i + 1).to_string()
                    } else {
                        pg.label.trim().to_owned()
                    };
                    if books > 1 && !pg.book.is_empty() {
                        format!("{}.{label}", pg.book)
                    } else {
                        label
                    }
                }
            };
            let mut n = base.clone();
            let mut k = 2;
            if used.contains(&n) {
                *renumbered += 1;
            }
            while !used.insert(n.clone()) {
                n = format!("{base}-{k}");
                k += 1;
            }
            n
        })
        .collect()
}

/// Build WiringCraft pages from a read project: one page per schematic page, numbered and titled
/// like the source, each symbol as a box with its connection points, each line as wires joined to
/// the pins and lines at its ends, with their cable cores; one device per component and per
/// cable. Returns the pages and devices, the warnings, and the counts report.
pub fn build(p: &SweProject, first_page_number: Option<usize>) -> (Built, Vec<String>, Vec<Count>) {
    let mut warnings = Vec::new();
    let mut built = Built {
        symbols: Vec::new(),
        parts: Vec::new(),
        pages: Vec::new(),
        devices: BTreeMap::new(),
        locations: Vec::new(),
    };
    // Locations, with their descriptions. One without a usable tag path cannot be named.
    let mut locations_untagged = 0;
    for l in &p.locations {
        let path = location_path(&l.tag_path);
        if path.is_empty() {
            locations_untagged += 1;
            continue;
        }
        let description = l.description.trim().to_owned();
        match built.locations.iter_mut().find(|have| have.path == path) {
            // A repeat fills in a description the first one lacked, like merge_location.
            Some(have) if have.description.is_empty() => have.description = description,
            Some(_) => {}
            None => built.locations.push(wc_core::Location { path, description }),
        }
    }
    let locations_merged = p.locations.len() - locations_untagged - built.locations.len();
    let drawn: Vec<&SwePage> = p.pages.iter().filter(|pg| DRAWN_KINDS.contains(&pg.kind)).collect();
    let others = p.pages.len() - drawn.len();
    let covers = p.pages.iter().filter(|pg| pg.kind == KIND_COVER).count();

    // Tags: one per component, made unique if two components give the same tag. Each pair of
    // origin-destination arrows becomes a pair of interruption points with its own tag. A
    // function aspect every device shares (a project with one function, `=F1`) says nothing on
    // the drawing: it is left out of the tags and kept as a device property.
    let functions: BTreeSet<Vec<String>> = p
        .components
        .values()
        .filter(|c| c.kind != COMPONENT_ARROW)
        .filter_map(|c| device_tag(&c.tag))
        .map(|t| t.function)
        .collect();
    let shared_function = match functions.iter().next() {
        Some(f) if functions.len() == 1 && !f.is_empty() => Some(f.clone()),
        _ => None,
    };
    if let Some(f) = &shared_function {
        warnings.push(format!(
            "every device has the function ={}; it is left out of the tags and kept as the device property Function",
            f.join(".")
        ));
    }
    let mut tags: BTreeMap<i64, DeviceTag> = BTreeMap::new();
    let mut taken: BTreeSet<DeviceTag> = BTreeSet::new();
    let mut untagged = 0;
    let mut renamed = 0;
    let mut arrows = 0;
    let strip_of = |c: &SweComponent| {
        c.parent
            .filter(|_| c.kind == COMPONENT_TERMINAL)
            .and_then(|s| p.components.get(&s))
            .filter(|s| s.kind == COMPONENT_TERMINAL_STRIP)
    };
    // Terminals come after everything else, so their strip's final tag is known.
    let (terminals, others_first): (Vec<&SweComponent>, Vec<&SweComponent>) =
        p.components.values().partition(|c| strip_of(c).is_some());
    let mut loose_terminals = 0usize;
    let mut strip_terminals = 0usize;
    for c in others_first.into_iter().chain(terminals) {
        let own = || {
            device_tag(&c.tag).map(|mut t| {
                if shared_function.is_some() {
                    t.function.clear();
                }
                t
            })
        };
        let tag = if c.kind == COMPONENT_ARROW {
            arrows += 1;
            DeviceTag::parse(&format!("-OD{arrows}")).ok()
        } else if let Some(strip) = strip_of(c) {
            // `-X1` and terminal 3 make `-X1.3`, whatever separator the source's tag path uses.
            // One level, whatever the number contains: `1.2` must not become a strip `-X1.1`.
            let number = device_tag(&c.name)
                .map(|n| n.product.join("_"))
                .filter(|n| !n.is_empty());
            match (tags.get(&strip.id), number) {
                (Some(strip_tag), Some(number)) => {
                    let mut t = strip_tag.clone();
                    t.product.push(number);
                    strip_terminals += 1;
                    Some(t)
                }
                _ => {
                    loose_terminals += 1;
                    own()
                }
            }
        } else {
            if c.kind == COMPONENT_TERMINAL {
                loose_terminals += 1;
            }
            own()
        };
        match tag {
            Some(t) => {
                tags.insert(c.id, unique(t, &mut taken, &mut renamed));
            }
            None => untagged += 1,
        }
    }
    // Terminal order: WiringCraft lists a strip's terminals by number. A strip whose source order
    // is not the number order is counted.
    let mut on_strip: BTreeMap<i64, Vec<&SweComponent>> = BTreeMap::new();
    for c in p.components.values() {
        if let Some(s) = strip_of(c) {
            on_strip.entry(s.id).or_default().push(c);
        }
    }
    // The terminal plan sorts by the tag's last level with the same natural sort.
    let number_of = |c: &SweComponent| {
        tags.get(&c.id)
            .and_then(|t| t.product.last().cloned())
            .unwrap_or_default()
    };
    let out_of_order: Vec<String> = on_strip
        .iter()
        .filter(|(_, list)| {
            // In source order the numbers must rise strictly. Two numbers that sort alike (`1`
            // and `01`) leave the plan's order to the drawing, so that strip is counted too.
            let mut by_order: Vec<&SweComponent> = list.to_vec();
            by_order.sort_by_key(|c| (c.order, c.id));
            let keys: Vec<_> = by_order
                .iter()
                .map(|c| wc_core::project::natural_key(&number_of(c)))
                .collect();
            keys.windows(2).any(|w| matches!(w, [a, b] if a >= b))
        })
        .filter_map(|(strip, _)| tags.get(strip).map(ToString::to_string))
        .collect();
    let strips_out_of_order = out_of_order.len();
    if !out_of_order.is_empty() {
        warnings.push(format!(
            "terminal strip(s) {} have their terminals in another order in the source than by number; WiringCraft lists them by number",
            out_of_order.join(", ")
        ));
    }
    let mut cable_tags: BTreeMap<i64, DeviceTag> = BTreeMap::new();
    let mut cables_untagged = 0;
    for c in p.cables.values() {
        match device_tag(&c.tag) {
            Some(t) => {
                cable_tags.insert(c.id, unique(t, &mut taken, &mut renamed));
            }
            None => cables_untagged += 1,
        }
    }
    if renamed > 0 {
        warnings.push(format!(
            "{renamed} device or cable tag(s) were already taken and got a suffix (-K1 became -K1_2)"
        ));
    }

    // Symbols and lines by page, in one pass each.
    let mut syms_of: BTreeMap<i64, Vec<&SweSymbol>> = BTreeMap::new();
    for s in &p.symbols {
        syms_of.entry(s.page).or_default().push(s);
    }
    let mut lines_of: BTreeMap<i64, Vec<&SweLine>> = BTreeMap::new();
    for l in &p.lines {
        lines_of.entry(l.page).or_default().push(l);
    }

    let mut renumbered = 0;
    let numbers = page_numbers(&drawn, p.books, first_page_number, &mut renumbered);
    if renumbered > 0 {
        warnings.push(format!(
            "{renumbered} page number(s) were used twice in the source and got a suffix (5 became 5-2)"
        ));
    }
    // Potential names the source uses, so a fallback name never takes one of them.
    let potential_names: BTreeSet<String> = p.lines.iter().filter_map(|l| l.potential.clone()).collect();
    let mut symbol_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut per_block: BTreeMap<String, usize> = BTreeMap::new();
    let mut no_points = 0;
    let mut unknown_component = 0;
    let mut used_components: BTreeSet<i64> = BTreeSet::new();
    let mut symbols_in = 0;
    let mut symbols_out = 0;
    let mut lines_in = 0;
    let mut lines_out = 0usize;
    let mut zero_length = 0;
    let mut segments_out = 0usize;
    let mut wires_drawn: BTreeSet<i64> = BTreeSet::new();
    let mut cables_used: BTreeSet<i64> = BTreeSet::new();
    let mut multi_core_lines = 0usize;
    let mut cores_not_shown = 0usize;
    let mut multi_number_lines = 0usize;
    let mut numbers_not_shown = 0usize;
    let mut wires_on_zero_length: BTreeSet<i64> = BTreeSet::new();
    // Component terminal (`cel_id`, `cte_no`) -> the pin drawn for it, and a label for messages.
    let mut drawn_at: BTreeMap<(i64, i64), (PinRef, String)> = BTreeMap::new();

    for (i, pg) in drawn.iter().enumerate() {
        let syms: Vec<&SweSymbol> = syms_of.remove(&pg.id).unwrap_or_default();
        let lines: Vec<&SweLine> = lines_of.remove(&pg.id).unwrap_or_default();
        symbols_in += syms.len();
        lines_in += lines.len();
        let mut extent = Point::new(0.0, 0.0);
        let mut grow = |q: Point| {
            extent.x = extent.x.max(q.x);
            extent.y = extent.y.max(q.y);
        };
        for s in &syms {
            grow(s.at);
            s.pins.iter().for_each(|q| grow(q.at));
            if let Some(c) = s.corner {
                grow(c);
            }
        }
        for l in &lines {
            grow(l.a);
            grow(l.b);
        }
        // The sheet the title block names, unless the content does not fit on it.
        let (fitted, fits) = page_size(extent);
        let size = match frame_size(&pg.frame) {
            Some(named) if extent.x <= named.mm().0 + SNAP && extent.y <= named.mm().1 + SNAP => named,
            _ => fitted,
        };
        if !fits {
            warnings.push(format!(
                "page {} reaches beyond A2; content outside the sheet is kept as is",
                numbers[i]
            ));
        }
        let height = size.mm().1;
        let flip = |q: Point| Point::new(round(q.x), round(height - q.y));

        let title = match (pg.folder.is_empty(), pg.title.is_empty()) {
            (false, false) => format!("{}: {}", pg.folder, pg.title),
            (false, true) => pg.folder.clone(),
            (true, false) => pg.title.clone(),
            (true, true) => String::new(),
        };
        let mut page = Page::new(format!("swe-page-{}", pg.id), numbers[i].clone(), title);
        page.size = size;
        // A line diagram repeats devices the schematics draw in detail.
        page.overview = pg.kind == KIND_LINE_DIAGRAM;

        // Pin positions on this page (y down) for joining wire ends.
        let mut pin_at: Vec<(Point, String, String)> = Vec::new();
        for s in &syms {
            if s.pins.is_empty() && s.corner.is_none() {
                no_points += 1;
                continue;
            }
            symbols_out += 1;
            let origin = flip(s.at);
            let pins: Vec<(String, Point)> = s.pins.iter().map(|q| (q.name.clone(), flip(q.at))).collect();
            let (graphics, out_pins) = box_symbol(&s.block, origin, s.corner.map(flip), &pins);
            let component = s.component.and_then(|c| p.components.get(&c));
            if s.component.is_some() && component.is_none() {
                unknown_component += 1;
            }
            let role = match component.map(|c| c.kind) {
                Some(COMPONENT_ARROW) => SymbolRole::Interruption,
                Some(COMPONENT_TERMINAL) => SymbolRole::Terminal,
                _ => SymbolRole::Standalone,
            };
            let key = format!("{}|{:?}|{:?}|{:?}", s.block, role, graphics, out_pins);
            let sym_id = match symbol_ids.get(&key) {
                Some(id) => id.clone(),
                None => {
                    // Numbered per id stem, so blocks whose names clean up alike (`Relay A`,
                    // `Relay-A`) never share an id.
                    let stem = slug(&s.block);
                    let n = per_block.entry(stem.clone()).or_insert(0);
                    *n += 1;
                    let id = format!("swe.{stem}.{n}");
                    built.symbols.push(Symbol {
                        id: id.clone(),
                        name: if s.block.is_empty() {
                            "SolidWorks Electrical symbol".to_owned()
                        } else {
                            s.block.clone()
                        },
                        category: "SolidWorks Electrical import".to_owned(),
                        letter_code: String::new(),
                        role,
                        graphics,
                        pins: out_pins.clone(),
                        tag_at: Point::new(0.0, -MARGIN - 1.0),
                        description: String::new(),
                    });
                    symbol_ids.insert(key, id.clone());
                    id
                }
            };
            let item = format!("s{}", s.id);
            if !page.overview {
                let label = s.component.and_then(|c| tags.get(&c)).map(ToString::to_string);
                // `box_symbol` gives one pin per point, in order; a repeated name became `A#2`,
                // so the pin is taken by position, not by name.
                for (q, o) in s.pins.iter().zip(&out_pins) {
                    let Some(t) = q.terminal else { continue };
                    drawn_at.entry(t).or_insert_with(|| {
                        (
                            (page.id.clone(), item.clone(), o.name.clone()),
                            format!(
                                "{}:{} (page {})",
                                label.clone().unwrap_or_else(|| "?".to_owned()),
                                o.name,
                                page.number
                            ),
                        )
                    });
                }
            }
            for pin in &out_pins {
                pin_at.push((
                    Point::new(origin.x + pin.at.x, origin.y + pin.at.y),
                    item.clone(),
                    pin.name.clone(),
                ));
            }
            let tag = s.component.and_then(|c| tags.get(&c).cloned());
            if let Some(c) = s.component.filter(|c| tags.contains_key(c)) {
                used_components.insert(c);
            }
            page.symbols.push(PlacedSymbol {
                id: item,
                symbol: sym_id,
                at: origin,
                rotation: Default::default(),
                mirror: false,
                tag,
            });
        }

        // Wire ends snap to a pin within JOIN, else to a wire end already seen within JOIN, so
        // ends the source drew a little apart meet. Then lines are split where another line's
        // end lands on them (a T-joint), so the joint is a shared wire end.
        // Ends only join ends of the same net (or of a line without one): two nets drawn close
        // together stay apart.
        let same_net = |x: Option<i64>, y: Option<i64>| x.is_none() || y.is_none() || x == y;
        // The nets that touch each pin exactly: a pin one net reaches is that net's, and a nearby
        // end of another net stays off it (the source gave them different nets on purpose).
        let mut pin_nets: Vec<BTreeSet<i64>> = vec![BTreeSet::new(); pin_at.len()];
        for l in &lines {
            for q in [flip(l.a), flip(l.b)] {
                for (k, (at, _, _)) in pin_at.iter().enumerate() {
                    if let Some(net) = l.net
                        && near(*at, q)
                    {
                        pin_nets[k].insert(net);
                    }
                }
            }
        }
        let mut seen: Vec<(Point, Option<i64>)> = Vec::new();
        // Where an end lands: a pin (and its index), a wire end seen before, or itself (new).
        // Nothing is recorded here: a line that turns out to have zero length is left out and
        // must not take a pin or leave an end for others to join.
        enum Landing {
            Pin(usize, Point),
            Seen(Point),
            New(Point),
        }
        let snap = |q: Point, net: Option<i64>, pin_nets: &[BTreeSet<i64>], seen: &[(Point, Option<i64>)]| -> Landing {
            let pin = pin_at
                .iter()
                .enumerate()
                .filter(|(k, (at, _, _))| {
                    dist(*at, q) <= JOIN
                        && (near(*at, q)
                            || net.is_none()
                            || pin_nets[*k].is_empty()
                            || net.is_some_and(|n| pin_nets[*k].contains(&n)))
                })
                .map(|(k, (at, _, _))| (k, *at))
                .min_by(|x, y| dist(x.1, q).total_cmp(&dist(y.1, q)));
            if let Some((k, at)) = pin {
                return Landing::Pin(k, at);
            }
            match seen.iter().find(|(s, n)| dist(*s, q) <= JOIN && same_net(*n, net)) {
                Some((s, _)) => Landing::Seen(*s),
                None => Landing::New(q),
            }
        };
        let mut flipped: Vec<(Point, Point, &SweLine)> = Vec::new();
        for l in &lines {
            let ends = [
                snap(flip(l.a), l.net, &pin_nets, &seen),
                snap(flip(l.b), l.net, &pin_nets, &seen),
            ];
            let [a, b] = [&ends[0], &ends[1]].map(|e| match e {
                Landing::Pin(_, p) | Landing::Seen(p) | Landing::New(p) => *p,
            });
            if near(a, b) {
                zero_length += 1;
                wires_on_zero_length.extend(l.wires.iter().copied());
                continue;
            }
            // The line stays: the first net that snaps onto a free pin takes it, and its new
            // ends are there for later lines to join.
            for e in ends {
                match e {
                    Landing::Pin(k, _) => {
                        if let Some(n) = l.net {
                            pin_nets[k].insert(n);
                        }
                    }
                    Landing::New(q) => seen.push((q, l.net)),
                    Landing::Seen(_) => {}
                }
            }
            flipped.push((a, b, *l));
        }
        let ends: Vec<(Point, Option<i64>)> = flipped
            .iter()
            .flat_map(|(a, b, l)| [(*a, l.net), (*b, l.net)])
            .collect();
        let end = |q: Point| -> Endpoint {
            match pin_at.iter().find(|(at, _, _)| near(*at, q)) {
                Some((_, item, pin)) => Endpoint::Pin {
                    item: item.clone(),
                    pin: pin.clone(),
                },
                None => Endpoint::Point { at: q },
            }
        };
        let mut n = 0;
        for (a, b, l) in &flipped {
            lines_out += 1;
            // Cut at the landing end itself, so the two wires share that exact point.
            let mut cuts: Vec<(f64, Point)> = ends
                .iter()
                .filter(|(_, net)| same_net(*net, l.net))
                .filter_map(|(q, _)| on_segment(*a, *b, *q).map(|t| (t, *q)))
                .collect();
            cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
            cuts.dedup_by(|x, y| near(x.1, y.1));
            let mut points = vec![*a];
            points.extend(cuts.iter().map(|(_, q)| *q));
            points.push(*b);

            // The cable core the line carries: its first wire in a cable.
            let wires: Vec<&SweWire> = l.wires.iter().filter_map(|w| p.wires.get(w)).collect();
            wires_drawn.extend(wires.iter().map(|w| w.id));
            // A WiringCraft wire is on one cable core; a line carrying several shows the first.
            let cores: BTreeSet<(i64, i64)> = wires
                .iter()
                .filter_map(|w| w.cable.filter(|c| cable_tags.contains_key(c)).map(|c| (c, w.core)))
                .collect();
            if cores.len() > 1 {
                multi_core_lines += 1;
                cores_not_shown += cores.len() - 1;
            }
            let cored = wires
                .iter()
                .find(|w| w.cable.is_some_and(|c| cable_tags.contains_key(&c)));
            let cable = cored.and_then(|w| {
                let cab = w.cable?;
                cables_used.insert(cab);
                let colour = p
                    .cables
                    .get(&cab)
                    .and_then(|c| c.cores.get(&w.core))
                    .cloned()
                    .unwrap_or_default();
                let core = match (w.core >= 0, colour.is_empty()) {
                    (true, true) => w.core.to_string(),
                    (true, false) => format!("{} {colour}", w.core),
                    (false, _) => colour,
                };
                Some(CableCore {
                    cable: cable_tags.get(&cab)?.clone(),
                    core,
                })
            });
            // The equipotential's number is the net (potential) name: wires with the same one are
            // connected across gaps and pages. The wire's own tag is its number.
            let numbers: BTreeSet<&str> = wires.iter().map(|w| w.tag.trim()).filter(|t| !t.is_empty()).collect();
            if numbers.len() > 1 {
                multi_number_lines += 1;
                numbers_not_shown += numbers.len() - 1;
            }
            let number = wires
                .iter()
                .map(|w| w.tag.trim())
                .find(|t| !t.is_empty())
                .map(str::to_owned);
            // Without a number the equipotential is still one net: name it after its id, so its
            // wires stay connected and two such nets stay apart.
            let potential = l
                .potential
                .clone()
                .or_else(|| l.net.map(|id| fallback_net(id, &potential_names)));
            for pair in points.windows(2) {
                let (from, to) = (pair[0], pair[1]);
                if near(from, to) {
                    continue;
                }
                n += 1;
                segments_out += 1;
                page.wires.push(Wire {
                    id: format!("w{n}"),
                    from: end(from),
                    to: end(to),
                    via: Vec::new(),
                    number: number.clone(),
                    potential: potential.clone(),
                    cable: cable.clone(),
                    route: Default::default(),
                });
            }
        }
        built.pages.push(page);
    }

    // Each wire record names its two terminals; they must be connected on the drawing too.
    let interruptions: BTreeSet<&str> = built
        .symbols
        .iter()
        .filter(|s| s.role == SymbolRole::Interruption)
        .map(|s| s.id.as_str())
        .collect();
    let mut probe = wc_core::Project::new("");
    // Schematic pages only: a line diagram's potential names could join nets no schematic joins.
    probe.pages = built.pages.iter().filter(|pg| !pg.overview).cloned().collect();
    let mut net_of: BTreeMap<PinRef, usize> = BTreeMap::new();
    for (n, net) in nets_linked(&probe, |id| interruptions.contains(id))
        .into_iter()
        .enumerate()
    {
        for pin in net.pins {
            net_of.insert(pin, n);
        }
    }
    let mut wire_ends = 0usize;
    let mut one_end_named = 0usize;
    let mut ends_agree = 0usize;
    let mut ends_not_drawn = 0usize;
    let mut ends_apart: Vec<String> = Vec::new();
    for w in p.wires.values() {
        let (Some(a), Some(b)) = (w.from, w.to) else {
            if w.from.is_some() || w.to.is_some() {
                one_end_named += 1;
            }
            continue;
        };
        wire_ends += 1;
        let (Some((pa, la)), Some((pb, lb))) = (drawn_at.get(&a), drawn_at.get(&b)) else {
            ends_not_drawn += 1;
            continue;
        };
        match (net_of.get(pa), net_of.get(pb)) {
            (Some(x), Some(y)) if x == y => ends_agree += 1,
            _ => {
                let name = if w.tag.trim().is_empty() {
                    format!("wire {}", w.id)
                } else {
                    format!("wire {}", w.tag.trim())
                };
                ends_apart.push(format!("{name}: {la} and {lb}"));
            }
        }
    }
    if !ends_apart.is_empty() {
        let shown: Vec<&str> = ends_apart.iter().take(5).map(String::as_str).collect();
        let more = ends_apart.len().saturating_sub(shown.len());
        warnings.push(format!(
            "{} wire(s) join two terminals that are not connected on the imported drawing: {}{}",
            ends_apart.len(),
            shown.join("; "),
            if more > 0 {
                format!("; and {more} more")
            } else {
                String::new()
            }
        ));
    }

    // Devices: every tagged component that is not an arrow, and every cable.
    let mut devices_out = 0usize;
    for c in p.components.values() {
        if c.kind == COMPONENT_ARROW {
            continue;
        }
        let Some(tag) = tags.get(&c.id) else {
            continue;
        };
        let mut properties = BTreeMap::new();
        if !c.part_number.is_empty() {
            properties.insert("PartNumber".to_owned(), c.part_number.clone());
        }
        if !c.manufacturer.is_empty() {
            properties.insert("Manufacturer".to_owned(), c.manufacturer.clone());
        }
        if let Some(f) = &shared_function {
            properties.insert("Function".to_owned(), format!("={}", f.join(".")));
        }
        devices_out += 1;
        built.devices.insert(
            tag.clone(),
            Device {
                part: None,
                description: c.description.clone(),
                properties,
            },
        );
    }
    let mut cables_out = 0usize;
    for c in p.cables.values() {
        let Some(tag) = cable_tags.get(&c.id) else {
            continue;
        };
        let mut properties = BTreeMap::new();
        if !c.part_number.is_empty() {
            properties.insert("PartNumber".to_owned(), c.part_number.clone());
        }
        if !c.manufacturer.is_empty() {
            properties.insert("Manufacturer".to_owned(), c.manufacturer.clone());
        }
        properties.insert("Cores".to_owned(), c.cores.len().to_string());
        cables_out += 1;
        built.devices.insert(
            tag.clone(),
            Device {
                part: None,
                description: "Cable".to_owned(),
                properties,
            },
        );
    }

    if unknown_component > 0 {
        warnings.push(format!(
            "{unknown_component} symbol(s) refer to a component the database does not have; they are drawn without a tag"
        ));
    }
    let non_arrows = p.components.values().filter(|c| c.kind != COMPONENT_ARROW).count();
    let arrow_count = p.components.len() - non_arrows;
    let symbols_total = p.symbols.len();
    let off_page = symbols_total.saturating_sub(symbols_in);
    let lines_off_page = p.lines.len().saturating_sub(lines_in);
    // A wire left out only because its lines were of zero length is counted as such.
    let wires_zero_only = wires_on_zero_length
        .iter()
        .filter(|w| p.wires.contains_key(w) && !wires_drawn.contains(w))
        .count();
    let wires_missing = p
        .wires
        .len()
        .saturating_sub(wires_drawn.len())
        .saturating_sub(wires_zero_only);
    let note = |parts: Vec<(usize, &str)>| -> String {
        parts
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, why)| format!("{n} {why}"))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let report = vec![
        Count {
            what: "pages",
            source: p.pages.len(),
            imported: built.pages.len(),
            note: note(vec![
                (covers, "cover page(s)"),
                (
                    others.saturating_sub(covers),
                    "report, layout or other non-schematic page(s)",
                ),
            ]),
        },
        Count {
            what: "components",
            source: non_arrows,
            imported: devices_out,
            note: note(vec![(untagged, "without a usable tag")]),
        },
        Count {
            what: "page link arrows",
            source: arrow_count,
            imported: arrow_count,
            note: "as interruption points -OD1, -OD2, ...".to_owned(),
        },
        Count {
            what: "symbols",
            source: symbols_total,
            imported: symbols_out,
            note: note(vec![
                (off_page, "on pages that are not imported"),
                (no_points, "without connection points (frames, labels, texts)"),
            ]),
        },
        Count {
            what: "wire lines",
            source: p.lines.len(),
            imported: lines_out,
            note: note(vec![
                (lines_off_page, "on pages that are not imported"),
                (zero_length, "of zero length"),
                (
                    segments_out.saturating_sub(lines_out),
                    "extra segments from splitting at T-joints",
                ),
            ]),
        },
        Count {
            what: "wires",
            source: p.wires.len(),
            imported: wires_drawn.len(),
            note: note(vec![
                (wires_missing, "not on an imported page's line"),
                (wires_zero_only, "only on lines of zero length"),
                (
                    multi_number_lines,
                    "line(s) carry several wire numbers; their wire shows the first",
                ),
                (
                    numbers_not_shown,
                    "other wire number(s) on those lines are left off that line's wire",
                ),
                (
                    multi_core_lines,
                    "line(s) carry several cable cores; their wire shows the first core only",
                ),
                (
                    cores_not_shown,
                    "other cable core(s) on those lines are left off that line's wire",
                ),
            ]),
        },
        Count {
            what: "cables",
            source: p.cables.len(),
            imported: cables_out,
            note: note(vec![
                (cables_untagged, "without a usable tag"),
                (
                    cables_out.saturating_sub(cables_used.len()),
                    "not on any imported wire (device only)",
                ),
            ]),
        },
        Count {
            what: "terminal strips",
            source: p
                .components
                .values()
                .filter(|c| c.kind == COMPONENT_TERMINAL_STRIP)
                .count(),
            imported: p
                .components
                .values()
                .filter(|c| c.kind == COMPONENT_TERMINAL_STRIP && tags.contains_key(&c.id))
                .count(),
            note: note(vec![(
                strips_out_of_order,
                "whose source order differs from the terminal numbers (listed by number)",
            )]),
        },
        Count {
            what: "terminals",
            source: p.components.values().filter(|c| c.kind == COMPONENT_TERMINAL).count(),
            imported: strip_terminals,
            note: note(vec![(
                loose_terminals,
                "not on a strip or without a number (tagged from their own tag path, when it has one)",
            )]),
        },
        Count {
            what: "wire ends (from/to terminals)",
            source: wire_ends,
            imported: ends_agree,
            note: note(vec![
                (
                    ends_apart.len(),
                    "join terminals that are not connected on the drawing (see the warnings)",
                ),
                (ends_not_drawn, "with a terminal that is not drawn on a schematic page"),
                (one_end_named, "more name only one terminal (not checked)"),
            ]),
        },
        Count {
            what: "locations",
            source: p.locations.len(),
            imported: built.locations.len(),
            note: note(vec![
                (locations_untagged, "without a usable tag path"),
                (locations_merged, "with the same tag path as another (merged)"),
            ]),
        },
    ];
    (built, warnings, report)
}

#[cfg(test)]
mod tests;
