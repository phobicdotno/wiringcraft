//! The objects of a DWG drawing: the second step of reading DWG files (#162), on top of the
//! sections [`super::read_sections`] returns.
//!
//! `AcDb:Handles` is the object map: each object's handle and where it starts in
//! `AcDb:AcDbObjects`. Each object there is a size, then bit-packed data:
//! its type, its own handle and fields, and at the end a stream of references to other objects
//! by handle (its owner, its layer, the block an INSERT shows). In AutoCAD 2010 drawings the
//! texts of an object are in a third stream at the end of its data.
//!
//! The entities read are LINE, ARC, CIRCLE, LWPOLYLINE, POLYLINE (2D and 3D) with their
//! VERTEXes, TEXT, MTEXT, ATTDEF, ATTRIB, INSERT, DIMENSION (drawn by its anonymous block), ELLIPSE, SPLINE, SOLID,
//! TRACE and POINT. Each entity belongs to
//! the block whose BLOCK_HEADER owns it, or to model space; vertices belong to their polyline
//! and attributes to their INSERT. The entities become the same entities the DXF reader works
//! with, so a DWG drawing is drawn exactly like a DXF one: y flipped, block references expanded.
//! Other entity types, and objects that cannot be decoded, are counted and skipped.
//!
//! Written from the published Open Design Specification for .dwg files only, and checked with
//! objects the tests write themselves. Input is treated as hostile: every read is checked, and a
//! damaged object is counted, never a panic.

use std::collections::{BTreeMap, HashMap, HashSet};

use wc_core::geom::{Point, Shape};

use super::bits::{Bits, modular_char, modular_short};
use super::{Sections, Version, bad, read_sections};
use crate::Result;
use crate::dxf::{self, Entity};

/// The most objects a drawing's object map may list.
pub const MAX_OBJECTS: usize = 4_000_000;

/// The key under which objects that could not be decoded are counted in [`Drawing::skipped`].
pub const DAMAGED: &str = "damaged objects";

/// The skipped-count key for blocks whose base point could not be read: they are kept, placed
/// as if their base point were (0, 0).
pub const BASE_UNREAD: &str = "block base points not read (placed from 0, 0)";
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
    /// Why objects counted under [`DAMAGED`] could not be decoded (handle, offset and reason),
    /// then why base points counted under [`BASE_UNREAD`] could not be read, for the first
    /// [`MAX_REASONS`] of them.
    pub damaged: Vec<String>,
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
    })
}

/// What [`dxf_drawing`] reads from a file.
pub(crate) struct Read {
    pub(crate) drawing: dxf::Drawing,
    pub(crate) skipped: BTreeMap<String, usize>,
    pub(crate) damaged: Vec<String>,
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
    for (handle, at) in map {
        if let Err(e) = decode(objects, at, handle, sections.version, &mut decoded, &mut skipped) {
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
    kind: u16,
    handle: u64,
    data: Bits<'a>,
    handles: Bits<'a>,
    strings: Option<Bits<'a>>,
}

impl Object<'_> {
    /// TV: a text, from the data stream (2004) or the string stream (2010).
    fn text(&mut self) -> Result<String> {
        match self.version {
            Version::R2004 => self.data.t8(),
            Version::R2010 => self
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
fn object(objects: &[u8], at: usize, version: Version) -> Result<Object<'_>> {
    let rest = objects.get(at..).ok_or_else(|| bad("an object is past the end"))?;
    let (size, n) = modular_short(rest)?;
    let mut start = at + n;
    let handle_bits = match version {
        Version::R2004 => 0,
        Version::R2010 => {
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
    let kind = match version {
        Version::R2004 => head.bs()?,
        Version::R2010 => match head.bb()? {
            0 => u16::from(head.rc()?),
            1 => u16::from(head.rc()?) + 0x1F0,
            _ => head.rs()?,
        },
    };
    let end = match version {
        Version::R2004 => head.rl()? as usize,
        Version::R2010 => total
            .checked_sub(handle_bits)
            .ok_or_else(|| bad("an object's handle stream is larger than the object"))?,
    };
    if end > total || end < head.pos() {
        return Err(bad("an object's data size is out of range"));
    }
    let mut data = Bits::new(bytes, head.pos(), end);
    let handle = data.h()?.1;
    let strings = match version {
        Version::R2004 => None,
        Version::R2010 => string_stream(bytes, end)?,
    };
    Ok(Object {
        version,
        kind,
        handle,
        data,
        handles: Bits::new(bytes, end, total),
        strings,
    })
}

/// The string stream of a 2010 object whose data ends at bit `end`: a flag in the last bit, a
/// size before it (in one or two 16-bit parts), and the texts before that.
fn string_stream(bytes: &[u8], end: usize) -> Result<Option<Bits<'_>>> {
    let all = Bits::new(bytes, 0, end);
    let Some(flag_at) = end.checked_sub(1) else {
        return Ok(None);
    };
    if !all.bit_at(flag_at)? {
        return Ok(None);
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
    Ok(Some(Bits::new(bytes, start, at)))
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
        78 => "HATCH",
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
        _ => return None,
    })
}

/// The base point of a BLOCK_HEADER whose name has been read.
fn block_base(o: &mut Object) -> Result<(f64, f64)> {
    // The xref flags of a table entry: 64-flag, xref index and xref dependent up to 2004, only
    // the xref index from 2007 on.
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
/// and weights (41), or fit points (11/21). Before 2013 a scenario says which: 1 control
/// points, 2 fit points.
fn spline(o: &mut Object, e: &mut Codes) -> Result<()> {
    let scenario = o.data.bl()?;
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
        // Fit tolerance and the start and end tangents, then the fit points.
        o.data.bd()?;
        o.data.bd3()?;
        o.data.bd3()?;
        let fit = spline_count(o, "fit points")?;
        e.i(70, 0).i(71, i64::from(degree));
        for _ in 0..fit {
            let (x, y, _) = o.data.bd3()?;
            e.f(11, x).f(21, y);
        }
    }
    Ok(())
}

/// Decode the object at `at` into `out`.
fn decode(
    objects: &[u8],
    at: usize,
    handle: u64,
    version: Version,
    out: &mut Decoded,
    skipped: &mut BTreeMap<String, usize>,
) -> Result<()> {
    let mut o = object(objects, at, version)?;
    if o.handle != handle {
        return Err(bad("an object's handle differs from the object map"));
    }
    if o.kind == BLOCK_HEADER || o.kind == LAYER {
        // A table entry: common object data, then its name.
        o.skip_eed()?;
        o.data.bl()?;
        o.data.b()?;
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
        1..=3 => text(&mut o, &mut e)?,
        7 => block = Some(insert(&mut o, &mut e)?),
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
            // Control points of a spline-fitted polyline are not on the curve.
            if flags & 0x10 != 0 {
                return Ok(());
            }
            e.f(10, x).f(20, y);
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
struct Codes(Vec<(i32, String)>);

impl Codes {
    fn f(&mut self, code: i32, v: f64) -> &mut Self {
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
    let r2010 = o.version == Version::R2010;
    o.skip_eed()?;
    if o.data.b()? {
        // A preview picture of the entity.
        let size = if r2010 { o.data.bll()? } else { u64::from(o.data.rl()?) };
        o.data.skip_bytes(size)?;
    }
    let mode = o.data.bb()?;
    let reactors = o.data.bl()?;
    let no_xdict = o.data.b()?;
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
    if r2010 {
        material = o.data.bb()?;
        o.data.rc()?;
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

/// TEXT, and the text part of ATTRIB and ATTDEF.
fn text(o: &mut Object, e: &mut Codes) -> Result<()> {
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
        return Ok(());
    }
    // ATTRIB and ATTDEF: the tag and flags, and for ATTDEF the prompt.
    if o.version == Version::R2010 {
        o.data.rc()?;
    }
    let tag = o.text()?;
    o.data.bs()?;
    let attr_flags = o.data.rc()?;
    if o.version == Version::R2010 {
        o.data.b()?;
    }
    e.s(1, value).s(2, tag).i(70, i64::from(attr_flags));
    if o.kind == 3 {
        if o.version == Version::R2010 {
            o.data.rc()?;
        }
        let prompt = o.text()?;
        e.s(3, prompt);
    }
    Ok(())
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
    if o.version == Version::R2010 {
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
    if flags & 16 != 0 {
        d.bl()?;
    }
    if o.version == Version::R2010 && flags & 1024 != 0 {
        d.bl()?;
    }
    if flags & 32 != 0 {
        d.bl()?;
    }
    e.i(70, i64::from(flags & 512 != 0)).f(230, z);
    let (mut x, mut y) = (0.0, 0.0);
    for i in 0..points {
        if i == 0 {
            (x, y) = d.rd2()?;
        } else {
            x = d.dd(x)?;
            y = d.dd(y)?;
        }
        e.f(10, x).f(20, y);
    }
    Ok(())
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
                    if let Some(name) = item.block.and_then(|b| blocks.get(&b)) {
                        entity.codes.push((2, name.clone()));
                    }
                    list.push(entity);
                    for a in own.iter().filter(|c| c.entity.kind == "ATTRIB") {
                        if (a.entity.f(70) as i64) & 1 == 0 {
                            list.push(with_layer(&a.entity, a.layer, &layers));
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
