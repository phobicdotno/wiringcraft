use wc_core::geom::{Align, Point};

use super::super::Section;
use super::super::tests::{TestSection, write_dwg};
use super::*;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

// ---- A bit writer for the tests, written from the same description as the reader ----

#[derive(Default, Clone)]
struct W(Vec<bool>);

impl W {
    fn bits(&mut self, v: u64, n: u32) -> &mut Self {
        for i in (0..n).rev() {
            self.0.push(v >> i & 1 == 1);
        }
        self
    }
    fn b(&mut self, v: bool) -> &mut Self {
        self.0.push(v);
        self
    }
    fn bb(&mut self, v: u8) -> &mut Self {
        self.bits(u64::from(v), 2)
    }
    fn rc(&mut self, v: u8) -> &mut Self {
        self.bits(u64::from(v), 8)
    }
    fn rs(&mut self, v: u16) -> &mut Self {
        for b in v.to_le_bytes() {
            self.rc(b);
        }
        self
    }
    fn rl(&mut self, v: u32) -> &mut Self {
        for b in v.to_le_bytes() {
            self.rc(b);
        }
        self
    }
    fn rd(&mut self, v: f64) -> &mut Self {
        for b in v.to_le_bytes() {
            self.rc(b);
        }
        self
    }
    fn bs(&mut self, v: u16) -> &mut Self {
        match v {
            0 => self.bb(2),
            256 => self.bb(3),
            1..=255 => self.bb(1).rc(v as u8),
            _ => self.bb(0).rs(v),
        }
    }
    fn bl(&mut self, v: u32) -> &mut Self {
        match v {
            0 => self.bb(2),
            1..=255 => self.bb(1).rc(v as u8),
            _ => self.bb(0).rl(v),
        }
    }
    fn bll(&mut self, v: u64) -> &mut Self {
        let bytes = v.to_le_bytes();
        let n = 8 - bytes.iter().rev().take_while(|b| **b == 0).count();
        self.bits(n as u64, 3);
        for b in &bytes[..n] {
            self.rc(*b);
        }
        self
    }
    fn bd(&mut self, v: f64) -> &mut Self {
        if v == 0.0 {
            self.bb(2)
        } else if v == 1.0 {
            self.bb(1)
        } else {
            self.bb(0).rd(v)
        }
    }
    /// DD in its shortest form.
    fn dd(&mut self, v: f64, default: f64) -> &mut Self {
        let (a, d) = (v.to_le_bytes(), default.to_le_bytes());
        if a == d {
            self.bb(0)
        } else if a[4..] == d[4..] {
            self.bb(1);
            a[..4].iter().for_each(|b| {
                self.rc(*b);
            });
            self
        } else if a[6..] == d[6..] {
            self.bb(2).rc(a[4]).rc(a[5]);
            a[..4].iter().for_each(|b| {
                self.rc(*b);
            });
            self
        } else {
            self.bb(3).rd(v)
        }
    }
    fn bd3(&mut self, (x, y, z): (f64, f64, f64)) -> &mut Self {
        self.bd(x).bd(y).bd(z)
    }
    fn bt(&mut self, v: f64) -> &mut Self {
        if v == 0.0 { self.b(true) } else { self.b(false).bd(v) }
    }
    fn be(&mut self, v: (f64, f64, f64)) -> &mut Self {
        if v == (0.0, 0.0, 1.0) {
            self.b(true)
        } else {
            self.b(false).bd3(v)
        }
    }
    fn h(&mut self, code: u8, v: u64) -> &mut Self {
        let bytes: Vec<u8> = v.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
        self.rc((code << 4) | bytes.len() as u8);
        for b in bytes {
            self.rc(b);
        }
        self
    }
    fn t8(&mut self, s: &str) -> &mut Self {
        self.bs(s.len() as u16);
        for b in s.bytes() {
            self.rc(b);
        }
        self
    }
    fn tu(&mut self, s: &str) -> &mut Self {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.bs(units.len() as u16);
        for u in units {
            self.rs(u);
        }
        self
    }
    fn append(&mut self, o: &W) -> &mut Self {
        self.0.extend(&o.0);
        self
    }
    fn bytes(&self) -> Vec<u8> {
        self.0
            .chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |a, (i, b)| a | (u8::from(*b) << (7 - i)))
            })
            .collect()
    }
}

fn ms(v: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut v = v;
    loop {
        let word = (v & 0x7FFF) as u16;
        v >>= 15;
        if v == 0 {
            out.extend(word.to_le_bytes());
            return out;
        }
        out.extend((word | 0x8000).to_le_bytes());
    }
}

fn umc(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while v >= 0x80 {
        out.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
    out
}

fn smc(v: i64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut m = v.unsigned_abs();
    while m >= 0x40 {
        out.push((m & 0x7F) as u8 | 0x80);
        m >>= 7;
    }
    out.push(m as u8 | if v < 0 { 0x40 } else { 0 });
    out
}

// ---- Objects ----

/// One object being written: its data after its own handle, its texts and its handle stream.
struct Obj {
    version: Version,
    kind: u16,
    handle: u64,
    data: W,
    strings: W,
    handles: W,
}

impl Obj {
    fn new(version: Version, kind: u16, handle: u64) -> Self {
        Obj {
            version,
            kind,
            handle,
            data: W::default(),
            strings: W::default(),
            handles: W::default(),
        }
    }

    fn text(&mut self, s: &str) -> &mut Self {
        match self.version {
            Version::R2004 => {
                self.data.t8(s);
            }
            Version::R2010 => {
                self.strings.tu(s);
            }
        }
        self
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = W::default();
        match self.version {
            Version::R2004 => {
                w.bs(self.kind);
                let rl_at = w.0.len();
                w.rl(0).h(0, self.handle).append(&self.data);
                let size = w.0.len() as u32;
                let mut rl = W::default();
                rl.rl(size);
                w.0.splice(rl_at..rl_at + 32, rl.0);
                w.append(&self.handles);
                let bytes = w.bytes();
                let mut out = ms(bytes.len());
                out.extend(bytes);
                out
            }
            Version::R2010 => {
                match self.kind {
                    0..=255 => w.bb(0).rc(self.kind as u8),
                    k if (0x1F0..0x1F0 + 256).contains(&k) => w.bb(1).rc((k - 0x1F0) as u8),
                    k => w.bb(2).rs(k),
                };
                w.h(0, self.handle).append(&self.data);
                if self.strings.0.is_empty() {
                    w.b(false);
                } else {
                    let n = self.strings.0.len();
                    w.append(&self.strings);
                    if n >= 0x8000 {
                        w.rs((n >> 15) as u16).rs((n & 0x7FFF) as u16 | 0x8000);
                    } else {
                        w.rs(n as u16);
                    }
                    w.b(true);
                }
                let end = w.0.len();
                w.append(&self.handles);
                let bytes = w.bytes();
                let mut out = ms(bytes.len());
                out.extend(umc((bytes.len() * 8 - end) as u64));
                out.extend(bytes);
                out
            }
        }
    }
}

/// A table entry (BLOCK_HEADER, LAYER) with a name.
fn entry(v: Version, kind: u16, handle: u64, name: &str) -> Obj {
    if kind == BLOCK_HEADER {
        return block_header(v, handle, name, (0.0, 0.0), false);
    }
    named(v, kind, handle, name)
}

/// A table entry's common data and name, nothing after it.
fn named(v: Version, kind: u16, handle: u64, name: &str) -> Obj {
    let mut o = Obj::new(v, kind, handle);
    o.data.bs(0).bl(0).b(true);
    o.text(name);
    o
}

/// How an entity's common data is written.
#[derive(Clone, Copy)]
struct Head {
    /// The BLOCK_HEADER, polyline or INSERT owning it; model space when `None`.
    owner: Option<u64>,
    layer: u64,
    invisible: bool,
    /// Write the extras most entities do not have: extended data, a preview, a reactor, an
    /// extension dictionary, a true colour and linetype/plot style handles.
    extras: bool,
    /// With `extras`, the true colour also names a colour-book colour (0x4000), whose handle
    /// comes before the layer's.
    book: bool,
}

fn head(owner: Option<u64>) -> Head {
    Head {
        owner,
        layer: LAYER_H,
        invisible: false,
        extras: false,
        book: false,
    }
}

fn entity(v: Version, kind: u16, handle: u64, h: Head) -> Obj {
    let mut o = Obj::new(v, kind, handle);
    let d = &mut o.data;
    if h.extras {
        d.bs(3).h(5, 0x99).rc(1).rc(2).rc(3).bs(0);
        d.b(true);
        match v {
            Version::R2004 => d.rl(2),
            Version::R2010 => d.bll(2),
        };
        d.rc(0xAB).rc(0xCD);
    } else {
        d.bs(0).b(false);
    }
    d.bb(if h.owner.is_some() { 0 } else { 2 });
    d.bl(u32::from(h.extras)).b(!h.extras);
    if h.extras {
        let book = if h.book { 0x4000 } else { 0 };
        d.bs(0x8000 | book | 0x2000 | 7).bl(0x00FF_0000).bl(0x0200_0080);
    } else {
        d.bs(256);
    }
    d.bd(1.0)
        .bb(if h.extras { 3 } else { 0 })
        .bb(if h.extras { 3 } else { 0 });
    if v == Version::R2010 {
        d.bb(0).rc(0).b(false).b(h.extras).b(false);
    }
    d.bs(u16::from(h.invisible)).rc(0);
    let hs = &mut o.handles;
    if let Some(owner) = h.owner {
        hs.h(4, owner);
    }
    if h.extras {
        // A reactor, the extension dictionary and the colour-book colour.
        hs.h(4, 0x77).h(3, 0x78);
        if h.book {
            hs.h(5, 0x79);
        }
        // The layer as an offset back from the entity's own handle.
        hs.h(0xC, handle - h.layer);
        hs.h(5, 0x14).h(5, 0x15);
        if v == Version::R2010 {
            hs.h(5, 0x16);
        }
    } else {
        hs.h(5, h.layer);
    }
    o
}

const LAYER_H: u64 = 0x10;
const MODEL_H: u64 = 0x1F;
const PAPER_H: u64 = 0x1E;
const RELAY_H: u64 = 0x20;
const COIL_H: u64 = 0x21;

fn line(v: Version, handle: u64, h: Head, a: (f64, f64), b: (f64, f64)) -> Obj {
    let mut o = entity(v, 19, handle, h);
    o.data
        .b(true)
        .rd(a.0)
        .dd(b.0, a.0)
        .rd(a.1)
        .dd(b.1, a.1)
        .bt(0.0)
        .be((0.0, 0.0, 1.0));
    o
}

fn text_data(o: &mut Obj, at: (f64, f64), value: &str, height: f64, rotation: f64) {
    text_data_with(o, at, value, height, rotation, None);
}

/// [`text_data`], with a vertical alignment written out when given.
fn text_data_with(o: &mut Obj, at: (f64, f64), value: &str, height: f64, rotation: f64, valign: Option<u16>) {
    // Elevation, alignment point, oblique, width factor, generation and alignments left out.
    let mut flags = 0x01 | 0x02 | 0x04 | 0x10 | 0x20 | 0x40;
    if valign.is_none() {
        flags |= 0x80;
    }
    if rotation == 0.0 {
        flags |= 0x08;
    }
    o.data.rc(flags).rd(at.0).rd(at.1).be((0.0, 0.0, 1.0)).bt(0.0);
    if rotation != 0.0 {
        o.data.rd(rotation);
    }
    o.data.rd(height);
    o.text(value);
    if let Some(v) = valign {
        o.data.bs(v);
    }
}

#[allow(clippy::too_many_arguments)]
fn attrib(v: Version, kind: u16, handle: u64, h: Head, at: (f64, f64), value: &str, tag: &str, flags: u8) -> Obj {
    let mut o = entity(v, kind, handle, h);
    // Attributes carry a vertical alignment, so the field after the text is read too.
    text_data_with(&mut o, at, value, 2.5, 0.0, Some(1));
    if v == Version::R2010 {
        o.data.rc(0);
    }
    o.text(tag);
    o.data.bs(0).rc(flags);
    if v == Version::R2010 {
        o.data.b(false);
    }
    if kind == 3 {
        if v == Version::R2010 {
            o.data.rc(0);
        }
        o.text("Device tag");
    }
    o
}

/// A drawing with a block in a block, every entity type, and things to skip.
fn sample(v: Version) -> Vec<Obj> {
    let relay = head(Some(RELAY_H));
    let coil = head(Some(COIL_H));
    let model = head(None);
    let mut objs = vec![
        entry(v, LAYER, LAYER_H, "SYMBOL"),
        entry(v, BLOCK_HEADER, PAPER_H, "*Paper_Space"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        entry(v, BLOCK_HEADER, RELAY_H, "RELAY"),
        entry(v, BLOCK_HEADER, COIL_H, "COIL"),
    ];
    // COIL: a circle.
    let mut c = entity(v, 18, 0x30, coil);
    c.data.bd3((0.0, 0.0, 0.0)).bd(2.0).bt(0.0).be((0.0, 0.0, 1.0));
    objs.push(c);
    // RELAY: a line with every extra, an arc, a closed lightweight polyline, a text, an
    // attribute definition, a turned and scaled COIL and a solid.
    objs.push(line(
        v,
        0x40,
        Head {
            extras: true,
            book: true,
            ..relay
        },
        (0.0, 0.0),
        (10.0, 0.0),
    ));
    let mut a = entity(v, 17, 0x41, relay);
    a.data
        .bd3((5.0, 5.0, 0.0))
        .bd(2.0)
        .bt(0.0)
        .be((0.0, 0.0, 1.0))
        .bd(0.0)
        .bd(std::f64::consts::FRAC_PI_2);
    objs.push(a);
    let mut p = entity(v, 77, 0x42, relay);
    p.data
        .bs(512)
        .bl(3)
        .rd(0.0)
        .rd(0.0)
        .dd(4.0, 0.0)
        .dd(0.0, 0.0)
        .dd(4.0, 4.0)
        .dd(3.0, 0.0);
    objs.push(p);
    let mut t = entity(v, 1, 0x43, relay);
    text_data(&mut t, (1.0, 2.0), "K1", 3.5, std::f64::consts::FRAC_PI_2);
    objs.push(t);
    objs.push(attrib(v, 3, 0x44, relay, (0.0, 8.0), "?", "TAG", 0));
    // The COIL insert has a colour-book colour: its handle must not be taken for the block's.
    let mut i = entity(
        v,
        7,
        0x45,
        Head {
            extras: true,
            book: true,
            ..relay
        },
    );
    i.data
        .bd3((20.0, 0.0, 0.0))
        .bb(2)
        .rd(2.0)
        .bd(std::f64::consts::FRAC_PI_2)
        .bd3((0.0, 0.0, 1.0))
        .b(false);
    i.handles.h(5, COIL_H);
    objs.push(i);
    let mut s = entity(v, 31, 0x46, relay);
    s.data
        .bt(0.0)
        .bd(0.0)
        .rd(0.0)
        .rd(0.0)
        .rd(1.0)
        .rd(0.0)
        .rd(0.0)
        .rd(1.0)
        .rd(1.0)
        .rd(1.0);
    s.data.be((0.0, 0.0, 1.0));
    objs.push(s);
    // A TRACE, read like a SOLID: a triangle (third and fourth corner the same).
    let mut tr = entity(v, 32, 0x47, relay);
    tr.data.bt(0.0).bd(0.0);
    for (x, y) in [(5.0, 5.0), (8.0, 5.0), (5.0, 8.0), (5.0, 8.0)] {
        tr.data.rd(x).rd(y);
    }
    tr.data.be((0.0, 0.0, 1.0));
    objs.push(tr);
    // Model space: RELAY with a visible and a hidden attribute, a 2D polyline of three
    // vertices, an MTEXT, a point, an invisible line, an ellipse and a custom object.
    // The RELAY insert has a true colour without a colour book (no colour handle).
    let mut i = entity(v, 7, 0x50, Head { extras: true, ..model });
    i.data
        .bd3((100.0, 50.0, 0.0))
        .bb(3)
        .bd(0.0)
        .bd3((0.0, 0.0, 1.0))
        .b(true);
    i.data.bl(2);
    i.handles.h(5, RELAY_H);
    objs.push(i);
    objs.push(attrib(v, 2, 0x51, head(Some(0x50)), (100.0, 58.0), "-K1", "TAG", 0));
    objs.push(attrib(v, 2, 0x52, head(Some(0x50)), (100.0, 60.0), "secret", "NOTE", 1));
    let mut pl = entity(v, 15, 0x53, model);
    pl.data
        .bs(0)
        .bs(0)
        .bd(0.0)
        .bd(0.0)
        .bt(0.0)
        .bd(0.0)
        .be((0.0, 0.0, 1.0))
        .bl(3);
    // The polyline lists its vertices in another order than their handles.
    pl.handles.h(4, 0x54).h(4, 0x56).h(4, 0x55).h(3, 0x57);
    objs.push(pl);
    for (n, (x, y)) in [(0.0, 0.0), (0.0, 10.0), (10.0, 10.0)].into_iter().enumerate() {
        let mut vx = entity(v, 10, 0x54 + n as u64, head(Some(0x53)));
        vx.data.rc(0).bd3((x, y, 0.0)).bd(0.0).bd(0.0).bd(0.0);
        objs.push(vx);
    }
    objs.push(entity(v, 6, 0x57, head(Some(0x53))));
    // A closed 3D polyline of three vertices, listed in handle order.
    let mut p3 = entity(v, 16, 0x60, model);
    p3.data.rc(0).rc(1).bl(3);
    p3.handles.h(4, 0x61).h(4, 0x62).h(4, 0x63).h(3, 0x64);
    objs.push(p3);
    for (n, (x, y)) in [(50.0, 0.0), (60.0, 0.0), (60.0, 5.0)].into_iter().enumerate() {
        let mut vx = entity(v, 11, 0x61 + n as u64, head(Some(0x60)));
        vx.data.rc(0).bd3((x, y, 2.0));
        objs.push(vx);
    }
    objs.push(entity(v, 6, 0x64, head(Some(0x60))));
    let mut m = entity(v, 44, 0x58, model);
    m.data
        .bd3((30.0, 40.0, 0.0))
        .bd3((0.0, 0.0, 1.0))
        .bd3((1.0, 0.0, 0.0))
        .bd(50.0);
    if v == Version::R2010 {
        m.data.bd(10.0);
    }
    m.data.bd(2.5).bs(5).bs(1).bd(0.0).bd(0.0);
    m.text("{\\fArial;Pump}\\Pstart");
    objs.push(m);
    let mut pt = entity(v, 27, 0x59, model);
    pt.data.bd3((7.0, 8.0, 0.0)).bt(0.0).be((0.0, 0.0, 1.0)).bd(0.0);
    objs.push(pt);
    objs.push(line(
        v,
        0x5A,
        Head {
            invisible: true,
            ..model
        },
        (0.0, 0.0),
        (1.0, 1.0),
    ));
    objs.push(entity(v, 35, 0x5B, model));
    objs.push(Obj::new(v, 0x1F3, 0x5C));
    objs
}

/// The object map and objects sections for `objs`.
fn sections_bytes(objs: &[Obj], extra: &[(u64, usize)]) -> (Vec<u8>, Vec<u8>) {
    let mut objects = 0x0DCAu32.to_le_bytes().to_vec();
    let mut entries = Vec::new();
    for o in objs {
        entries.push((o.handle, objects.len()));
        objects.extend(o.encode());
    }
    entries.extend(extra);
    entries.sort();
    (map_bytes(&entries), objects)
}

/// An object map in parts of at most 2032 bytes.
fn map_bytes(entries: &[(u64, usize)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut body = Vec::new();
    let (mut handle, mut at) = (0u64, 0i64);
    let flush = |body: &mut Vec<u8>, out: &mut Vec<u8>| {
        out.extend(((body.len() + 2) as u16).to_be_bytes());
        out.append(body);
        out.extend([0, 0]);
    };
    for &(h, a) in entries {
        let mut pair = umc(h - handle);
        pair.extend(smc(a as i64 - at));
        if body.len() + pair.len() > 2030 {
            // Each part counts from zero again.
            flush(&mut body, &mut out);
            pair = umc(h);
            pair.extend(smc(a as i64));
        }
        body.extend(pair);
        (handle, at) = (h, a as i64);
    }
    if !body.is_empty() {
        flush(&mut body, &mut out);
    }
    out.extend([0, 2, 0, 0]);
    out
}

fn sections(v: Version, objs: &[Obj], extra: &[(u64, usize)]) -> Sections {
    let (map, objects) = sections_bytes(objs, extra);
    Sections {
        version: v,
        sections: vec![
            Section {
                name: "AcDb:Handles".into(),
                id: 1,
                data: map,
            },
            Section {
                name: "AcDb:AcDbObjects".into(),
                id: 2,
                data: objects,
            },
        ],
        skipped: Vec::new(),
        maint: 0,
    }
}

fn near(a: Point, b: Point) -> bool {
    a.distance(b) < 1e-9
}

fn has_line(shapes: &[Shape], from: Point, to: Point) -> bool {
    shapes
        .iter()
        .any(|s| matches!(s, Shape::Line { a, b } if near(*a, from) && near(*b, to)))
}

fn texts(shapes: &[Shape]) -> Vec<&str> {
    shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn check_sample(d: &Drawing) {
    assert_eq!(
        d.blocks.keys().collect::<Vec<_>>(),
        ["COIL", "RELAY"],
        "spaces are not blocks"
    );
    assert!(matches!(
        d.blocks["COIL"].as_slice(),
        [Shape::Circle { center, radius, .. }] if near(*center, Point::new(0.0, 0.0)) && *radius == 2.0
    ));

    let relay = &d.blocks["RELAY"];
    assert!(
        has_line(relay, Point::new(0.0, 0.0), Point::new(10.0, 0.0)),
        "{relay:?}"
    );
    assert!(relay.iter().any(|s| matches!(s,
        Shape::Arc { center, radius, start, end }
            if near(*center, Point::new(5.0, -5.0)) && *radius == 2.0 && *start == -90.0 && *end == 0.0)));
    assert!(relay.iter().any(|s| matches!(s,
        Shape::Polyline { points, closed: true, filled: false }
            if points.len() == 3 && near(points[2], Point::new(4.0, -3.0)))));
    assert!(relay.iter().any(|s| matches!(s,
        Shape::Text { at, text, height, angle, align: Align::Left }
            if text == "K1" && near(*at, Point::new(1.0, -2.0)) && *height == 3.5 && (*angle - 90.0).abs() < 1e-9)));
    // The COIL inside, turned a quarter and twice as large: its circle is at the insert point.
    assert!(relay.iter().any(|s| matches!(s,
        Shape::Circle { center, radius, .. } if near(*center, Point::new(20.0, 0.0)) && *radius == 4.0)));
    assert!(relay.iter().any(|s| matches!(s,
        Shape::Polyline { points, closed: true, filled: true }
            if points.len() == 4 && near(points[2], Point::new(1.0, -1.0)))));
    assert!(
        relay.iter().any(|s| matches!(s,
        Shape::Polyline { points, closed: true, filled: true }
            if points.len() == 3 && near(points[1], Point::new(8.0, -5.0)))),
        "a TRACE"
    );
    assert!(!texts(relay).contains(&"?"), "an attribute definition draws nothing");

    let model = &d.model;
    // RELAY at (100, 50): its line, and its coil at (120, 50).
    assert!(
        has_line(model, Point::new(100.0, -50.0), Point::new(110.0, -50.0)),
        "{model:?}"
    );
    assert!(model.iter().any(|s| matches!(s,
        Shape::Circle { center, radius, .. } if near(*center, Point::new(120.0, -50.0)) && *radius == 4.0)));
    let words = texts(model);
    assert!(words.contains(&"-K1"), "{words:?}");
    assert!(!words.contains(&"secret"), "a hidden attribute is left out");
    assert!(words.contains(&"Pump start"), "MTEXT formatting is stripped: {words:?}");
    assert!(model.iter().any(|s| matches!(s,
        Shape::Text { text, align: Align::Center, .. } if text == "Pump start")));
    assert!(model.iter().any(|s| matches!(s,
        Shape::Polyline { points, closed: false, .. }
            if points.len() == 3 && near(points[1], Point::new(10.0, -10.0)) && near(points[2], Point::new(0.0, -10.0)))),
        "the vertices in the polyline's order, not their handles'");
    assert!(
        model.iter().any(|s| matches!(s,
        Shape::Polyline { points, closed: true, .. }
            if points.len() == 3 && near(points[0], Point::new(50.0, 0.0)) && near(points[2], Point::new(60.0, -5.0)))),
        "a 3D polyline"
    );
    assert_eq!(d.model_points.len(), 1);
    assert!(
        near(d.model_points[0], Point::new(7.0, -8.0)),
        "POINT positions are kept"
    );
    assert!(
        !has_line(model, Point::new(0.0, 0.0), Point::new(1.0, -1.0)),
        "an invisible line is left out"
    );

    assert_eq!(d.skipped.get("ELLIPSE"), Some(&1));
    assert_eq!(d.skipped.len(), 1, "{:?}", d.skipped);
}

#[test]
fn a_2010_drawing_gives_its_blocks_and_model_space() -> TestResult {
    let v = Version::R2010;
    check_sample(&drawing(&sections(v, &sample(v), &[]))?);
    Ok(())
}

#[test]
fn a_2004_drawing_gives_its_blocks_and_model_space() -> TestResult {
    let v = Version::R2004;
    check_sample(&drawing(&sections(v, &sample(v), &[]))?);
    Ok(())
}

#[test]
fn a_whole_file_is_read() -> TestResult {
    for (tag, v) in [(b"AC1024", Version::R2010), (b"AC1018", Version::R2004)] {
        let (map, objects) = sections_bytes(&sample(v), &[]);
        let file = write_dwg(
            tag,
            &[
                TestSection {
                    name: "AcDb:AcDbObjects",
                    data: objects,
                    ..Default::default()
                },
                TestSection {
                    name: "AcDb:Handles",
                    data: map,
                    ..Default::default()
                },
            ],
        );
        check_sample(&read_drawing(&file)?);
    }
    Ok(())
}

#[test]
fn layers_go_with_the_entities() -> TestResult {
    let v = Version::R2010;
    let d = dxf_drawing(&sections(v, &sample(v), &[]))?.drawing;
    let relay = &d.blocks["RELAY"];
    assert!(
        relay.iter().all(|e| e.codes.contains(&(8, "SYMBOL".into()))),
        "{relay:?}"
    );
    Ok(())
}

#[test]
fn a_long_text_and_a_big_object_map_are_read() -> TestResult {
    let v = Version::R2010;
    let long = "x".repeat(3000);
    let mut objs = vec![
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        entry(v, LAYER, LAYER_H, "0"),
    ];
    let mut t = entity(v, 1, 0x100, head(Some(MODEL_H)));
    text_data(&mut t, (0.0, 0.0), &long, 2.5, 0.0);
    objs.push(t);
    // Enough lines for the object map to need several parts, at far-apart handles.
    for n in 0..900u64 {
        objs.push(line(v, 0x200 + n * 1000, head(None), (n as f64, 0.0), (n as f64, 1.0)));
    }
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(texts(&d.model), [long.as_str()]);
    assert_eq!(d.model.len(), 901);
    assert!(d.skipped.is_empty(), "{:?}", d.skipped);
    Ok(())
}

#[test]
fn bit_values_are_read_in_every_form() -> TestResult {
    let mut w = W::default();
    w.bs(0).bs(256).bs(7).bs(4000);
    w.bl(0).bl(9).bl(70_000);
    w.bd(0.0).bd(1.0).bd(-2.75);
    let mid = f64::from_bits(5.0f64.to_bits() ^ 0x0000_1234_0000_0000);
    w.dd(5.0, 5.0).dd(5.0000001, 5.0).dd(mid, 5.0).dd(-1e300, 5.0);
    w.bll(0).bll(0x0001_0203_0405);
    w.bt(0.0).bt(3.0).be((0.0, 0.0, 1.0)).be((0.0, 0.0, -1.0));
    w.h(3, 0).h(5, 0x1234_5678);
    let bytes = w.bytes();
    let mut r = Bits::new(&bytes, 0, w.0.len());
    assert_eq!([r.bs()?, r.bs()?, r.bs()?, r.bs()?], [0, 256, 7, 4000]);
    assert_eq!([r.bl()?, r.bl()?, r.bl()?], [0, 9, 70_000]);
    assert_eq!([r.bd()?, r.bd()?, r.bd()?], [0.0, 1.0, -2.75]);
    assert_eq!(
        [r.dd(5.0)?, r.dd(5.0)?, r.dd(5.0)?, r.dd(5.0)?],
        [5.0, 5.0000001, mid, -1e300]
    );
    assert_eq!([r.bll()?, r.bll()?], [0, 0x0001_0203_0405]);
    assert_eq!([r.bt()?, r.bt()?], [0.0, 3.0]);
    assert_eq!([r.be()?, r.be()?], [(0.0, 0.0, 1.0), (0.0, 0.0, -1.0)]);
    assert_eq!([r.h()?, r.h()?], [(3, 0), (5, 0x1234_5678)]);
    assert!(r.b().is_err(), "reading past the end is an error");
    // Relative handles count from the object's own.
    let mut w = W::default();
    w.h(6, 0).h(8, 0).h(0xA, 5).h(0xC, 5).h(4, 0x42);
    let bytes = w.bytes();
    let mut r = Bits::new(&bytes, 0, w.0.len());
    let got: Vec<u64> = (0..5).map(|_| r.handle(100)).collect::<Result<_>>()?;
    assert_eq!(got, [101, 99, 105, 95, 0x42]);
    Ok(())
}

#[test]
fn modular_numbers_are_read() -> TestResult {
    for v in [0usize, 5, 0x7FFF, 0x8000, 0x12_3456] {
        assert_eq!(modular_short(&ms(v))?, (v, ms(v).len()));
    }
    for v in [0u64, 0x7F, 0x80, 0x1_0000] {
        assert_eq!(modular_char(&umc(v), false)?, (v as i64, umc(v).len()));
    }
    for v in [0i64, 0x3F, -0x3F, 0x40, -5000, 1 << 40] {
        assert_eq!(modular_char(&smc(v), true)?, (v, smc(v).len()));
    }
    assert!(modular_char(&[0x80, 0x80], false).is_err(), "cut off");
    assert!(modular_short(&[0xFF, 0xFF]).is_err(), "cut off");
    Ok(())
}

#[test]
fn a_damaged_object_is_counted_and_the_rest_read() -> TestResult {
    let v = Version::R2010;
    let objs = sample(v);
    // An object past the end of the objects, and one whose handle differs from the map.
    let (_, objects) = sections_bytes(&objs, &[]);
    let d = drawing(&sections(v, &objs, &[(0x900, objects.len() + 10), (0x901, 4)]))?;
    assert_eq!(d.skipped.get(DAMAGED), Some(&2));
    assert_eq!(d.blocks.len(), 2);
    assert_eq!(d.damaged.len(), 2, "with their reasons: {:?}", d.damaged);
    assert!(
        d.damaged
            .iter()
            .any(|r| r.starts_with("object 900 at") && r.contains("past the end"))
    );
    assert!(
        d.damaged
            .iter()
            .any(|r| r.starts_with("object 901 at 4") && r.contains("differs"))
    );
    Ok(())
}

#[test]
fn a_damaged_object_map_or_missing_section_is_an_error() {
    let v = Version::R2004;
    let mut s = sections(v, &sample(v), &[]);
    // A part that claims more bytes than there are.
    s.sections[0].data = vec![0x10, 0x00, 1, 2];
    assert!(drawing(&s).is_err());
    s.sections.remove(0);
    assert!(drawing(&s).is_err());
}

#[test]
fn entities_without_a_known_owner_are_counted() -> TestResult {
    let v = Version::R2004;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        line(v, 0x30, head(Some(0x999)), (0.0, 0.0), (1.0, 0.0)),
    ];
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d.skipped.get(ORPHANS), Some(&1));
    assert!(d.model.is_empty());
    Ok(())
}

#[test]
fn a_block_inserting_itself_ends() -> TestResult {
    let v = Version::R2010;
    let mut objs = vec![entry(v, BLOCK_HEADER, RELAY_H, "LOOP")];
    let mut i = entity(v, 7, 0x40, head(Some(RELAY_H)));
    i.data.bd3((1.0, 0.0, 0.0)).bb(3).bd(0.0).bd3((0.0, 0.0, 1.0)).b(false);
    i.handles.h(5, RELAY_H);
    objs.push(i);
    objs.push(line(v, 0x41, head(Some(RELAY_H)), (0.0, 0.0), (1.0, 0.0)));
    let d = drawing(&sections(v, &objs, &[]))?;
    assert!(!d.blocks["LOOP"].is_empty());
    Ok(())
}

#[test]
fn flipped_and_cut_objects_give_no_panic() {
    for v in [Version::R2004, Version::R2010] {
        let s = sections(v, &sample(v), &[]);
        let objects = s.sections[1].data.clone();
        let map = s.sections[0].data.clone();
        for i in (0..objects.len()).step_by(3) {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut s2 = s.clone();
                s2.sections[1].data[i] ^= flip;
                let _ = drawing(&s2);
            }
            let mut s2 = s.clone();
            s2.sections[1].data.truncate(i);
            let _ = drawing(&s2);
        }
        for i in 0..map.len() {
            let mut s2 = s.clone();
            s2.sections[0].data[i] ^= 0xFF;
            let _ = drawing(&s2);
        }
    }
}

#[test]
fn an_object_map_needs_its_crc_and_its_end() -> TestResult {
    let entry = [0u8, 4, 1, 4];
    for cut in [
        &[][..],
        &[0],
        &entry,
        &[0, 4, 1, 4, 0],
        &[0, 4, 1, 4, 0, 0],
        &[0, 4, 1, 4, 0, 0, 0],
        &[0, 4, 1, 4, 0, 0, 0, 2],
        &[0, 4, 1, 4, 0, 0, 0, 2, 0],
        &[0, 2],
        &[0, 2, 0],
    ] {
        assert!(object_map(cut).is_err(), "{cut:?}");
    }
    assert!(object_map(&[0, 1]).is_err(), "a part too short to hold its size");
    assert_eq!(object_map(&[0, 4, 1, 4, 0, 0, 0, 2, 0, 0])?, [(1, 4)]);
    assert!(object_map(&[0, 2, 0, 0])?.is_empty());
    Ok(())
}

#[test]
fn a_handle_listed_twice_is_refused_even_across_parts() {
    // Handle 1 at 4, then in a second part handle 1 again.
    let map = [0u8, 4, 1, 4, 0, 0, 0, 4, 1, 8, 0, 0, 0, 2, 0, 0];
    assert!(object_map(&map).is_err_and(|e| e.to_string().contains("listed twice")));
    // The same within one part: a handle step of zero.
    let map = [0u8, 6, 1, 4, 0, 4, 0, 0, 0, 2, 0, 0];
    assert!(object_map(&map).is_err());
    // Different handles across parts are fine.
    let map = [0u8, 4, 1, 4, 0, 0, 0, 4, 2, 8, 0, 0, 0, 2, 0, 0];
    assert_eq!(object_map(&map).ok(), Some(vec![(1, 4), (2, 8)]));
}

#[test]
fn blocks_inserting_blocks_many_times_stop_at_the_limit() -> TestResult {
    let v = Version::R2010;
    // B0 to B6 each insert the next block 16 times; B7 holds a line: 16^7 lines in all.
    let mut objs = vec![entry(v, LAYER, LAYER_H, "0")];
    let mut next = 0x1000u64;
    for b in 0..8u64 {
        objs.push(entry(v, BLOCK_HEADER, 0x100 + b, &format!("B{b}")));
        if b == 7 {
            objs.push(line(v, next, head(Some(0x107)), (0.0, 0.0), (1.0, 0.0)));
            next += 1;
            continue;
        }
        for _ in 0..16 {
            let mut i = entity(v, 7, next, head(Some(0x100 + b)));
            i.data.bd3((1.0, 0.0, 0.0)).bb(3).bd(0.0).bd3((0.0, 0.0, 1.0)).b(false);
            i.handles.h(5, 0x101 + b);
            objs.push(i);
            next += 1;
        }
    }
    let mut i = entity(v, 7, next, head(None));
    i.data.bd3((0.0, 0.0, 0.0)).bb(3).bd(0.0).bd3((0.0, 0.0, 1.0)).b(false);
    i.handles.h(5, 0x100);
    objs.push(i);
    let d = drawing(&sections(v, &objs, &[]))?;
    let drawn = d.model.len() + d.blocks.values().map(Vec::len).sum::<usize>();
    assert!(drawn <= dxf::MAX_DRAWN, "{drawn}");
    assert!(d.skipped.contains_key(dxf::OVER_LIMIT), "{:?}", d.skipped);
    Ok(())
}

// ---- Drawings for other modules' tests ----

/// One entity of a test drawing, in drawing units (y up).
pub(crate) enum TestEntity {
    Line((f64, f64), (f64, f64)),
    Circle((f64, f64), f64),
    /// Centre, radius, start and end angle in degrees, counter-clockwise.
    Arc((f64, f64), f64, f64, f64),
    Text((f64, f64), &'static str),
    /// A reference to a block `FRAME` holding a 10 mm line, at this point.
    Insert((f64, f64)),
    /// An object the handle map places past the end of the objects: it cannot be read.
    Damaged,
}

/// An AutoCAD 2010 drawing whose model space holds `items`, for tests elsewhere in the crate.
pub(crate) fn model_space_dwg(items: &[TestEntity]) -> Vec<u8> {
    const FRAME_H: u64 = 0x22;
    let v = Version::R2010;
    let model = head(None);
    let mut objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, PAPER_H, "*Paper_Space"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        entry(v, BLOCK_HEADER, FRAME_H, "FRAME"),
        line(v, 0x30, head(Some(FRAME_H)), (0.0, 0.0), (10.0, 0.0)),
    ];
    let mut damaged = Vec::new();
    for (n, item) in items.iter().enumerate() {
        let handle = 0x100 + n as u64;
        objs.push(match *item {
            TestEntity::Damaged => {
                damaged.push(handle);
                continue;
            }
            TestEntity::Line(a, b) => line(v, handle, model, a, b),
            TestEntity::Circle(c, r) => {
                let mut o = entity(v, 18, handle, model);
                o.data.bd3((c.0, c.1, 0.0)).bd(r).bt(0.0).be((0.0, 0.0, 1.0));
                o
            }
            TestEntity::Arc(c, r, start, end) => {
                let mut o = entity(v, 17, handle, model);
                o.data
                    .bd3((c.0, c.1, 0.0))
                    .bd(r)
                    .bt(0.0)
                    .be((0.0, 0.0, 1.0))
                    .bd(start.to_radians())
                    .bd(end.to_radians());
                o
            }
            TestEntity::Text(at, value) => {
                let mut o = entity(v, 1, handle, model);
                text_data(&mut o, at, value, 2.5, 0.0);
                o
            }
            TestEntity::Insert(at) => {
                let mut o = entity(v, 7, handle, model);
                o.data
                    .bd3((at.0, at.1, 0.0))
                    .bb(3)
                    .bd(0.0)
                    .bd3((0.0, 0.0, 1.0))
                    .b(false);
                o.handles.h(5, FRAME_H);
                o
            }
        });
    }
    let (_, objects) = sections_bytes(&objs, &[]);
    let past_end: Vec<(u64, usize)> = damaged.iter().map(|&h| (h, objects.len() + 10)).collect();
    let (map, objects) = sections_bytes(&objs, &past_end);
    write_dwg(
        b"AC1024",
        &[
            TestSection {
                name: "AcDb:AcDbObjects",
                data: objects,
                ..Default::default()
            },
            TestSection {
                name: "AcDb:Handles",
                data: map,
                ..Default::default()
            },
        ],
    )
}

/// A BLOCK_HEADER with every field up to its base point; an xref has no owned-object count.
fn block_header(v: Version, handle: u64, name: &str, base: (f64, f64), xref: bool) -> Obj {
    let mut o = named(v, BLOCK_HEADER, handle, name);
    match v {
        Version::R2004 => o.data.b(true).bs(0).b(false),
        Version::R2010 => o.data.bs(0),
    };
    o.data.b(false).b(false).b(xref).b(false).b(false);
    if !xref {
        o.data.bl(1);
    }
    o.data.bd3((base.0, base.1, 0.0));
    o
}

#[test]
fn a_block_is_placed_by_its_base_point() -> TestResult {
    for v in [Version::R2004, Version::R2010] {
        let mut objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            block_header(v, 0x100, "B", (5.0, 5.0), false),
            block_header(v, 0x101, "X", (5.0, 5.0), true),
            line(v, 0x202, head(Some(0x101)), (5.0, 5.0), (5.0, 10.0)),
            line(v, 0x200, head(Some(0x100)), (5.0, 5.0), (10.0, 5.0)),
            {
                let mut i = entity(v, 7, 0x201, head(None));
                i.data.bd3((0.0, 0.0, 0.0)).bb(3).bd(0.0).bd3((0.0, 0.0, 1.0)).b(false);
                i.handles.h(5, 0x100);
                i
            },
        ];
        let mut x = entity(v, 7, 0x203, head(None));
        x.data.bd3((0.0, 0.0, 0.0)).bb(3).bd(0.0).bd3((0.0, 0.0, 1.0)).b(false);
        x.handles.h(5, 0x101);
        objs.push(x);
        let d = drawing(&sections(v, &objs, &[]))?;
        // An xref block, without the owned-object count, is read right too.
        assert!(
            has_line(&d.model, Point::new(0.0, 0.0), Point::new(0.0, -5.0)),
            "{v:?}: {:?}",
            d.model
        );
        assert!(
            has_line(&d.model, Point::new(0.0, 0.0), Point::new(5.0, 0.0)),
            "{v:?}: {:?}",
            d.model
        );
        // The block on its own keeps its coordinates.
        assert!(
            has_line(&d.blocks["B"], Point::new(5.0, -5.0), Point::new(10.0, -5.0)),
            "{:?}",
            d.blocks["B"]
        );
    }
    Ok(())
}

#[test]
fn a_block_whose_base_point_is_cut_off_is_kept_and_said() -> TestResult {
    // A 2004 BLOCK_HEADER whose data ends after its name.
    let v = Version::R2004;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        named(v, BLOCK_HEADER, 0x100, "CUT"),
        line(v, 0x200, head(Some(0x100)), (5.0, 5.0), (10.0, 5.0)),
    ];
    let read = dxf_drawing(&sections(v, &objs, &[]))?;
    assert!(
        read.drawing.blocks.contains_key("CUT"),
        "{:?}",
        read.drawing.blocks.keys()
    );
    assert_eq!(read.skipped.get(BASE_UNREAD), Some(&1), "{:?}", read.skipped);
    assert!(!read.skipped.contains_key(DAMAGED), "{:?}", read.skipped);
    assert!(
        read.damaged.iter().any(|r| r.contains("base point of block CUT")),
        "{:?}",
        read.damaged
    );
    Ok(())
}

#[test]
fn a_dimension_is_drawn_by_its_block() -> TestResult {
    for v in [Version::R2004, Version::R2010] {
        let d1 = head(Some(0x100));
        let mut objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            entry(v, BLOCK_HEADER, 0x100, "*D1"),
            line(v, 0x200, d1, (0.0, 0.0), (0.0, 5.0)),
            line(v, 0x201, d1, (20.0, 0.0), (20.0, 5.0)),
        ];
        let mut t = entity(v, 1, 0x202, d1);
        text_data(&mut t, (10.0, 6.0), "20", 2.5, 0.0);
        objs.push(t);
        // A linear dimension (type 21) in model space, then one whose block is missing. Its data
        // is not read: the block comes after the dimension style in the handle stream.
        for (h, block) in [(0x300, 0x100), (0x301, 0x999)] {
            let mut dim = entity(v, 21, h, head(None));
            dim.handles.h(5, 0x50).h(5, block);
            objs.push(dim);
        }
        let d = drawing(&sections(v, &objs, &[]))?;
        assert!(
            has_line(&d.model, Point::new(0.0, 0.0), Point::new(0.0, -5.0)),
            "{v:?}: {:?}",
            d.model
        );
        assert!(
            has_line(&d.model, Point::new(20.0, 0.0), Point::new(20.0, -5.0)),
            "{:?}",
            d.model
        );
        assert_eq!(texts(&d.model), ["20"]);
        assert_eq!(
            d.skipped.get("DIMENSION"),
            Some(&1),
            "only the one without its block: {:?}",
            d.skipped
        );
    }
    Ok(())
}

/// An `AcDb:Classes` section listing `classes` (number, DXF name, entity?) as AutoCAD 2004 or
/// 2010 (maintenance release 6) writes it.
fn classes_section(v: Version, classes: &[(u16, &str, bool)]) -> Vec<u8> {
    const SENTINEL: [u8; 16] = [
        0x8D, 0xA1, 0xC4, 0xB8, 0xC4, 0xA9, 0xF8, 0xC5, 0xC0, 0xDC, 0xF4, 0x5F, 0xE7, 0xCF, 0xB6, 0x8A,
    ];
    let max = classes.iter().map(|c| c.0).max().unwrap_or(499);
    let mut data = W::default();
    let mut strings = W::default();
    data.bs(max).rc(0).rc(0).b(true);
    for &(number, name, entity) in classes {
        data.bs(number).bs(0);
        match v {
            Version::R2004 => {
                data.t8("ObjectDBX Classes").t8("AcDbThing").t8(name);
            }
            Version::R2010 => {
                strings.tu("ObjectDBX Classes").tu("AcDbThing").tu(name);
            }
        }
        data.b(false).bs(if entity { 0x1F2 } else { 0x1F3 });
        data.bl(1).bl(0).bl(0).bl(0).bl(0);
    }
    let mut out = W::default();
    for b in SENTINEL {
        out.rc(b);
    }
    match v {
        Version::R2004 => {
            out.rl(data.bytes().len() as u32).append(&data);
        }
        Version::R2010 => {
            // Data, texts, their size and the flag; the flag sits `bitsize` after bit 191.
            let mut body = W::default();
            body.append(&data).append(&strings);
            body.rs(strings.0.len() as u16).b(true);
            let flag_at = 28 * 8 + body.0.len() - 1;
            out.rl(data.bytes().len() as u32)
                .rl(0)
                .rl((flag_at - 191) as u32)
                .append(&body);
        }
    }
    out.bytes()
}

/// `sections` plus a class section, maintenance release 6.
fn sections_with_classes(v: Version, objs: &[Obj], classes: Vec<u8>) -> Sections {
    let mut s = sections(v, objs, &[]);
    s.sections.push(Section {
        name: "AcDb:Classes".into(),
        id: 3,
        data: classes,
    });
    s.maint = 6;
    s
}

#[test]
fn custom_entities_are_counted_by_their_class_name_and_custom_objects_not() -> TestResult {
    for v in [Version::R2004, Version::R2010] {
        let classes = classes_section(v, &[(500, "WIPEOUT", true), (501, "ACDBDICTIONARYWDFLT", false)]);
        let mut wipeout = entity(v, 500, 0x300, head(None));
        wipeout.data.bd(0.0);
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            wipeout,
            Obj::new(v, 501, 0x301),
        ];
        let d = drawing(&sections_with_classes(v, &objs, classes))?;
        assert_eq!(d.skipped.get("WIPEOUT"), Some(&1), "{v:?}: {:?}", d.skipped);
        assert_eq!(d.skipped.len(), 1, "nothing for the object: {:?}", d.skipped);
    }
    Ok(())
}

#[test]
fn a_damaged_class_section_is_named_and_the_rest_read() -> TestResult {
    for v in [Version::R2004, Version::R2010] {
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            line(v, 0x300, head(None), (0.0, 0.0), (1.0, 0.0)),
        ];
        let whole = classes_section(v, &[(500, "WIPEOUT", true), (501, "IMAGE", true)]);
        // A wrong sentinel, a section cut in its header, and one cut inside its second class.
        for bad in [vec![0u8; 40], whole[..22].to_vec(), whole[..whole.len() - 6].to_vec()] {
            let d = drawing(&sections_with_classes(v, &objs, bad))?;
            assert_eq!(d.model.len(), 1, "{v:?}: the drawing is still read");
            assert!(
                d.damaged.iter().any(|r| r.contains("AcDb:Classes")),
                "{v:?}: {:?}",
                d.damaged
            );
            assert!(d.damaged.iter().all(|r| !r.contains("DWG: DWG:")), "{:?}", d.damaged);
            assert_eq!(d.skipped.get(CLASSES_DAMAGED), Some(&1), "{v:?}: {:?}", d.skipped);
            assert!(
                !d.skipped.contains_key(DAMAGED),
                "a section is not an object: {:?}",
                d.skipped
            );
        }
        // An empty class list (highest class 499) is no damage.
        let d = drawing(&sections_with_classes(v, &objs, classes_section(v, &[])))?;
        assert!(d.damaged.is_empty(), "{v:?}: {:?}", d.damaged);
    }
    Ok(())
}
