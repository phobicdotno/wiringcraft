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
}

impl SweDrawings {
    /// Read a symbol's block file (DWG bytes) for block `name`. A second file for the same
    /// name (ignoring case) replaces the first.
    pub fn add_block(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        let d = self.read(bytes)?;
        self.blocks.insert(name.to_lowercase(), d);
        Ok(())
    }

    /// Read the drawing of page `id` (DWG bytes, as in an `.ewg` file).
    pub fn add_page(&mut self, id: i64, bytes: &[u8]) -> Result<()> {
        let d = self.read(bytes)?;
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

    fn read(&mut self, bytes: &[u8]) -> Result<dxf::Drawing> {
        let sections = dwg::read_sections(bytes)?;
        let read = dwg::objects::dxf_drawing(&sections)?;
        for (k, n) in read.skipped {
            *self.skipped.entry(k).or_default() += n;
        }
        Ok(read.drawing)
    }

    /// The shapes of block `name` placed like `s` (turned by its angle, then scaled), in the
    /// symbol's own frame on the page: y down, the insertion point at the origin. `None` when
    /// the archive has no drawing for the block or it draws nothing.
    pub(crate) fn symbol_graphics(&self, s: &SweSymbol, budget: &mut usize) -> Option<Vec<Shape>> {
        let d = self.blocks.get(&s.block.to_lowercase())?;
        // SolidWorks Electrical turns, then scales; the drawing code scales, then turns. The
        // two agree when both scales are the same size, with the angle reversed when exactly
        // one of them mirrors.
        let (sx, sy) = s.scale;
        let angle = if sx * sy < 0.0 { -s.angle } else { s.angle };
        let mut skipped = BTreeMap::new();
        let (shapes, _) = dxf::draw_turned(d, &d.entities, (angle, sx, sy), &mut skipped, budget);
        let shapes: Vec<Shape> = shapes.into_iter().filter_map(|(_, sh)| rounded(&sh)).collect();
        (!shapes.is_empty()).then_some(shapes)
    }

    /// The extra graphics of page `id` (texts, frames, notes) on a sheet `height` mm high, in
    /// page coordinates (y down): block references and lines on one of `lines` (the page's wire
    /// lines, y up) are left out. Empty when the archive has no drawing for the page.
    pub(crate) fn page_graphics(&self, id: i64, height: f64, lines: &[&SweLine], budget: &mut usize) -> Vec<Shape> {
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
        let mut skipped = BTreeMap::new();
        let (shapes, _) = dxf::draw(d, &kept, &mut skipped, budget);
        shapes
            .into_iter()
            .filter_map(|(_, sh)| rounded(&sh.translate(Point::new(0.0, height))))
            .collect()
    }
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
    if ext.eq_ignore_ascii_case("dwg") && dir.split('/').any(|d| d.eq_ignore_ascii_case("block")) {
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
        if n >= MAX_DRAWING_FILES || total >= MAX_DRAWINGS_TOTAL {
            left_out += 1;
            continue;
        }
        let bytes = match read_entry(&mut zip, i, &name) {
            Ok(b) => b,
            Err(e) => {
                failed.push(e.to_string());
                continue;
            }
        };
        total += bytes.len() as u64;
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
            MAX_DRAWINGS_TOTAL >> 20
        ));
    }
    Ok(out)
}

fn read_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, i: usize, name: &str) -> Result<Vec<u8>> {
    let f = zip.by_index(i).map_err(|e| Error::Format(format!("{name}: {e}")))?;
    let mut bytes = Vec::new();
    // Read at most one byte past the limit, whatever the zip header claims.
    f.take(MAX_DRAWING_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Format(format!("{name}: {e}")))?;
    if bytes.len() as u64 > MAX_DRAWING_BYTES {
        return Err(Error::Format(format!(
            "{name} is larger than {} MB",
            MAX_DRAWING_BYTES >> 20
        )));
    }
    Ok(bytes)
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
        assert_eq!(kind_of("Projects/4/Drawings/cover.ewg", f), None);
        assert_eq!(kind_of("Projects/4/Drawings/sub/17.ewg", f), None);
        assert_eq!(kind_of("Block/.dwg", f), None);
        assert_eq!(kind_of("Drawings/Coil.dwg", f), None);
        assert_eq!(kind_of("Projects/4/Database/x.sqlite", f), None);
        // Non-ASCII names next to the folder never split a character.
        assert_eq!(kind_of("Projects/ø/Drawings/1.ewg", f), None);
    }
}
