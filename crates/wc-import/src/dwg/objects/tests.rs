use wc_core::geom::{Align, Point};

use super::super::Section;
use super::super::tests::{TestSection, write_dwg, write_dwg_2007};
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
    /// A SPLINE's scenario (1 control points, 2 fit points); from 2013 on followed by the
    /// spline flags (bit 1: fit points) and the knot parameter (15: custom knots) that say
    /// the same.
    fn scenario(&mut self, v: Version, scenario: u32) -> &mut Self {
        self.bl(scenario);
        if v >= Version::R2013 {
            let fit = scenario == 2;
            self.bl(u32::from(fit)).bl(if scenario == 1 { 15 } else { 0 });
        }
        self
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
        self.t8_bytes(s.as_bytes())
    }
    /// A TV of these bytes (2004): its length, then the bytes.
    fn t8_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.bs(bytes.len() as u16);
        for b in bytes {
            self.rc(*b);
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
            _ => {
                self.strings.tu(s);
            }
        }
        self
    }

    /// The texts, their size and a flag, at the end of the data (2007 on).
    fn string_stream(&self, w: &mut W) {
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
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = W::default();
        match self.version {
            Version::R2004 | Version::R2007 => {
                w.bs(self.kind);
                let rl_at = w.0.len();
                w.rl(0).h(0, self.handle).append(&self.data);
                // 2007 ends the data with the texts, inside the size in bits.
                if self.version == Version::R2007 {
                    self.string_stream(&mut w);
                }
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
            _ => {
                match self.kind {
                    0..=255 => w.bb(0).rc(self.kind as u8),
                    k if (0x1F0..0x1F0 + 256).contains(&k) => w.bb(1).rc((k - 0x1F0) as u8),
                    k => w.bb(2).rs(k),
                };
                w.h(0, self.handle).append(&self.data);
                self.string_stream(&mut w);
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
    // Data in the data store, from 2013 on.
    if v >= Version::R2013 {
        o.data.b(false);
    }
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
    /// The shadow flags (2007 on), which have no handle.
    shadow: u8,
}

fn head(owner: Option<u64>) -> Head {
    Head {
        owner,
        layer: LAYER_H,
        invisible: false,
        extras: false,
        book: false,
        shadow: 0,
    }
}

fn entity(v: Version, kind: u16, handle: u64, h: Head) -> Obj {
    let mut o = Obj::new(v, kind, handle);
    let d = &mut o.data;
    if h.extras {
        d.bs(3).h(5, 0x99).rc(1).rc(2).rc(3).bs(0);
        d.b(true);
        match v {
            Version::R2004 | Version::R2007 => d.rl(2),
            _ => d.bll(2),
        };
        d.rc(0xAB).rc(0xCD);
    } else {
        d.bs(0).b(false);
    }
    d.bb(if h.owner.is_some() { 0 } else { 2 });
    d.bl(u32::from(h.extras)).b(!h.extras);
    // Data in the data store, from 2013 on: set with the extras.
    if v >= Version::R2013 {
        d.b(h.extras);
    }
    if h.extras {
        let book = if h.book { 0x4000 } else { 0 };
        d.bs(0x8000 | book | 0x2000 | 7).bl(0x00FF_0000).bl(0x0200_0080);
    } else {
        d.bs(256);
    }
    d.bd(1.0)
        .bb(if h.extras { 3 } else { 0 })
        .bb(if h.extras { 3 } else { 0 });
    // Material and shadow flags from 2007 on, visual styles from 2010 on.
    if v != Version::R2004 {
        d.bb(if h.extras { 3 } else { 0 }).rc(h.shadow);
    }
    if v >= Version::R2010 {
        d.b(false).b(h.extras).b(false);
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
        // Linetype, material (from 2007 on) and plot style, then a visual style (2010).
        hs.h(5, 0x14);
        if v != Version::R2004 {
            hs.h(5, 0x17);
        }
        hs.h(5, 0x15);
        if v >= Version::R2010 {
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
    if v >= Version::R2010 {
        o.data.rc(0);
    }
    // A single-line attribute, from 2018 on.
    if v >= Version::R2018 {
        o.data.rc(1);
    }
    o.text(tag);
    o.data.bs(0).rc(flags);
    // The lock position flag, from 2007 on.
    if v != Version::R2004 {
        o.data.b(false);
    }
    if kind == 3 {
        if v >= Version::R2010 {
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
    // Closed; from 2010 on with vertex ids too, their count before the points and the ids
    // after them.
    let ids = v >= Version::R2010;
    let mut p = entity(v, 77, 0x42, relay);
    p.data.bs(if ids { 512 | 1024 } else { 512 }).bl(3);
    if ids {
        p.data.bl(3);
    }
    p.data
        .rd(0.0)
        .rd(0.0)
        .dd(4.0, 0.0)
        .dd(0.0, 0.0)
        .dd(4.0, 4.0)
        .dd(3.0, 0.0);
    if ids {
        p.data.bl(1).bl(2).bl(3);
    }
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
    // vertices, an MTEXT, a point, an invisible line, a ray (not read) and a custom object.
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
    // The reference rectangle's height, from 2007 on.
    if v != Version::R2004 {
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
    objs.push(entity(v, 40, 0x5B, model));
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
        codepage: 30,
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

    assert_eq!(d.skipped.get("RAY"), Some(&1));
    assert_eq!(d.skipped.len(), 1, "{:?}", d.skipped);
}

#[test]
fn a_2010_drawing_gives_its_blocks_and_model_space() -> TestResult {
    let v = Version::R2010;
    check_sample(&drawing(&sections(v, &sample(v), &[]))?);
    Ok(())
}

#[test]
fn a_2007_drawing_gives_its_blocks_and_model_space() -> TestResult {
    let v = Version::R2007;
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
fn a_2013_drawing_gives_its_blocks_and_model_space() -> TestResult {
    let v = Version::R2013;
    check_sample(&drawing(&sections(v, &sample(v), &[]))?);
    Ok(())
}

#[test]
fn a_2018_drawing_gives_its_blocks_and_model_space() -> TestResult {
    let v = Version::R2018;
    check_sample(&drawing(&sections(v, &sample(v), &[]))?);
    Ok(())
}

#[test]
fn the_2013_data_store_flag_is_read() -> TestResult {
    // 2010 objects (without the flag) read as 2013 ones come out wrong: the flag is needed.
    let s = sections(Version::R2013, &sample(Version::R2010), &[]);
    let d = drawing(&s)?;
    assert!(d.skipped.get(DAMAGED).is_some_and(|n| *n > 0), "{:?}", d.skipped);
    Ok(())
}

#[test]
fn a_2018_multi_line_attribute_is_counted_and_skipped() -> TestResult {
    let v = Version::R2018;
    let mut objs = sample(v);
    // A second attribute of the RELAY insert in model space, of type 2 (multi-line): its
    // embedded MTEXT is not read, so nothing after the type is written.
    let mut a = entity(v, 2, 0x5D, head(Some(0x50)));
    text_data_with(&mut a, (100.0, 62.0), "two lines", 2.5, 0.0, Some(1));
    a.data.rc(0).rc(2);
    objs.push(a);
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d.skipped.get("ATTRIB (multi-line)"), Some(&1), "{:?}", d.skipped);
    // A multi-line ATTDEF (type 4) in the RELAY block likewise.
    let mut objs = sample(v);
    let mut a = entity(v, 3, 0x5D, head(Some(RELAY_H)));
    text_data_with(&mut a, (0.0, 12.0), "?", 2.5, 0.0, Some(1));
    a.data.rc(0).rc(4);
    objs.push(a);
    let d2 = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d2.skipped.get("ATTDEF (multi-line)"), Some(&1), "{:?}", d2.skipped);
    assert!(!d2.skipped.contains_key(DAMAGED), "{:?}", d2.damaged);
    assert!(!texts(&d.model).contains(&"two lines"));
    assert!(!d.skipped.contains_key(DAMAGED), "{:?}", d.damaged);
    assert!(texts(&d.model).contains(&"-K1"), "the single-line one is still drawn");
    Ok(())
}

#[test]
fn a_2018_attribute_of_an_unknown_type_is_damage() -> TestResult {
    let v = Version::R2018;
    let mut objs = sample(v);
    let mut a = entity(v, 2, 0x5D, head(Some(0x50)));
    text_data_with(&mut a, (100.0, 62.0), "odd", 2.5, 0.0, Some(1));
    a.data.rc(0).rc(42);
    objs.push(a);
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d.skipped.get(DAMAGED), Some(&1), "{:?}", d.skipped);
    assert!(!d.skipped.contains_key("ATTRIB (multi-line)"), "{:?}", d.skipped);
    assert!(
        d.damaged.iter().any(|m| m.contains("unknown type 42")),
        "{:?}",
        d.damaged
    );
    Ok(())
}

#[test]
fn cut_or_flipped_2013_and_2018_files_give_errors_not_panics() -> TestResult {
    for (tag, v) in [(b"AC1027", Version::R2013), (b"AC1032", Version::R2018)] {
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
        for len in (0..file.len()).step_by(5) {
            let _ = read_drawing(&file[..len]);
        }
        let mut seed = 9u32;
        for i in 0..1000 {
            let mut damaged = file.clone();
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let at = (seed as usize >> 4) % damaged.len();
            damaged[at] ^= 1 << (i % 8);
            let _ = read_drawing(&damaged);
        }
    }
    Ok(())
}

#[test]
fn a_whole_file_is_read() -> TestResult {
    for (tag, v) in [
        (b"AC1032", Version::R2018),
        (b"AC1027", Version::R2013),
        (b"AC1024", Version::R2010),
        (b"AC1018", Version::R2004),
    ] {
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
    // And a 2007 file, in its own container.
    let (map, objects) = sections_bytes(&sample(Version::R2007), &[]);
    let file = write_dwg_2007(&[
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
    ]);
    check_sample(&read_drawing(&file)?);
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
    for v in [
        Version::R2004,
        Version::R2007,
        Version::R2010,
        Version::R2013,
        Version::R2018,
    ] {
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
    model_space_dwg_with_classes(items, None)
}

/// [`model_space_dwg`] with an `AcDb:Classes` section holding `classes` as given.
pub(crate) fn model_space_dwg_with_classes(items: &[TestEntity], classes: Option<Vec<u8>>) -> Vec<u8> {
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
    let mut sections = vec![
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
    ];
    if let Some(data) = classes {
        sections.push(TestSection {
            name: "AcDb:Classes",
            data,
            ..Default::default()
        });
    }
    write_dwg(b"AC1024", &sections)
}

/// A BLOCK_HEADER with every field up to its base point; an xref has no owned-object count.
fn block_header(v: Version, handle: u64, name: &str, base: (f64, f64), xref: bool) -> Obj {
    block_fields(named(v, BLOCK_HEADER, handle, name), base, xref)
}

/// The fields a BLOCK_HEADER has after its name, written onto `o`.
fn block_fields(mut o: Obj, base: (f64, f64), xref: bool) -> Obj {
    let v = o.version;
    match v {
        Version::R2004 => o.data.b(true).bs(0).b(false),
        _ => o.data.bs(0),
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
    for v in [
        Version::R2004,
        Version::R2007,
        Version::R2010,
        Version::R2013,
        Version::R2018,
    ] {
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
    // A page says why, not only how many.
    let mut objs = objs;
    objs.push(line(v, 0x201, head(None), (0.0, 0.0), (10.0, 0.0)));
    let page = crate::dwg::page_of(&sections(v, &objs, &[]), 1.0, &Default::default())?;
    assert!(
        page.warnings
            .iter()
            .any(|w| w.starts_with("1 block base points not read") && w.contains("base point of block CUT")),
        "{:?}",
        page.warnings
    );
    Ok(())
}

/// A drawing with nothing to draw names the block whose base point it could not read; the
/// name comes from the file, so `import page-dwg` escapes it before it reaches the terminal.
#[test]
fn a_block_name_in_a_page_error_is_escaped_for_the_terminal() -> TestResult {
    let v = Version::R2004;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        named(v, BLOCK_HEADER, 0x100, "CUT\u{1b}[2J"),
        line(v, 0x200, head(Some(0x100)), (5.0, 5.0), (10.0, 5.0)),
    ];
    let drawing = crate::dwg::page_drawing(&sections(v, &objs, &[]))?;
    let Err(raw) = drawing.page(1.0, &Default::default()) else {
        unreachable!("nothing to draw");
    };
    assert!(raw.to_string().contains("CUT\u{1b}[2J"), "{raw}");
    let Err(shown) = crate::run::dwg_page(&drawing, 1.0, &Default::default()) else {
        unreachable!("nothing to draw");
    };
    assert!(!shown.contains('\u{1b}'), "{shown:?}");
    assert!(shown.contains("CUT\\u{1b}[2J"), "{shown}");
    Ok(())
}

#[test]
fn a_reference_to_a_missing_block_is_counted_and_the_rest_drawn() -> TestResult {
    for v in [Version::R2004, Version::R2010, Version::R2018] {
        let mut i = entity(v, 7, 0x201, head(None));
        i.data.bd3((0.0, 0.0, 0.0)).bb(3).bd(0.0).bd3((0.0, 0.0, 1.0)).b(false);
        i.handles.h(5, 0x999);
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            line(v, 0x200, head(None), (0.0, 0.0), (10.0, 0.0)),
            i,
        ];
        let read = dxf_drawing(&sections(v, &objs, &[]))?;
        assert_eq!(
            read.skipped.get(UNRESOLVED_INSERT),
            Some(&1),
            "{v:?}: {:?}",
            read.skipped
        );
        let page = crate::dwg::page_of(&sections(v, &objs, &[]), 1.0, &Default::default())?;
        assert!(!page.graphics.is_empty(), "{v:?}: the line is drawn");
        assert!(
            page.warnings
                .iter()
                .any(|w| w.starts_with("1 block references to a block that is not in the drawing")),
            "{v:?}: {:?}",
            page.warnings
        );
    }
    Ok(())
}

#[test]
fn the_attributes_of_a_reference_to_a_missing_block_stay_its_own() -> TestResult {
    for v in [Version::R2004, Version::R2010, Version::R2018] {
        let insert = |handle: u64, block: u64, attribs: bool| {
            let mut i = entity(v, 7, handle, head(None));
            i.data
                .bd3((0.0, 0.0, 0.0))
                .bb(3)
                .bd(0.0)
                .bd3((0.0, 0.0, 1.0))
                .b(attribs);
            if attribs {
                i.data.bl(1);
            }
            i.handles.h(5, block);
            i
        };
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            block_header(v, 0x100, "B", (0.0, 0.0), false),
            line(v, 0x200, head(Some(0x100)), (0.0, 0.0), (10.0, 0.0)),
            insert(0x201, 0x100, false),
            insert(0x202, 0x999, true),
            attrib(v, 2, 0x203, head(Some(0x202)), (0.0, 20.0), "-K9", "TAG", 0),
        ];
        let known: std::collections::BTreeSet<String> = ["b".to_owned()].into();
        let page = crate::dwg::page_of(&sections(v, &objs, &[]), 1.0, &known)?;
        assert_eq!(page.symbols.len(), 1, "{v:?}: {page:?}");
        assert!(
            page.symbols[0].tag.is_none(),
            "{v:?}: not the tag of the symbol before it: {:?}",
            page.symbols[0].tag
        );
        assert!(
            page.graphics
                .iter()
                .any(|s| matches!(s, Shape::Text { text, .. } if text == "-K9")),
            "{v:?}: the text is kept: {:?}",
            page.graphics
        );
    }
    Ok(())
}

#[test]
fn an_encrypted_class_section_is_said() -> TestResult {
    let v = Version::R2010;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        line(v, 0x200, head(None), (0.0, 0.0), (10.0, 0.0)),
    ];
    let mut s = sections(v, &objs, &[]);
    s.skipped.push("AcDb:Classes".to_owned());
    let read = dxf_drawing(&s)?;
    assert_eq!(read.skipped.get(CLASSES_DAMAGED), Some(&1), "{:?}", read.skipped);
    let page = crate::dwg::page_of(&s, 1.0, &Default::default())?;
    assert!(!page.graphics.is_empty());
    assert!(
        page.warnings
            .iter()
            .any(|w| w.contains("the class section is encrypted") && w.contains("custom entities are not counted")),
        "{:?}",
        page.warnings
    );
    Ok(())
}

/// A lightweight polyline declaring far more bulges than points: the extra ones are read past
/// (not kept), and the polyline is drawn as with one bulge per point.
#[test]
fn extra_bulges_of_a_lightweight_polyline_are_not_kept() -> TestResult {
    let v = Version::R2004;
    let poly = |bulges: u32| {
        let mut p = entity(v, 77, 0x200, head(None));
        p.data.bs(16).bl(2).bl(bulges);
        p.data.rd(0.0).rd(0.0).dd(10.0, 0.0).dd(0.0, 0.0).bd(1.0);
        for _ in 1..bulges {
            p.data.bd(0.0);
        }
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            p,
        ];
        drawing(&sections(v, &objs, &[]))
    };
    let (two, many) = (poly(2)?, poly(10_000)?);
    assert!(many.skipped.is_empty(), "{:?}", many.skipped);
    assert_eq!(format!("{:?}", many.model), format!("{:?}", two.model));
    Ok(())
}

/// The bulges kept for a polyline: one per point at most, however many the object declares,
/// and the reader ends past all of them.
#[test]
fn bulges_past_the_points_are_read_but_not_kept() -> TestResult {
    let mut w = W::default();
    w.bd(1.0);
    for _ in 1..10_000 {
        w.bd(0.0);
    }
    w.bs(7);
    let bytes = w.bytes();
    let mut r = Bits::new(&bytes, 0, w.0.len());
    assert_eq!(kept_bulges(&mut r, 10_000, 2)?, [1.0, 0.0]);
    assert_eq!(r.bs()?, 7, "read past every declared bulge");
    Ok(())
}

#[test]
fn a_bulge_that_is_not_a_number_is_damage() -> TestResult {
    for bad in [f64::NAN, f64::INFINITY] {
        let mut w = W::default();
        w.bd(0.0);
        w.bd(bad);
        let bytes = w.bytes();
        let mut r = Bits::new(&bytes, 0, w.0.len());
        let Err(e) = kept_bulges(&mut r, 2, 2) else {
            unreachable!("a bulge of {bad} was kept");
        };
        assert!(e.to_string().contains("bulge is not a number"), "{e}");
    }
    // Past the points it is read but not kept, so it does no harm.
    let mut w = W::default();
    w.bd(0.5);
    w.bd(f64::NAN);
    let bytes = w.bytes();
    let mut r = Bits::new(&bytes, 0, w.0.len());
    assert_eq!(kept_bulges(&mut r, 2, 1)?, [0.5]);
    Ok(())
}

#[test]
fn a_bulged_lightweight_polyline_is_drawn_with_its_arc() -> TestResult {
    for v in [Version::R2004, Version::R2010, Version::R2018] {
        // Open, two points 10 apart, the segment between them a half circle (bulge 1); from
        // 2010 on with vertex ids too, which come after the bulges.
        let ids = v >= Version::R2010;
        let mut p = entity(v, 77, 0x200, head(None));
        p.data.bs(if ids { 16 | 1024 } else { 16 }).bl(2).bl(2);
        if ids {
            p.data.bl(2);
        }
        p.data.rd(0.0).rd(0.0).dd(10.0, 0.0).dd(0.0, 0.0).bd(1.0).bd(0.0);
        if ids {
            p.data.bl(1).bl(2);
        }
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            p,
        ];
        let d = drawing(&sections(v, &objs, &[]))?;
        assert!(d.skipped.is_empty(), "{v:?}: {:?}", d.skipped);
        // Not a straight line: the arc's middle is 5 off the chord.
        let off_chord = |s: &Shape| match s {
            Shape::Polyline { points, .. } => points.iter().any(|p| (p.y.abs() - 5.0).abs() < 1e-6),
            Shape::Arc { radius, .. } => (*radius - 5.0).abs() < 1e-6,
            _ => false,
        };
        assert!(d.model.iter().any(off_chord), "{v:?}: {:?}", d.model);
    }
    Ok(())
}

#[test]
fn a_dimension_is_drawn_by_its_block() -> TestResult {
    for v in [
        Version::R2004,
        Version::R2007,
        Version::R2010,
        Version::R2013,
        Version::R2018,
    ] {
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

#[test]
fn shadow_flags_have_no_handle() -> TestResult {
    // The specification gives entities shadow flags (an RC, 2007 on) but no shadow handle: with
    // every flag set, the material and plot style handles and an INSERT's block still line up.
    for v in [Version::R2007, Version::R2010, Version::R2013, Version::R2018] {
        let mut objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            entry(v, BLOCK_HEADER, 0x100, "B"),
            line(v, 0x200, head(Some(0x100)), (0.0, 0.0), (5.0, 0.0)),
        ];
        let shadowed = Head {
            extras: true,
            shadow: 3,
            ..head(None)
        };
        let mut i = entity(v, 7, 0x201, shadowed);
        i.data.bd3((0.0, 0.0, 0.0)).bb(3).bd(0.0).bd3((0.0, 0.0, 1.0)).b(false);
        i.handles.h(5, 0x100);
        objs.push(i);
        let d = drawing(&sections(v, &objs, &[]))?;
        assert!(
            has_line(&d.model, Point::new(0.0, 0.0), Point::new(5.0, 0.0)),
            "{v:?}: {:?} {:?}",
            d.model,
            d.damaged
        );
        assert!(d.damaged.is_empty(), "{v:?}: {:?}", d.damaged);
    }
    Ok(())
}

/// A HATCH in model space: no gradient, pattern `name`, the paths written by `paths`.
fn hatch_obj(v: Version, handle: u64, name: &str, solid: bool, count: u32, paths: impl Fn(&mut W)) -> Obj {
    hatch_with(v, handle, name, solid, count, 1.0, 0, paths)
}

/// [`hatch_obj`] with an extrusion of (0, 0, `z`), a gradient of two colours (the second with a
/// colour name and a book name) and `boundary` boundary object handles. Written whole: after
/// the style and pattern type, a pattern's angle, scale and (no) lines, then (no) seed points.
#[allow(clippy::too_many_arguments)]
fn hatch_with(
    v: Version,
    handle: u64,
    name: &str,
    solid: bool,
    count: u32,
    z: f64,
    boundary: u64,
    paths: impl Fn(&mut W),
) -> Obj {
    let mut o = entity(v, 78, handle, head(None));
    o.data.bl(1).bl(0).bd(0.0).bd(0.0).bl(0).bd(0.0).bl(2);
    o.data.bd(0.0).bs(5).bl(0xC300_0005).rc(0);
    o.data.bd(1.0).bs(7).bl(0xC200_FF00).rc(3);
    o.text("Green");
    o.text("Book");
    o.text("LINEAR");
    o.data.bd(0.0).bd3((0.0, 0.0, z));
    o.text(name);
    o.data.b(solid).b(false).bl(count);
    paths(&mut o.data);
    o.data.bs(0).bs(1);
    if !solid {
        o.data.bd(0.0).bd(1.0).b(false).bs(0);
    }
    o.data.bl(0);
    for b in 0..boundary {
        o.handles.h(4, 0x700 + b);
    }
    o
}

#[test]
fn hatches_become_filled_and_unfilled_outlines() -> TestResult {
    for v in [Version::R2004, Version::R2010, Version::R2018] {
        // A solid square: a polyline path without bulges, filled by its solid flag (its name
        // is not SOLID).
        let solid = hatch_obj(v, 0x300, "_USER", true, 1, |d| {
            d.bl(2).b(false).b(true).bl(4);
            for p in [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)] {
                d.rd(p.0).rd(p.1);
            }
            d.bl(0);
        });
        // A pattern: an edge path of a line and a half circle over it.
        let pattern = hatch_obj(v, 0x301, "ANSI31", false, 1, |d| {
            d.bl(1).bl(2);
            d.rc(1).rd(20.0).rd(0.0).rd(30.0).rd(0.0);
            d.rc(2)
                .rd(25.0)
                .rd(0.0)
                .bd(5.0)
                .bd(0.0)
                .bd(std::f64::consts::PI)
                .b(true);
            d.bl(0);
        });
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            solid,
            pattern,
        ];
        let d = drawing(&sections(v, &objs, &[]))?;
        assert!(!d.skipped.contains_key("HATCH"), "{v:?}: {:?}", d.skipped);
        let polylines: Vec<(usize, bool)> = d
            .model
            .iter()
            .filter_map(|s| match s {
                Shape::Polyline { points, filled, .. } => Some((points.len(), *filled)),
                _ => None,
            })
            .collect();
        assert!(polylines.contains(&(4, true)), "{v:?}: {polylines:?}");
        // The half circle's points plus the line's start: well over 3, and not filled.
        assert!(polylines.iter().any(|(n, f)| *n > 5 && !f), "{v:?}: {polylines:?}");
        assert_eq!(polylines.len(), 2, "{:?}", d.model);
    }
    Ok(())
}

#[test]
fn a_field_missing_before_the_texts_does_not_read_into_them() -> TestResult {
    for v in [Version::R2007, Version::R2010, Version::R2013, Version::R2018] {
        let mut objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        ];
        // An ATTRIB without its last field, the lock position flag: the data ends where the
        // texts start, so the flag is missing rather than taken from the texts.
        let mut a = entity(v, 2, 0x300, head(None));
        text_data_with(&mut a, (0.0, 0.0), "value", 2.5, 0.0, Some(1));
        if v >= Version::R2010 {
            a.data.rc(0);
        }
        if v >= Version::R2018 {
            a.data.rc(1);
        }
        a.text("TAG");
        a.data.bs(0).rc(0);
        objs.push(a);
        let d = drawing(&sections(v, &objs, &[]))?;
        assert_eq!(d.skipped.get(DAMAGED), Some(&1), "{v:?}: {:?}", d.skipped);
        assert!(
            d.damaged.iter().any(|m| m.contains("ends too early")),
            "{v:?}: {:?}",
            d.damaged
        );
    }
    Ok(())
}

#[test]
fn a_hatch_listing_too_much_is_a_damaged_object() -> TestResult {
    let v = Version::R2010;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        hatch_obj(v, 0x300, "SOLID", true, 50_000, |_| {}),
        hatch_obj(v, 0x301, "SOLID", true, 1, |d| {
            // An edge of unknown type.
            d.bl(1).bl(1).rc(9);
        }),
        hatch_obj(v, 0x302, "SOLID", true, 1, |d| {
            // A point that is not a number.
            d.bl(2)
                .b(false)
                .b(true)
                .bl(3)
                .rd(f64::NAN)
                .rd(0.0)
                .rd(1.0)
                .rd(0.0)
                .rd(0.0)
                .rd(1.0)
                .bl(0);
        }),
        hatch_obj(v, 0x303, "SOLID", true, 1, |d| {
            // One boundary object declared, its handle missing.
            d.bl(2)
                .b(false)
                .b(true)
                .bl(3)
                .rd(0.0)
                .rd(0.0)
                .rd(1.0)
                .rd(0.0)
                .rd(0.0)
                .rd(1.0)
                .bl(1);
        }),
    ];
    let mut objs = objs;
    // A radius that is not a number, and an arc angle too large to be degrees.
    objs.push(hatch_obj(v, 0x304, "SOLID", true, 1, |d| {
        d.bl(1)
            .bl(1)
            .rc(2)
            .rd(0.0)
            .rd(0.0)
            .bd(f64::NAN)
            .bd(0.0)
            .bd(1.0)
            .b(true)
            .bl(0);
    }));
    objs.push(hatch_obj(v, 0x305, "SOLID", true, 1, |d| {
        d.bl(1)
            .bl(1)
            .rc(2)
            .rd(0.0)
            .rd(0.0)
            .bd(1.0)
            .bd(f64::MAX)
            .bd(0.0)
            .b(true)
            .bl(0);
    }));
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d.skipped.get(DAMAGED), Some(&6), "{:?}", d.skipped);
    assert!(
        d.damaged.iter().any(|r| r.contains("boundary value is not a number")),
        "{:?}",
        d.damaged
    );
    assert!(
        d.damaged.iter().any(|r| r.contains("edge angle is too large")),
        "{:?}",
        d.damaged
    );
    assert!(
        d.damaged.iter().any(|r| r.contains("boundary point is not a number")),
        "{:?}",
        d.damaged
    );
    assert!(
        d.damaged.iter().any(|r| r.contains("50000 boundary paths")),
        "{:?}",
        d.damaged
    );
    Ok(())
}

#[test]
fn hatch_edges_of_every_kind_and_a_mirrored_hatch_are_read() -> TestResult {
    use std::f64::consts::PI;
    for v in [Version::R2004, Version::R2010, Version::R2018] {
        // A solid slot: a polyline path with bulges (two half circles joined by lines).
        let slot = hatch_obj(v, 0x300, "SOLID", true, 1, |d| {
            d.bl(2).b(true).b(true).bl(4);
            for (p, bulge) in [
                ((0.0, 0.0), 0.0),
                ((10.0, 0.0), 1.0),
                ((10.0, 4.0), 0.0),
                ((0.0, 4.0), 1.0),
            ] {
                d.rd(p.0).rd(p.1).bd(bulge);
            }
            d.bl(0);
        });
        // An edge path: half an ellipse over (40, 0)..(60, 0), closed by a spline below it,
        // with fit points (and tangents) in 2010.
        let edges = hatch_with(v, 0x301, "ANSI31", false, 1, 1.0, 2, |d| {
            d.bl(1).bl(2);
            d.rc(3).rd(50.0).rd(0.0).rd(10.0).rd(0.0).bd(0.5).bd(0.0).bd(PI).b(true);
            d.rc(4).bl(2).b(false).b(false).bl(6).bl(3);
            for k in [0.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
                d.bd(k);
            }
            for p in [(40.0, 0.0), (50.0, -10.0), (60.0, 0.0)] {
                d.rd(p.0).rd(p.1);
            }
            if v >= Version::R2010 {
                d.bl(2)
                    .rd(40.0)
                    .rd(0.0)
                    .rd(60.0)
                    .rd(0.0)
                    .rd(1.0)
                    .rd(0.0)
                    .rd(1.0)
                    .rd(0.0);
            }
            // Two boundary objects (their handles are in the handle stream).
            d.bl(2);
        });
        // A solid triangle with a downward extrusion: mirrored in x.
        let mirrored = hatch_with(v, 0x302, "SOLID", true, 1, -1.0, 0, |d| {
            d.bl(2).b(false).b(true).bl(3);
            for p in [(100.0, 0.0), (110.0, 0.0), (100.0, 10.0)] {
                d.rd(p.0).rd(p.1);
            }
            d.bl(0);
        });
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            slot,
            edges,
            mirrored,
        ];
        let d = drawing(&sections(v, &objs, &[]))?;
        assert!(d.skipped.is_empty(), "{v:?}: {:?}", d.skipped);
        let polys: Vec<(&Vec<Point>, bool)> = d
            .model
            .iter()
            .filter_map(|s| match s {
                Shape::Polyline { points, filled, .. } => Some((points, *filled)),
                _ => None,
            })
            .collect();
        assert_eq!(polys.len(), 3, "{v:?}: {:?}", d.model);
        let near = |p: &Point, x: f64, y: f64| p.distance(Point::new(x, y)) < 1e-6;
        // The slot's bulges reach 2 past its ends.
        let (slot, filled) = polys[0];
        assert!(
            filled && slot.iter().any(|p| p.x > 11.9) && slot.iter().any(|p| p.x < -1.9),
            "{slot:?}"
        );
        // The ellipse's top (y 5 up) and the spline's bottom (y 5 down), not filled.
        let (edge, filled) = polys[1];
        assert!(!filled, "{v:?}");
        assert!(edge.iter().any(|p| near(p, 50.0, -5.0)), "{edge:?}");
        assert!(edge.iter().any(|p| near(p, 50.0, 5.0)), "{edge:?}");
        // The mirrored triangle lies at negative x.
        let (tri, _) = polys[2];
        assert!(tri.iter().all(|p| p.x <= -100.0 + 1e-9), "{tri:?}");
    }
    Ok(())
}

#[test]
fn a_hatch_loop_that_overflows_is_left_out_and_counted() -> TestResult {
    // An arc of a finite but huge radius: its points overflow to infinity when sampled.
    let v = Version::R2010;
    let huge = hatch_obj(v, 0x300, "SOLID", true, 1, |d| {
        d.bl(1)
            .bl(1)
            .rc(2)
            .rd(f64::MAX)
            .rd(0.0)
            .bd(f64::MAX)
            .bd(0.0)
            .bd(1.0)
            .b(true)
            .bl(0);
    });
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        huge,
    ];
    let d = drawing(&sections(v, &objs, &[]))?;
    assert!(d.model.is_empty(), "{:?}", d.model);
    assert!(d.skipped.keys().any(|k| k.contains("not numbers")), "{:?}", d.skipped);
    Ok(())
}

#[test]
fn far_apart_arc_angles_still_make_an_arc_and_a_lost_hole_unfills() -> TestResult {
    let v = Version::R2010;
    // A half circle whose angles are 2e306 apart in radians (both finite in degrees), closed by
    // a line: the arc is still drawn.
    let far = hatch_obj(v, 0x300, "ANSI31", false, 1, |d| {
        d.bl(1).bl(2);
        d.rc(1).rd(20.0).rd(0.0).rd(30.0).rd(0.0);
        d.rc(2).rd(25.0).rd(0.0).bd(5.0).bd(-2e306).bd(2e306).b(true);
        d.bl(0);
    });
    // A solid square with a hole whose arc overflows: drawn as outlines, not filled over.
    let lost = hatch_obj(v, 0x301, "SOLID", true, 2, |d| {
        d.bl(2).b(false).b(true).bl(4);
        for p in [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)] {
            d.rd(p.0).rd(p.1);
        }
        d.bl(0);
        d.bl(1)
            .bl(1)
            .rc(2)
            .rd(f64::MAX)
            .rd(0.0)
            .bd(f64::MAX)
            .bd(0.0)
            .bd(1.0)
            .b(true)
            .bl(0);
    });
    // Each end is a number of degrees, their difference is not.
    let (d0, d1) = ((-2e306f64).to_degrees(), 2e306f64.to_degrees());
    assert!(d0.is_finite() && d1.is_finite() && !(d1 - d0).is_finite());
    // A solid triangle whose bulged side overflows: left out and counted, not filled straight.
    let bulge = hatch_obj(v, 0x302, "SOLID", true, 1, |d| {
        d.bl(2).b(true).b(true).bl(3);
        d.rd(-1e308)
            .rd(0.0)
            .bd(1.0)
            .rd(1e308)
            .rd(0.0)
            .bd(0.0)
            .rd(0.0)
            .rd(1.0)
            .bd(0.0);
        d.bl(0);
    });
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        far,
        lost,
        bulge,
    ];
    let d = drawing(&sections(v, &objs, &[]))?;
    let polys: Vec<(usize, bool)> = d
        .model
        .iter()
        .filter_map(|s| match s {
            Shape::Polyline { points, filled, .. } => Some((points.len(), *filled)),
            _ => None,
        })
        .collect();
    assert!(polys.iter().any(|(n, _)| *n > 5), "the arc is drawn: {polys:?}");
    assert!(polys.contains(&(4, false)), "the square is not filled: {polys:?}");
    assert!(
        !polys.contains(&(3, true)),
        "the bulged triangle is not filled straight: {polys:?}"
    );
    assert_eq!(
        d.skipped.get("HATCH with a boundary that is not numbers"),
        Some(&2),
        "{:?}",
        d.skipped
    );
    Ok(())
}

#[test]
fn ellipses_and_splines_are_drawn() -> TestResult {
    use std::f64::consts::PI;
    for v in [Version::R2004, Version::R2010, Version::R2018] {
        let model = head(None);
        // Half an ellipse 20 wide and 10 high round (50, 0): from (60, 0) over the top to (40, 0).
        let mut el = entity(v, 35, 0x300, model);
        el.data
            .bd3((50.0, 0.0, 0.0))
            .bd3((10.0, 0.0, 0.0))
            .bd3((0.0, 0.0, 1.0))
            .bd(0.5)
            .bd(0.0)
            .bd(PI);
        // A quadratic spline by control points: (0, 0), (5, 10), (10, 0).
        let mut sp = entity(v, 36, 0x301, model);
        sp.data
            .scenario(v, 1)
            .bl(2)
            .b(false)
            .b(false)
            .b(false)
            .bd(1e-7)
            .bd(1e-7)
            .bl(6)
            .bl(3)
            .b(false);
        for k in [0.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
            sp.data.bd(k);
        }
        for p in [(0.0, 0.0, 0.0), (5.0, 10.0, 0.0), (10.0, 0.0, 0.0)] {
            sp.data.bd3(p);
        }
        // A spline by fit points only.
        let mut fit = entity(v, 36, 0x302, model);
        fit.data
            .scenario(v, 2)
            .bl(3)
            .bd(1e-7)
            .bd3((1.0, 0.0, 0.0))
            .bd3((1.0, 0.0, 0.0))
            .bl(3);
        for p in [(0.0, 20.0, 0.0), (5.0, 25.0, 0.0), (10.0, 20.0, 0.0)] {
            fit.data.bd3(p);
        }
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            el,
            sp,
            fit,
        ];
        let d = drawing(&sections(v, &objs, &[]))?;
        assert!(d.skipped.is_empty(), "{v:?}: {:?}", d.skipped);
        let lines: Vec<&Vec<Point>> = d
            .model
            .iter()
            .filter_map(|s| match s {
                Shape::Polyline { points, .. } => Some(points),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 3, "{:?}", d.model);
        let near = |a: &Point, x: f64, y: f64| a.distance(Point::new(x, y)) < 1e-6;
        // The ellipse: ends on the axis, its top 5 above (y flipped on a page).
        let el = lines[0];
        assert!(
            near(&el[0], 60.0, 0.0) && el.last().is_some_and(|p| near(p, 40.0, 0.0)),
            "{el:?}"
        );
        assert!(el.iter().any(|p| near(p, 50.0, -5.0)), "{el:?}");
        // The spline: from (0, 0) to (10, 0), its middle at height 5.
        let sp = lines[1];
        assert!(
            near(&sp[0], 0.0, 0.0) && sp.last().is_some_and(|p| near(p, 10.0, 0.0)),
            "{sp:?}"
        );
        assert!(sp.iter().any(|p| near(p, 5.0, -5.0)), "{sp:?}");
        // The fit-point spline goes through its fit points.
        assert!(lines[2].iter().any(|p| near(p, 5.0, -25.0)), "{:?}", lines[2]);
    }
    Ok(())
}

#[test]
fn a_spline_listing_too_much_is_a_damaged_object() -> TestResult {
    let v = Version::R2010;
    let mut sp = entity(v, 36, 0x301, head(None));
    sp.data
        .scenario(v, 1)
        .bl(3)
        .b(false)
        .b(false)
        .b(false)
        .bd(0.0)
        .bd(0.0)
        .bl(50_000)
        .bl(4)
        .b(false);
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        sp,
    ];
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d.skipped.get(DAMAGED), Some(&1), "{:?}", d.skipped);
    assert!(d.damaged.iter().any(|r| r.contains("50000 knots")), "{:?}", d.damaged);
    Ok(())
}

#[test]
fn a_weighted_closed_spline_a_flipped_ellipse_and_an_unknown_scenario() -> TestResult {
    use std::f64::consts::PI;
    for v in [Version::R2004, Version::R2010, Version::R2018] {
        let model = head(None);
        // A quarter ellipse round (0, 0) with a downward extrusion: its axis is in world
        // coordinates, so it runs the other way, from (10, 0) down to (0, -10).
        let mut el = entity(v, 35, 0x300, model);
        el.data
            .bd3((0.0, 0.0, 0.0))
            .bd3((10.0, 0.0, 0.0))
            .bd3((0.0, 0.0, -1.0))
            .bd(1.0)
            .bd(0.0)
            .bd(PI / 2.0);
        // A rational quadratic: the middle weight pulls the curve towards (5, 10); closed.
        let weighted = |h: u64, w: f64| {
            let mut sp = entity(v, 36, h, model);
            sp.data
                .scenario(v, 1)
                .bl(2)
                .b(true)
                .b(true)
                .b(false)
                .bd(1e-7)
                .bd(1e-7)
                .bl(6)
                .bl(3)
                .b(true);
            for k in [0.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
                sp.data.bd(k);
            }
            for (p, wt) in [((0.0, 0.0, 0.0), 1.0), ((5.0, 10.0, 0.0), w), ((10.0, 0.0, 0.0), 1.0)] {
                sp.data.bd3(p).bd(wt);
            }
            sp
        };
        // A spline with scenario 3: damaged.
        let mut odd = entity(v, 36, 0x303, model);
        odd.data.scenario(v, 3).bl(2);
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            el,
            weighted(0x301, 1.0),
            weighted(0x302, 4.0),
            odd,
        ];
        let d = drawing(&sections(v, &objs, &[]))?;
        assert_eq!(d.skipped.get(DAMAGED), Some(&1), "{v:?}: {:?}", d.skipped);
        assert!(
            d.damaged.iter().any(|r| r.contains("unknown scenario 3")),
            "{:?}",
            d.damaged
        );
        let polys: Vec<(&Vec<Point>, bool)> = d
            .model
            .iter()
            .filter_map(|s| match s {
                Shape::Polyline { points, closed, .. } => Some((points, *closed)),
                _ => None,
            })
            .collect();
        assert_eq!(polys.len(), 3, "{:?}", d.model);
        let (el, _) = polys[0];
        assert!(
            el.iter().all(|p| p.y >= -1e-9) && el.iter().any(|p| p.y > 9.9),
            "{el:?}"
        );
        // Closed, and the heavier middle weight lifts the middle (page y is down).
        let top = |pts: &Vec<Point>| pts.iter().map(|p| -p.y).fold(f64::MIN, f64::max);
        let ((plain, closed), (heavy, _)) = (polys[1], polys[2]);
        assert!(closed, "{v:?}");
        assert!(top(heavy) > top(plain) + 1.0, "{} vs {}", top(heavy), top(plain));
    }
    Ok(())
}

#[test]
fn an_ellipse_not_a_number_and_a_spline_short_of_knots_are_damaged() -> TestResult {
    let v = Version::R2010;
    let model = head(None);
    let mut el = entity(v, 35, 0x300, model);
    el.data
        .bd3((0.0, 0.0, 0.0))
        .bd3((10.0, 0.0, 0.0))
        .bd3((0.0, 0.0, 1.0))
        .bd(f64::NAN)
        .bd(0.0)
        .bd(1.0);
    let mut sp = entity(v, 36, 0x301, model);
    sp.data
        .scenario(v, 1)
        .bl(2)
        .b(false)
        .b(false)
        .b(false)
        .bd(0.0)
        .bd(0.0)
        .bl(3)
        .bl(3)
        .b(false);
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        el,
        sp,
    ];
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d.skipped.get(DAMAGED), Some(&2), "{:?}", d.skipped);
    assert!(d.damaged.iter().any(|r| r.contains("not a number")), "{:?}", d.damaged);
    assert!(
        d.damaged.iter().any(|r| r.contains("3 knots for 3 control points")),
        "{:?}",
        d.damaged
    );
    // A z that is not a number, 4 knots for a non-periodic quadratic over 3 points, and
    // decreasing knots: each damaged.
    let mut el = entity(v, 35, 0x302, model);
    el.data
        .bd3((0.0, 0.0, f64::NAN))
        .bd3((10.0, 0.0, 0.0))
        .bd3((0.0, 0.0, 1.0))
        .bd(0.5)
        .bd(0.0)
        .bd(1.0);
    let spline = |h: u64, knots: &[f64]| control_spline(v, h, 2, knots);
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        el,
        spline(0x303, &[0.0, 0.0, 1.0, 1.0]),
        spline(0x304, &[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]),
        spline(0x305, &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
        spline(0x306, &[0.0, 0.0, 0.0, f64::NAN, 1.0, 1.0]),
    ];
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d.skipped.get(DAMAGED), Some(&4), "{:?}", d.skipped);
    assert!(
        d.damaged.iter().any(|r| r.starts_with("object 306")),
        "the NaN knot: {:?}",
        d.damaged
    );
    assert!(d.damaged.iter().any(|r| r.contains("4 knots for 3")), "{:?}", d.damaged);
    assert!(
        d.damaged.iter().any(|r| r.contains("not numbers in order")),
        "{:?}",
        d.damaged
    );
    assert_eq!(d.model.len(), 1, "the good quadratic is drawn: {:?}", d.model);
    Ok(())
}

#[test]
fn a_spline_whose_knots_make_no_curve_is_drawn_through_its_points_and_said() -> TestResult {
    let v = Version::R2010;
    // A cubic over three control points (degree too high), and a quadratic whose knots leave
    // no interval: both pass the count and order checks.
    let spline = |h: u64, degree: u32, knots: &[f64]| control_spline(v, h, degree, knots);
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        spline(0x300, 3, &[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
        spline(0x301, 2, &[0.0, 0.0, 0.0, 0.0, 1.0, 1.0]),
    ];
    let d = drawing(&sections(v, &objs, &[]))?;
    assert_eq!(d.model.len(), 2, "{:?}", d.model);
    assert_eq!(
        d.skipped.get("SPLINE whose knots make no curve"),
        Some(&2),
        "{:?}",
        d.skipped
    );
    Ok(())
}

/// A scenario-1 SPLINE in model space of `degree` with `knots`, over the control points
/// (0, 0), (5, 10), (10, 0), unweighted.
fn control_spline(v: Version, handle: u64, degree: u32, knots: &[f64]) -> Obj {
    let mut sp = entity(v, 36, handle, head(None));
    sp.data
        .scenario(v, 1)
        .bl(degree)
        .b(false)
        .b(false)
        .b(false)
        .bd(0.0)
        .bd(0.0)
        .bl(knots.len() as u32)
        .bl(3)
        .b(false);
    for k in knots {
        sp.data.bd(*k);
    }
    for p in [(0.0, 0.0, 0.0), (5.0, 10.0, 0.0), (10.0, 0.0, 0.0)] {
        sp.data.bd3(p);
    }
    sp
}

#[test]
fn a_fit_spline_with_one_point_is_counted_and_its_tangents_are_kept() -> TestResult {
    let v = Version::R2010;
    let mut one = entity(v, 36, 0x300, head(None));
    one.data
        .scenario(v, 2)
        .bl(3)
        .bd(0.0)
        .bd3((1.0, 0.0, 0.0))
        .bd3((1.0, 0.0, 0.0))
        .bl(1)
        .bd3((5.0, 5.0, 0.0));
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        one,
    ];
    let d = drawing(&sections(v, &objs, &[]))?;
    assert!(d.model.is_empty(), "{:?}", d.model);
    assert_eq!(d.skipped.get("SPLINE with too few points"), Some(&1), "{:?}", d.skipped);
    Ok(())
}

#[test]
fn a_dwg_spline_by_fit_points_is_a_smooth_curve() -> TestResult {
    for v in [Version::R2004, Version::R2010, Version::R2013, Version::R2018] {
        // Scenario 2: fit tolerance, start and end tangents (none given: zero), three fit points.
        let mut fit = entity(v, 36, 0x300, head(None));
        fit.data
            .scenario(v, 2)
            .bl(3)
            .bd(1e-7)
            .bd3((0.0, 0.0, 0.0))
            .bd3((0.0, 0.0, 0.0))
            .bl(3);
        for p in [(0.0, 20.0, 0.0), (5.0, 25.0, 0.0), (10.0, 20.0, 0.0)] {
            fit.data.bd3(p);
        }
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            fit,
        ];
        let d = drawing(&sections(v, &objs, &[]))?;
        let pts = match d.model.first() {
            Some(Shape::Polyline { points, .. }) => points.clone(),
            other => return Err(format!("no polyline: {other:?}").into()),
        };
        // A curve, not two segments: many points, and above the straight segment at x = 2.5
        // (page y is down: above means a smaller y than -22.5).
        assert!(pts.len() > 10, "{v:?}: {pts:?}");
        let quarter = pts
            .iter()
            .min_by(|a, b| (a.x - 2.5).abs().total_cmp(&(b.x - 2.5).abs()));
        assert!(quarter.is_some_and(|q| q.y < -22.5 - 0.3), "{v:?}: {quarter:?}");
        // And it reaches its three fit points (page y down).
        for (x, y) in [(0.0, -20.0), (5.0, -25.0), (10.0, -20.0)] {
            let d = pts.iter().map(|q| (q.x - x).hypot(q.y - y)).fold(f64::MAX, f64::min);
            assert!(d < 0.05, "{v:?}: ({x}, {y}) is {d} away");
        }
    }
    Ok(())
}

#[test]
fn from_2013_a_splines_flags_and_knot_parameter_decide_its_scenario() -> TestResult {
    for v in [Version::R2013, Version::R2018] {
        // Scenario 2 stored, but knot parameter 15 (custom knots): control points.
        let mut by_knots = entity(v, 36, 0x300, head(None));
        by_knots
            .data
            .bl(2)
            .bl(0)
            .bl(15)
            .bl(2)
            .b(false)
            .b(false)
            .b(false)
            .bd(0.0)
            .bd(0.0)
            .bl(6)
            .bl(3)
            .b(false);
        for k in [0.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
            by_knots.data.bd(k);
        }
        for p in [(0.0, 0.0, 0.0), (5.0, 10.0, 0.0), (10.0, 0.0, 0.0)] {
            by_knots.data.bd3(p);
        }
        // Scenario 1 stored, but flag bit 1 (by fit points) and knot parameter 0: fit points.
        let mut by_fit = entity(v, 36, 0x301, head(None));
        by_fit
            .data
            .bl(1)
            .bl(1)
            .bl(0)
            .bl(3)
            .bd(1e-7)
            .bd3((0.0, 0.0, 0.0))
            .bd3((0.0, 0.0, 0.0))
            .bl(3);
        for p in [(0.0, 20.0, 0.0), (5.0, 25.0, 0.0), (10.0, 20.0, 0.0)] {
            by_fit.data.bd3(p);
        }
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            by_knots,
            by_fit,
        ];
        let d = drawing(&sections(v, &objs, &[]))?;
        assert!(d.damaged.is_empty(), "{v:?}: {:?}", d.damaged);
        assert_eq!(d.model.len(), 2, "{v:?}: {:?}", d.model);
        let reaches = |x: f64, y: f64| {
            d.model.iter().any(|s| match s {
                Shape::Polyline { points, .. } => points.iter().any(|q| (q.x - x).hypot(q.y - y) < 0.05),
                _ => false,
            })
        };
        // The control-point curve starts at (0, 0); the fit-point curve passes (5, 25).
        assert!(reaches(0.0, 0.0), "{v:?}: {:?}", d.model);
        assert!(reaches(5.0, -25.0), "{v:?}: {:?}", d.model);
    }
    Ok(())
}

/// An `AcDb:Classes` section listing `classes` (number, DXF name, entity?) as AutoCAD 2004,
/// 2007 or 2010 (maintenance release 6) writes it.
fn classes_section(v: Version, classes: &[(u16, &str, bool)]) -> Vec<u8> {
    // One class per entry, so an entry listed twice is read twice.
    let max = classes
        .iter()
        .map(|c| c.0)
        .max()
        .unwrap_or(499)
        .max(499 + classes.len() as u16);
    classes_section_max(v, max, classes)
}

/// [`classes_section`] declaring `max` as the highest class number.
fn classes_section_max(v: Version, max: u16, classes: &[(u16, &str, bool)]) -> Vec<u8> {
    let classes: Vec<_> = classes
        .iter()
        .map(|&(number, name, entity)| (number, name, if entity { 0x1F2 } else { 0x1F3 }))
        .collect();
    classes_section_ids(v, max, &classes)
}

/// [`classes_section_max`] with each class's item class id given as written.
fn classes_section_ids(v: Version, max: u16, classes: &[(u16, &str, u16)]) -> Vec<u8> {
    classes_section_layout(v, true, max, classes)
}

/// [`classes_section_ids`] with or without the second size field a 2010 or 2013 drawing of
/// maintenance release 4 and later (and every 2018 drawing) has (`second`; ignored for 2004 and
/// 2007, which never have it).
fn classes_section_layout(v: Version, second: bool, max: u16, classes: &[(u16, &str, u16)]) -> Vec<u8> {
    let second = second && v >= Version::R2010;
    classes_section_raw(v, second, max, classes)
}

/// [`classes_section_layout`] writing the second size field whenever `second` is set, also for
/// 2007 (not ignored for 2004).
fn classes_section_raw(v: Version, second: bool, max: u16, classes: &[(u16, &str, u16)]) -> Vec<u8> {
    const SENTINEL: [u8; 16] = [
        0x8D, 0xA1, 0xC4, 0xB8, 0xC4, 0xA9, 0xF8, 0xC5, 0xC0, 0xDC, 0xF4, 0x5F, 0xE7, 0xCF, 0xB6, 0x8A,
    ];
    let mut data = W::default();
    let mut strings = W::default();
    data.bs(max).rc(0).rc(0).b(true);
    for &(number, name, id) in classes {
        data.bs(number).bs(0);
        match v {
            Version::R2004 => {
                data.t8("ObjectDBX Classes").t8("AcDbThing").t8(name);
            }
            _ => {
                strings.tu("ObjectDBX Classes").tu("AcDbThing").tu(name);
            }
        }
        data.b(false).bs(id);
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
        _ => {
            // Data, texts, their size and the flag; the flag sits `bitsize` after bit 191, or
            // after bit 159 without the second size.
            let mut body = W::default();
            body.append(&data).append(&strings);
            body.rs(strings.0.len() as u16).b(true);
            let head = if second { 28 } else { 24 };
            let flag_at = head * 8 + body.0.len() - 1;
            // As real files: the size covers the bit size field, the data and the texts.
            out.rl(body.bytes().len() as u32 + 4);
            if second {
                out.rl(0);
            }
            out.rl((flag_at - (head * 8 - 33)) as u32).append(&body);
        }
    }
    // A CRC and the closing sentinel after the class data area.
    let mut bytes = out.bytes();
    bytes.extend([0, 0]);
    bytes.extend(SENTINEL.iter().map(|b| !b));
    bytes
}

/// `sections` plus a class section, maintenance release 6.
fn sections_with_classes(v: Version, objs: &[Obj], classes: Vec<u8>) -> Sections {
    sections_with_classes_maint(v, objs, classes, 6)
}

/// [`sections_with_classes`] of maintenance release `maint`.
fn sections_with_classes_maint(v: Version, objs: &[Obj], classes: Vec<u8>, maint: u8) -> Sections {
    let mut s = sections(v, objs, &[]);
    s.sections.push(Section {
        name: "AcDb:Classes".into(),
        id: 3,
        data: classes,
    });
    s.maint = maint;
    s
}

/// A 2004 drawing with a block named "Реле" holding the text "Пуск", both in Windows-1251.
fn cyrillic_2004(codepage: u16) -> Sections {
    let v = Version::R2004;
    let mut block = Obj::new(v, BLOCK_HEADER, 0x100);
    block.data.bs(0).bl(0).b(true).t8_bytes(&[0xD0, 0xE5, 0xEB, 0xE5]);
    let block = block_fields(block, (0.0, 0.0), false);
    let mut text = entity(v, 1, 0x200, head(Some(0x100)));
    text.data
        .rc(0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x40 | 0x80)
        .rd(0.0)
        .rd(0.0)
        .be((0.0, 0.0, 1.0))
        .bt(0.0)
        .rd(2.5);
    text.data.t8_bytes(&[0xCF, 0xF3, 0xF1, 0xEA]);
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        block,
        text,
    ];
    let mut s = sections(v, &objs, &[]);
    s.codepage = codepage;
    s
}

#[test]
fn a_2010_class_section_of_release_3_has_no_second_size() -> TestResult {
    // Releases 0 to 3: the bit size follows the first size, the flag sits `bitsize` after bit 159.
    let v = Version::R2010;
    let ids = [(500, "WIPEOUT", 0x1F2), (501, "ACDBDICTIONARYWDFLT", 0x1F3)];
    let mut wipeout = entity(v, 500, 0x300, head(None));
    wipeout.data.bd(0.0);
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        wipeout,
        Obj::new(v, 501, 0x301),
    ];
    for maint in [0, 3] {
        let classes = classes_section_layout(v, false, 501, &ids);
        let d = drawing(&sections_with_classes_maint(v, &objs, classes, maint))?;
        assert_eq!(
            d.skipped.get("WIPEOUT"),
            Some(&1),
            "{maint}: {:?} {:?}",
            d.skipped,
            d.damaged
        );
        assert_eq!(d.skipped.len(), 1, "{maint}: {:?}", d.skipped);
        assert!(d.damaged.is_empty(), "{maint}: {:?}", d.damaged);
    }
    // The same section read as release 4 looks for a second size that is not there.
    let classes = classes_section_layout(v, false, 501, &ids);
    let d = drawing(&sections_with_classes_maint(v, &objs, classes, 4))?;
    assert_eq!(d.skipped.get(CLASSES_DAMAGED), Some(&1), "{:?}", d.skipped);
    Ok(())
}

#[test]
fn a_2013_class_section_follows_the_release_and_2018_always_has_the_second_size() -> TestResult {
    let ids = [(500, "WIPEOUT", 0x1F2), (501, "ACDBDICTIONARYWDFLT", 0x1F3)];
    // (version, maintenance release, second size written)
    for (v, maint, second) in [
        (Version::R2013, 0, false),
        (Version::R2013, 3, false),
        (Version::R2013, 4, true),
        (Version::R2018, 0, true),
        (Version::R2018, 6, true),
    ] {
        let mut wipeout = entity(v, 500, 0x300, head(None));
        wipeout.data.bd(0.0);
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            wipeout,
            Obj::new(v, 501, 0x301),
        ];
        let classes = classes_section_raw(v, second, 501, &ids);
        let d = drawing(&sections_with_classes_maint(v, &objs, classes, maint))?;
        assert_eq!(
            d.skipped.get("WIPEOUT"),
            Some(&1),
            "{v:?} {maint}: {:?} {:?}",
            d.skipped,
            d.damaged
        );
        assert_eq!(d.skipped.len(), 1, "{v:?} {maint}: {:?}", d.skipped);
        assert!(d.damaged.is_empty(), "{v:?} {maint}: {:?}", d.damaged);
    }
    // A 2018 section without the second size is damage, whatever the release.
    let v = Version::R2018;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
    ];
    let classes = classes_section_raw(v, false, 501, &ids);
    let d = drawing(&sections_with_classes_maint(v, &objs, classes, 0))?;
    assert_eq!(d.skipped.get(CLASSES_DAMAGED), Some(&1), "{:?}", d.skipped);
    Ok(())
}

#[test]
fn a_2007_class_section_never_has_the_second_size() -> TestResult {
    // 2007 names its classes in a string stream as 2010 does, but whatever its maintenance
    // release, the bit size follows the first size: a second size is read as damage.
    let v = Version::R2007;
    let ids = [(500, "WIPEOUT", 0x1F2), (501, "ACDBDICTIONARYWDFLT", 0x1F3)];
    let mut wipeout = entity(v, 500, 0x300, head(None));
    wipeout.data.bd(0.0);
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        wipeout,
        Obj::new(v, 501, 0x301),
    ];
    for maint in [0, 3, 4, 6] {
        let classes = classes_section_raw(v, false, 501, &ids);
        let d = drawing(&sections_with_classes_maint(v, &objs, classes, maint))?;
        assert_eq!(
            d.skipped.get("WIPEOUT"),
            Some(&1),
            "{maint}: {:?} {:?}",
            d.skipped,
            d.damaged
        );
        assert_eq!(d.skipped.len(), 1, "{maint}: {:?}", d.skipped);
        assert!(d.damaged.is_empty(), "{maint}: {:?}", d.damaged);
    }
    let classes = classes_section_raw(v, true, 501, &ids);
    let d = drawing(&sections_with_classes_maint(v, &objs, classes, 6))?;
    assert_eq!(d.skipped.get(CLASSES_DAMAGED), Some(&1), "{:?}", d.skipped);
    Ok(())
}

#[test]
fn custom_entities_are_counted_by_their_class_name_and_custom_objects_not() -> TestResult {
    for v in [
        Version::R2004,
        Version::R2007,
        Version::R2010,
        Version::R2013,
        Version::R2018,
    ] {
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
    for v in [
        Version::R2004,
        Version::R2007,
        Version::R2010,
        Version::R2013,
        Version::R2018,
    ] {
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

#[test]
fn twice_listed_classes_and_odd_names_are_handled() -> TestResult {
    let v = Version::R2010;
    let objs = |kind: u16| {
        let mut e = entity(v, kind, 0x300, head(None));
        e.data.bd(0.0);
        vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            e,
        ]
    };
    // Class 500 listed twice: the section is damaged.
    let twice = classes_section(v, &[(500, "WIPEOUT", true), (500, "IMAGE", false)]);
    let d = drawing(&sections_with_classes(v, &objs(500), twice))?;
    assert!(d.damaged.iter().any(|r| r.contains("listed twice")), "{:?}", d.damaged);
    // A name that is no DXF name (it could pass for a counter) is counted by type number.
    let odd = classes_section(v, &[(500, "damaged objects", true)]);
    let d = drawing(&sections_with_classes(v, &objs(500), odd))?;
    assert_eq!(d.skipped.get("custom entity type 500"), Some(&1), "{:?}", d.skipped);
    assert!(!d.skipped.contains_key(DAMAGED), "{:?}", d.skipped);
    Ok(())
}

#[test]
fn the_maintenance_release_comes_from_the_file_header() -> TestResult {
    // A whole 2010 file whose header says release 6, so the class section has its second size.
    let v = Version::R2010;
    let mut wipeout = entity(v, 500, 0x300, head(None));
    wipeout.data.bd(0.0);
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        wipeout,
    ];
    let (map, objects) = sections_bytes(&objs, &[]);
    let section = |name: &'static str, data: Vec<u8>| TestSection {
        name,
        data,
        ..Default::default()
    };
    let mut file = write_dwg(
        b"AC1024",
        &[
            section("AcDb:Handles", map),
            section("AcDb:AcDbObjects", objects),
            section("AcDb:Classes", classes_section(v, &[(500, "WIPEOUT", true)])),
        ],
    );
    file[0x0B] = 6;
    let read = read_sections(&file)?;
    assert_eq!(read.maint, 6);
    let d = drawing(&read)?;
    assert_eq!(d.skipped.get("WIPEOUT"), Some(&1), "{:?} {:?}", d.skipped, d.damaged);
    Ok(())
}

#[test]
fn a_2004_class_section_cut_inside_a_record_or_with_a_wrong_end_is_damaged() -> TestResult {
    let v = Version::R2004;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        line(v, 0x300, head(None), (0.0, 0.0), (1.0, 0.0)),
    ];
    let whole = classes_section(v, &[(500, "WIPEOUT", true), (501, "IMAGE", true)]);
    // The data area follows the 16-byte sentinel and the 4-byte size; then a CRC and the
    // closing sentinel.
    let size = u32::from_le_bytes(whole[16..20].try_into()?) as usize;
    let trailer = whole[20 + size..].to_vec();
    // The declared body ends 4 bytes early (inside the second record), the trailer is whole.
    let mut cut = whole[..16].to_vec();
    cut.extend(((size - 4) as u32).to_le_bytes());
    cut.extend(&whole[20..20 + size - 4]);
    cut.extend(&trailer);
    // The same length, a closing sentinel with one byte wrong.
    let mut wrong = whole.clone();
    let last = wrong.len() - 1;
    wrong[last] ^= 0xFF;
    // A class numbered past the declared highest one.
    let over = classes_section_max(v, 500, &[(501, "WIPEOUT", true)]);
    for bad in [cut, wrong, over] {
        let d = drawing(&sections_with_classes(v, &objs, bad))?;
        assert_eq!(d.model.len(), 1, "the drawing is still read");
        assert_eq!(
            d.skipped.get(CLASSES_DAMAGED),
            Some(&1),
            "{:?} {:?}",
            d.skipped,
            d.damaged
        );
    }
    Ok(())
}

#[test]
fn a_2010_class_record_that_cannot_be_read_and_a_cut_trailer_are_damage() -> TestResult {
    let v = Version::R2010;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        line(v, 0x300, head(None), (0.0, 0.0), (1.0, 0.0)),
    ];
    // Sizes, texts and trailer consistent, but the header declares a second class that is not
    // there: reading its record fails.
    let short = classes_section_max(v, 501, &[(500, "WIPEOUT", true)]);
    // The trailer cut off.
    let whole = classes_section(v, &[(500, "WIPEOUT", true)]);
    let cut = whole[..whole.len() - 10].to_vec();
    for bad in [short, cut] {
        let d = drawing(&sections_with_classes(v, &objs, bad))?;
        assert_eq!(d.model.len(), 1, "the drawing is still read");
        assert_eq!(
            d.skipped.get(CLASSES_DAMAGED),
            Some(&1),
            "{:?} {:?}",
            d.skipped,
            d.damaged
        );
    }
    Ok(())
}

#[test]
fn a_class_with_an_unknown_item_class_id_is_damage() -> TestResult {
    for v in [
        Version::R2004,
        Version::R2007,
        Version::R2010,
        Version::R2013,
        Version::R2018,
    ] {
        let objs = vec![
            entry(v, LAYER, LAYER_H, "0"),
            entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
            line(v, 0x300, head(None), (0.0, 0.0), (1.0, 0.0)),
        ];
        let zero = classes_section_ids(v, 500, &[(500, "WIPEOUT", 0)]);
        let d = drawing(&sections_with_classes(v, &objs, zero))?;
        assert_eq!(d.model.len(), 1, "{v:?}: the drawing is still read");
        assert!(
            d.damaged
                .iter()
                .any(|r| r.contains("class 500 has item class ID 0x0, not 0x1F2 or 0x1F3")),
            "{v:?}: {:?}",
            d.damaged
        );
        assert_eq!(d.skipped.get(CLASSES_DAMAGED), Some(&1), "{v:?}: {:?}", d.skipped);
    }
    Ok(())
}

#[test]
fn a_2010_class_section_with_its_closing_sentinel_4_bytes_early_is_refused() -> TestResult {
    let v = Version::R2010;
    let objs = vec![
        entry(v, LAYER, LAYER_H, "0"),
        entry(v, BLOCK_HEADER, MODEL_H, "*Model_Space"),
        line(v, 0x300, head(None), (0.0, 0.0), (1.0, 0.0)),
    ];
    // Release 6: the CRC (2 bytes) and the closing sentinel (16) end the section. Move them 4
    // bytes earlier, over the end of the class data area, once with the section 4 bytes shorter
    // and once padded back to its length.
    let whole = classes_section(v, &[(500, "WIPEOUT", true)]);
    let trailer_at = whole.len() - 18;
    let mut short = whole[..trailer_at - 4].to_vec();
    short.extend(&whole[trailer_at..]);
    let mut padded = short.clone();
    padded.extend([0; 4]);
    for bad in [short, padded] {
        let d = drawing(&sections_with_classes(v, &objs, bad))?;
        assert_eq!(d.model.len(), 1, "the drawing is still read");
        assert!(
            d.damaged
                .iter()
                .any(|r| r.contains("AcDb:Classes") && r.contains("closing sentinel")),
            "{:?}",
            d.damaged
        );
        assert_eq!(d.skipped.get(CLASSES_DAMAGED), Some(&1), "{:?}", d.skipped);
    }
    Ok(())
}

#[test]
fn a_2004_drawing_in_windows_1251_reads_cyrillic() -> TestResult {
    let d = drawing(&cyrillic_2004(29))?;
    assert!(d.blocks.contains_key("Реле"), "{:?}", d.blocks.keys());
    assert_eq!(texts(&d.blocks["Реле"]), ["Пуск"]);
    assert!(d.skipped.is_empty(), "{:?}", d.skipped);
    // An unknown code page still reads, as Windows-1252, and is named in a warning.
    let d = drawing(&cyrillic_2004(31))?;
    assert_eq!(d.blocks.len(), 1);
    // Its text is kept, decoded the legacy way (not UTF-8, so Windows-1252).
    assert_eq!(texts(&d.blocks["Ðåëå"]), ["Ïóñê"]);
    assert!(d.skipped.is_empty(), "not an entity count: {:?}", d.skipped);
    assert_eq!(
        d.warnings.iter().filter(|w| w.contains("code page 31")).count(),
        1,
        "one warning per drawing: {:?}",
        d.warnings
    );
    Ok(())
}
