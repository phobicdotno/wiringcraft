//! SolidWorks Electrical projects.
//!
//! A project archive (`.tewzip`) is a zip. Each project in it has a SQLite database,
//! `Projects/<n>/Database/tew_project_data.sqlite`, which holds the electrical model: pages
//! (`tew_file`), placed symbols (`tew_symbol`) and their connection points (`tew_symbolpt`),
//! components (`tew_component`) with their terminals, wire lines (`tew_line`) with their
//! potentials (`tew_equipotential`), parts (`tew_buildofmaterial`) and translated texts
//! (`tew_translatedtext`). Page drawings and symbol graphics are DWG files and are not read: each
//! placed symbol becomes a box with its connection points where the database puts them.
//!
//! Database coordinates are millimetres with y up. A connection point is an offset in its
//! symbol's own frame: scaled by the symbol's scale (a negative scale mirrors), then rotated by
//! the symbol's angle, then moved to the symbol's position.
//!
//! Input is treated as hostile: missing tables or columns are errors, odd rows are skipped and
//! counted, nothing panics.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use rusqlite::Connection;
use wc_core::geom::{Point, Shape};
use wc_core::symbol::{Pin, PinDir};
use wc_core::{Device, DeviceTag, Endpoint, Page, PageSize, PlacedSymbol, Symbol, Wire};

use crate::aml::Built;
use crate::{Error, Result};

/// `tew_file.fil_filetype` values (the product's `EwFileType`) of pages drawn as schematics:
/// schematic sheet, line diagram, mixed schematic, P&ID, fluid. The others are folders, reports
/// (bill of materials, terminal tables), cover pages, cabinet layouts and appendices.
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

/// Pin and wire ends closer than this (mm) are the same point.
const SNAP: f64 = 0.05;
/// Box margin around a symbol's connection points (mm).
const MARGIN: f64 = 2.5;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SwePage {
    pub id: i64,
    pub order: i64,
    pub kind: i64,
    /// The page's own label in its folder, e.g. `03`.
    pub label: String,
    pub title: String,
    pub folder: String,
    /// The title block (drawing frame) name; it names the sheet size, e.g. `..._A2_...`.
    pub frame: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwePin {
    pub name: String,
    /// Page position in millimetres, y up.
    pub at: Point,
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
    pub page: i64,
    pub a: Point,
    pub b: Point,
    pub potential: Option<String>,
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
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SweProject {
    pub pages: Vec<SwePage>,
    pub symbols: Vec<SweSymbol>,
    pub lines: Vec<SweLine>,
    pub components: BTreeMap<i64, SweComponent>,
    pub warnings: Vec<String>,
}

fn sql(e: rusqlite::Error) -> Error {
    Error::Format(format!("SolidWorks Electrical database: {e}"))
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
    let mut f = zip.by_name(name).map_err(|e| Error::Format(format!("{name}: {e}")))?;
    let mut bytes = Vec::with_capacity(usize::try_from(f.size()).unwrap_or(0).min(1 << 30));
    f.read_to_end(&mut bytes)
        .map_err(|e| Error::Format(format!("{name}: {e}")))?;
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

/// The databases of all projects in an archive held in memory, by project folder, in order.
pub fn databases_in_archive(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| Error::Format(format!("not a SolidWorks Electrical archive: {e}")))?;
    let names = project_databases(&zip);
    let mut out = Vec::new();
    for n in names {
        let mut f = zip.by_name(&n).map_err(|e| Error::Format(format!("{n}: {e}")))?;
        let mut data = Vec::new();
        f.read_to_end(&mut data)
            .map_err(|e| Error::Format(format!("{n}: {e}")))?;
        let folder = n.rsplitn(3, '/').nth(2).unwrap_or_default().to_owned();
        out.push((folder, data));
    }
    if out.is_empty() {
        return Err(Error::Format(
            "the archive holds no SolidWorks Electrical project database".into(),
        ));
    }
    Ok(out)
}

/// Open a project database held in memory.
pub fn open(bytes: &[u8]) -> Result<Connection> {
    let mut conn = Connection::open_in_memory().map_err(sql)?;
    conn.deserialize_read_exact(rusqlite::MAIN_DB, std::io::Cursor::new(bytes), bytes.len(), true)
        .map_err(sql)?;
    Ok(conn)
}

/// Texts by object id, English first, then any language.
fn texts(conn: &Connection) -> Result<BTreeMap<i64, String>> {
    let mut out: BTreeMap<i64, String> = BTreeMap::new();
    let mut st = conn
        .prepare(
            "SELECT tra_objectid, tra_0 FROM tew_translatedtext WHERE tra_objectno = -1 AND tra_0 <> '' \
             ORDER BY tra_lan_strid = 'en'",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))
        .map_err(sql)?;
    // English sorts last, so it wins.
    for (id, text) in rows.flatten() {
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
    let text = texts(conn)?;

    let mut st = conn
        .prepare(
            "SELECT f.fil_id, COALESCE(f.fil_orderno, 0), COALESCE(f.fil_filetype, 0), COALESCE(f.fil_title, ''), \
             COALESCE(d.fol_name, ''), COALESCE(f.fil_titleblock, '') \
             FROM tew_file f LEFT JOIN tew_folder d ON d.fol_id = f.fil_fol_id \
             ORDER BY f.fil_orderno, f.fil_id",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok(SwePage {
                id: r.get(0)?,
                order: r.get(1)?,
                kind: r.get(2)?,
                label: r.get(3)?,
                title: String::new(),
                folder: r.get(4)?,
                frame: r.get(5)?,
            })
        })
        .map_err(sql)?;
    for mut pg in rows.flatten() {
        pg.title = text.get(&pg.id).cloned().unwrap_or_default();
        p.pages.push(pg);
    }

    // Components with their tag, description and first part.
    let mut st = conn
        .prepare(
            "SELECT com_id, COALESCE(NULLIF(com_tagpath, ''), com_tag, ''), COALESCE(com_type, -1) FROM tew_component",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(sql)?;
    for (id, tag, kind) in rows.flatten() {
        p.components.insert(
            id,
            SweComponent {
                id,
                tag,
                kind,
                description: text.get(&id).cloned().unwrap_or_default(),
                ..Default::default()
            },
        );
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
    for (id, part, maker) in rows.flatten() {
        if let Some(c) = p.components.get_mut(&id)
            && c.part_number.is_empty()
        {
            c.part_number = part;
            c.manufacturer = maker;
        }
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
    }
    let mut pins: BTreeMap<i64, Vec<RawPin>> = BTreeMap::new();
    let mut st = conn
        .prepare(
            "SELECT p.spt_sym_id, COALESCE(p.spt_sct_no, 0), COALESCE(p.spt_no, 0), p.spt_dx, p.spt_dy, t.cte_txt, \
             CASE WHEN (SELECT COUNT(*) FROM tew_componentelement x WHERE x.cel_com_id = e.cel_com_id AND x.cel_sym_id > 0) > 1 \
             THEN e.cel_orderno END \
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
                },
            ))
        })
        .map_err(sql)?;
    for (sym, pin) in rows.flatten() {
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
    for (id, page, component, block, x, y, angle, sx, sy, ex, ey) in rows.flatten() {
        let (Some(x), Some(y)) = (x, y) else {
            unplaced += 1;
            continue;
        };
        if !(x.is_finite() && y.is_finite() && angle.is_finite()) {
            unplaced += 1;
            continue;
        }
        let at = Point::new(x, y);
        let scale = (if sx == 0.0 { 1.0 } else { sx }, if sy == 0.0 { 1.0 } else { sy });
        let mut out_pins: Vec<SwePin> = Vec::new();
        for raw in pins.remove(&id).unwrap_or_default() {
            // Unnamed terminals: numbered by element (terminal strips, connectors drawn pin by
            // pin, so each symbol of the device has its own names), else by circuit, else 1, 2...
            let name = raw.terminal.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
                if let Some(el) = raw.element {
                    format!("{}.{}", el + 1, raw.no + 1)
                } else if raw.circuit > 0 {
                    format!("{}.{}", raw.circuit, raw.no + 1)
                } else {
                    (raw.no + 1).to_string()
                }
            });
            out_pins.push(SwePin {
                name,
                at: place(at, angle, scale, raw.offset),
            });
        }
        // `sym_posend*` is a box size (width, minus height) in the symbol's frame; 0, 0 is none.
        let corner = match (ex, ey) {
            (Some(ex), Some(ey)) if ex.is_finite() && ey.is_finite() && (ex != 0.0 || ey != 0.0) => {
                Some(place(at, angle, scale, Point::new(ex, ey)))
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
        p.warnings
            .push(format!("{unplaced} symbol(s) without a position were left out"));
    }

    let mut st = conn
        .prepare(
            "SELECT l.lin_fil_id, l.lin_ptstartx, l.lin_ptstarty, l.lin_ptendx, l.lin_ptendy, q.equ_text \
             FROM tew_line l LEFT JOIN tew_equipotential q ON q.equ_id = l.lin_equ_id ORDER BY l.lin_fil_id, l.lin_id",
        )
        .map_err(sql)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<f64>>(1)?,
                r.get::<_, Option<f64>>(2)?,
                r.get::<_, Option<f64>>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(sql)?;
    for (page, ax, ay, bx, by, potential) in rows.flatten() {
        let (Some(ax), Some(ay), Some(bx), Some(by)) = (ax, ay, bx, by) else {
            continue;
        };
        if ![ax, ay, bx, by].iter().all(|v| v.is_finite()) {
            continue;
        }
        p.lines.push(SweLine {
            page,
            a: Point::new(ax, ay),
            b: Point::new(bx, by),
            potential: potential.filter(|s| !s.trim().is_empty()),
        });
    }
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
        } else if c.is_alphanumeric() || c == '_' {
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

/// The sheet size a title block name gives (`..._A2_...`, `...BASEA3...`), if any.
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
        let mut n = name.clone();
        if !used.insert(n.clone()) {
            n = format!("{name}#{}", i + 1);
            used.insert(n.clone());
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

/// Build WiringCraft pages from a read project: one page per schematic or line-diagram page,
/// each symbol as a box with its connection points, each line as a wire joined to the pins at
/// its ends, and one device per component with its part data.
pub fn build(p: &SweProject, first_page_number: usize) -> (Built, Vec<String>) {
    let mut warnings = Vec::new();
    let mut built = Built {
        symbols: Vec::new(),
        parts: Vec::new(),
        pages: Vec::new(),
        devices: BTreeMap::new(),
    };
    let drawn: Vec<&SwePage> = p.pages.iter().filter(|pg| DRAWN_KINDS.contains(&pg.kind)).collect();
    let others = p.pages.len() - drawn.len();
    if others > 0 {
        warnings.push(format!(
            "{others} page(s) that are not schematics (cover, reports, cabinet layouts) were not imported"
        ));
    }

    // Tags: one per component, made unique if two components give the same tag. Each pair of
    // origin-destination arrows becomes a pair of interruption points with its own tag.
    let mut tags: BTreeMap<i64, DeviceTag> = BTreeMap::new();
    let mut taken: BTreeSet<DeviceTag> = BTreeSet::new();
    let mut untagged = 0;
    let mut arrows = 0;
    // A function aspect every device shares (a project with one function, `=F1`) says nothing on
    // the drawing; it is left out of the tags.
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
            "every device has the function ={}; it is left out of the tags",
            f.join(".")
        ));
    }
    for c in p.components.values() {
        let tag = if c.kind == COMPONENT_ARROW {
            arrows += 1;
            DeviceTag::parse(&format!("-OD{arrows}")).ok()
        } else {
            device_tag(&c.tag).map(|mut t| {
                if shared_function.is_some() {
                    t.function.clear();
                }
                t
            })
        };
        let Some(mut tag) = tag else {
            untagged += 1;
            continue;
        };
        if taken.contains(&tag) {
            let base = tag.clone();
            for n in 2.. {
                let mut t = base.clone();
                if let Some(last) = t.product.last_mut() {
                    *last = format!("{last}_{n}");
                }
                if !taken.contains(&t) {
                    tag = t;
                    break;
                }
            }
        }
        taken.insert(tag.clone());
        tags.insert(c.id, tag);
    }
    if untagged > 0 {
        warnings.push(format!(
            "{untagged} component(s) without a usable tag are drawn without one"
        ));
    }

    // Symbols by layout, so identical boxes share one library symbol.
    let mut symbol_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut per_block: BTreeMap<String, usize> = BTreeMap::new();
    let mut skipped_blocks = 0;
    let mut used_components: BTreeSet<i64> = BTreeSet::new();
    let mut wires_total = 0;
    let mut wires_free = 0;

    // Symbols and lines by page, in one pass each.
    let mut syms_of: BTreeMap<i64, Vec<&SweSymbol>> = BTreeMap::new();
    for s in &p.symbols {
        syms_of.entry(s.page).or_default().push(s);
    }
    let mut lines_of: BTreeMap<i64, Vec<&SweLine>> = BTreeMap::new();
    for l in &p.lines {
        lines_of.entry(l.page).or_default().push(l);
    }

    for (i, pg) in drawn.iter().enumerate() {
        let syms: Vec<&SweSymbol> = syms_of.remove(&pg.id).unwrap_or_default();
        let lines: Vec<&SweLine> = lines_of.remove(&pg.id).unwrap_or_default();
        let mut extent = Point::new(0.0, 0.0);
        let mut grow = |q: Point| {
            extent.x = extent.x.max(q.x);
            extent.y = extent.y.max(q.y);
        };
        for s in &syms {
            grow(s.at);
            s.pins.iter().for_each(|q| grow(q.at));
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
                first_page_number + i
            ));
        }
        let height = size.mm().1;
        let flip = |q: Point| Point::new(round(q.x), round(height - q.y));

        let title = match (pg.folder.is_empty(), pg.title.is_empty()) {
            (false, false) => format!("{} {}: {}", pg.folder, pg.label, pg.title),
            (false, true) => format!("{} {}", pg.folder, pg.label),
            (true, false) => format!("{}: {}", pg.label, pg.title),
            (true, true) => pg.label.clone(),
        };
        let mut page = Page::new(
            format!("swe-page-{}", pg.id),
            (first_page_number + i).to_string(),
            title,
        );
        page.size = size;

        // Pin positions on this page (y down) for joining wire ends.
        let mut pin_at: Vec<(Point, String, String)> = Vec::new();
        for s in &syms {
            if s.pins.is_empty() && s.corner.is_none() {
                skipped_blocks += 1;
                continue;
            }
            let origin = flip(s.at);
            let pins: Vec<(String, Point)> = s.pins.iter().map(|q| (q.name.clone(), flip(q.at))).collect();
            let (graphics, out_pins) = box_symbol(&s.block, origin, s.corner.map(flip), &pins);
            let arrow = s
                .component
                .and_then(|c| p.components.get(&c))
                .is_some_and(|c| c.kind == COMPONENT_ARROW);
            let role = if arrow {
                wc_core::symbol::SymbolRole::Interruption
            } else {
                wc_core::symbol::SymbolRole::Standalone
            };
            let key = format!("{}|{:?}|{:?}|{:?}", s.block, role, graphics, out_pins);
            let sym_id = match symbol_ids.get(&key) {
                Some(id) => id.clone(),
                None => {
                    let n = per_block.entry(s.block.clone()).or_insert(0);
                    *n += 1;
                    let id = format!("swe.{}.{}", slug(&s.block), n);
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

        let end = |q: Point| -> Endpoint {
            match pin_at.iter().find(|(at, _, _)| near(*at, q)) {
                Some((_, item, pin)) => Endpoint::Pin {
                    item: item.clone(),
                    pin: pin.clone(),
                },
                None => Endpoint::Point { at: q },
            }
        };
        for (n, l) in lines.iter().enumerate() {
            let (a, b) = (flip(l.a), flip(l.b));
            if near(a, b) {
                continue;
            }
            let (from, to) = (end(a), end(b));
            if matches!(from, Endpoint::Point { .. }) && matches!(to, Endpoint::Point { .. }) {
                wires_free += 1;
            }
            wires_total += 1;
            page.wires.push(Wire {
                id: format!("w{}", n + 1),
                from,
                to,
                via: Vec::new(),
                number: l.potential.clone(),
                potential: None,
                cable: None,
            });
        }
        built.pages.push(page);
    }
    if skipped_blocks > 0 {
        warnings.push(format!(
            "{skipped_blocks} drawn item(s) without connection points (frames, labels, link arrows) were left out"
        ));
    }
    if wires_total > 0 && wires_free > 0 {
        warnings.push(format!(
            "{wires_free} of {wires_total} wire segment(s) touch no symbol pin at either end (they join other segments)"
        ));
    }

    for c in p.components.values() {
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
        // Arrows join through their placed tags; they are not devices.
        if c.kind == COMPONENT_ARROW
            || (!used_components.contains(&c.id) && properties.is_empty() && c.description.is_empty())
        {
            continue;
        }
        built.devices.insert(
            tag.clone(),
            Device {
                part: None,
                description: c.description.clone(),
                properties,
            },
        );
    }
    (built, warnings)
}

#[cfg(test)]
mod tests;
