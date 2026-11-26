//! PDF schematics as a trace layer: vector paths and text (simple fonts, and composite fonts
//! through their ToUnicode map, as Eplan and most CAD exports embed them) from each page become page
//! graphics in millimetres, to redraw over or keep as reference.
//!
//! This is a small, forgiving PDF reader: it finds objects by scanning (so damaged cross-reference
//! tables do not matter), expands compressed object streams, inflates Flate streams, follows the
//! page tree and form XObjects, and interprets the path-construction and painting operators.
//! Scanned pages yield their largest picture (`TracePage::scan`), which the caller can put
//! under the page to trace over.

use std::collections::BTreeMap;

use wc_core::geom::{Align, Point, Shape};

use crate::pdf_codecs;
use crate::raster::{Orientation, Raster, RasterFormat, samples_to_png, upright};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
enum Obj {
    Null,
    Bool(bool),
    Num(f64),
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(BTreeMap<String, Obj>),
    Ref(u32),
    Stream(BTreeMap<String, Obj>, Vec<u8>),
    Op(String),
}

impl Obj {
    fn dict(&self) -> Option<&BTreeMap<String, Obj>> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            Obj::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn name(&self) -> Option<&str> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
}

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
}

fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

impl<'a> Lexer<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, i: 0 }
    }

    fn skip_ws(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if is_ws(c) {
                self.i += 1;
            } else if c == b'%' {
                while self.i < self.b.len() && self.b[self.i] != b'\n' && self.b[self.i] != b'\r' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> &'a [u8] {
        let s = self.i;
        while self.i < self.b.len() && !is_ws(self.b[self.i]) && !is_delim(self.b[self.i]) {
            self.i += 1;
        }
        &self.b[s..self.i]
    }

    /// One object or operator. `depth` stops runaway nesting.
    fn next(&mut self, depth: usize) -> Option<Obj> {
        if depth > 64 {
            return None;
        }
        self.skip_ws();
        let c = *self.b.get(self.i)?;
        match c {
            b'/' => {
                self.i += 1;
                let w = self.word();
                let mut name = String::new();
                let mut k = 0;
                while k < w.len() {
                    if w[k] == b'#' && k + 2 < w.len() {
                        let hex = std::str::from_utf8(&w[k + 1..k + 3])
                            .ok()
                            .and_then(|h| u8::from_str_radix(h, 16).ok());
                        if let Some(h) = hex {
                            name.push(h as char);
                            k += 3;
                            continue;
                        }
                    }
                    name.push(w[k] as char);
                    k += 1;
                }
                Some(Obj::Name(name))
            }
            b'(' => {
                self.i += 1;
                let mut out = Vec::new();
                let mut level = 1;
                while self.i < self.b.len() {
                    let c = self.b[self.i];
                    self.i += 1;
                    match c {
                        b'\\' => {
                            let Some(&n) = self.b.get(self.i) else { break };
                            self.i += 1;
                            match n {
                                b'n' => out.push(b'\n'),
                                b'r' => out.push(b'\r'),
                                b't' => out.push(b'\t'),
                                b'b' => out.push(8),
                                b'f' => out.push(12),
                                b'0'..=b'7' => {
                                    let mut v = (n - b'0') as u32;
                                    for _ in 0..2 {
                                        match self.b.get(self.i) {
                                            Some(&d @ b'0'..=b'7') => {
                                                v = v * 8 + (d - b'0') as u32;
                                                self.i += 1;
                                            }
                                            _ => break,
                                        }
                                    }
                                    out.push(v as u8);
                                }
                                b'\r' | b'\n' => {}
                                other => out.push(other),
                            }
                        }
                        b'(' => {
                            level += 1;
                            out.push(c);
                        }
                        b')' => {
                            level -= 1;
                            if level == 0 {
                                break;
                            }
                            out.push(c);
                        }
                        _ => out.push(c),
                    }
                }
                Some(Obj::Str(out))
            }
            b'<' if self.b.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                let mut toks = Vec::new();
                loop {
                    self.skip_ws();
                    if self.i >= self.b.len() {
                        break;
                    }
                    if self.b.get(self.i..self.i + 2) == Some(b">>") {
                        self.i += 2;
                        break;
                    }
                    match self.next(depth + 1) {
                        Some(t) => toks.push(t),
                        None => break,
                    }
                    if toks.len() > 1_000_000 {
                        break;
                    }
                }
                let mut d = BTreeMap::new();
                let mut it = fold_refs(toks).into_iter();
                while let Some(k) = it.next() {
                    if let Obj::Name(k) = k {
                        d.insert(k, it.next().unwrap_or(Obj::Null));
                    }
                }
                // A stream follows its dictionary.
                let save = self.i;
                self.skip_ws();
                if self.b[self.i.min(self.b.len())..].starts_with(b"stream") {
                    self.i += 6;
                    if self.b.get(self.i) == Some(&b'\r') {
                        self.i += 1;
                    }
                    if self.b.get(self.i) == Some(&b'\n') {
                        self.i += 1;
                    }
                    let start = self.i;
                    let declared = d.get("Length").and_then(Obj::num).map(|n| n as usize);
                    let end = match declared {
                        Some(n)
                            if start + n <= self.b.len()
                                && self.b[start + n..].windows(9).take(20).any(|w| w == b"endstream") =>
                        {
                            start + n
                        }
                        _ => find(&self.b[start..], b"endstream")
                            .map(|p| start + p)
                            .unwrap_or(self.b.len()),
                    };
                    self.i = end;
                    if let Some(p) = find(&self.b[self.i..], b"endstream") {
                        self.i += p + 9;
                    }
                    let mut data = self.b[start..end].to_vec();
                    if declared.is_none() || end != start + declared.unwrap_or(0) {
                        // Found by searching: drop the one end-of-line that precedes `endstream`.
                        if data.ends_with(b"\r\n") {
                            data.truncate(data.len() - 2);
                        } else if data.ends_with(b"\n") || data.ends_with(b"\r") {
                            data.truncate(data.len() - 1);
                        }
                    }
                    return Some(Obj::Stream(d, data));
                }
                self.i = save;
                Some(Obj::Dict(d))
            }
            b'<' => {
                self.i += 1;
                let mut hex = Vec::new();
                while self.i < self.b.len() && self.b[self.i] != b'>' {
                    if self.b[self.i].is_ascii_hexdigit() {
                        hex.push(self.b[self.i]);
                    }
                    self.i += 1;
                }
                self.i += 1;
                if hex.len() % 2 == 1 {
                    hex.push(b'0');
                }
                let bytes = hex
                    .chunks(2)
                    .filter_map(|p| std::str::from_utf8(p).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
                    .collect();
                Some(Obj::Str(bytes))
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                loop {
                    self.skip_ws();
                    match self.b.get(self.i) {
                        None => break,
                        Some(b']') => {
                            self.i += 1;
                            break;
                        }
                        _ => match self.next(depth + 1) {
                            Some(o) => a.push(o),
                            None => break,
                        },
                    }
                }
                Some(Obj::Array(fold_refs(a)))
            }
            b']' | b'>' | b')' | b'{' | b'}' => {
                self.i += 1;
                Some(Obj::Op((c as char).to_string()))
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.i += 1;
                    return Some(Obj::Op(String::new()));
                }
                let s = String::from_utf8_lossy(w);
                if let Ok(n) = s.parse::<f64>() {
                    return Some(Obj::Num(if n.is_finite() { n } else { 0.0 }));
                }
                Some(match s.as_ref() {
                    "true" => Obj::Bool(true),
                    "false" => Obj::Bool(false),
                    "null" => Obj::Null,
                    _ => Obj::Op(s.into_owned()),
                })
            }
        }
    }
}

/// `12 0 R` inside arrays arrives as three tokens; fold them into references.
fn fold_refs(a: Vec<Obj>) -> Vec<Obj> {
    let mut out: Vec<Obj> = Vec::with_capacity(a.len());
    for o in a {
        if let Obj::Op(op) = &o
            && op == "R"
            && out.len() >= 2
            && let (Some(Obj::Num(n)), Some(Obj::Num(_))) = (out.get(out.len() - 2), out.last())
        {
            let n = *n as u32;
            out.truncate(out.len() - 2);
            out.push(Obj::Ref(n));
            continue;
        }
        out.push(o);
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

struct Doc {
    objs: BTreeMap<u32, Obj>,
}

const MAX_INFLATE: usize = 64 * 1024 * 1024;

/// Undo PDF's run-length compression (`RunLengthDecode`): a length byte `n` below 128 is
/// followed by `n + 1` bytes to copy, one above 128 by a byte to repeat `257 - n` times, and 128
/// ends the data. Fails with the reason for a run that goes past the end of the data, data
/// without its end mark, or output over [`MAX_INFLATE`].
fn run_length_decode(data: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let past_end = || "the RunLength data is damaged (a run goes past the end of the data)".to_owned();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let Some(&n) = data.get(i) else {
            return Err("the RunLength data is damaged (it has no end mark)".into());
        };
        i += 1;
        let more = match n {
            128 => return Ok(out),
            ..128 => usize::from(n) + 1,
            _ => 257 - usize::from(n),
        };
        if out.len() + more > MAX_INFLATE {
            return Err(format!(
                "the RunLength data unpacks to more than {} MiB",
                MAX_INFLATE / (1024 * 1024)
            ));
        }
        if n < 128 {
            out.extend_from_slice(data.get(i..i + more).ok_or_else(past_end)?);
            i += more;
        } else {
            let &b = data.get(i).ok_or_else(past_end)?;
            out.resize(out.len() + more, b);
            i += 1;
        }
    }
}

/// Undo PDF's LZW compression (`LZWDecode`): codes of 9 to 12 bits, most significant bit
/// first, 256 clears the table and 257 ends the data. With `early_change` (the default) the
/// code length grows one code early. Fails with the reason for a code that is not in the table,
/// or for output over [`MAX_INFLATE`].
fn lzw_decode(data: &[u8], early_change: bool) -> std::result::Result<Vec<u8>, String> {
    const CLEAR: usize = 256;
    const END: usize = 257;
    // The 256 single bytes, then the places of the clear and end codes.
    let mut table: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).chain([vec![], vec![]]).collect();
    let mut out = Vec::new();
    let mut prev: Option<Vec<u8>> = None;
    let mut width = 9;
    let (mut acc, mut held) = (0u32, 0u32);
    let mut input = data.iter();
    loop {
        while held < width {
            let Some(&b) = input.next() else {
                return Ok(out);
            };
            acc = (acc << 8) | u32::from(b);
            held += 8;
        }
        held -= width;
        let code = ((acc >> held) & ((1 << width) - 1)) as usize;
        acc &= (1 << held) - 1;
        if code == CLEAR {
            table.truncate(END + 1);
            prev = None;
            width = 9;
            continue;
        }
        if code == END {
            return Ok(out);
        }
        let damaged = || format!("the LZW data is damaged (code {code} is not in the table)");
        let entry = match (table.get(code), &prev) {
            (Some(e), _) => e.clone(),
            (None, Some(p)) if code == table.len() => {
                let mut e = p.clone();
                e.push(*p.first().ok_or_else(damaged)?);
                e
            }
            _ => return Err(damaged()),
        };
        if out.len() + entry.len() > MAX_INFLATE {
            return Err(format!(
                "the LZW data unpacks to more than {} MiB",
                MAX_INFLATE / (1024 * 1024)
            ));
        }
        out.extend_from_slice(&entry);
        if let Some(mut p) = prev.take()
            && table.len() < 4096
        {
            p.push(*entry.first().ok_or_else(damaged)?);
            table.push(p);
        }
        prev = Some(entry);
        let next = table.len() + usize::from(early_change);
        width = match next {
            ..512 => 9,
            512..1024 => 10,
            1024..2048 => 11,
            _ => 12,
        };
    }
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, MAX_INFLATE)
        .ok()
        .or_else(|| miniz_oxide::inflate::decompress_to_vec_with_limit(data, MAX_INFLATE).ok())
}

impl Doc {
    fn parse(bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"%PDF") && find(&bytes[..bytes.len().min(1024)], b"%PDF").is_none() {
            return Err(Error::Format("not a PDF file".into()));
        }
        let mut objs = BTreeMap::new();
        // Scan for "<num> <gen> obj".
        let mut i = 0;
        while let Some(p) = find(&bytes[i..], b" obj") {
            let at = i + p;
            i = at + 4;
            // Walk back over "<num> <gen>".
            let head = &bytes[at.saturating_sub(24)..at];
            let text = String::from_utf8_lossy(head);
            let mut parts = text.split_whitespace().rev();
            let (Some(g), Some(n)) = (parts.next(), parts.next()) else {
                continue;
            };
            let (Ok(_), Ok(n)) = (
                g.parse::<u32>(),
                n.trim_start_matches(|c: char| !c.is_ascii_digit()).parse::<u32>(),
            ) else {
                continue;
            };
            if let Some(o) = parse_object_dict_aware(&bytes[i..]) {
                objs.insert(n, o);
            }
        }
        // Stream lengths given as references are only known now.
        let lengths: BTreeMap<u32, usize> = objs
            .iter()
            .filter_map(|(n, o)| match o {
                Obj::Stream(d, _) => match d.get("Length") {
                    Some(Obj::Ref(r)) => objs.get(r).and_then(Obj::num).map(|l| (*n, l as usize)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        for (n, len) in lengths {
            if let Some(Obj::Stream(_, data)) = objs.get_mut(&n)
                && len <= data.len()
            {
                data.truncate(len);
            }
        }
        // Expand object streams.
        let streams: Vec<Obj> = objs
            .values()
            .filter(|o| matches!(o.dict().and_then(|d| d.get("Type")).and_then(Obj::name), Some("ObjStm")))
            .cloned()
            .collect();
        for s in streams {
            let Obj::Stream(d, data) = &s else { continue };
            let Some(data) = decode_stream(d, data) else { continue };
            let n = d.get("N").and_then(Obj::num).unwrap_or(0.0) as usize;
            let first = d.get("First").and_then(Obj::num).unwrap_or(0.0) as usize;
            let mut lx = Lexer::new(&data);
            let mut index = Vec::new();
            for _ in 0..n.min(100_000) {
                match (lx.next(0), lx.next(0)) {
                    (Some(Obj::Num(num)), Some(Obj::Num(off))) => index.push((num as u32, off as usize)),
                    _ => break,
                }
            }
            for (num, off) in index {
                if let Some(slice) = data.get(first + off..)
                    && let Some(o) = parse_object_dict_aware(slice)
                {
                    objs.entry(num).or_insert(o);
                }
            }
        }
        Ok(Self { objs })
    }

    fn get<'a>(&'a self, o: &'a Obj) -> &'a Obj {
        let mut cur = o;
        for _ in 0..16 {
            match cur {
                Obj::Ref(n) => cur = self.objs.get(n).unwrap_or(&Obj::Null),
                _ => return cur,
            }
        }
        &Obj::Null
    }

    /// Pages in document order with their inherited resources and media box. A rotation set
    /// only on a parent `/Pages` node is put into the page's dictionary.
    fn pages(&self) -> Vec<(BTreeMap<String, Obj>, Obj, [f64; 4])> {
        let root = self.objs.values().find(|o| {
            matches!(
                o.dict().and_then(|d| d.get("Type")).and_then(Obj::name),
                Some("Catalog")
            )
        });
        let mut out = Vec::new();
        if let Some(pages) = root.and_then(|r| r.dict()).and_then(|d| d.get("Pages")) {
            self.walk(self.get(pages), None, None, None, &mut out, 0);
        }
        if out.is_empty() {
            // Damaged tree: fall back to every page object in object order.
            for o in self.objs.values() {
                if let Some(d) = o.dict()
                    && d.get("Type").and_then(Obj::name) == Some("Page")
                {
                    let mb = self.media_box(d.get("MediaBox")).unwrap_or([0.0, 0.0, 595.0, 842.0]);
                    out.push((
                        d.clone(),
                        d.get("Resources").map(|r| self.get(r).clone()).unwrap_or(Obj::Null),
                        mb,
                    ));
                }
            }
        }
        out
    }

    fn media_box(&self, o: Option<&Obj>) -> Option<[f64; 4]> {
        match self.get(o?) {
            Obj::Array(a) if a.len() == 4 => {
                let v: Vec<f64> = a.iter().filter_map(|x| self.get(x).num()).collect();
                (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
            }
            _ => None,
        }
    }

    fn walk(
        &self,
        node: &Obj,
        res: Option<&Obj>,
        mb: Option<[f64; 4]>,
        rotate: Option<f64>,
        out: &mut Vec<(BTreeMap<String, Obj>, Obj, [f64; 4])>,
        depth: usize,
    ) {
        if depth > 32 || out.len() > 10_000 {
            return;
        }
        let Some(d) = node.dict() else { return };
        let res = d.get("Resources").map(|r| self.get(r)).or(res);
        let mb = self.media_box(d.get("MediaBox")).or(mb);
        // Only a number rotates; a missing or null /Rotate keeps what the parent has.
        let rotate = d.get("Rotate").and_then(|r| self.get(r).num()).or(rotate);
        match d.get("Type").and_then(Obj::name) {
            Some("Page") => {
                let mut page = d.clone();
                match rotate {
                    Some(r) => page.insert("Rotate".into(), Obj::Num(r)),
                    None => page.remove("Rotate"),
                };
                out.push((
                    page,
                    res.cloned().unwrap_or(Obj::Null),
                    mb.unwrap_or([0.0, 0.0, 595.0, 842.0]),
                ));
            }
            _ => {
                if let Some(Obj::Array(kids)) = d.get("Kids").map(|k| self.get(k)) {
                    for k in kids {
                        self.walk(self.get(k), res, mb, rotate, out, depth + 1);
                    }
                }
            }
        }
    }

    fn content(&self, page: &BTreeMap<String, Obj>) -> Vec<u8> {
        let mut out = Vec::new();
        let parts: Vec<&Obj> = match page.get("Contents").map(|c| self.get(c)) {
            Some(Obj::Array(a)) => a.iter().map(|x| self.get(x)).collect(),
            Some(o) => vec![o],
            None => vec![],
        };
        for p in parts {
            if let Obj::Stream(d, data) = p
                && let Some(bytes) = decode_stream(d, data)
            {
                out.extend_from_slice(&bytes);
                out.push(b'\n');
            }
        }
        out
    }
}

/// Lex one object.
fn parse_object_dict_aware(b: &[u8]) -> Option<Obj> {
    Lexer::new(b).next(0)
}

fn decode_stream(d: &BTreeMap<String, Obj>, data: &[u8]) -> Option<Vec<u8>> {
    let filters: Vec<String> = match d.get("Filter") {
        None => vec![],
        Some(Obj::Name(n)) => vec![n.clone()],
        Some(Obj::Array(a)) => a.iter().filter_map(|x| x.name().map(str::to_owned)).collect(),
        _ => return None,
    };
    let mut cur = data.to_vec();
    for f in filters {
        cur = match f.as_str() {
            "FlateDecode" | "Fl" => inflate(&cur)?,
            _ => return None,
        };
    }
    Some(cur)
}

#[derive(Clone, Copy)]
struct Ctm([f64; 6]);

/// Whether the current stroke and fill colours are white, which on paper means nothing is drawn.
#[derive(Clone, Copy, Default)]
struct Paint {
    stroke_white: bool,
    fill_white: bool,
}

/// Whether the colour operands at the end of the stack are white: one grey, three RGB or four
/// CMYK numbers. A pattern name or anything else counts as not white.
fn is_white(stack: &[Obj]) -> bool {
    let nums: Vec<f64> = stack.iter().rev().map_while(Obj::num).take(5).collect();
    let near = |v: f64, to: f64| (v - to).abs() < 0.01;
    match nums.as_slice() {
        [g] => near(*g, 1.0),
        [b, g, r] => near(*r, 1.0) && near(*g, 1.0) && near(*b, 1.0),
        [k, y, m, c] => [c, m, y, k].iter().all(|v| near(**v, 0.0)),
        _ => false,
    }
}

impl Ctm {
    const ID: Ctm = Ctm([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    fn pre(&self, m: [f64; 6]) -> Ctm {
        let [a, b, c, d, e, f] = m;
        let [a2, b2, c2, d2, e2, f2] = self.0;
        Ctm([
            a * a2 + b * c2,
            a * b2 + b * d2,
            c * a2 + d * c2,
            c * b2 + d * d2,
            e * a2 + f * c2 + e2,
            e * b2 + f * d2 + f2,
        ])
    }
    fn p(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }
}

/// One imported page.
#[derive(Debug, Default)]
pub struct TracePage {
    /// Page size in millimetres.
    pub size_mm: (f64, f64),
    pub graphics: Vec<Shape>,
    /// Paths that looked like images or fills only.
    pub images: usize,
    /// The largest picture that covers a good part of the page (a scanned sheet), turned
    /// upright, with where it sits on the page in millimetres.
    pub scan: Option<Scan>,
    /// Why a large picture could not be read, such as a compression this reader lacks.
    pub scan_problem: Option<String>,
    /// What is wrong with `scan` where it was read only in part, such as a damaged fax picture
    /// whose rows after the damage are left white.
    pub scan_warning: Option<String>,
}

/// A picture taken from a PDF page.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub raster: Raster,
    pub min: Point,
    pub max: Point,
}

/// A picture must cover at least this share of the page to count as a scanned sheet; smaller
/// ones are logos and stamps.
const SCAN_SHARE: f64 = 0.3;

/// Undo the predictor a filter's parameters name: PNG predictors (10 and up) and the TIFF
/// predictor (2) for 8-bit samples; 1 or none leaves the data as it is.
fn unpredict(data: Vec<u8>, parms: Option<&BTreeMap<String, Obj>>) -> std::result::Result<Vec<u8>, String> {
    let Some(p) = parms else {
        return Ok(data);
    };
    let predictor = p.get("Predictor").and_then(Obj::num).unwrap_or(1.0);
    if predictor < 2.0 {
        return Ok(data);
    }
    let colors = p.get("Colors").and_then(Obj::num).unwrap_or(1.0) as usize;
    let bpc = p.get("BitsPerComponent").and_then(Obj::num).unwrap_or(8.0) as usize;
    let columns = p.get("Columns").and_then(Obj::num).unwrap_or(1.0) as usize;
    if colors == 0 || colors > 4 || bpc == 0 || bpc > 16 || columns == 0 || columns > 1_000_000 {
        return Err("the picture has impossible predictor settings".into());
    }
    if predictor >= 10.0 {
        return png_unpredict(&data, colors, bpc, columns).ok_or_else(|| "the picture data is damaged".into());
    }
    if predictor == 2.0 && bpc == 8 {
        return Ok(tiff_unpredict(data, colors, columns));
    }
    Err(format!(
        "pictures with predictor {predictor} at {bpc} bit(s) are not supported yet"
    ))
}

/// Undo the TIFF predictor for 8-bit samples: each sample was stored as its difference from the
/// same channel of the pixel to its left.
fn tiff_unpredict(mut data: Vec<u8>, colors: usize, columns: usize) -> Vec<u8> {
    for line in data.chunks_mut(colors * columns) {
        for i in colors..line.len() {
            let left = line.get(i - colors).copied().unwrap_or(0);
            if let Some(v) = line.get_mut(i) {
                *v = v.wrapping_add(left);
            }
        }
    }
    data
}

/// Undo PNG predictors (PDF `/Predictor` 10 to 15) on decoded rows.
fn png_unpredict(data: &[u8], colors: usize, bits: usize, columns: usize) -> Option<Vec<u8>> {
    let bpp = (colors * bits).div_ceil(8).max(1);
    let row = (colors.checked_mul(bits)?.checked_mul(columns)?).div_ceil(8);
    let mut out: Vec<u8> = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    for chunk in data.chunks(row + 1) {
        let (&kind, line) = chunk.split_first()?;
        if line.len() < row {
            break;
        }
        let mut cur = line.to_vec();
        for i in 0..row {
            let left = if i >= bpp { cur[i - bpp] } else { 0 };
            let up = prev[i];
            let up_left = if i >= bpp { prev[i - bpp] } else { 0 };
            let add = match kind {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => {
                    let p = i16::from(left) + i16::from(up) - i16::from(up_left);
                    let (pa, pb, pc) = (
                        (p - i16::from(left)).abs(),
                        (p - i16::from(up)).abs(),
                        (p - i16::from(up_left)).abs(),
                    );
                    if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        up_left
                    }
                }
                _ => return None,
            };
            cur[i] = cur[i].wrapping_add(add);
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    Some(out)
}

const MM_PER_PT: f64 = 25.4 / 72.0;

struct Interp<'a> {
    doc: &'a Doc,
    page_h: f64,
    origin: (f64, f64),
    out: TracePage,
    budget: usize,
}

impl Interp<'_> {
    fn to_mm(&self, p: (f64, f64)) -> Point {
        Point::new(
            (p.0 - self.origin.0) * MM_PER_PT,
            (self.page_h - (p.1 - self.origin.1)) * MM_PER_PT,
        )
    }

    /// The text decoding of a font resource, when it needs one: composite (two-byte) fonts and
    /// fonts with a ToUnicode map.
    fn font_map(&self, resources: &Obj, name: &str) -> Option<FontMap> {
        let font = resources
            .dict()
            .and_then(|r| r.get("Font"))
            .map(|f| self.doc.get(f))
            .and_then(Obj::dict)
            .and_then(|f| f.get(name))
            .map(|f| self.doc.get(f))
            .and_then(Obj::dict)?;
        let two_byte = font.get("Subtype").and_then(Obj::name) == Some("Type0");
        let map = match font.get("ToUnicode").map(|t| self.doc.get(t)) {
            Some(Obj::Stream(d, data)) => decode_stream(d, data).map(|c| parse_cmap(&c)).unwrap_or_default(),
            _ => BTreeMap::new(),
        };
        (two_byte || !map.is_empty()).then_some(FontMap { two_byte, map })
    }

    /// Keep an image XObject as the page's scan when it covers enough of the page and is
    /// bigger than the one kept so far. The image fills the unit square of `ctm`.
    fn take_scan(&mut self, d: &BTreeMap<String, Obj>, data: &[u8], ctm: Ctm) {
        let top_left = self.to_mm(ctm.p(0.0, 1.0));
        let along_cols = self.to_mm(ctm.p(1.0, 1.0));
        let along_rows = self.to_mm(ctm.p(0.0, 0.0));
        let (cx, cy) = (along_cols.x - top_left.x, along_cols.y - top_left.y);
        let (rx, ry) = (along_rows.x - top_left.x, along_rows.y - top_left.y);
        let tiny = |v: f64| v.abs() < 1e-6 * (cx.abs() + cy.abs() + rx.abs() + ry.abs()).max(1e-9);
        let orientation = if tiny(cy) && tiny(rx) {
            Orientation {
                transposed: false,
                flip_x: cx < 0.0,
                flip_y: ry < 0.0,
            }
        } else if tiny(cx) && tiny(ry) {
            Orientation {
                transposed: true,
                flip_x: rx < 0.0,
                flip_y: cy < 0.0,
            }
        } else {
            return; // Skewed or sheared: not a scanned sheet.
        };
        let (w, h) = ((cx + rx).abs(), (cy + ry).abs());
        let area = w * h;
        let page_area = self.out.size_mm.0 * self.out.size_mm.1;
        if !area.is_finite() || area < SCAN_SHARE * page_area {
            return;
        }
        if let Some(old) = &self.out.scan
            && (old.max.x - old.min.x) * (old.max.y - old.min.y) >= area
        {
            return;
        }
        let (raster, warning) = match self.image_raster(d, data) {
            Ok(r) => r,
            Err(problem) => {
                self.out.scan_problem = Some(problem);
                return;
            }
        };
        let Some(raster) = upright(raster, orientation) else {
            self.out.scan_problem = Some("the picture could not be decoded".into());
            return;
        };
        let xs = [top_left.x, along_cols.x, along_rows.x, along_cols.x + rx];
        let ys = [top_left.y, along_cols.y, along_rows.y, along_cols.y + ry];
        let lo = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
        let hi = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
        self.out.scan = Some(Scan {
            raster,
            min: Point::new(lo(xs), lo(ys)),
            max: Point::new(hi(xs), hi(ys)),
        });
        self.out.scan_problem = None;
        self.out.scan_warning = warning;
    }

    /// An image XObject as JPEG (kept as stored) or PNG (from raw samples), with a warning when
    /// only part of it could be read.
    fn image_raster(
        &self,
        d: &BTreeMap<String, Obj>,
        data: &[u8],
    ) -> std::result::Result<(Raster, Option<String>), String> {
        let get = |k: &str| d.get(k).map(|o| self.doc.get(o));
        let filters: Vec<String> = match get("Filter") {
            None => vec![],
            Some(Obj::Name(n)) => vec![n.clone()],
            Some(Obj::Array(a)) => a
                .iter()
                .filter_map(|x| self.doc.get(x).name().map(str::to_owned))
                .collect(),
            Some(_) => return Err("the picture has a damaged filter list".into()),
        };
        // The parameters of filter `i`: one dictionary for a single filter, or the entry at the
        // same place in an array (`null` for none).
        let parms_for = |i: usize| -> Option<&BTreeMap<String, Obj>> {
            match get("DecodeParms") {
                Some(Obj::Dict(p)) if i == 0 => Some(p),
                Some(Obj::Array(a)) => a.get(i).map(|o| self.doc.get(o)).and_then(Obj::dict),
                _ => None,
            }
        };
        let mut bytes = data.to_vec();
        let mut warning = None;
        for (i, f) in filters.iter().enumerate() {
            let parms = parms_for(i);
            match f.as_str() {
                "FlateDecode" | "Fl" => {
                    bytes = inflate(&bytes).ok_or("the picture data is damaged")?;
                    bytes = unpredict(bytes, parms)?;
                }
                "LZWDecode" | "LZW" => {
                    let early = parms
                        .and_then(|p| p.get("EarlyChange"))
                        .and_then(Obj::num)
                        .unwrap_or(1.0)
                        != 0.0;
                    bytes = unpredict(lzw_decode(&bytes, early)?, parms)?;
                }
                "RunLengthDecode" | "RL" => bytes = run_length_decode(&bytes)?,
                "DCTDecode" | "DCT" if i + 1 == filters.len() => {
                    return Ok((
                        Raster {
                            format: RasterFormat::Jpeg,
                            bytes,
                        },
                        warning,
                    ));
                }
                "CCITTFaxDecode" | "CCF" => {
                    let pnum = |k: &str| parms.and_then(|p| p.get(k)).and_then(|o| self.doc.get(o).num());
                    let pbool = |k: &str, default: bool| match parms.and_then(|p| p.get(k)).map(|o| self.doc.get(o)) {
                        Some(Obj::Bool(b)) => *b,
                        _ => default,
                    };
                    let width = get("Width").and_then(Obj::num);
                    let columns = pnum("Columns").or(width).unwrap_or(1728.0);
                    if width.is_some_and(|w| w != columns) {
                        return Err("the fax picture's columns do not match its width".into());
                    }
                    let rows = pnum("Rows")
                        .filter(|r| *r >= 1.0)
                        .or_else(|| get("Height").and_then(Obj::num))
                        .unwrap_or(0.0);
                    let in_range = |v: f64| (1.0..=1e6).contains(&v);
                    if !in_range(columns) || !in_range(rows) {
                        return Err("the fax picture has no size or is too large".into());
                    }
                    (bytes, warning) = pdf_codecs::ccitt(
                        &bytes,
                        &pdf_codecs::FaxParms {
                            k: pnum("K").unwrap_or(0.0) as i64,
                            columns: columns as u32,
                            rows: rows as u32,
                            end_of_line: pbool("EndOfLine", false),
                            encoded_byte_align: pbool("EncodedByteAlign", false),
                            end_of_block: pbool("EndOfBlock", true),
                            black_is_1: pbool("BlackIs1", false),
                        },
                    )?;
                }
                "JBIG2Decode" => {
                    let globals = parms
                        .and_then(|p| p.get("JBIG2Globals"))
                        .map(|o| match self.doc.get(o) {
                            Obj::Stream(gd, gdata) => {
                                decode_stream(gd, gdata).ok_or("the shared part of the JBIG2 picture is damaged")
                            }
                            _ => Err("the shared part of the JBIG2 picture is missing"),
                        })
                        .transpose()?;
                    let (w, h, samples) = pdf_codecs::jbig2(&bytes, globals.as_deref())?;
                    let num = |k: &str| get(k).and_then(Obj::num);
                    if num("Width") != Some(f64::from(w)) || num("Height") != Some(f64::from(h)) {
                        return Err("the JBIG2 picture's size does not match the picture's".into());
                    }
                    bytes = samples;
                }
                "JPXDecode" if i + 1 == filters.len() => {
                    let declared = match get("ColorSpace") {
                        Some(Obj::Name(n)) if n == "DeviceCMYK" || n == "CMYK" => Some(4),
                        Some(Obj::Name(n)) if n == "DeviceRGB" || n == "RGB" => Some(3),
                        Some(Obj::Name(n)) if n == "DeviceGray" || n == "G" => Some(1),
                        _ => None,
                    };
                    let jpx = pdf_codecs::jpx(&bytes, declared)?;
                    return samples_to_png(jpx.width, jpx.height, jpx.channels, 8, false, &jpx.samples)
                        .map(|r| (r, warning))
                        .ok_or_else(|| "the JPEG 2000 picture could not be converted".to_owned());
                }
                other => return Err(format!("pictures compressed as {other} are not supported yet")),
            }
        }
        let num = |k: &str| get(k).and_then(Obj::num);
        let width = num("Width")
            .filter(|v| *v >= 1.0 && *v <= 1e6)
            .ok_or("the picture has no width")? as u32;
        let height = num("Height")
            .filter(|v| *v >= 1.0 && *v <= 1e6)
            .ok_or("the picture has no height")? as u32;
        let mask = matches!(get("ImageMask"), Some(Obj::Bool(true)));
        let bits = if mask {
            1
        } else {
            num("BitsPerComponent").unwrap_or(8.0) as u8
        };
        let channels: u8 = if mask {
            1
        } else {
            match get("ColorSpace") {
                Some(Obj::Name(n)) if n == "DeviceGray" || n == "G" || n == "CalGray" => 1,
                Some(Obj::Name(n)) if n == "DeviceRGB" || n == "RGB" => 3,
                Some(Obj::Name(n)) if n == "DeviceCMYK" || n == "CMYK" => 4,
                Some(Obj::Array(a)) => match a.first().and_then(Obj::name) {
                    Some("CalGray") => 1,
                    Some("CalRGB") => 3,
                    Some("ICCBased") => match a.get(1).map(|o| self.doc.get(o)).and_then(Obj::dict) {
                        Some(icc) => icc.get("N").and_then(Obj::num).unwrap_or(0.0) as u8,
                        None => 0,
                    },
                    _ => 0,
                },
                _ => 0,
            }
        };
        if !matches!((channels, bits), (1, 1) | (1, 8) | (3, 8) | (1, 16) | (3, 16) | (4, 8)) {
            return Err(format!(
                "pictures with {channels} colour channel(s) at {bits} bit(s) are not supported yet"
            ));
        }
        // Decode [1 0] swaps black and white; an image mask paints its 0 samples by default. Only
        // one pair per channel is read, each [0 1] or [1 0], and every channel must use the same.
        let decode_inverted = match get("Decode") {
            Some(Obj::Array(a)) => {
                let kinds: Vec<Option<bool>> = a
                    .chunks(2)
                    .map(
                        |pair| match (pair.first().and_then(Obj::num), pair.get(1).and_then(Obj::num)) {
                            (Some(0.0), Some(1.0)) => Some(false),
                            (Some(1.0), Some(0.0)) => Some(true),
                            _ => None,
                        },
                    )
                    .collect();
                match kinds.first() {
                    Some(&Some(inverted))
                        if kinds.len() == usize::from(channels) && kinds.iter().all(|k| *k == Some(inverted)) =>
                    {
                        inverted
                    }
                    _ => {
                        return Err(
                            "pictures that map their colour channels in other ways (Decode) are not supported yet"
                                .into(),
                        );
                    }
                }
            }
            _ => false,
        };
        samples_to_png(width, height, channels, bits, decode_inverted, &bytes)
            .map(|r| (r, warning))
            .ok_or_else(|| "the picture data is too short or too large".to_owned())
    }

    fn run(&mut self, content: &[u8], resources: &Obj, ctm: Ctm, depth: usize) {
        if depth > 8 {
            return;
        }
        let mut lx = Lexer::new(content);
        let mut stack: Vec<Obj> = Vec::new();
        let mut gs: Vec<(Ctm, Paint)> = Vec::new();
        let mut ctm = ctm;
        let mut paint = Paint::default();
        let mut sub: Vec<(f64, f64)> = Vec::new();
        let mut path: Vec<(Vec<(f64, f64)>, bool)> = Vec::new();
        let mut cur = (0.0, 0.0);
        // Text state: matrix origin and font size are enough for positioned labels.
        let mut tm = Ctm::ID;
        let mut tlm = Ctm::ID;
        let mut font_size = 10.0;
        let mut font: Option<FontMap> = None;
        let mut leading = 0.0;
        while let Some(tok) = lx.next(0) {
            self.budget = self.budget.saturating_sub(1);
            if self.budget == 0 {
                return;
            }
            let Obj::Op(op) = tok else {
                stack.push(tok);
                if stack.len() > 4096 {
                    stack.clear();
                }
                continue;
            };
            let n = |k: usize| -> f64 {
                stack
                    .len()
                    .checked_sub(k)
                    .and_then(|i| stack.get(i))
                    .and_then(Obj::num)
                    .unwrap_or(0.0)
            };
            match op.as_str() {
                "q" => gs.push((ctm, paint)),
                "Q" => (ctm, paint) = gs.pop().unwrap_or((ctm, paint)),
                "G" | "RG" | "K" | "SC" | "SCN" | "CS" => paint.stroke_white = op != "CS" && is_white(&stack),
                "g" | "rg" | "k" | "sc" | "scn" | "cs" => paint.fill_white = op != "cs" && is_white(&stack),
                "cm" => ctm = ctm.pre([n(6), n(5), n(4), n(3), n(2), n(1)]),
                "m" => {
                    if sub.len() >= 2 {
                        path.push((std::mem::take(&mut sub), false));
                    }
                    sub.clear();
                    cur = ctm.p(n(2), n(1));
                    sub.push(cur);
                }
                "l" => {
                    cur = ctm.p(n(2), n(1));
                    sub.push(cur);
                }
                "c" | "v" | "y" => {
                    // Cubic curve flattened in 6 steps.
                    let (p0, (c1, c2, p3)) = match op.as_str() {
                        "c" => (cur, (ctm.p(n(6), n(5)), ctm.p(n(4), n(3)), ctm.p(n(2), n(1)))),
                        "v" => (cur, (cur, ctm.p(n(4), n(3)), ctm.p(n(2), n(1)))),
                        _ => (cur, (ctm.p(n(4), n(3)), ctm.p(n(2), n(1)), ctm.p(n(2), n(1)))),
                    };
                    for k in 1..=6 {
                        let t = k as f64 / 6.0;
                        let u = 1.0 - t;
                        let x = u * u * u * p0.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * p3.0;
                        let y = u * u * u * p0.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * p3.1;
                        sub.push((x, y));
                    }
                    cur = p3;
                }
                "h" => {
                    if sub.len() >= 2 {
                        path.push((std::mem::take(&mut sub), true));
                    }
                }
                "re" => {
                    let (x, y, w, h) = (n(4), n(3), n(2), n(1));
                    if sub.len() >= 2 {
                        path.push((std::mem::take(&mut sub), false));
                    }
                    sub.clear();
                    path.push((
                        vec![ctm.p(x, y), ctm.p(x + w, y), ctm.p(x + w, y + h), ctm.p(x, y + h)],
                        true,
                    ));
                }
                "S" | "s" | "B" | "B*" | "b" | "b*" | "f" | "F" | "f*" | "n" => {
                    if sub.len() >= 2 {
                        path.push((std::mem::take(&mut sub), matches!(op.as_str(), "s" | "b" | "b*")));
                    }
                    sub.clear();
                    let stroke = matches!(op.as_str(), "S" | "s" | "B" | "B*" | "b" | "b*");
                    let fill_only = matches!(op.as_str(), "f" | "F" | "f*");
                    // White on white paper is not drawing: Eplan, for one, strokes a large white cross
                    // over every page.
                    let filled = fill_only || matches!(op.as_str(), "B" | "B*" | "b" | "b*");
                    let visible = (stroke && !paint.stroke_white) || (filled && !paint.fill_white);
                    if !visible {
                        path.clear();
                    }
                    for (pts, closed) in path.drain(..) {
                        // Small filled shapes are junction dots or arrowheads: keep them; large
                        // fills are backgrounds: skip.
                        if !stroke && !fill_only {
                            continue;
                        }
                        let mm: Vec<Point> = pts.iter().map(|p| self.to_mm(*p)).collect();
                        if fill_only {
                            let (a, b) = wc_core::geom::bounds_of(&mm);
                            if (b.x - a.x) > 3.0 || (b.y - a.y) > 3.0 {
                                continue;
                            }
                        }
                        if mm.len() == 2 {
                            self.out.graphics.push(Shape::Line { a: mm[0], b: mm[1] });
                        } else {
                            self.out.graphics.push(Shape::Polyline {
                                points: mm,
                                closed,
                                filled: false,
                            });
                        }
                    }
                }
                "BT" => {
                    tm = Ctm::ID;
                    tlm = Ctm::ID;
                }
                "Tf" => {
                    font_size = n(1);
                    let name = stack
                        .len()
                        .checked_sub(2)
                        .and_then(|i| stack.get(i))
                        .and_then(Obj::name)
                        .unwrap_or("");
                    font = self.font_map(resources, name);
                }
                "TL" => leading = n(1),
                "Td" | "TD" => {
                    if op == "TD" {
                        leading = -n(1);
                    }
                    tlm = tlm.pre([1.0, 0.0, 0.0, 1.0, n(2), n(1)]);
                    tm = tlm;
                }
                "Tm" => {
                    tlm = Ctm([n(6), n(5), n(4), n(3), n(2), n(1)]);
                    tm = tlm;
                }
                "T*" => {
                    tlm = tlm.pre([1.0, 0.0, 0.0, 1.0, 0.0, -leading]);
                    tm = tlm;
                }
                "Tj" | "TJ" | "'" | "\"" => {
                    if op == "'" || op == "\"" {
                        tlm = tlm.pre([1.0, 0.0, 0.0, 1.0, 0.0, -leading]);
                        tm = tlm;
                    }
                    let bytes: Vec<u8> = match stack.last() {
                        Some(Obj::Str(s)) => s.clone(),
                        Some(Obj::Array(a)) => a
                            .iter()
                            .filter_map(|x| if let Obj::Str(s) = x { Some(s.clone()) } else { None })
                            .flatten()
                            .collect(),
                        _ => Vec::new(),
                    };
                    // Through the font's ToUnicode map when it has one (composite fonts always
                    // need it); otherwise printable single-byte text.
                    let (text, plausible) = match &font {
                        Some(f) => (f.decode(&bytes), true),
                        None => {
                            let t: String = bytes
                                .iter()
                                .filter(|b| (0x20..0x7f).contains(*b) || **b >= 0xa0)
                                .map(|b| *b as char)
                                .collect();
                            let ok = t.chars().count() * 2 >= bytes.len();
                            (t, ok)
                        }
                    };
                    if !text.trim().is_empty() && plausible {
                        let placed = ctm.pre(tm.0);
                        let at_pt = placed.p(0.0, 0.0);
                        let [a, b, c, d, _, _] = placed.0;
                        let scale = (c.powi(2) + d.powi(2)).sqrt();
                        // The text's baseline direction on paper; PDF space is y up, as the angle is.
                        let angle = if a == 0.0 && b == 0.0 {
                            0.0
                        } else {
                            b.atan2(a).to_degrees().round()
                        };
                        let angle = if angle == -0.0 || !angle.is_finite() {
                            0.0
                        } else {
                            angle.rem_euclid(360.0)
                        };
                        let h = (font_size * scale * MM_PER_PT * 0.72).clamp(0.5, 50.0);
                        self.out.graphics.push(Shape::Text {
                            at: self.to_mm(at_pt),
                            text: text.trim().to_owned(),
                            height: h,
                            align: Align::Left,
                            angle,
                        });
                    }
                }
                "Do" => {
                    let name = stack.last().and_then(Obj::name).unwrap_or("").to_owned();
                    let xo = resources
                        .dict()
                        .and_then(|r| r.get("XObject"))
                        .map(|x| self.doc.get(x))
                        .and_then(|x| x.dict())
                        .and_then(|x| x.get(&name))
                        .map(|x| self.doc.get(x));
                    if let Some(Obj::Stream(d, data)) = xo {
                        match d.get("Subtype").and_then(Obj::name) {
                            Some("Form") => {
                                let m = match d.get("Matrix").map(|m| self.doc.get(m)) {
                                    Some(Obj::Array(a)) if a.len() == 6 => {
                                        let v: Vec<f64> = a.iter().map(|x| x.num().unwrap_or(0.0)).collect();
                                        [v[0], v[1], v[2], v[3], v[4], v[5]]
                                    }
                                    _ => Ctm::ID.0,
                                };
                                let res = d
                                    .get("Resources")
                                    .map(|r| self.doc.get(r).clone())
                                    .unwrap_or_else(|| resources.clone());
                                if let Some(bytes) = decode_stream(d, data) {
                                    let (d2, data2) = (d.clone(), bytes);
                                    let _ = d2;
                                    self.run(&data2, &res, ctm.pre(m), depth + 1);
                                }
                            }
                            Some("Image") => {
                                self.out.images += 1;
                                self.take_scan(d, data, ctm);
                            }
                            _ => {}
                        }
                    }
                }
                "BI" => {
                    // Inline image: skip to EI.
                    if let Some(p) = find(&content[lx.i..], b"EI") {
                        lx.i += p + 2;
                    }
                    self.out.images += 1;
                }
                _ => {}
            }
            stack.clear();
        }
    }
}

/// Read every page of a PDF as trace graphics.
pub fn pages(bytes: &[u8]) -> Result<Vec<TracePage>> {
    let doc = Doc::parse(bytes)?;
    let pages = doc.pages();
    if pages.is_empty() {
        return Err(Error::Format("no pages found (encrypted or damaged PDF?)".into()));
    }
    if doc.objs.values().any(|o| {
        o.dict()
            .is_some_and(|d| d.contains_key("Filter") && d.get("Filter").and_then(Obj::name) == Some("Standard"))
    }) {
        return Err(Error::Format("the PDF is encrypted".into()));
    }
    let mut out = Vec::new();
    for (page, res, mb) in pages {
        let content = doc.content(&page);
        let (w, h) = ((mb[2] - mb[0]).abs(), (mb[3] - mb[1]).abs());
        let (ox, oy) = (mb[0].min(mb[2]), mb[1].min(mb[3]));
        // /Rotate turns the page clockwise for display; draw it the way it is displayed.
        let rotate = page
            .get("Rotate")
            .and_then(|r| doc.get(r).num())
            .unwrap_or(0.0)
            .rem_euclid(360.0)
            .round() as i64;
        let (start, (dw, dh)) = match rotate {
            90 => (Ctm([0.0, -1.0, 1.0, 0.0, -oy, w + ox]), (h, w)),
            180 => (Ctm([-1.0, 0.0, 0.0, -1.0, w + ox, h + oy]), (w, h)),
            270 => (Ctm([0.0, 1.0, -1.0, 0.0, h + oy, -ox]), (h, w)),
            _ => (Ctm([1.0, 0.0, 0.0, 1.0, -ox, -oy]), (w, h)),
        };
        let mut it = Interp {
            doc: &doc,
            page_h: dh,
            origin: (0.0, 0.0),
            out: TracePage::default(),
            budget: 5_000_000,
        };
        it.out.size_mm = (dw * MM_PER_PT, dh * MM_PER_PT);
        it.run(&content, &res, start, 0);
        out.push(it.out);
    }
    Ok(out)
}

/// How a font's string bytes become text.
#[derive(Debug, Clone, Default)]
struct FontMap {
    /// Composite fonts use two-byte codes (Identity-H and the like).
    two_byte: bool,
    /// Code to text, from the ToUnicode map.
    map: BTreeMap<u32, String>,
}

impl FontMap {
    fn decode(&self, bytes: &[u8]) -> String {
        let codes: Vec<u32> = if self.two_byte {
            bytes
                .chunks(2)
                .map(|c| (u32::from(c[0]) << 8) | u32::from(c.get(1).copied().unwrap_or(0)))
                .collect()
        } else {
            bytes.iter().map(|b| u32::from(*b)).collect()
        };
        let mut out = String::new();
        for code in codes {
            match self.map.get(&code) {
                Some(t) => out.push_str(t),
                // A simple font's code without a mapping is most likely plain text.
                None if !self.two_byte && ((0x20..0x7f).contains(&code) || code >= 0xa0) => {
                    out.extend(char::from_u32(code));
                }
                None => {}
            }
        }
        out.chars().filter(|c| !c.is_control()).collect()
    }
}

/// Hex tokens of a CMap section, with `[` and `]` as tokens of their own.
fn cmap_tokens(s: &[u8]) -> Vec<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'<' => {
                let end = s[i + 1..]
                    .iter()
                    .position(|&b| b == b'>')
                    .map_or(s.len(), |e| i + 1 + e);
                let hex: Vec<u8> = s[i + 1..end].iter().copied().filter(u8::is_ascii_hexdigit).collect();
                let bytes = hex
                    .chunks(2)
                    .filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
                    .collect();
                out.push(Some(bytes));
                i = end + 1;
            }
            b'[' | b']' => {
                out.push(None);
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

fn code_of(b: &[u8]) -> u32 {
    b.iter().take(4).fold(0, |a, x| (a << 8) | u32::from(*x))
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks(2)
        .map(|c| (u16::from(c[0]) << 8) | u16::from(c.get(1).copied().unwrap_or(0)))
        .collect();
    String::from_utf16_lossy(&units)
}

/// The bfchar and bfrange sections of a ToUnicode CMap.
fn parse_cmap(data: &[u8]) -> BTreeMap<u32, String> {
    const MAX_RANGE: u32 = 65536;
    let mut map = BTreeMap::new();
    let text = String::from_utf8_lossy(data);
    let mut rest = text.as_ref();
    while let Some(start) = rest.find("begin") {
        let after = &rest[start + 5..];
        let kind = if after.starts_with("bfchar") {
            "bfchar"
        } else if after.starts_with("bfrange") {
            "bfrange"
        } else {
            rest = after;
            continue;
        };
        let body_start = kind.len();
        let Some(end) = after.find(&format!("end{kind}")) else {
            break;
        };
        let toks = cmap_tokens(&after.as_bytes()[body_start.min(end)..end]);
        if kind == "bfchar" {
            for pair in toks.chunks(2) {
                if let [Some(src), Some(dst)] = pair {
                    map.insert(code_of(src), utf16(dst));
                }
            }
        } else {
            let mut k = 0;
            while k + 2 < toks.len() + 1 {
                let (Some(Some(lo)), Some(Some(hi))) = (toks.get(k), toks.get(k + 1)) else {
                    break;
                };
                let (lo, hi) = (code_of(lo), code_of(hi));
                match toks.get(k + 2) {
                    Some(Some(dst)) => {
                        let base = code_of(dst);
                        let width = dst.len();
                        for (n, code) in (lo..=hi).take(MAX_RANGE as usize).enumerate() {
                            let v = base.wrapping_add(n as u32);
                            let bytes: Vec<u8> = (0..width).rev().map(|s| (v >> (8 * s)) as u8).collect();
                            map.insert(code, utf16(&bytes));
                        }
                        k += 3;
                    }
                    Some(None) => {
                        // An array of destinations, one per code.
                        let mut j = k + 3;
                        let mut code = lo;
                        while let Some(Some(dst)) = toks.get(j) {
                            if code <= hi {
                                map.insert(code, utf16(dst));
                            }
                            code = code.saturating_add(1);
                            j += 1;
                        }
                        k = j + 1;
                    }
                    None => break,
                }
            }
        }
        rest = &after[end..];
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_pdf(content: &str, compress: bool) -> Vec<u8> {
        let stream: Vec<u8> = if compress {
            miniz_oxide::deflate::compress_to_vec_zlib(content.as_bytes(), 6)
        } else {
            content.as_bytes().to_vec()
        };
        let filter = if compress { "/Filter /FlateDecode " } else { "" };
        let mut b = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 1190.55 841.89] >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n".to_vec();
        b.extend_from_slice(format!("4 0 obj\n<< {filter}/Length {} >>\nstream\n", stream.len()).as_bytes());
        b.extend_from_slice(&stream);
        b.extend_from_slice(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
        b
    }

    #[test]
    fn lines_rects_curves_and_text() -> Result<()> {
        let content = "q 2 0 0 2 0 0 cm 10 10 m 110 10 l S Q 100 100 50 20 re S 0 0 m 10 0 20 10 30 30 c S BT /F1 12 Tf 72 72 Td (K1) Tj ET";
        for compress in [false, true] {
            let p = pages(&simple_pdf(content, compress))?;
            assert_eq!(p.len(), 1);
            let g = &p[0].graphics;
            assert!((p[0].size_mm.0 - 420.0).abs() < 0.5, "A3 landscape");
            let Some(Shape::Line { a, b }) = g.first() else {
                return Err(Error::Format(format!("{g:?}")));
            };
            assert!(
                (a.x - 20.0 * MM_PER_PT).abs() < 1e-6 && (b.x - 220.0 * MM_PER_PT).abs() < 1e-6,
                "cm applied"
            );
            assert!((a.y - (841.89 - 20.0) * MM_PER_PT).abs() < 1e-6, "y flipped");
            assert!(
                g.iter()
                    .any(|s| matches!(s, Shape::Polyline { closed: true, points, .. } if points.len() == 4))
            );
            assert!(g.iter().any(|s| matches!(s, Shape::Text { text, .. } if text == "K1")));
        }
        Ok(())
    }

    #[test]
    fn white_paint_is_not_drawing() -> Result<()> {
        // A white cross over the page (as Eplan strokes one), a white fill, then visible black
        // lines in grey, RGB and CMYK; q/Q restores the colour.
        let content = "1 1 1 RG 0 0 m 800 800 l S 1 g 10 10 20 20 re f /Cs6 CS 1 1 1 SCN 0 800 m 800 0 l S \
                       q 0 0 0 1 K 10 10 m 110 10 l S Q 10 20 m 110 20 l S \
                       0 G 10 30 m 110 30 l S 1 0 0 RG 10 40 m 110 40 l S 0 0 0 0 K 10 50 m 110 50 l S";
        let p = pages(&simple_pdf(content, false))?;
        let lines = p[0].graphics.iter().filter(|s| matches!(s, Shape::Line { .. })).count();
        assert_eq!(lines, 3, "{:?}", p[0].graphics);
        assert!(!p[0].graphics.iter().any(|s| matches!(s, Shape::Polyline { .. })));
        Ok(())
    }

    #[test]
    fn composite_font_text_through_to_unicode() -> Result<()> {
        // Glyph codes 0x31.. map to "No" by a range, and code 3 to a space by a single entry, as
        // subset fonts do; the bytes alone are not text.
        let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n1 begincodespacerange <0000> <ffff> endcodespacerange\n1 beginbfchar\n<0003> <0020>\nendbfchar\n2 beginbfrange\n<0031> <0032> <004E>\n<0050> <0051> [<006F> <00C5>]\nendbfrange\nendcmap\n";
        let content = "BT /F1 12 Tf 72 72 Td <003100500003003200510003FFFF> Tj ET BT /F2 10 Tf 72 300 Td (K1) Tj ET BT /F2 10 Tf 0 1 -1 0 200 100 Tm (UP) Tj ET";
        let mut b = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 1190.55 841.89] >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 7 0 R >> >> >>\nendobj\n".to_vec();
        b.extend_from_slice(
            format!(
                "4 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n",
                content.len()
            )
            .as_bytes(),
        );
        b.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type0 /Encoding /Identity-H /ToUnicode 6 0 R >>\nendobj\n",
        );
        b.extend_from_slice(
            format!(
                "6 0 obj\n<< /Length {} >>\nstream\n{cmap}\nendstream\nendobj\n",
                cmap.len()
            )
            .as_bytes(),
        );
        b.extend_from_slice(b"7 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
        let p = pages(&b)?;
        let texts: Vec<(&str, f64)> = p[0]
            .graphics
            .iter()
            .filter_map(|g| {
                if let Shape::Text { text, angle, .. } = g {
                    Some((text.as_str(), *angle))
                } else {
                    None
                }
            })
            .collect();
        // Code 0xFFFF has no mapping and is dropped; a simple font without a map reads as before, and
        // a text matrix turned a quarter keeps that turn.
        assert_eq!(texts, [("No OÅ", 0.0), ("K1", 0.0), ("UP", 90.0)]);
        let m = parse_cmap(cmap.as_bytes());
        assert_eq!(m.get(&0x32).map(String::as_str), Some("O"));
        assert!(parse_cmap(b"beginbfrange <0000> <ffffffff> <0041> endbfrange").len() <= 65536);
        assert!(parse_cmap(b"beginbfchar <00").is_empty());
        Ok(())
    }

    #[test]
    fn object_streams_and_indirect_lengths() -> Result<()> {
        // Catalog and page tree inside a compressed object stream; content length by reference.
        let a = "<< /Type /Catalog /Pages 2 0 R >>\n";
        let b2 = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
        let header = format!("1 0 2 {} ", a.len());
        let body2 = format!("{header}{a}{b2}");
        let first2 = header.len();
        let packed2 = miniz_oxide::deflate::compress_to_vec_zlib(body2.as_bytes(), 6);
        let content = miniz_oxide::deflate::compress_to_vec_zlib(b"0 0 m 72 72 l S", 6);
        let mut b = b"%PDF-1.5\n".to_vec();
        b.extend_from_slice(
            format!("9 0 obj\n<< /Type /ObjStm /N 2 /First {first2} /Length 10 0 R /Filter /FlateDecode >>\nstream\n")
                .as_bytes(),
        );
        b.extend_from_slice(&packed2);
        b.extend_from_slice(format!("\nendstream\nendobj\n10 0 obj\n{}\nendobj\n", packed2.len()).as_bytes());
        b.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>\nendobj\n",
        );
        b.extend_from_slice(b"4 0 obj\n<< /Length 5 0 R /Filter /FlateDecode >>\nstream\n");
        b.extend_from_slice(&content);
        b.extend_from_slice(format!("\nendstream\nendobj\n5 0 obj\n{}\nendobj\n%%EOF\n", content.len()).as_bytes());
        let p = pages(&b)?;
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].graphics.len(), 1);
        Ok(())
    }

    #[test]
    fn rotated_pages_are_drawn_as_displayed() -> Result<()> {
        // A portrait A4 page shown rotated 90 degrees clockwise (how many CAD tools print
        // landscape sheets). A line along the bottom edge of the page ends up on the left edge.
        let mut b = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec();
        b.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Rotate 90 /Contents 4 0 R >>\nendobj\n",
        );
        let content = "0 0 m 100 0 l S";
        b.extend_from_slice(
            format!(
                "4 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n%%EOF\n",
                content.len()
            )
            .as_bytes(),
        );
        let p = pages(&b)?;
        assert!(
            (p[0].size_mm.0 - 297.0).abs() < 0.5 && (p[0].size_mm.1 - 210.0).abs() < 0.5,
            "{:?}",
            p[0].size_mm
        );
        let Some(Shape::Line { a, b: e }) = p[0].graphics.first() else {
            return Err(Error::Format(format!("expected a line, got {:?}", p[0].graphics)));
        };
        assert!(a.x.abs() < 0.01 && e.x.abs() < 0.01, "{a:?} {e:?}");
        assert!(
            (a.y - 0.0).abs() < 0.01 && (e.y - 100.0 * 25.4 / 72.0).abs() < 0.01,
            "{a:?} {e:?}"
        );
        Ok(())
    }

    #[test]
    fn a_rotation_set_on_the_page_tree_turns_its_pages() -> Result<()> {
        let doc = |page_rotate: &str| {
            let mut b = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /Rotate 90 >>\nendobj\n".to_vec();
            b.extend_from_slice(
                format!("3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] {page_rotate} /Contents 4 0 R >>\nendobj\n").as_bytes(),
            );
            b.extend_from_slice(b"4 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n%%EOF\n");
            b
        };
        // Inherited from the parent: shown landscape.
        let p = pages(&doc(""))?;
        assert!((p[0].size_mm.0 - 297.0).abs() < 0.5, "{:?}", p[0].size_mm);
        // The page's own rotation wins.
        let p = pages(&doc("/Rotate 0"))?;
        assert!((p[0].size_mm.0 - 210.0).abs() < 0.5, "{:?}", p[0].size_mm);
        // A null one is no rotation of its own: the parent's still applies.
        let p = pages(&doc("/Rotate null"))?;
        assert!((p[0].size_mm.0 - 297.0).abs() < 0.5, "{:?}", p[0].size_mm);
        Ok(())
    }

    #[test]
    fn hostile() {
        assert!(pages(b"").is_err());
        assert!(pages(b"hello").is_err());
        assert!(pages(b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 9 0 R >> endobj").is_err());
        let _ = pages(b"%PDF-1.4\n3 0 obj\n<< /Type /Page /Contents 4 0 R >>\nendobj\n4 0 obj\n<< /Length 999999 /Filter /FlateDecode >>\nstream\nxx\nendstream\nendobj");
        let _ = pages(&simple_pdf("q q q Q Q Q Q Q 1 2 3 4 5 6 cm m l c re S ( ((( ", false));
        let _ = pages(&simple_pdf(
            "<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<",
            false,
        ));
    }

    /// A one-page PDF that draws image XObject `/Im` with `cm` and maybe a small `/Logo`.
    fn picture_pdf(rotate: u32, image_dict: &str, samples: &[u8], content: &str) -> Vec<u8> {
        let mut b = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec();
        b.extend_from_slice(
            format!("3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Rotate {rotate} /Contents 4 0 R /Resources << /XObject << /Im 5 0 R /Logo 5 0 R >> >> >>\nendobj\n").as_bytes(),
        );
        b.extend_from_slice(
            format!(
                "4 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n",
                content.len()
            )
            .as_bytes(),
        );
        b.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XObject /Subtype /Image {image_dict} /Length {} >>\nstream\n",
                samples.len()
            )
            .as_bytes(),
        );
        b.extend_from_slice(samples);
        b.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");
        b
    }

    /// LZW-compress `data` as PDF's `LZWDecode` reads it, starting with a clear code and
    /// clearing again before the table is full.
    fn lzw_encode(data: &[u8], early_change: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut acc, mut held) = (0u64, 0u32);
        let mut emit = |code: usize, width: u32, out: &mut Vec<u8>| {
            acc = (acc << width) | code as u64;
            held += width;
            while held >= 8 {
                held -= 8;
                out.push((acc >> held) as u8);
            }
        };
        // What the reader's table holds before it reads the next code decides its length.
        let width_for = |len: usize| -> u32 {
            match len.saturating_sub(1) + usize::from(early_change) {
                ..512 => 9,
                512..1024 => 10,
                1024..2048 => 11,
                _ => 12,
            }
        };
        let fresh =
            || -> std::collections::HashMap<Vec<u8>, usize> { (0..=255u8).map(|b| (vec![b], b as usize)).collect() };
        let mut table = fresh();
        let mut next = 258;
        emit(256, 9, &mut out);
        let mut w: Vec<u8> = Vec::new();
        for &c in data {
            let mut wc = w.clone();
            wc.push(c);
            if table.contains_key(&wc) {
                w = wc;
                continue;
            }
            emit(table[&w], width_for(next), &mut out);
            table.insert(wc, next);
            next += 1;
            w = vec![c];
            if next >= 4000 {
                emit(256, width_for(next), &mut out);
                table = fresh();
                next = 258;
            }
        }
        if !w.is_empty() {
            emit(table[&w], width_for(next), &mut out);
            next += 1;
        }
        emit(257, width_for(next), &mut out);
        if held > 0 {
            out.push((acc << (8 - held)) as u8);
        }
        out
    }

    #[test]
    fn lzw_data_is_read() {
        // The example in the PDF reference: "-----A---B".
        let packed = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw_decode(&packed, true).as_deref(), Ok(&b"-----A---B"[..]));
        // Long enough for 12-bit codes and a cleared table, with and without early change.
        let mut long: Vec<u8> = (0..40_000u32).map(|i| (i * 7 % 251) as u8 ^ (i / 300) as u8).collect();
        long.extend([b'x'; 3000]);
        for early in [true, false] {
            assert_eq!(
                lzw_decode(&lzw_encode(&long, early), early),
                Ok(long.clone()),
                "early {early}"
            );
        }
        // A code that is not in the table yet.
        assert!(lzw_decode(&[0x80, 0x4B, 0x00], true).is_err_and(|e| e.contains("code 300")));
        // Many clear codes in a row: each is cheap, and they give no bytes.
        let clears: Vec<u8> = std::iter::repeat_n([0x80u8, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01, 0x00], 1000)
            .flatten()
            .collect();
        assert_eq!(lzw_decode(&clears, true), Ok(vec![]));
    }

    /// Run-length compress `data` as PDF's `RunLengthDecode` reads it: runs of three or more
    /// equal bytes repeated, the rest copied, with the end mark.
    fn run_length_encode(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let run = data[i..].iter().take(128).take_while(|b| **b == data[i]).count();
            if run >= 3 {
                out.extend([(257 - run) as u8, data[i]]);
                i += run;
            } else {
                let start = i;
                while i < data.len()
                    && i - start < 128
                    && data[i..].iter().take(3).filter(|b| **b == data[i]).count() < 3
                {
                    i += 1;
                }
                out.push((i - start - 1) as u8);
                out.extend_from_slice(&data[start..i]);
            }
        }
        out.push(128);
        out
    }

    #[test]
    fn run_length_data_is_read() {
        // A copy of three bytes, a run of four, the end mark; what follows it is not read.
        assert_eq!(
            run_length_decode(&[2, b'a', b'b', b'c', 253, b'x', 128, 9, 9]).as_deref(),
            Ok(&b"abcxxxx"[..])
        );
        // The longest copy and the longest run.
        let mut long: Vec<u8> = (0..128u8).collect();
        long.extend([7u8; 128]);
        long.extend((0..1000u32).map(|i| (i * 31 % 7) as u8));
        assert_eq!(run_length_decode(&run_length_encode(&long)), Ok(long));
        assert_eq!(run_length_decode(&[128]), Ok(vec![]));
        // Damaged data gives a reason: a copy or a run past the end, or no end mark.
        for (damaged, why) in [
            (&[5u8, b'a', b'b'][..], "goes past the end"),
            (&[250], "goes past the end"),
            (&[1, b'a', b'b'], "no end mark"),
            (&[], "no end mark"),
        ] {
            assert!(
                run_length_decode(damaged).is_err_and(|e| e.contains("RunLength data is damaged") && e.contains(why)),
                "{damaged:?}"
            );
        }
    }

    #[test]
    fn lzw_cmyk_and_16_bit_pictures_are_read() -> Result<()> {
        let full = "q 595 0 0 842 0 0 cm /Im Do Q";
        let grey = [10u8, 20, 30, 40, 50, 60];
        let scan = |dict: &str, samples: &[u8]| -> Result<Option<(u32, u32, Vec<u8>)>> {
            let p = pages(&picture_pdf(0, dict, samples, full))?;
            assert!(p[0].scan_problem.is_none(), "{dict}: {:?}", p[0].scan_problem);
            Ok(p[0].scan.as_ref().and_then(luma))
        };
        let want = Some((3, 2, grey.to_vec()));
        let gray = "/Width 3 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8";

        // LZW, with the default early change and without it.
        assert_eq!(
            scan(&format!("{gray} /Filter /LZWDecode"), &lzw_encode(&grey, true))?,
            want
        );
        assert_eq!(
            scan(
                &format!("{gray} /Filter [/LZWDecode] /DecodeParms [<< /EarlyChange 0 >>]"),
                &lzw_encode(&grey, false)
            )?,
            want
        );
        // LZW with a PNG predictor: each row starts with its filter type (2 = Up).
        let predicted = [0u8, 10, 20, 30, 2, 30, 30, 30];
        assert_eq!(
            scan(
                &format!("{gray} /Filter /LZWDecode /DecodeParms << /Predictor 15 /Columns 3 >>"),
                &lzw_encode(&predicted, true)
            )?,
            want
        );

        // RunLength alone, and before Flate (the run-length stage gives the Flate data).
        assert_eq!(
            scan(&format!("{gray} /Filter /RunLengthDecode"), &run_length_encode(&grey))?,
            want
        );
        let rl_of_flate = run_length_encode(&miniz_oxide::deflate::compress_to_vec_zlib(&grey, 6));
        assert_eq!(scan(&format!("{gray} /Filter [/RL /FlateDecode]"), &rl_of_flate)?, want);
        // Damaged run data is reported with its reason.
        let p = pages(&picture_pdf(
            0,
            &format!("{gray} /Filter /RunLengthDecode"),
            &[5, 10, 20],
            full,
        ))?;
        assert!(p[0].scan.is_none());
        assert!(
            p[0].scan_problem
                .as_deref()
                .is_some_and(|m| m.contains("RunLength data is damaged")),
            "{:?}",
            p[0].scan_problem
        );

        // Two filters, the predictor in the second one's parameters (a null placeholder first).
        let flate_of_lzw = miniz_oxide::deflate::compress_to_vec_zlib(&lzw_encode(&predicted, true), 6);
        assert_eq!(
            scan(
                &format!("{gray} /Filter [/FlateDecode /LZWDecode] /DecodeParms [null << /Predictor 15 /Columns 3 >>]"),
                &flate_of_lzw
            )?,
            want
        );
        // A predictor on the first of two filters is undone before the second one runs: the LZW
        // stage gives one predicted row (type 0) holding the Flate data.
        let inner = miniz_oxide::deflate::compress_to_vec_zlib(&grey, 6);
        let mut row = vec![0u8];
        row.extend(&inner);
        assert_eq!(
            scan(
                &format!(
                    "{gray} /Filter [/LZWDecode /FlateDecode] /DecodeParms [<< /Predictor 10 /Columns {} >> null]",
                    inner.len()
                ),
                &lzw_encode(&row, true)
            )?,
            want
        );
        // TIFF predictor: differences 10 10 10 are the samples 10 20 30.
        assert_eq!(
            scan(
                &format!("{gray} /Filter /LZWDecode /DecodeParms << /Predictor 2 /Columns 3 >>"),
                &lzw_encode(&[10, 10, 10, 40, 10, 10], true)
            )?,
            want
        );

        // 16-bit grey: the high byte of each sample.
        let wide: Vec<u8> = grey.iter().flat_map(|v| [*v, 0xAB]).collect();
        assert_eq!(
            scan("/Width 3 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 16", &wide)?,
            want
        );
        // 16-bit RGB with the same value in every channel reads as the same grey.
        let rgb16: Vec<u8> = grey.iter().flat_map(|v| [*v, 0, *v, 0, *v, 0]).collect();
        assert_eq!(
            scan("/Width 3 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 16", &rgb16)?,
            want
        );

        // CMYK: no ink is white, full black is black, full cyan is no red.
        let cmyk = [0u8, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0];
        let p = pages(&picture_pdf(
            0,
            "/Width 3 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8",
            &cmyk,
            full,
        ))?;
        let rgb = p[0]
            .scan
            .as_ref()
            .and_then(|s| crate::raster::decode(&s.raster))
            .map(|img| img.to_rgb8().into_raw());
        assert_eq!(rgb, Some(vec![255, 255, 255, 0, 0, 0, 0, 255, 255]));
        // A Decode array that swaps only cyan is refused, not drawn wrong.
        let p = pages(&picture_pdf(
            0,
            "/Width 3 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Decode [1 0 0 1 0 1 0 1]",
            &cmyk,
            full,
        ))?;
        assert!(p[0].scan_problem.as_deref().is_some_and(|m| m.contains("Decode")));
        // A Decode array that only narrows the range, or has too few pairs, is refused too.
        for decode in ["[0 0.5 0 1 0 1 0 1]", "[1 0]"] {
            let p = pages(&picture_pdf(
                0,
                &format!("/Width 3 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Decode {decode}"),
                &cmyk,
                full,
            ))?;
            assert!(
                p[0].scan_problem.as_deref().is_some_and(|m| m.contains("Decode")),
                "{decode}"
            );
        }
        // Swapping all four inks is read.
        let p = pages(&picture_pdf(
            0,
            "/Width 3 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Decode [1 0 1 0 1 0 1 0]",
            &cmyk,
            full,
        ))?;
        assert!(p[0].scan.is_some());
        Ok(())
    }

    fn luma(scan: &Scan) -> Option<(u32, u32, Vec<u8>)> {
        let img = crate::raster::decode(&scan.raster)?.to_luma8();
        Some((img.width(), img.height(), img.into_raw()))
    }

    /// Group 4 (T.6) data for two 8-pixel rows, each black from pixel 2 to 4: the first row in
    /// horizontal mode (white 2, black 3) and V0 to its end, the second in three V0 codes.
    const G4_ROWS: [u8; 2] = [0b0010_1111, 0b0111_1000];
    /// What those rows look like once decoded, as grey levels.
    const G4_LUMA: [u8; 16] = [255, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255];

    /// A JBIG2 page in the embedded organization: page information (8 x 2) and one immediate
    /// lossless generic region coded with MMR, holding [`G4_ROWS`].
    fn jbig2_page() -> Vec<u8> {
        let mut b = Vec::new();
        // Segment 0, page information (type 48), page 1, 19 bytes.
        b.extend_from_slice(&[0, 0, 0, 0, 48, 0, 1, 0, 0, 0, 19]);
        b.extend_from_slice(&[0, 0, 0, 8, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        // Segment 1, immediate lossless generic region (type 38), page 1, 20 bytes.
        b.extend_from_slice(&[0, 0, 0, 1, 38, 0, 1, 0, 0, 0, 20]);
        b.extend_from_slice(&[0, 0, 0, 8, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        b.push(1); // MMR
        b.extend_from_slice(&G4_ROWS);
        b
    }

    /// A 3 x 2 grey JPEG 2000 codestream, lossless: 10 20 30 / 40 50 60 (written by OpenJPEG).
    const JPX_GREY: [u8; 136] = [
        0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x01, 0xFF, 0x52, 0x00, 0x0C, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x01, 0x04, 0x04, 0x00, 0x01, 0xFF, 0x5C, 0x00, 0x07, 0x40, 0x40, 0x48, 0x48, 0x50, 0xFF, 0x64, 0x00, 0x25,
        0x00, 0x01, 0x43, 0x72, 0x65, 0x61, 0x74, 0x65, 0x64, 0x20, 0x62, 0x79, 0x20, 0x4F, 0x70, 0x65, 0x6E, 0x4A,
        0x50, 0x45, 0x47, 0x20, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6F, 0x6E, 0x20, 0x32, 0x2E, 0x35, 0x2E, 0x34, 0xFF,
        0x90, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1B, 0x00, 0x01, 0xFF, 0x93, 0xCF, 0xB4, 0x0C, 0x08, 0x2B,
        0x1F, 0xA0, 0xF9, 0xC1, 0x80, 0x01, 0x9F, 0x7F, 0xFF, 0xD9,
    ];

    /// A 2 x 1 CMYK JPEG 2000 codestream, lossless: no ink, then full black (K 255).
    const JPX_CMYK: [u8; 150] = [
        0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x07, 0x01, 0x01, 0x07, 0x01, 0x01, 0x07, 0x01, 0x01, 0x07, 0x01, 0x01,
        0xFF, 0x52, 0x00, 0x0C, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x04, 0x04, 0x00, 0x01, 0xFF, 0x5C, 0x00, 0x04,
        0x40, 0x40, 0xFF, 0x64, 0x00, 0x25, 0x00, 0x01, 0x43, 0x72, 0x65, 0x61, 0x74, 0x65, 0x64, 0x20, 0x62, 0x79,
        0x20, 0x4F, 0x70, 0x65, 0x6E, 0x4A, 0x50, 0x45, 0x47, 0x20, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6F, 0x6E, 0x20,
        0x32, 0x2E, 0x35, 0x2E, 0x34, 0xFF, 0x90, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x23, 0x00, 0x01, 0xFF,
        0x93, 0xDF, 0x80, 0x10, 0x09, 0x3F, 0xDF, 0x80, 0x10, 0x09, 0x3F, 0xDF, 0x80, 0x10, 0x09, 0x3F, 0xDF, 0x80,
        0x18, 0x05, 0xA2, 0xDD, 0xFF, 0xD9,
    ];

    /// `codestream` in a JP2 file: signature, file type, a header (image header, grey colour,
    /// then `extra` boxes) and the codestream box, its length raised by `overrun`.
    fn jp2(codestream: &[u8], extra: &[u8], overrun: u32) -> Vec<u8> {
        let boxed = |kind: &[u8], body: &[u8]| {
            let mut b = (8 + body.len() as u32).to_be_bytes().to_vec();
            b.extend_from_slice(kind);
            b.extend_from_slice(body);
            b
        };
        let mut header = boxed(b"ihdr", &[0, 0, 0, 2, 0, 0, 0, 3, 0, 1, 7, 7, 0, 0]);
        header.extend(boxed(b"colr", &[1, 0, 0, 0, 0, 0, 17]));
        header.extend_from_slice(extra);
        let mut out = vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];
        out.extend(boxed(b"ftyp", b"jp2 \0\0\0\0jp2 "));
        out.extend(boxed(b"jp2h", &header));
        out.extend_from_slice(&(8 + codestream.len() as u32 + overrun).to_be_bytes());
        out.extend_from_slice(b"jp2c");
        out.extend_from_slice(codestream);
        out
    }

    #[test]
    fn jp2_files_with_ambiguous_boxes_are_refused_before_decoding() {
        use crate::pdf_codecs::jpx;
        // A plain JP2 file decodes.
        let plain = jpx(&jp2(&JPX_GREY, &[], 0), Some(1)).map(|p| (p.width, p.height, p.channels, p.samples));
        assert_eq!(plain, Ok((3, 2, 1, vec![10, 20, 30, 40, 50, 60])));
        // A codestream box longer than the file (the decoder would read what is there).
        let over = jpx(&jp2(&JPX_GREY, &[], 100), Some(1));
        assert!(
            over.as_ref().is_err_and(|e| e.contains("cut short")),
            "{:?}",
            over.err()
        );
        // A valid nine-column palette, then an invalid one the decoder skips: the nine count.
        let mut pclr = vec![0, 0, 0, 38];
        pclr.extend_from_slice(b"pclr");
        pclr.extend_from_slice(&[0, 2, 9]);
        pclr.extend_from_slice(&[7; 9]);
        pclr.extend_from_slice(&[0; 18]);
        pclr.extend_from_slice(&[0, 0, 0, 10]);
        pclr.extend_from_slice(b"pclr");
        pclr.extend_from_slice(&[0, 0]);
        let wide = jpx(&jp2(&JPX_GREY, &pclr, 0), Some(1));
        assert!(
            wide.as_ref().is_err_and(|e| e.contains("too many channels")),
            "{:?}",
            wide.err()
        );
    }

    #[test]
    fn scanner_pictures_in_fax_jbig2_and_jpeg_2000_are_read() -> Result<()> {
        let full = "q 595 0 0 842 0 0 cm /Im Do Q";
        type Seen = (Option<(u32, u32, Vec<u8>)>, Option<String>);
        let scan = |dict: &str, data: &[u8]| -> Result<Seen> {
            let p = pages(&picture_pdf(0, dict, data, full))?;
            Ok((p[0].scan.as_ref().and_then(luma), p[0].scan_problem.clone()))
        };
        let bilevel = "/Width 8 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 1";

        // Group 4 fax, as office scanners write it: 0 samples are black.
        let g4 = format!("{bilevel} /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 8 /Rows 2 >>");
        assert_eq!(scan(&g4, &G4_ROWS)?.0, Some((8, 2, G4_LUMA.to_vec())));
        // Without Rows the picture's height is used.
        let g4 = format!("{bilevel} /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 8 >>");
        assert_eq!(scan(&g4, &G4_ROWS)?.0, Some((8, 2, G4_LUMA.to_vec())));
        // BlackIs1 turns the samples round.
        let g4 = format!("{bilevel} /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 8 /BlackIs1 true >>");
        let inverted: Vec<u8> = G4_LUMA.iter().map(|v| 255 - v).collect();
        assert_eq!(scan(&g4, &G4_ROWS)?.0, Some((8, 2, inverted)));
        // Columns that disagree with the width are refused with the reason.
        let g4 = format!("{bilevel} /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 16 >>");
        let (picture, problem) = scan(&g4, &G4_ROWS)?;
        assert!(picture.is_none());
        assert!(problem.is_some_and(|m| m.contains("columns")));
        // A picture damaged part way keeps its rows before the damage, and says so.
        let three = "/Width 8 /Height 3 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 8 >>";
        let p = pages(&picture_pdf(0, three, &G4_ROWS, full))?;
        let mut rows = G4_LUMA.to_vec();
        rows.extend([255; 8]);
        assert_eq!(p[0].scan.as_ref().and_then(luma), Some((8, 3, rows)));
        let warning = p[0].scan_warning.clone().unwrap_or_default();
        assert!(warning.contains("row 2 of 3"), "{warning}");
        // A whole picture comes with no warning.
        let g4 = format!("{bilevel} /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 8 >>");
        assert!(pages(&picture_pdf(0, &g4, &G4_ROWS, full))?[0].scan_warning.is_none());
        // Data that is not fax at all is refused, never a panic.
        let g4 = format!("{bilevel} /Filter /CCITTFaxDecode /DecodeParms << /K -1 >>");
        let (picture, problem) = scan(&g4, &[0, 0, 0, 0])?;
        assert!(picture.is_none() && problem.is_some(), "{problem:?}");

        // JBIG2, embedded: black pixels come out as 0 samples, like the fax.
        let jbig2 = format!("{bilevel} /Filter /JBIG2Decode");
        assert_eq!(scan(&jbig2, &jbig2_page())?.0, Some((8, 2, G4_LUMA.to_vec())));
        // A size that disagrees with the page is refused.
        let wrong = "/Width 9 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode";
        let (picture, problem) = scan(wrong, &jbig2_page())?;
        assert!(picture.is_none());
        assert!(problem.is_some_and(|m| m.contains("JBIG2")));
        // Shared segments that are not a stream: refused with the reason.
        let jbig2 = format!("{bilevel} /Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 3 0 R >>");
        let (picture, problem) = scan(&jbig2, &jbig2_page())?;
        assert!(picture.is_none());
        assert!(problem.is_some_and(|m| m.contains("shared part")));
        // A region far larger than its page is refused once it goes over the decoder's memory
        // limit.
        let mut huge = jbig2_page();
        huge[41..49].copy_from_slice(&[0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF]);
        let (picture, problem) = scan(&format!("{bilevel} /Filter /JBIG2Decode"), &huge)?;
        assert!(picture.is_none());
        assert!(problem.is_some_and(|m| m.contains("too much memory")));
        // Damaged data.
        let jbig2 = format!("{bilevel} /Filter /JBIG2Decode");
        let (picture, problem) = scan(&jbig2, &jbig2_page()[..15])?;
        assert!(picture.is_none());
        assert!(problem.is_some_and(|m| m.contains("JBIG2")));

        // JPEG 2000: the codestream says its own size and channels.
        let jpx = "/Width 3 /Height 2 /Filter /JPXDecode";
        assert_eq!(scan(jpx, &JPX_GREY)?.0, Some((3, 2, vec![10, 20, 30, 40, 50, 60])));
        // A bare CMYK codestream names no colour space; the PDF's /DeviceCMYK keeps its fourth
        // channel as K (no ink stays white, full K is black) rather than alpha.
        let cmyk = "/Width 2 /Height 1 /ColorSpace /DeviceCMYK /Filter /JPXDecode";
        assert_eq!(scan(cmyk, &JPX_CMYK)?.0, Some((2, 1, vec![255, 0])));
        // More components than a picture can use are refused before any is decoded: the decoder
        // gives each a buffer of the whole picture.
        let mut six = JPX_GREY[..41].to_vec();
        six[5] = 38 + 3 * 6; // Lsiz
        six.push(6); // Csiz
        for _ in 0..6 {
            six.extend_from_slice(&[0x07, 0x01, 0x01]);
        }
        six.extend_from_slice(&JPX_GREY[45..]);
        let (picture, problem) = scan(jpx, &six)?;
        assert!(picture.is_none());
        assert!(
            problem.as_deref().is_some_and(|m| m.contains("too many channels")),
            "{problem:?}"
        );
        let (picture, problem) = scan(jpx, &JPX_GREY[..40])?;
        assert!(picture.is_none());
        assert!(problem.is_some_and(|m| m.contains("JPEG 2000")));
        Ok(())
    }

    #[test]
    fn scanned_sheets_become_upright_pictures() -> Result<()> {
        // 3 x 2 grey picture: 10 20 30 / 40 50 60, filling the whole page; a small logo too.
        let grey = "/Width 3 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8";
        let samples = [10u8, 20, 30, 40, 50, 60];
        let full = "q 50 0 0 50 10 10 cm /Logo Do Q q 595 0 0 842 0 0 cm /Im Do Q";
        let p = pages(&picture_pdf(0, grey, &samples, full))?;
        let Some(scan) = p[0].scan.as_ref() else {
            return Err(Error::Format(format!("no scan: {:?}", p[0].scan_problem)));
        };
        assert_eq!(scan.raster.format, RasterFormat::Png);
        assert_eq!(luma(scan), Some((3, 2, samples.to_vec())));
        assert!(scan.min.x.abs() < 0.01 && scan.min.y.abs() < 0.01, "{scan:?}");
        assert!(
            (scan.max.x - 209.9).abs() < 0.2 && (scan.max.y - 297.0).abs() < 0.2,
            "{scan:?}"
        );
        assert_eq!(p[0].images, 2);

        // The same sheet shown rotated 90 degrees: the picture turns with it.
        let p = pages(&picture_pdf(90, grey, &samples, full))?;
        let turned = p[0].scan.as_ref().and_then(luma);
        assert_eq!(turned, Some((2, 3, vec![40, 10, 50, 20, 60, 30])));

        // Only a small logo: no scan.
        let p = pages(&picture_pdf(0, grey, &samples, "q 50 0 0 50 10 10 cm /Logo Do Q"))?;
        assert!(p[0].scan.is_none());

        // A JPEG is kept byte for byte.
        let jpeg = [0xFFu8, 0xD8, 0xFF, 0xD9];
        let p = pages(&picture_pdf(0, "/Width 3 /Height 2 /Filter /DCTDecode", &jpeg, full))?;
        assert_eq!(p[0].scan.as_ref().map(|s| s.raster.bytes.clone()), Some(jpeg.to_vec()));
        // A filter the reader does not know is reported, not guessed at.
        let p = pages(&picture_pdf(0, "/Width 3 /Height 2 /Filter /AHx", &jpeg, full))?;
        assert!(p[0].scan.is_none());
        assert!(
            p[0].scan_problem
                .as_deref()
                .is_some_and(|m| m.contains("compressed as AHx"))
        );

        // Short sample data never panics.
        let p = pages(&picture_pdf(0, grey, &samples[..2], full))?;
        assert!(p[0].scan.is_none() && p[0].scan_problem.is_some());
        Ok(())
    }
}
