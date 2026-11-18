//! Symbol and page graphics from the DWG files of a SolidWorks Electrical archive (#163), read
//! with the crate's own DWG reader ([`crate::dwg`]).
//!
//! An archive holds each symbol's drawing as `Block/<block name>.dwg` (the project's own copy
//! under `Projects/<n>/Block/`, the library's elsewhere) and each page's drawing as
//! `Projects/<n>/Drawings/<page id>.ewg`, a DWG file under another name. A symbol's drawing is
//! the model space of its block file, in the symbol's own frame: millimetres, y up, the
//! insertion point at the origin. A page's drawing is in page millimetres, y up.
//!
//! The import draws a placed symbol from its block file, turned and scaled like the symbol;
//! its pins stay where the database puts them. A symbol whose block file is missing, cannot be
//! read or draws nothing stays a box, and is counted. From a page drawing only what the
//! database does not already give is kept: texts, frames and notes. Block references (the
//! symbols themselves and the title block, which WiringCraft draws itself) and lines that lie
//! on one of the page's wire lines are left out.
//!
//! The archive is treated as hostile: file sizes, the number of files and the bytes read in all
//! are capped, and a file that cannot be read is reported, never a panic.

use std::collections::BTreeMap;
use std::io::Read;

use wc_core::geom::{Point, Shape};

use super::{JOIN, SweLine, SweSymbol, round};
use crate::dxf::{self, Entity};
use crate::{Error, Result, dwg};

/// The largest single drawing file read out of an archive.
pub const MAX_DRAWING_BYTES: u64 = 64 << 20;
/// The most drawing files read out of one archive.
pub const MAX_DRAWING_FILES: usize = 20_000;
/// The most bytes all drawing files of one archive may hold in all.
pub const MAX_DRAWINGS_TOTAL: u64 = 1 << 30;
/// The most files named one by one in the warnings; the rest are counted.
const MAX_NAMED: usize = 10;

/// The symbol and page drawings of an archive, ready for [`super::build_with`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SweDrawings {
    /// Symbol drawings by block name, lower case.
    blocks: BTreeMap<String, dxf::Drawing>,
    /// Page drawings by page id (`tew_file.fil_id`).
    pages: BTreeMap<i64, dxf::Drawing>,
    /// Entities of the drawings that were not read, by type.
    pub skipped: BTreeMap<String, usize>,
    /// Files that could not be read, and why.
    pub warnings: Vec<String>,
    /// Entities all drawings of one import may expand to; [`dxf::MAX_DRAWN`] when unset.
    limit: Option<usize>,
}

/// The key a block's drawing is kept under: its name without surrounding spaces, in lower case,
/// so `Coil .dwg` is the drawing of block `Coil` (the database's names are trimmed).
fn block_key(name: &str) -> String {
    name.trim().to_lowercase()
}

impl SweDrawings {
    /// Entities all drawings of one import may expand to.
    pub(crate) fn draw_limit(&self) -> usize {
        self.limit.unwrap_or(dxf::MAX_DRAWN)
    }

    /// Lower the entity limit, so a test reaches it with a small drawing.
    #[cfg(test)]
    pub(crate) fn set_draw_limit(&mut self, n: usize) {
        self.limit = Some(n);
    }

    /// Read a symbol's block file (DWG bytes) for block `name`. A second file for the same
    /// name (ignoring case and surrounding spaces) replaces the first.
    pub fn add_block(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        let d = self.read(bytes, &format!("block {name}"))?;
        self.blocks.insert(block_key(name), d);
        Ok(())
    }

    /// Read the drawing of page `id` (DWG bytes, as in an `.ewg` file).
    pub fn add_page(&mut self, id: i64, bytes: &[u8]) -> Result<()> {
        let d = self.read(bytes, &format!("page drawing {id}"))?;
        self.pages.insert(id, d);
        Ok(())
    }

    /// How many block files were read.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// How many page drawings were read.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.pages.is_empty()
    }

    /// Whether the archive has a drawing for block `name`.
    #[cfg(test)]
    pub(crate) fn has_block(&self, name: &str) -> bool {
        self.blocks.contains_key(&block_key(name))
    }

    fn read(&mut self, bytes: &[u8], what: &str) -> Result<dxf::Drawing> {
        let sections = dwg::read_sections(bytes)?;
        let read = dwg::objects::dxf_drawing(&sections)?;
        let damaged = read.skipped.get(dwg::objects::DAMAGED).copied().unwrap_or(0);
        for (k, n) in read.skipped {
            *self.skipped.entry(k).or_default() += n;
        }
        // A drawing read in part is used; why the rest is missing is said once per file.
        if damaged > 0 {
            let first = read.damaged.first().map(String::as_str).unwrap_or("");
            self.warnings.push(format!(
                "{what}: {damaged} object(s) could not be read and are not drawn, the first because of: {first}"
            ));
        }
        Ok(read.drawing)
    }

    /// The shapes of block `name` placed like `s` (turned by its angle, then scaled along the
    /// page axes, exactly as [`super::place`] places its points), in the symbol's own frame on
    /// the page: y down, the insertion point at the origin. `None` when the archive has no
    /// drawing for the block or it draws nothing. Entities left out are counted in `skipped`
    /// (`dxf::OVER_LIMIT` once `budget` runs out).
    pub(crate) fn symbol_graphics(
        &self,
        s: &SweSymbol,
        budget: &mut usize,
        skipped: &mut BTreeMap<String, usize>,
    ) -> Option<Vec<Shape>> {
        let (sx, sy) = s.scale;
        let (sin, cos) = s.angle.to_radians().sin_cos();
        let m = [cos * sx, -sin * sx, sin * sy, cos * sy];
        self.mapped(&s.block, m, s.angle, budget, skipped)
    }

    /// The box round block `name` placed like `s` (turned and scaled like
    /// [`Self::symbol_graphics`]), on the page: y up, in page millimetres. `None` when the
    /// archive has no drawing for the block, it draws nothing, its box has no width or no
    /// height (or one too large to keep), or the drawing is larger than `budget` still allows:
    /// a box round part of it would be too small. Used for cabinet layout footprints without a
    /// box size (#232); the entities read count against the import's `budget`.
    pub(crate) fn placed_extents(&self, s: &SweSymbol, budget: &mut usize) -> Option<(Point, Point)> {
        let mut skipped = BTreeMap::new();
        let shapes = self.symbol_graphics(s, budget, &mut skipped)?;
        if skipped.contains_key(crate::dxf::OVER_LIMIT) {
            return None;
        }
        let corners: Vec<Point> = shapes.iter().flat_map(corners_of).collect();
        // The symbol's frame is y down from the insertion point.
        let (min, max) = wc_core::geom::bounds_of(&corners);
        let (lo, hi) = (
            Point::new(s.at.x + min.x, s.at.y - max.y),
            Point::new(s.at.x + max.x, s.at.y - min.y),
        );
        // Kept in thousandths of a millimetre, so the sides must stay finite at that scale.
        let fits = |a: f64, b: f64| b > a && ((b - a) * 1000.0).round().is_finite() && a.is_finite();
        (fits(lo.x, hi.x) && fits(lo.y, hi.y)).then_some((lo, hi))
    }

    /// The shapes of block `name` as they are, unturned and unscaled, in the symbol's own frame:
    /// y down, the insertion point at the origin. `None` when the archive has no drawing for the
    /// block or it draws nothing. Used for library symbols (#230).
    pub(crate) fn block_graphics(
        &self,
        name: &str,
        budget: &mut usize,
        skipped: &mut BTreeMap<String, usize>,
    ) -> Option<Vec<Shape>> {
        self.mapped(name, [1.0, 0.0, 0.0, 1.0], 0.0, budget, skipped)
    }

    /// Block `name` drawn with the linear map `m` and texts turned by `rot` degrees, rounded.
    fn mapped(
        &self,
        name: &str,
        m: [f64; 4],
        rot: f64,
        budget: &mut usize,
        skipped: &mut BTreeMap<String, usize>,
    ) -> Option<Vec<Shape>> {
        let d = self.blocks.get(&block_key(name))?;
        let (shapes, _) = dxf::draw_mapped(d, &d.entities, m, rot, skipped, budget);
        let shapes: Vec<Shape> = shapes.into_iter().filter_map(|(_, sh)| rounded(&sh)).collect();
        (!shapes.is_empty()).then_some(shapes)
    }

    /// The extra graphics of page `id` (texts, frames, notes) with y flipped and not yet moved
    /// onto a sheet: `(x, -y)` for a drawing point `(x, y)`. Block references and lines on one
    /// of `lines` (the page's wire lines, y up) are left out. Empty when the archive has no
    /// drawing for the page. Entities left out are counted in `skipped`. [`on_sheet`] moves them
    /// onto a sheet once its size is known.
    pub(crate) fn page_graphics(
        &self,
        id: i64,
        lines: &[&SweLine],
        budget: &mut usize,
        skipped: &mut BTreeMap<String, usize>,
    ) -> Vec<Shape> {
        let Some(d) = self.pages.get(&id) else {
            return Vec::new();
        };
        let on_wire = |e: &Entity| {
            let (a, b) = (Point::new(e.f(10), e.f(20)), Point::new(e.f(11), e.f(21)));
            lines.iter().any(|l| {
                (a.distance(l.a) <= JOIN && b.distance(l.b) <= JOIN)
                    || (a.distance(l.b) <= JOIN && b.distance(l.a) <= JOIN)
            })
        };
        let kept: Vec<Entity> = d
            .entities
            .iter()
            .filter(|e| match e.kind.as_str() {
                "INSERT" => false,
                "LINE" => !on_wire(e),
                _ => true,
            })
            .cloned()
            .collect();
        let (shapes, _) = dxf::draw(d, &kept, skipped, budget);
        shapes
            .into_iter()
            .map(|(_, sh)| sh)
            .filter(|sh| {
                let (min, max) = sh.bounds();
                [min.x, min.y, max.x, max.y].iter().all(|v| v.is_finite())
            })
            .collect()
    }
}

/// Page graphics from [`SweDrawings::page_graphics`] moved onto a sheet `height` mm high, y
/// down, rounded like the rest of the import.
pub(crate) fn on_sheet(shapes: Vec<Shape>, height: f64) -> Vec<Shape> {
    shapes
        .iter()
        .filter_map(|sh| rounded(&sh.translate(Point::new(0.0, height))))
        .collect()
}

/// `sh` with its coordinates rounded like the rest of the import, or nothing when a coordinate
/// is not a finite number (a hostile file's huge scale).
fn rounded(sh: &Shape) -> Option<Shape> {
    let p = |q: &Point| Point::new(round(q.x), round(q.y));
    let shape = match sh {
        Shape::Line { a, b } => Shape::Line { a: p(a), b: p(b) },
        Shape::Polyline { points, closed, filled } => Shape::Polyline {
            points: points.iter().map(p).collect(),
            closed: *closed,
            filled: *filled,
        },
        Shape::Rect { min, max } => Shape::Rect {
            min: p(min),
            max: p(max),
        },
        Shape::Circle { center, radius, filled } => Shape::Circle {
            center: p(center),
            radius: round(*radius),
            filled: *filled,
        },
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => Shape::Arc {
            center: p(center),
            radius: round(*radius),
            start: round(*start),
            end: round(*end),
        },
        Shape::Text {
            at,
            text,
            height,
            align,
            angle,
        } => Shape::Text {
            at: p(at),
            text: text.clone(),
            height: round(*height),
            align: *align,
            angle: round(*angle),
        },
    };
    let (min, max) = shape.bounds();
    [min.x, min.y, max.x, max.y]
        .iter()
        .all(|v| v.is_finite())
        .then_some(shape)
}

/// What a file name in an archive is, for this module.
#[derive(Debug, PartialEq)]
enum Kind {
    /// A block file, with its block name; `true` when it is the picked project's own copy.
    Block(String, bool),
    /// A page drawing of the picked project, with the page id.
    Page(i64),
}

fn kind_of(name: &str, folder: &str) -> Option<Kind> {
    let (dir, file) = name.rsplit_once('/').unwrap_or(("", name));
    let (stem, ext) = file.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let own = !folder.is_empty()
        && dir.len() > folder.len()
        && dir.is_char_boundary(folder.len())
        && dir[..folder.len()].eq_ignore_ascii_case(folder)
        && dir.as_bytes()[folder.len()] == b'/';
    // Another project's own blocks are never used for this one.
    let other_project = !own
        && dir
            .split('/')
            .next()
            .is_some_and(|top| top.eq_ignore_ascii_case("projects"));
    if ext.eq_ignore_ascii_case("dwg") && !other_project && dir.split('/').any(|d| d.eq_ignore_ascii_case("block")) {
        return Some(Kind::Block(stem.to_owned(), own));
    }
    if ext.eq_ignore_ascii_case("ewg") && own && dir[folder.len() + 1..].eq_ignore_ascii_case("drawings") {
        return stem.trim().parse().ok().map(Kind::Page);
    }
    None
}

/// Read the symbol and page drawings of the project in `folder` (`Projects/4`, as
/// [`super::database_from_archive`] names it) out of an archive. Library block files are read
/// too; the project's own copy of a block wins over the library's. Files that cannot be read are
/// in [`SweDrawings::warnings`].
pub fn drawings_from_archive<R: Read + std::io::Seek>(reader: R, folder: &str) -> Result<SweDrawings> {
    drawings_within(reader, folder, Limits::ARCHIVE)
}

/// Byte limits on the drawing files read out of one archive.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The most bytes one file may hold.
    pub file: u64,
    /// The most bytes all files may hold in all.
    pub total: u64,
}

impl Limits {
    const ARCHIVE: Limits = Limits {
        file: MAX_DRAWING_BYTES,
        total: MAX_DRAWINGS_TOTAL,
    };
}

/// [`drawings_from_archive`] with byte limits `lim`.
pub(crate) fn drawings_within<R: Read + std::io::Seek>(reader: R, folder: &str, lim: Limits) -> Result<SweDrawings> {
    let mut zip =
        zip::ZipArchive::new(reader).map_err(|e| Error::Format(format!("not a SolidWorks Electrical archive: {e}")))?;
    let folder = folder.trim_end_matches('/');
    let mut wanted: Vec<(usize, String, Kind)> = (0..zip.len())
        .filter_map(|i| {
            let name = zip.name_for_index(i)?.to_owned();
            let kind = kind_of(&name, folder)?;
            Some((i, name, kind))
        })
        .collect();
    // Library blocks first, so the project's own copy of a block replaces them.
    wanted.sort_by_key(|(i, _, k)| (matches!(k, Kind::Block(_, true) | Kind::Page(_)), *i));
    let mut out = SweDrawings::default();
    let mut failed: Vec<String> = Vec::new();
    let mut left_out = 0usize;
    let mut total = 0u64;
    for (n, (i, name, kind)) in wanted.into_iter().enumerate() {
        if n >= MAX_DRAWING_FILES || total >= lim.total {
            left_out += 1;
            continue;
        }
        // Every byte read counts against the total, also of a file then refused.
        let (got, read) = read_entry(&mut zip, i, &name, lim, lim.total - total);
        total += read;
        let bytes = match got {
            Ok(b) => b,
            Err(e) => {
                failed.push(e.to_string());
                continue;
            }
        };
        let got = match kind {
            Kind::Block(block, _) => out.add_block(&block, &bytes),
            Kind::Page(id) => out.add_page(id, &bytes),
        };
        if let Err(e) = got {
            failed.push(format!("{name}: {e}"));
        }
    }
    if !failed.is_empty() {
        let more = failed.len().saturating_sub(MAX_NAMED);
        failed.truncate(MAX_NAMED);
        let mut w = format!(
            "{} drawing file(s) could not be read: {}",
            failed.len() + more,
            failed.join("; ")
        );
        if more > 0 {
            w.push_str(&format!("; and {more} more"));
        }
        out.warnings.push(w);
    }
    if left_out > 0 {
        out.warnings.push(format!(
            "{left_out} drawing file(s) were not read: the archive has more than {MAX_DRAWING_FILES} files or {} MB of drawings",
            lim.total >> 20
        ));
    }
    Ok(out)
}

/// Read entry `i`, at most `lim.file` and at most `left` bytes. Returns the bytes
/// (or why not) and how many bytes were read either way.
fn read_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    i: usize,
    name: &str,
    lim: Limits,
    left: u64,
) -> (Result<Vec<u8>>, u64) {
    let f = match zip.by_index(i) {
        Ok(f) => f,
        Err(e) => return (Err(Error::Format(format!("{name}: {e}"))), 0),
    };
    let mut bytes = Vec::new();
    // Read at most one byte past the limit, whatever the zip header claims.
    let limit = lim.file.min(left);
    let got = f.take(limit + 1).read_to_end(&mut bytes);
    let read = bytes.len() as u64;
    if let Err(e) = got {
        return (Err(Error::Format(format!("{name}: {e}"))), read);
    }
    if read > lim.file {
        return (
            Err(Error::Format(format!("{name} is larger than {} MB", lim.file >> 20))),
            read,
        );
    }
    if read > limit {
        return (
            Err(Error::Format(format!(
                "{name} would take the drawings past {} MB in all",
                lim.total >> 20
            ))),
            read,
        );
    }
    (Ok(bytes), read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_are_sorted_into_blocks_and_pages() {
        let f = "Projects/4";
        assert_eq!(kind_of("Block/Coil.dwg", f), Some(Kind::Block("Coil".into(), false)));
        assert_eq!(
            kind_of("Library/BLOCK/a b.DWG", f),
            Some(Kind::Block("a b".into(), false))
        );
        assert_eq!(
            kind_of("Projects/4/Block/Coil.dwg", f),
            Some(Kind::Block("Coil".into(), true))
        );
        assert_eq!(kind_of("Projects/4/Drawings/17.ewg", f), Some(Kind::Page(17)));
        assert_eq!(kind_of("projects/4/drawings/0017.EWG", f), Some(Kind::Page(17)));
        // Another project's pages, a page not named by its id, other files.
        assert_eq!(kind_of("Projects/40/Drawings/17.ewg", f), None);
        assert_eq!(kind_of("Projects/5/Drawings/17.ewg", f), None);
        // Another project's blocks are not the library's.
        assert_eq!(kind_of("Projects/5/Block/Coil.dwg", f), None);
        assert_eq!(kind_of("Projects/40/Block/Coil.dwg", f), None);
        assert_eq!(kind_of("Projects/4/Drawings/cover.ewg", f), None);
        assert_eq!(kind_of("Projects/4/Drawings/sub/17.ewg", f), None);
        assert_eq!(kind_of("Block/.dwg", f), None);
        assert_eq!(kind_of("Drawings/Coil.dwg", f), None);
        assert_eq!(kind_of("Projects/4/Database/x.sqlite", f), None);
        // Non-ASCII names next to the folder never split a character.
        assert_eq!(kind_of("Projects/ø/Drawings/1.ewg", f), None);
    }
}

/// Points that bound `shape`: for an arc only what it sweeps (its ends and the quarter points
/// it passes), not its whole circle.
fn corners_of(shape: &Shape) -> Vec<Point> {
    let Shape::Arc {
        center,
        radius,
        start,
        end,
    } = shape
    else {
        let (a, b) = shape.bounds();
        return vec![a, b];
    };
    let mut sweep = end - start;
    if sweep <= 0.0 {
        sweep += 360.0;
    }
    let at = |deg: f64| {
        let t = deg.to_radians();
        Point::new(center.x + radius * t.cos(), center.y + radius * t.sin())
    };
    let mut points = vec![at(*start), at(start + sweep)];
    let first = (start / 90.0).ceil() as i64;
    let last = ((start + sweep) / 90.0).floor() as i64;
    // At most one turn: a sweep never passes more than four quarter points.
    for q in first..=last.min(first + 4) {
        points.push(at(q as f64 * 90.0));
    }
    points
}
