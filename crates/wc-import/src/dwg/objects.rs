//! The objects of a DWG drawing: the second step of reading DWG files (#162), on top of the
//! sections [`super::read_sections`] returns.
//!
//! `AcDb:Handles` is the object map: each object's handle and where it starts in
//! `AcDb:AcDbObjects`. Each object there is a size, then bit-packed data:
//! its type, its own handle and fields, and at the end a stream of references to other objects
//! by handle (its owner, its layer, the block an INSERT shows). From AutoCAD 2007 on the texts
//! of an object are in a third stream at the end of its data. 2007 objects are otherwise written
//! like 2004 ones (type as a bit short, data size in bits), with the entity fields added in 2007
//! (material, shadow, attribute lock, MTEXT rectangle height). 2013 and 2018 objects are written
//! like 2010 ones, with a flag for data in the data store after the extension dictionary flag
//! (2013, #292) and an attribute type in ATTRIB and ATTDEF (2018). Multi-line attributes (2018)
//! are counted and skipped.
//!
//! The entities read are LINE, ARC, CIRCLE, LWPOLYLINE, POLYLINE (2D and 3D) with their
//! VERTEXes, TEXT, MTEXT, ATTDEF, ATTRIB, INSERT, DIMENSION (drawn by its anonymous block), HATCH,
//! ELLIPSE, SPLINE, SOLID,
//! TRACE and POINT. Each entity belongs to
//! the block whose BLOCK_HEADER owns it, or to model space; vertices belong to their polyline
//! and attributes to their INSERT. The entities become the same entities the DXF reader works
//! with, so a DWG drawing is drawn exactly like a DXF one: y flipped, block references expanded.
//! Other entity types (custom ones, numbered 500 and up, under the DXF name their class in
//! `AcDb:Classes` gives), and objects that cannot be decoded, are counted and skipped.
//!
//! Written from the published Open Design Specification for .dwg files only, and checked with
//! objects the tests write themselves. Input is treated as hostile: every read is checked, and a
//! damaged object is counted, never a panic.

use std::collections::{BTreeMap, HashMap, HashSet};

use wc_core::geom::{Point, Shape};

use super::bits::{Bits, modular_char, modular_short};
use super::classes::{Class, read_classes};
use super::codepage::Decoding;
use super::{Sections, Version, bad, read_sections};
use crate::Result;
use crate::dxf::{self, Entity};

/// The most objects a drawing's object map may list.
pub const MAX_OBJECTS: usize = 4_000_000;

/// The key under which objects that could not be decoded are counted in [`Drawing::skipped`].
pub const DAMAGED: &str = "damaged objects";
/// A polyline left out because one of its vertices could not be read.
pub const BROKEN_POLYLINE: &str = "POLYLINE (a vertex damaged)";

/// The skipped-count key for a class section that could not be read: custom entities (types 500
/// and up) are then not named or counted.
pub const CLASSES_DAMAGED: &str = "class section damaged (custom entities not counted)";

/// The skipped-count key for blocks whose base point could not be read: they are kept, placed
/// as if their base point were (0, 0).
pub const BASE_UNREAD: &str = "block base points not read (placed from 0, 0)";
/// The skipped-count key for INSERTs whose block is not in the drawing (or could not be read).
pub const UNRESOLVED_INSERT: &str = "block references to a block that is not in the drawing (not drawn)";
/// The key under which entities whose owner is not in the drawing are counted.
pub const ORPHANS: &str = "entities without an owner";

/// The drawing of a DWG file: its blocks and its model space, as page shapes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    /// Each block by name, with its shapes (nested block references expanded). Model and paper
    /// space are not in here.
    pub blocks: BTreeMap<String, Vec<Shape>>,
    /// The shapes of model space.
    pub model: Vec<Shape>,
    /// The POINT positions of each block (connection points in symbol drawings).
    pub block_points: BTreeMap<String, Vec<Point>>,
    /// The POINT positions of model space.
    pub model_points: Vec<Point>,
    /// How many of each kind of entity or object were not read, by type name.
    pub skipped: BTreeMap<String, usize>,
    /// Why the class section could not be read when it is counted under [`CLASSES_DAMAGED`]
    /// (always first), then why objects counted under [`DAMAGED`] could not be decoded (handle,
    /// offset and reason), then why base points counted under [`BASE_UNREAD`] could not be
    /// read, for the first [`MAX_REASONS`] of them.
    pub damaged: Vec<String>,
    /// Notes about the file as a whole, such as a code page its texts cannot be decoded in.
    pub warnings: Vec<String>,
}

/// The most reasons [`Drawing::damaged`] keeps.
pub const MAX_REASONS: usize = 100;

/// Read the drawing of a DWG file of a supported version.
pub fn read_drawing(bytes: &[u8]) -> Result<Drawing> {
    drawing(&read_sections(bytes)?)
}

/// The drawing held by the sections of a DWG file.
pub fn drawing(sections: &Sections) -> Result<Drawing> {
    let Read {
        drawing: d,
        mut skipped,
        damaged,
        warnings,
    } = dxf_drawing(sections)?;
    // One budget for model space and every block, so nested blocks cannot multiply the work.
    let mut budget = dxf::MAX_DRAWN;
    let mut draw = |ents: &[Entity], skipped: &mut BTreeMap<String, usize>| {
        let (shapes, points) = dxf::draw(&d, ents, skipped, &mut budget);
        (
            shapes.into_iter().map(|(_, s)| s).collect::<Vec<_>>(),
            points.into_iter().map(|(_, p)| p).collect::<Vec<_>>(),
        )
    };
    let (model, model_points) = draw(&d.entities, &mut skipped);
    let mut blocks = BTreeMap::new();
    let mut block_points = BTreeMap::new();
    for (name, ents) in &d.blocks {
        let (shapes, points) = draw(ents, &mut skipped);
        blocks.insert(name.clone(), shapes);
        if !points.is_empty() {
            block_points.insert(name.clone(), points);
        }
    }
    Ok(Drawing {
        blocks,
        model,
        block_points,
        model_points,
        skipped,
        damaged,
        warnings,
    })
}

/// What [`dxf_drawing`] reads from a file.
pub(crate) struct Read {
    pub(crate) drawing: dxf::Drawing,
    pub(crate) skipped: BTreeMap<String, usize>,
    /// As [`Drawing::damaged`]: the class section's reason first when it is damaged.
    pub(crate) damaged: Vec<String>,
    /// Notes about the file as a whole (an unknown code page).
    pub(crate) warnings: Vec<String>,
}

impl Read {
    /// Why the class section could not be read (empty when it was) and why objects could not
    /// be decoded: the class section's reason comes first in `damaged`, before those of the
    /// objects.
    pub(crate) fn damage_reasons(&self) -> (&str, &[String]) {
        match self.damaged.split_first() {
            Some((first, rest)) if self.skipped.contains_key(CLASSES_DAMAGED) => (first.as_str(), rest),
            _ => ("", self.damaged.as_slice()),
        }
    }
}

/// The entities of a DWG file as the DXF reader's entities, by block and model space, with a
/// count of what was not read.
pub(crate) fn dxf_drawing(sections: &Sections) -> Result<Read> {
    let map = sections
        .get("AcDb:Handles")
        .ok_or_else(|| bad("the drawing has no object map"))?;
    let objects = sections
        .get("AcDb:AcDbObjects")
        .ok_or_else(|| bad("the drawing has no objects"))?;
    let map = object_map(map)?;
    let mut skipped = BTreeMap::new();
    let mut decoded = Decoded::default();
    let mut damaged = Vec::new();
    // The classes name the custom types (500 and up); without them those are not counted.
    let classes = match sections.get("AcDb:Classes") {
        // Stored encrypted (a password-protected drawing): as unreadable as a damaged one.
        None if sections.skipped.iter().any(|name| name == "AcDb:Classes") => {
            *skipped.entry(CLASSES_DAMAGED.to_owned()).or_default() += 1;
            damaged.push("the class section is encrypted".to_owned());
            HashMap::new()
        }
        None => HashMap::new(),
        Some(data) => read_classes(data, sections.version, sections.maint).unwrap_or_else(|e| {
            // A section, not an object: counted under its own key, its reason with the others.
            *skipped.entry(CLASSES_DAMAGED.to_owned()).or_default() += 1;
            damaged.push(e.to_string());
            HashMap::new()
        }),
    };
    // 2004 texts are in the drawing's code page; one this reader does not know is read as
    // before (UTF-8 or Windows-1252) and said.
    let (decoding, known) = Decoding::of(sections.codepage);
    let mut warnings = Vec::new();
    if sections.version == Version::R2004 && !known {
        warnings.push(format!(
            "its texts are in {}, which is not supported: read with the legacy decoder (UTF-8 \
             when they are, else byte for byte as Latin-1)",
            Decoding::name(sections.codepage)
        ));
    }
    for (handle, at) in map {
        if let Err(e) = decode(
            objects,
            at,
            handle,
            sections.version,
            &classes,
            decoding,
            &mut decoded,
            &mut skipped,
        ) {
            *skipped.entry(DAMAGED.to_owned()).or_default() += 1;
            if damaged.len() < MAX_REASONS {
                damaged.push(format!("object {handle:X} at {at}: {e}"));
            }
        }
    }
    for reason in std::mem::take(&mut decoded.base_errors) {
        *skipped.entry(BASE_UNREAD.to_owned()).or_default() += 1;
        if damaged.len() < MAX_REASONS {
            damaged.push(reason);
        }
    }
    Ok(Read {
        drawing: decoded.assemble(&mut skipped),
        skipped,
        damaged,
        warnings,
    })
}

/// Read the object map: each object's handle and its offset in `AcDb:AcDbObjects`, in order.
pub(crate) fn object_map(data: &[u8]) -> Result<Vec<(u64, usize)>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut pos = 0usize;
    let cut = || bad("the object map is cut off");
    // Each part is a big-endian size (counting the size itself), pairs of handle and offset
    // changes, and a two-byte CRC. A part of size 2 (no entries, still with its CRC) ends the map.
    loop {
        let head = data.get(pos..pos + 2).ok_or_else(cut)?;
        let size = usize::from(u16::from_be_bytes([head[0], head[1]]));
        if size == 2 {
            data.get(pos + 2..pos + 4).ok_or_else(cut)?;
            break;
        }
        if size < 2 {
            return Err(bad(format!("the object map has a part of size {size}")));
        }
        let body = data.get(pos + 2..pos + size).ok_or_else(cut)?;
        data.get(pos + size..pos + size + 2).ok_or_else(cut)?;
        let (mut handle, mut at) = (0u64, 0i64);
        let mut i = 0;
        while i < body.len() {
            let (dh, n) = modular_char(body.get(i..).unwrap_or_default(), false)?;
            i += n;
            let (da, n) = modular_char(body.get(i..).unwrap_or_default(), true)?;
            i += n;
            handle = handle
                .checked_add(dh as u64)
                .ok_or_else(|| bad("a handle in the object map is out of range"))?;
            at = at
                .checked_add(da)
                .ok_or_else(|| bad("an object offset in the object map is out of range"))?;
            let at = usize::try_from(at).map_err(|_| bad("an object offset in the object map is negative"))?;
            // A handle listed twice would decode (and keep) the same object twice.
            if !seen.insert(handle) {
                return Err(bad(format!("handle {handle:X} is listed twice in the object map")));
            }
            out.push((handle, at));
            if out.len() > MAX_OBJECTS {
                return Err(bad("the object map lists too many objects"));
            }
        }
        pos += size + 2;
    }
    Ok(out)
}

/// One object's streams, after its type and own handle are read.
struct Object<'a> {
    version: Version,
    /// How its 2004 texts are decoded.
    decoding: Decoding,
    kind: u16,
    handle: u64,
    data: Bits<'a>,
    handles: Bits<'a>,
    strings: Option<Bits<'a>>,
}

impl Object<'_> {
    /// TV: a text, from the data stream (2004) or the string stream (2007 on).
    fn text(&mut self) -> Result<String> {
        match self.version {
            Version::R2004 => Ok(self.decoding.decode(&self.data.t8_bytes()?)),
            _ => self
                .strings
                .as_mut()
                .ok_or_else(|| bad("an object's text is missing"))?
                .tu(),
        }
    }

    fn handle_ref(&mut self) -> Result<u64> {
        self.handles.handle(self.handle)
    }

    /// Extended entity data: skipped.
    fn skip_eed(&mut self) -> Result<()> {
        loop {
            let size = self.data.bs()?;
            if size == 0 {
                return Ok(());
            }
            self.data.h()?;
            self.data.skip_bytes(u64::from(size))?;
        }
    }
}

/// Split the object at `at` of the objects section into its streams.
fn object(objects: &[u8], at: usize, version: Version, decoding: Decoding) -> Result<Object<'_>> {
    let rest = objects.get(at..).ok_or_else(|| bad("an object is past the end"))?;
    let (size, n) = modular_short(rest)?;
    let mut start = at + n;
    let handle_bits = match version {
        Version::R2004 | Version::R2007 => 0,
        _ => {
            let (h, m) = modular_char(objects.get(start..).unwrap_or_default(), false)?;
            start += m;
            usize::try_from(h).map_err(|_| bad("an object's handle stream is out of range"))?
        }
    };
    let bytes = start
        .checked_add(size)
        .and_then(|end| objects.get(start..end))
        .ok_or_else(|| bad("an object is cut off"))?;
    let total = size * 8;
    let mut head = Bits::new(bytes, 0, total);
    // A bit short up to 2007, a bit pair and one or two bytes from 2010 on.
    let kind = match version {
        Version::R2004 | Version::R2007 => head.bs()?,
        _ => match head.bb()? {
            0 => u16::from(head.rc()?),
            1 => u16::from(head.rc()?) + 0x1F0,
            _ => head.rs()?,
        },
    };
    // Where the data ends and the handles start: a size in bits up to 2007, from 2010 on
    // the handle stream's size before the type.
    let end = match version {
        Version::R2004 | Version::R2007 => head.rl()? as usize,
        _ => total
            .checked_sub(handle_bits)
            .ok_or_else(|| bad("an object's handle stream is larger than the object"))?,
    };
    if end > total || end < head.pos() {
        return Err(bad("an object's data size is out of range"));
    }
    // From 2007 on the texts and their size and flag end the data: the other fields stop
    // where they start.
    let (data_end, strings) = match version {
        Version::R2004 => (end, None),
        _ => string_stream(bytes, end)?,
    };
    if data_end < head.pos() {
        return Err(bad("an object's texts overlap its type"));
    }
    let mut data = Bits::new(bytes, head.pos(), data_end);
    let handle = data.h()?.1;
    Ok(Object {
        version,
        decoding,
        kind,
        handle,
        data,
        handles: Bits::new(bytes, end, total),
        strings,
    })
}

/// The string stream of a 2007 or 2010 object whose data ends at bit `end`: a flag in the last
/// bit, a size before it (in one or two 16-bit parts), and the texts before that. Also returns
/// where the other fields end: where the texts start, or at the flag when there are none.
pub(crate) fn string_stream(bytes: &[u8], end: usize) -> Result<(usize, Option<Bits<'_>>)> {
    let all = Bits::new(bytes, 0, end);
    let Some(flag_at) = end.checked_sub(1) else {
        return Ok((0, None));
    };
    if !all.bit_at(flag_at)? {
        return Ok((flag_at, None));
    }
    let cut = || bad("an object's texts are cut off");
    let mut at = flag_at.checked_sub(16).ok_or_else(cut)?;
    let mut size = usize::from(all.rs_at(at)?);
    if size & 0x8000 != 0 {
        at = at.checked_sub(16).ok_or_else(cut)?;
        let hi = usize::from(all.rs_at(at)?);
        size = (size & 0x7FFF) | (hi << 15);
    }
    let start = at.checked_sub(size).ok_or_else(cut)?;
    Ok((start, Some(Bits::new(bytes, start, at))))
}

/// Where an entity belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Model,
    Paper,
    Object(u64),
}

/// An entity read from the file, not yet put in its block.
#[derive(Debug)]
struct Item {
    handle: u64,
    owner: Owner,
    layer: u64,
    entity: Entity,
    /// The block an INSERT shows.
    block: Option<u64>,
    /// The vertices a POLYLINE owns, in their order.
    vertices: Vec<u64>,
}

/// What the objects of a file hold, before the entities are put in their blocks.
#[derive(Debug, Default)]
struct Decoded {
    items: Vec<Item>,
    /// Block names by the handle of their BLOCK_HEADER.
    blocks: HashMap<u64, String>,
    /// Block base points other than (0, 0), by the handle of their BLOCK_HEADER.
    bases: HashMap<u64, (f64, f64)>,
    /// Why a block's base point could not be read, per block.
    base_errors: Vec<String>,
    /// Layer names by handle.
    layers: HashMap<u64, String>,
    /// Invisible entities, whose vertices and attributes are left out with them.
    hidden: HashSet<u64>,
}

const BLOCK_HEADER: u16 = 49;
const LAYER: u16 = 51;

/// The name of a fixed object type that is an entity this reader does not draw.
fn unread_entity(kind: u16) -> Option<&'static str> {
    Some(match kind {
        8 => "MINSERT",
        12 => "VERTEX_MESH",
        13 => "VERTEX_PFACE",
        14 => "VERTEX_PFACE_FACE",
        28 => "3DFACE",
        29 => "POLYLINE_PFACE",
        30 => "POLYLINE_MESH",
        33 => "SHAPE",
        37 => "REGION",
        38 => "3DSOLID",
        39 => "BODY",
        40 => "RAY",
        41 => "XLINE",
        45 => "LEADER",
        46 => "TOLERANCE",
        47 => "MLINE",
        498 => "ACAD_PROXY_ENTITY",
        _ => return None,
    })
}

/// Entity types read here, with the DXF name they become.
fn read_entity(kind: u16) -> Option<&'static str> {
    Some(match kind {
        1 => "TEXT",
        2 => "ATTRIB",
        3 => "ATTDEF",
        // Kept to be put in their block; they draw nothing.
        4 => "BLOCK",
        5 => "ENDBLK",
        6 => "SEQEND",
        7 => "INSERT",
        20..=26 => "DIMENSION",
        10 | 11 => "VERTEX",
        15 | 16 => "POLYLINE",
        17 => "ARC",
        18 => "CIRCLE",
        19 => "LINE",
        27 => "POINT",
        31 | 32 => "SOLID",
        34 => "VIEWPORT",
        35 => "ELLIPSE",
        36 => "SPLINE",
        44 => "MTEXT",
        77 => "LWPOLYLINE",
        78 => "HATCH",
        _ => return None,
    })
}

/// The base point of a BLOCK_HEADER whose name has been read.
fn block_base(o: &mut Object) -> Result<(f64, f64)> {
    // The xref flags of a table entry: 64-flag, xref index and xref dependent up to 2004, only
    // the xref index from 2007 on (the specification lists all three for every version; a
    // real 2007 file has only the index, as 2010 files do).
    if o.version == Version::R2004 {
        o.data.b()?;
        o.data.bs()?;
        o.data.b()?;
    } else {
        o.data.bs()?;
    }
    // Anonymous, has attributes, is an xref, is an overlaid xref, xref loaded.
    o.data.b()?;
    o.data.b()?;
    let xref = o.data.b()?;
    let overlaid = o.data.b()?;
    o.data.b()?;
    // How many objects it owns, for a block that is not an xref.
    if !xref && !overlaid {
        o.data.bl()?;
    }
    let (x, y, _) = o.data.bd3()?;
    if !(x.is_finite() && y.is_finite()) {
        return Err(bad("a block's base point is not a number"));
    }
    Ok((x, y))
}

/// The most boundary paths, edges, knots, control or fit points or gradient colours a HATCH
/// may list; more is a damaged object.
const MAX_HATCH_ITEMS: u32 = 10_000;

/// A count of a HATCH list, refused above [`MAX_HATCH_ITEMS`].
fn hatch_count(o: &mut Object, what: &str) -> Result<u32> {
    let n = o.data.bl()?;
    if n > MAX_HATCH_ITEMS {
        return Err(bad(format!("a hatch lists {n} {what}")));
    }
    Ok(n)
}

/// A colour (CMC) of 2004 and later: index, true colour, flags, then a name and a book name
/// when the flags say so.
fn skip_color(o: &mut Object) -> Result<()> {
    o.data.bs()?;
    o.data.bl()?;
    let flags = o.data.rc()?;
    if flags < 4 {
        if flags & 1 != 0 {
            o.text()?;
        }
        if flags & 2 != 0 {
            o.text()?;
        }
    }
    Ok(())
}

/// A point of a HATCH boundary: both coordinates numbers.
fn hatch_point(o: &mut Object) -> Result<(f64, f64)> {
    let (x, y) = o.data.rd2()?;
    if !(x.is_finite() && y.is_finite()) {
        return Err(bad("a hatch boundary point is not a number"));
    }
    Ok((x, y))
}

/// Two edge angles in degrees: numbers still after the conversion (a huge angle in radians is
/// no number of degrees).
fn hatch_degrees(a0: f64, a1: f64) -> Result<(f64, f64)> {
    let (d0, d1) = (a0.to_degrees(), a1.to_degrees());
    if !(d0.is_finite() && d1.is_finite()) {
        return Err(bad("a hatch edge angle is too large"));
    }
    // Angles so far apart that their difference is no number are taken round the circle, so
    // the sweep can be measured; ordinary angles are kept as they are.
    if !(d1 - d0).is_finite() {
        return Ok((d0.rem_euclid(360.0), d1.rem_euclid(360.0)));
    }
    Ok((d0, d1))
}

/// A value of a HATCH boundary (radius, angle, ratio, knot, weight): a number.
fn hatch_value(o: &mut Object) -> Result<f64> {
    let v = o.data.bd()?;
    if !v.is_finite() {
        return Err(bad("a hatch boundary value is not a number"));
    }
    Ok(v)
}

/// HATCH: the pattern name, the solid-fill flag, the boundary paths and the style, as the DXF
/// codes the DXF reader draws a hatch from (2, 70, 91, 92, 72, 73, 93, 10/20, 42, ..., 75).
/// Angles are radians in DWG and degrees in DXF. The pattern lines and seed points after the
/// style are not needed.
fn hatch(o: &mut Object, e: &mut Codes) -> Result<()> {
    // Gradient fill: kind, reserved, angle, shift, single colour, tint, the colours, a name.
    o.data.bl()?;
    o.data.bl()?;
    o.data.bd()?;
    o.data.bd()?;
    o.data.bl()?;
    o.data.bd()?;
    for _ in 0..hatch_count(o, "gradient colours")? {
        o.data.bd()?;
        skip_color(o)?;
    }
    o.text()?;
    // Elevation and extrusion (a downward one mirrors the hatch, as for other entities).
    o.data.bd()?;
    let (_, _, z) = o.data.bd3()?;
    let name = o.text()?;
    let solid = o.data.b()?;
    o.data.b()?;
    let paths = hatch_count(o, "boundary paths")?;
    let mut boundary = 0u32;
    e.f(230, z).s(2, name).i(70, i64::from(solid)).i(91, i64::from(paths));
    for _ in 0..paths {
        let flags = o.data.bl()?;
        e.i(92, i64::from(flags));
        if flags & 2 == 0 {
            let edges = hatch_count(o, "edges")?;
            e.i(93, i64::from(edges));
            for _ in 0..edges {
                let kind = o.data.rc()?;
                e.i(72, i64::from(kind));
                match kind {
                    1 => {
                        let (a, b) = (hatch_point(o)?, hatch_point(o)?);
                        e.f(10, a.0).f(20, a.1).f(11, b.0).f(21, b.1);
                    }
                    2 => {
                        let c = hatch_point(o)?;
                        let r = hatch_value(o)?;
                        let (a0, a1) = (hatch_value(o)?, hatch_value(o)?);
                        let ccw = o.data.b()?;
                        e.f(10, c.0).f(20, c.1).f(40, r);
                        let (d0, d1) = hatch_degrees(a0, a1)?;
                        e.f(50, d0).f(51, d1).i(73, i64::from(ccw));
                    }
                    3 => {
                        let (c, m) = (hatch_point(o)?, hatch_point(o)?);
                        let ratio = hatch_value(o)?;
                        let (a0, a1) = (hatch_value(o)?, hatch_value(o)?);
                        let ccw = o.data.b()?;
                        e.f(10, c.0).f(20, c.1).f(11, m.0).f(21, m.1).f(40, ratio);
                        let (d0, d1) = hatch_degrees(a0, a1)?;
                        e.f(50, d0).f(51, d1).i(73, i64::from(ccw));
                    }
                    4 => {
                        let degree = o.data.bl()?;
                        let rational = o.data.b()?;
                        let periodic = o.data.b()?;
                        let knots = hatch_count(o, "knots")?;
                        let points = hatch_count(o, "control points")?;
                        e.i(94, i64::from(degree))
                            .i(73, i64::from(rational))
                            .i(74, i64::from(periodic));
                        e.i(95, i64::from(knots)).i(96, i64::from(points));
                        for _ in 0..knots {
                            e.f(40, hatch_value(o)?);
                        }
                        for _ in 0..points {
                            let p = hatch_point(o)?;
                            e.f(10, p.0).f(20, p.1);
                            if rational {
                                e.f(42, hatch_value(o)?);
                            }
                        }
                        // Fit points and their end tangents, from 2010.
                        if o.version >= Version::R2010 {
                            let fit = hatch_count(o, "fit points")?;
                            e.i(97, i64::from(fit));
                            for _ in 0..fit {
                                let p = hatch_point(o)?;
                                e.f(11, p.0).f(21, p.1);
                            }
                            if fit > 0 {
                                let (s, t) = (hatch_point(o)?, hatch_point(o)?);
                                e.f(12, s.0).f(22, s.1).f(13, t.0).f(23, t.1);
                            }
                        }
                    }
                    k => return Err(bad(format!("a hatch edge has unknown type {k}"))),
                }
            }
        } else {
            let bulges = o.data.b()?;
            let closed = o.data.b()?;
            let n = hatch_count(o, "polyline points")?;
            e.i(72, i64::from(bulges)).i(73, i64::from(closed)).i(93, i64::from(n));
            for _ in 0..n {
                let p = hatch_point(o)?;
                e.f(10, p.0).f(20, p.1);
                if bulges {
                    e.f(42, hatch_value(o)?);
                }
            }
        }
        // The boundary objects: a count here, their handles in the handle stream (read after
        // the data, so a hatch missing them is damaged).
        boundary += hatch_count(o, "boundary objects")?;
        e.i(97, 0);
    }
    e.i(75, i64::from(o.data.bs()?));
    for _ in 0..boundary {
        o.handle_ref()?;
    }
    Ok(())
}

/// ELLIPSE: centre, major axis end (relative), extrusion, axis ratio and the start and end
/// parameters (radians, as in DXF).
fn ellipse(o: &mut Object, e: &mut Codes) -> Result<()> {
    let d = &mut o.data;
    let (cx, cy, cz) = d.bd3()?;
    let (mx, my, mz) = d.bd3()?;
    let (ex, ey, z) = d.bd3()?;
    let ratio = d.bd()?;
    let (start, end) = (d.bd()?, d.bd()?);
    if ![cx, cy, cz, mx, my, mz, ex, ey, z, ratio, start, end]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err(bad("an ellipse has a value that is not a number"));
    }
    e.f(10, cx).f(20, cy).f(11, mx).f(21, my).f(230, z);
    e.f(40, ratio).f(41, start).f(42, end);
    Ok(())
}

/// The most knots, control points or fit points a SPLINE may list; more is a damaged object.
const MAX_SPLINE_POINTS: u32 = 10_000;

fn spline_count(o: &mut Object, what: &str) -> Result<u32> {
    let n = o.data.bl()?;
    if n > MAX_SPLINE_POINTS {
        return Err(bad(format!("a spline lists {n} {what}")));
    }
    Ok(n)
}

/// SPLINE, as DXF codes: 70 (closed), 71 degree, and either knots (40), control points (10/20)
/// and weights (41), or fit points (11/21). A scenario says which: 1 control points, 2 fit
/// points. From 2013 on, spline flags and a knot parameter follow it and decide instead: flag
/// bit 1 means fit points, knot parameter 15 (custom knots) means control points.
fn spline(o: &mut Object, e: &mut Codes) -> Result<()> {
    let mut scenario = o.data.bl()?;
    if o.version >= Version::R2013 {
        let flags = o.data.bl()?;
        let knot_parameter = o.data.bl()?;
        if flags & 1 != 0 {
            scenario = 2;
        }
        if knot_parameter == 15 {
            scenario = 1;
        }
    }
    if scenario != 1 && scenario != 2 {
        return Err(bad(format!("a spline has unknown scenario {scenario}")));
    }
    let degree = o.data.bl()?;
    if scenario == 1 {
        let d = &mut o.data;
        let _rational = d.b()?;
        let closed = d.b()?;
        let periodic = d.b()?;
        d.bd()?;
        d.bd()?;
        let knots = spline_count(o, "knots")?;
        let points = spline_count(o, "control points")?;
        let weighted = o.data.b()?;
        // A curve of degree d over n control points needs n + d + 1 knots; a periodic one may
        // list fewer, but never fewer than n + 1.
        let full = points.saturating_add(degree).saturating_add(1);
        if knots < points.saturating_add(1) || (!periodic && knots != full) {
            return Err(bad(format!(
                "a spline of degree {degree} lists {knots} knots for {points} control points"
            )));
        }
        e.i(70, i64::from(closed)).i(71, i64::from(degree));
        let mut last = f64::NEG_INFINITY;
        for _ in 0..knots {
            let k = o.data.bd()?;
            // Knots are numbers that never decrease (repeats are fine).
            if !k.is_finite() || k < last {
                return Err(bad("a spline's knots are not numbers in order"));
            }
            last = k;
            e.f(40, k);
        }
        for _ in 0..points {
            let (x, y, _) = o.data.bd3()?;
            e.f(10, x).f(20, y);
            if weighted {
                e.f(41, o.data.bd()?);
            }
        }
    } else {
        // Fit tolerance, the start and end tangents (DXF 12/22 and 13/23), then the fit points.
        o.data.bd()?;
        let (sx, sy, _) = o.data.bd3()?;
        let (ex, ey, _) = o.data.bd3()?;
        let fit = spline_count(o, "fit points")?;
        e.i(70, 0).i(71, i64::from(degree));
        e.f(12, sx).f(22, sy).f(13, ex).f(23, ey);
        for _ in 0..fit {
            let (x, y, _) = o.data.bd3()?;
            e.f(11, x).f(21, y);
        }
    }
    Ok(())
}

/// Decode the object at `at` into `out`.
#[allow(clippy::too_many_arguments)]
fn decode(
    objects: &[u8],
    at: usize,
    handle: u64,
    version: Version,
    classes: &HashMap<u16, Class>,
    decoding: Decoding,
    out: &mut Decoded,
    skipped: &mut BTreeMap<String, usize>,
) -> Result<()> {
    let mut o = object(objects, at, version, decoding)?;
    if o.handle != handle {
        return Err(bad("an object's handle differs from the object map"));
    }
    if o.kind == BLOCK_HEADER || o.kind == LAYER {
        // A table entry: common object data (reactors, no extension dictionary, from 2013 on
        // has data in the data store), then its name.
        o.skip_eed()?;
        o.data.bl()?;
        o.data.b()?;
        if o.version >= Version::R2013 {
            o.data.b()?;
        }
        let name = o.text()?;
        if o.kind == LAYER {
            out.layers.insert(handle, name);
        } else {
            // A base point that cannot be read leaves the block at (0, 0) rather than unnamed.
            match block_base(&mut o) {
                Ok(base) if base != (0.0, 0.0) => {
                    out.bases.insert(handle, base);
                }
                Ok(_) => {}
                Err(e) => out
                    .base_errors
                    .push(format!("base point of block {name} (object {handle:X}): {e}")),
            }
            out.blocks.insert(handle, name);
        }
        return Ok(());
    }
    // A custom type: an entity of its class is counted under the class's DXF name; a custom
    // object (dictionary entries, layouts and the like) draws nothing and is not counted.
    if o.kind >= 500 {
        if let Some(class) = classes.get(&o.kind)
            && class.entity
        {
            // A DXF name is upper-case letters, digits and _ or $; anything else (which could
            // look like one of the reader's own counters) is named by its type number.
            let plain = !class.dxf_name.is_empty()
                && class
                    .dxf_name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '$');
            let name = if plain {
                class.dxf_name.clone()
            } else {
                format!("custom entity type {}", o.kind)
            };
            *skipped.entry(name).or_default() += 1;
        }
        return Ok(());
    }
    if let Some(name) = unread_entity(o.kind) {
        *skipped.entry(name.to_owned()).or_default() += 1;
        return Ok(());
    }
    let Some(dxf_kind) = read_entity(o.kind) else {
        // Not an entity (tables, dictionaries, ...), or a custom type.
        return Ok(());
    };
    let common = entity_common(&mut o)?;
    if common.invisible {
        out.hidden.insert(handle);
        return Ok(());
    }
    let mut e = Codes::default();
    let mut block = None;
    let mut vertices = Vec::new();
    match o.kind {
        1..=3 => {
            if !text(&mut o, &mut e)? {
                *skipped.entry(format!("{dxf_kind} (multi-line)")).or_default() += 1;
                return Ok(());
            }
        }
        7 => block = Some(insert(&mut o, &mut e)?),
        78 => hatch(&mut o, &mut e)?,
        35 => ellipse(&mut o, &mut e)?,
        36 => spline(&mut o, &mut e)?,
        // Drawn by its anonymous block, which comes after the dimension style in the handles.
        20..=26 => {
            o.handle_ref()?;
            block = Some(o.handle_ref()?);
        }
        10 | 11 => {
            let flags = o.data.rc()?;
            let (x, y, _) = o.data.bd3()?;
            // Control points of a spline-fitted polyline are not on the curve: left out on
            // purpose (as hidden), so the polyline does not take them for damaged vertices.
            if flags & 0x10 != 0 {
                out.hidden.insert(handle);
                return Ok(());
            }
            e.f(10, x).f(20, y);
            // A 2D vertex (a 3D one stops at its point) has its widths and then the bulge of
            // the segment it starts; a negative start width stands for both, with no end width.
            if o.kind == 10 {
                if o.data.bd()? >= 0.0 {
                    o.data.bd()?;
                }
                let bulge = o.data.bd()?;
                if bulge != 0.0 {
                    e.f(42, bulge);
                }
            }
        }
        15 => {
            let flags = o.data.bs()?;
            o.data.bs()?;
            o.data.bd()?;
            o.data.bd()?;
            o.data.bt()?;
            o.data.bd()?;
            let (_, _, z) = o.data.be()?;
            e.i(70, i64::from(flags & 1)).f(230, z);
            vertices = owned(&mut o)?;
        }
        16 => {
            o.data.rc()?;
            let flags = o.data.rc()?;
            e.i(70, i64::from(flags & 1));
            vertices = owned(&mut o)?;
        }
        17 | 18 => {
            let (x, y, _) = o.data.bd3()?;
            let r = o.data.bd()?;
            o.data.bt()?;
            let (_, _, z) = o.data.be()?;
            e.f(10, x).f(20, y).f(40, r).f(230, z);
            if o.kind == 17 {
                let start = o.data.bd()?;
                let end = o.data.bd()?;
                e.f(50, start.to_degrees()).f(51, end.to_degrees());
            }
        }
        19 => {
            let z_zero = o.data.b()?;
            let x1 = o.data.rd()?;
            let x2 = o.data.dd(x1)?;
            let y1 = o.data.rd()?;
            let y2 = o.data.dd(y1)?;
            if !z_zero {
                let z1 = o.data.rd()?;
                o.data.dd(z1)?;
            }
            // Stored as text, an end that is not a number would read back as 0.
            if ![x1, x2, y1, y2].iter().all(|v| v.is_finite()) {
                return Err(bad("a line's end is not a number"));
            }
            e.f(10, x1).f(20, y1).f(11, x2).f(21, y2);
        }
        27 => {
            let (x, y, _) = o.data.bd3()?;
            e.f(10, x).f(20, y);
        }
        31 | 32 => {
            o.data.bt()?;
            o.data.bd()?;
            for code in 10..14 {
                let (x, y) = o.data.rd2()?;
                e.f(code, x).f(code + 10, y);
            }
            let (_, _, z) = o.data.be()?;
            e.f(230, z);
        }
        44 => mtext(&mut o, &mut e)?,
        77 => lwpolyline(&mut o, &mut e)?,
        _ => {}
    }
    if e.1 {
        return Err(bad(format!("a {dxf_kind} has a value that is not a number")));
    }
    out.items.push(Item {
        handle,
        owner: common.owner,
        layer: common.layer,
        entity: Entity {
            kind: dxf_kind.to_owned(),
            codes: e.0,
        },
        block,
        vertices,
    });
    Ok(())
}

/// The objects a POLYLINE owns (its vertices), in order: a count in the data, their handles in
/// the handle stream.
fn owned(o: &mut Object) -> Result<Vec<u64>> {
    let n = o.data.bl()?;
    let mut out = Vec::new();
    for _ in 0..n {
        // Each handle takes at least a byte, so a damaged count ends with the stream.
        out.push(o.handle_ref()?);
    }
    Ok(out)
}

/// Group codes of an entity being built.
#[derive(Debug, Default)]
struct Codes(Vec<(i32, String)>, bool);

impl Codes {
    /// A number; one that is not a number (NaN or infinite) marks the entity as damaged, since
    /// written as text it would read back as 0 and be drawn at an invented place.
    fn f(&mut self, code: i32, v: f64) -> &mut Self {
        self.1 |= !v.is_finite();
        self.0.push((code, format!("{v}")));
        self
    }
    fn i(&mut self, code: i32, v: i64) -> &mut Self {
        self.0.push((code, v.to_string()));
        self
    }
    fn s(&mut self, code: i32, v: String) -> &mut Self {
        self.0.push((code, v));
        self
    }
}

/// What every entity starts with.
struct Common {
    owner: Owner,
    layer: u64,
    invisible: bool,
}

/// Read the data every entity has, from both the data and the handle streams.
fn entity_common(o: &mut Object) -> Result<Common> {
    let r2010 = o.version >= Version::R2010;
    // Material and shadow flags are there from 2007 on, visual styles from 2010 on.
    let r2007 = o.version != Version::R2004;
    o.skip_eed()?;
    if o.data.b()? {
        // A preview picture of the entity.
        let size = if r2010 { o.data.bll()? } else { u64::from(o.data.rl()?) };
        o.data.skip_bytes(size)?;
    }
    let mode = o.data.bb()?;
    let reactors = o.data.bl()?;
    let no_xdict = o.data.b()?;
    // Whether the entity has data in the data store, from 2013 on.
    if o.version >= Version::R2013 {
        o.data.b()?;
    }
    let color = o.data.bs()?;
    if color & 0x8000 != 0 {
        o.data.bl()?;
    }
    if color & 0x2000 != 0 {
        o.data.bl()?;
    }
    o.data.bd()?;
    let linetype = o.data.bb()?;
    let plotstyle = o.data.bb()?;
    let mut material = 0;
    let mut styles = [false; 3];
    if r2007 {
        material = o.data.bb()?;
        o.data.rc()?;
    }
    if r2010 {
        for s in &mut styles {
            *s = o.data.b()?;
        }
    }
    let invisible = o.data.bs()? & 1 != 0;
    o.data.rc()?;

    let owner = match mode {
        0 => Owner::Object(o.handle_ref()?),
        1 => Owner::Paper,
        _ => Owner::Model,
    };
    for _ in 0..reactors {
        o.handles.h()?;
    }
    if !no_xdict {
        o.handles.h()?;
    }
    // A colour from a colour book is a reference to its AcDbColor object.
    if color & 0x4000 != 0 {
        o.handles.h()?;
    }
    let layer = o.handle_ref()?;
    if linetype == 3 {
        o.handles.h()?;
    }
    if material == 3 {
        o.handles.h()?;
    }
    if plotstyle == 3 {
        o.handles.h()?;
    }
    for s in styles {
        if s {
            o.handles.h()?;
        }
    }
    Ok(Common {
        owner,
        layer,
        invisible,
    })
}

/// TEXT, and the text part of ATTRIB and ATTDEF. False for a multi-line attribute (2018 on),
/// which is not read.
fn text(o: &mut Object, e: &mut Codes) -> Result<bool> {
    let d = &mut o.data;
    let flags = d.rc()?;
    if flags & 0x01 == 0 {
        d.rd()?;
    }
    let (x, y) = d.rd2()?;
    let (ax, ay) = if flags & 0x02 == 0 {
        (d.dd(x)?, d.dd(y)?)
    } else {
        (x, y)
    };
    let (_, _, z) = d.be()?;
    d.bt()?;
    if flags & 0x04 == 0 {
        d.rd()?;
    }
    let rotation = if flags & 0x08 == 0 { d.rd()? } else { 0.0 };
    let height = d.rd()?;
    if flags & 0x10 == 0 {
        d.rd()?;
    }
    let value = o.text()?;
    let d = &mut o.data;
    if flags & 0x20 == 0 {
        d.bs()?;
    }
    let align = if flags & 0x40 == 0 { d.bs()? } else { 0 };
    if flags & 0x80 == 0 {
        d.bs()?;
    }
    // An aligned text sits on its alignment point.
    let (px, py) = if matches!(align, 1 | 2 | 4) { (ax, ay) } else { (x, y) };
    e.f(10, px).f(20, py).f(11, ax).f(21, ay).f(40, height);
    e.f(50, rotation.to_degrees()).i(72, i64::from(align)).f(230, z);
    if o.kind == 1 {
        e.s(1, value);
        return Ok(true);
    }
    // ATTRIB and ATTDEF: the tag and flags, and for ATTDEF the prompt. A version byte from
    // 2010 on, a lock position flag from 2007 on.
    if o.version >= Version::R2010 {
        o.data.rc()?;
    }
    // From 2018 on, the attribute type: 1 a single line, 2 and 4 multi-line. A multi-line one
    // holds a whole MTEXT before its tag and flags, which the specification does not lay out
    // clearly enough to read; without its flags it could be a hidden one, so it is left out.
    // Any other type is damage.
    if o.version >= Version::R2018 {
        match o.data.rc()? {
            1 => {}
            2 | 4 => return Ok(false),
            other => return Err(bad(format!("an attribute has an unknown type {other}"))),
        }
    }
    let tag = o.text()?;
    o.data.bs()?;
    let attr_flags = o.data.rc()?;
    if o.version != Version::R2004 {
        o.data.b()?;
    }
    e.s(1, value).s(2, tag).i(70, i64::from(attr_flags));
    if o.kind == 3 {
        if o.version >= Version::R2010 {
            o.data.rc()?;
        }
        let prompt = o.text()?;
        e.s(3, prompt);
    }
    Ok(true)
}

/// INSERT; returns the handle of the block it shows.
fn insert(o: &mut Object, e: &mut Codes) -> Result<u64> {
    let d = &mut o.data;
    let (x, y, _) = d.bd3()?;
    let (sx, sy) = match d.bb()? {
        3 => (1.0, 1.0),
        1 => {
            let sy = d.dd(1.0)?;
            d.dd(1.0)?;
            (1.0, sy)
        }
        2 => {
            let s = d.rd()?;
            (s, s)
        }
        _ => {
            let sx = d.rd()?;
            let sy = d.dd(sx)?;
            d.dd(sx)?;
            (sx, sy)
        }
    };
    let rotation = d.bd()?;
    let (_, _, z) = d.bd3()?;
    e.f(10, x)
        .f(20, y)
        .f(41, sx)
        .f(42, sy)
        .f(50, rotation.to_degrees())
        .f(230, z);
    o.handle_ref()
}

fn mtext(o: &mut Object, e: &mut Codes) -> Result<()> {
    let d = &mut o.data;
    let (x, y, _) = d.bd3()?;
    let (_, _, z) = d.bd3()?;
    let (dx, dy, _) = d.bd3()?;
    d.bd()?;
    // The reference rectangle's height, from 2007 on.
    if o.version != Version::R2004 {
        d.bd()?;
    }
    let height = d.bd()?;
    let attachment = d.bs()?;
    d.bs()?;
    d.bd()?;
    d.bd()?;
    let value = o.text()?;
    // Attachment 1..9 runs left, centre, right on each row.
    let align = match attachment {
        1..=9 => (attachment - 1) % 3,
        _ => 0,
    };
    let angle = if dx == 0.0 && dy == 0.0 {
        0.0
    } else {
        dy.atan2(dx).to_degrees()
    };
    e.f(10, x)
        .f(20, y)
        .f(40, height)
        .f(50, angle)
        .i(72, i64::from(align))
        .f(230, z);
    e.s(1, value);
    Ok(())
}

fn lwpolyline(o: &mut Object, e: &mut Codes) -> Result<()> {
    let d = &mut o.data;
    let flags = d.bs()?;
    if flags & 4 != 0 {
        d.bd()?;
    }
    if flags & 8 != 0 {
        d.bd()?;
    }
    if flags & 2 != 0 {
        d.bd()?;
    }
    let z = if flags & 1 != 0 { d.bd3()?.2 } else { 1.0 };
    let points = d.bl()?;
    let bulges = if flags & 16 != 0 { d.bl()? } else { 0 };
    if o.version >= Version::R2010 && flags & 1024 != 0 {
        d.bl()?;
    }
    if flags & 32 != 0 {
        d.bl()?;
    }
    e.i(70, i64::from(flags & 512 != 0)).f(230, z);
    // The points come first and their bulges after them; each bulge goes (as code 42) after
    // the point that starts its segment.
    let mut at = Vec::new();
    let (mut x, mut y) = (0.0, 0.0);
    for i in 0..points {
        if i == 0 {
            (x, y) = d.rd2()?;
        } else {
            x = d.dd(x)?;
            y = d.dd(y)?;
        }
        // A point that is not a number would be written as text and read back as 0.
        if !x.is_finite() || !y.is_finite() {
            return Err(bad("a polyline point is not a number"));
        }
        at.push((x, y));
    }
    let bulge = kept_bulges(d, bulges, at.len())?;
    for (i, (x, y)) in at.into_iter().enumerate() {
        e.f(10, x).f(20, y);
        if let Some(&b) = bulge.get(i)
            && b != 0.0
        {
            e.f(42, b);
        }
    }
    Ok(())
}

/// Read `count` bulges (BD) and keep at most one per point: a count beyond `points` (a damaged
/// or hostile file) is read past, not stored.
fn kept_bulges(d: &mut Bits<'_>, count: u32, points: usize) -> Result<Vec<f64>> {
    let mut kept = Vec::new();
    for _ in 0..count {
        let b = d.bd()?;
        if kept.len() < points {
            // A bulge that is not a number would silently draw its arc as a straight line.
            if !b.is_finite() {
                return Err(bad("a polyline bulge is not a number"));
            }
            kept.push(b);
        }
    }
    Ok(kept)
}

impl Decoded {
    /// Put each entity in its block or model space, vertices in their polylines and attributes
    /// after their INSERT.
    fn assemble(self, skipped: &mut BTreeMap<String, usize>) -> dxf::Drawing {
        let Decoded {
            items,
            blocks,
            bases,
            base_errors: _,
            layers,
            hidden,
        } = self;
        let kinds: HashMap<u64, &str> = items.iter().map(|i| (i.handle, i.entity.kind.as_str())).collect();
        // Vertices by polyline and attributes by INSERT, in handle order (a polyline then puts
        // its vertices in its own order).
        let mut children: HashMap<u64, Vec<&Item>> = HashMap::new();
        for item in &items {
            if let Owner::Object(owner) = item.owner
                && matches!(kinds.get(&owner), Some(&"POLYLINE" | &"INSERT"))
            {
                children.entry(owner).or_default().push(item);
            }
        }
        let mut d = dxf::Drawing::default();
        for (h, name) in &blocks {
            if !is_space(name) {
                d.blocks.entry(name.clone()).or_default();
                if let Some(base) = bases.get(h) {
                    d.bases.insert(name.clone(), *base);
                }
            }
        }
        for item in &items {
            let kind = item.entity.kind.as_str();
            if matches!(kind, "VERTEX" | "SEQEND" | "BLOCK" | "ENDBLK" | "VIEWPORT") {
                continue;
            }
            let list = match item.owner {
                Owner::Model => &mut d.entities,
                Owner::Paper => continue,
                Owner::Object(h) if hidden.contains(&h) => continue,
                Owner::Object(h) => match blocks.get(&h) {
                    Some(name) if name.eq_ignore_ascii_case("*Model_Space") => &mut d.entities,
                    Some(name) if is_space(name) => continue,
                    Some(name) => d.blocks.entry(name.clone()).or_default(),
                    // An attribute in an INSERT goes with it.
                    None if kinds.get(&h) == Some(&"INSERT") => continue,
                    None => {
                        *skipped.entry(ORPHANS.to_owned()).or_default() += 1;
                        continue;
                    }
                },
            };
            let mut entity = with_layer(&item.entity, item.layer, &layers);
            let own = children.get(&item.handle).map(Vec::as_slice).unwrap_or_default();
            match kind {
                "POLYLINE" => {
                    entity.kind = "LWPOLYLINE".into();
                    let vertices: Vec<&Item> = own.iter().copied().filter(|c| c.entity.kind == "VERTEX").collect();
                    // In the order the polyline lists them; handle order when it lists none.
                    let ordered: Vec<&Item> = if item.vertices.is_empty() {
                        vertices
                    } else {
                        let by_handle: HashMap<u64, &Item> = vertices.iter().map(|v| (v.handle, *v)).collect();
                        // A listed vertex that could not be read (damaged) would join its
                        // neighbours with a segment the drawing does not have: the polyline is
                        // left out and said instead.
                        if item
                            .vertices
                            .iter()
                            .any(|h| !by_handle.contains_key(h) && !hidden.contains(h))
                        {
                            *skipped.entry(BROKEN_POLYLINE.to_owned()).or_default() += 1;
                            continue;
                        }
                        item.vertices.iter().filter_map(|h| by_handle.get(h).copied()).collect()
                    };
                    for v in ordered {
                        entity.codes.extend(v.entity.codes.iter().cloned());
                    }
                    list.push(entity);
                }
                "DIMENSION" => match item.block.and_then(|b| blocks.get(&b)) {
                    Some(name) => {
                        entity.codes.push((2, name.clone()));
                        list.push(entity);
                    }
                    None => *skipped.entry("DIMENSION".to_owned()).or_default() += 1,
                },
                "INSERT" => {
                    // A reference to a block that is not there draws nothing: counted, while
                    // its attributes are still drawn, as plain texts (as ATTRIBs they would be
                    // taken for those of the INSERT before it).
                    let resolved = match item.block.and_then(|b| blocks.get(&b)) {
                        Some(name) => {
                            entity.codes.push((2, name.clone()));
                            list.push(entity);
                            true
                        }
                        None => {
                            *skipped.entry(UNRESOLVED_INSERT.to_owned()).or_default() += 1;
                            false
                        }
                    };
                    for a in own.iter().filter(|c| c.entity.kind == "ATTRIB") {
                        if (a.entity.f(70) as i64) & 1 == 0 {
                            let mut text = with_layer(&a.entity, a.layer, &layers);
                            if !resolved {
                                text.kind = "TEXT".into();
                            }
                            list.push(text);
                        }
                    }
                }
                // An attribute outside an INSERT draws nothing.
                "ATTRIB" => {}
                _ => list.push(entity),
            }
        }
        d
    }
}

/// Model and paper space, which are blocks in the file but not in [`Drawing::blocks`].
fn is_space(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "*model_space" || n.starts_with("*paper_space")
}

fn with_layer(e: &Entity, layer: u64, layers: &HashMap<u64, String>) -> Entity {
    let mut e = e.clone();
    if let Some(name) = layers.get(&layer) {
        e.codes.push((8, name.clone()));
    }
    e
}

#[cfg(test)]
pub(crate) mod tests;
