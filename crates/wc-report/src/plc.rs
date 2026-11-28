//! PLC variable declarations from the PLC I/O list: an IEC 61131-3 global variable list that a
//! PLC programming tool (TwinCAT, CODESYS and others that read structured text) can take in, so
//! the I/O names in the program match the device tags on the drawings.
//!
//! Each drawn PLC input or output becomes one variable named after its device tag, located at
//! its address. Addresses are read in the common spellings (`%IX0.0`, `%I0.0`, `I0.0`, `E0.0`,
//! `IW64`, `A4.1`); a point without an address, or with one that is not a PLC address, is
//! declared at `%I*` or `%Q*`, to be linked to the hardware in the PLC tool.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::plc_rack::PlcAddress;

use crate::{Inputs, PlcPoint, Table, natural_key, plc_io_list, plc_points};

/// What came out of writing the variable list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlcVariables {
    /// The declaration, as structured text.
    pub text: String,
    /// Variables declared.
    pub variables: usize,
    /// Of those, the ones declared at `%I*` or `%Q*` because their address was missing or not
    /// a PLC address.
    pub unlocated: usize,
}

/// A located address and the elementary type that goes with its size, in the reading shared
/// with the PLC card overview ([`PlcAddress`]); any bit number is taken, as some lists write
/// `%I0.10`.
fn location(address: &str, output: bool) -> Option<(String, &'static str)> {
    let a = PlcAddress::parse_any_bit(address).filter(|a| a.output == output)?;
    Some((a.iec(), a.elementary_type()))
}

/// Words a variable may not be called: the IEC 61131-3 (3rd edition) keywords and the
/// elementary and generic data type names, sorted.
const RESERVED: &[&str] = &[
    "ABS",
    "ABSTRACT",
    "ACTION",
    "AND",
    "ANY",
    "ANY_BIT",
    "ANY_CHAR",
    "ANY_CHARS",
    "ANY_DATE",
    "ANY_DERIVED",
    "ANY_DURATION",
    "ANY_ELEMENTARY",
    "ANY_INT",
    "ANY_MAGNITUDE",
    "ANY_NUM",
    "ANY_REAL",
    "ANY_SIGNED",
    "ANY_STRING",
    "ANY_UNSIGNED",
    "ARRAY",
    "AT",
    "BOOL",
    "BY",
    "BYTE",
    "CASE",
    "CHAR",
    "CLASS",
    "CONFIGURATION",
    "CONSTANT",
    "CONTINUE",
    "DATE",
    "DATE_AND_TIME",
    "DINT",
    "DO",
    "DT",
    "DWORD",
    "ELSE",
    "ELSIF",
    "EN",
    "END_ACTION",
    "END_CASE",
    "END_CLASS",
    "END_CONFIGURATION",
    "END_FOR",
    "END_FUNCTION",
    "END_FUNCTION_BLOCK",
    "END_IF",
    "END_INTERFACE",
    "END_METHOD",
    "END_NAMESPACE",
    "END_PROGRAM",
    "END_REPEAT",
    "END_RESOURCE",
    "END_STEP",
    "END_STRUCT",
    "END_TRANSITION",
    "END_TYPE",
    "END_VAR",
    "END_WHILE",
    "ENO",
    "EXIT",
    "EXTENDS",
    "FALSE",
    "FINAL",
    "FOR",
    "FROM",
    "FUNCTION",
    "FUNCTION_BLOCK",
    "F_EDGE",
    "IF",
    "IMPLEMENTS",
    "INITIAL_STEP",
    "INT",
    "INTERFACE",
    "INTERNAL",
    "INTERVAL",
    "JMP",
    "LDATE",
    "LDATE_AND_TIME",
    "LDT",
    "LINT",
    "LREAL",
    "LTIME",
    "LTIME_OF_DAY",
    "LTOD",
    "LWORD",
    "METHOD",
    "MOD",
    "NAMESPACE",
    "NON_RETAIN",
    "NOT",
    "NULL",
    "OF",
    "ON",
    "OR",
    "OVERLAP",
    "OVERRIDE",
    "PRIORITY",
    "PRIVATE",
    "PROGRAM",
    "PROTECTED",
    "PUBLIC",
    "READ_ONLY",
    "READ_WRITE",
    "REAL",
    "REF",
    "REF_TO",
    "REPEAT",
    "RESOURCE",
    "RETAIN",
    "RETURN",
    "R_EDGE",
    "SINGLE",
    "SINT",
    "STEP",
    "STRING",
    "STRUCT",
    "SUPER",
    "TASK",
    "THEN",
    "THIS",
    "TIME",
    "TIME_OF_DAY",
    "TO",
    "TOD",
    "TRANSITION",
    "TRUE",
    "TYPE",
    "UDINT",
    "UINT",
    "ULINT",
    "UNTIL",
    "USING",
    "USINT",
    "VAR",
    "VAR_ACCESS",
    "VAR_CONFIG",
    "VAR_EXTERNAL",
    "VAR_GLOBAL",
    "VAR_INPUT",
    "VAR_IN_OUT",
    "VAR_OUTPUT",
    "VAR_TEMP",
    "WCHAR",
    "WHILE",
    "WITH",
    "WORD",
    "WSTRING",
    "XOR",
];

/// A structured-text identifier from a device tag: letters, digits and single underscores,
/// never starting with a digit or naming a keyword.
fn identifier(tag: &str) -> String {
    let mut out = String::new();
    for c in tag.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty()
        || out.starts_with(|c: char| c.is_ascii_digit())
        || RESERVED.contains(&out.to_ascii_uppercase().as_str())
    {
        out.insert_str(0, "io_");
    }
    out
}

/// Text that is safe inside a `(* … *)` comment on one line.
fn comment(s: &str) -> String {
    s.replace("(*", "( *").replace("*)", "* )").replace(['\r', '\n'], " ")
}

/// The global variable list for every PLC input and output drawn in the project, in
/// the order of the PLC I/O list (by address).
pub fn plc_variables(i: &Inputs<'_>) -> PlcVariables {
    let t = plc_io_list(i);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // (still to be linked, address sort key), declaration
    type Line = ((bool, Vec<(String, u64)>), String);
    let mut lines: Vec<Line> = Vec::new();
    let mut out = PlcVariables::default();
    for row in &t.rows {
        let [address, kind, device, description, connected, page] = row.as_slice() else {
            continue;
        };
        let output = kind == "Output";
        let base = identifier(device.strip_suffix(" (no tag)").unwrap_or(device));
        let mut var = base.clone();
        let mut n = 2;
        while !seen.insert(var.to_ascii_uppercase()) {
            var = format!("{base}_{n}");
            n += 1;
        }
        let (at, ty) = location(address, output).unwrap_or_else(|| {
            out.unlocated += 1;
            ((if output { "%Q*" } else { "%I*" }).to_owned(), "BOOL")
        });
        let mut note = vec![device.clone()];
        if !description.is_empty() {
            note.push(description.clone());
        }
        if !address.is_empty() && !at.ends_with('*') && address.trim() != at {
            note.push(format!("address {}", address.trim()));
        } else if !address.is_empty() && at.ends_with('*') {
            // Worded as the rack warnings word it: an address in the other direction is still
            // a PLC address.
            note.push(match PlcAddress::parse_any_bit(address) {
                Some(a) => format!(
                    "address {} is an {}",
                    a.iec(),
                    if a.output { "output" } else { "input" }
                ),
                None => format!("address {:?} is not a PLC address", address.trim()),
            });
        }
        if !connected.is_empty() {
            note.push(format!("to {connected}"));
        }
        note.push(format!("page {page}"));
        let key = (at.ends_with('*'), natural_key(&at));
        lines.push((
            key,
            format!("    {var} AT {at} : {ty}; (* {} *)", comment(&note.join(", "))),
        ));
        out.variables += 1;
    }
    // Inputs, then outputs, by address; points still to be linked last.
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    out.text = format!(
        "(* PLC inputs and outputs of {}, written by WiringCraft from the drawings. *)\r\nVAR_GLOBAL\r\n{}{}END_VAR\r\n",
        comment(&i.project.meta.name),
        lines.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\r\n"),
        if lines.is_empty() { "" } else { "\r\n" },
    );
    out
}

/// The drawn PLC inputs and outputs whose address names a channel (a bit no higher than 7) in
/// their own direction, by that address.
fn points_by_address(points: &[PlcPoint]) -> BTreeMap<PlcAddress, Vec<&PlcPoint>> {
    let mut out: BTreeMap<PlcAddress, Vec<&PlcPoint>> = BTreeMap::new();
    for point in points {
        if let Some(a) = PlcAddress::parse(&point.address).filter(|a| a.output == (point.kind == "Output")) {
            out.entry(a).or_default().push(point);
        }
    }
    out
}

/// One card of a PLC rack with what is drawn at each of its channels, for the drawn card
/// overview sheets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlcCardSheet {
    pub rack: String,
    pub slot: u32,
    /// The card's tag, else its type, else `slot N`.
    pub card: String,
    pub kind: String,
    pub channels: Vec<CardChannel>,
}

/// One channel of a card: its number from 0, its address and the points drawn there (none for a
/// spare, two or more when several share the address).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CardChannel {
    pub channel: usize,
    pub address: String,
    pub points: Vec<CardPoint>,
}

/// A PLC input or output drawn at a card's channel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CardPoint {
    pub device: String,
    pub description: String,
    /// What it is wired to.
    pub connected: String,
    /// The number and the id of the page it is drawn on.
    pub page_number: String,
    pub page_id: String,
}

/// Every card of every rack in the project ([`wc_core::plc_rack`]), rack by rack and slot by
/// slot, with the PLC input or output drawn at each channel's address. Cards whose channels
/// cannot be worked out are left out ([`plc_rack_warnings`] says why).
pub fn plc_card_sheets(i: &Inputs<'_>) -> Vec<PlcCardSheet> {
    if i.project.meta.racks.is_empty() {
        return Vec::new();
    }
    let all = plc_points(i);
    let points = points_by_address(&all);
    let mut out = Vec::new();
    for rack in &i.project.meta.racks {
        let mut cards: Vec<_> = rack.cards.iter().collect();
        cards.sort_by_key(|c| c.slot);
        for card in cards {
            let Ok(addresses) = card.addresses() else { continue };
            let channels = addresses
                .iter()
                .enumerate()
                .map(|(channel, a)| CardChannel {
                    channel,
                    address: a.iec(),
                    points: points
                        .get(a)
                        .into_iter()
                        .flatten()
                        .map(|p| CardPoint {
                            device: p.device.clone(),
                            description: p.description.clone(),
                            connected: p.connected.clone(),
                            page_number: p.page_number.clone(),
                            page_id: p.page_id.clone(),
                        })
                        .collect(),
                })
                .collect();
            out.push(PlcCardSheet {
                rack: rack.name.clone(),
                slot: card.slot,
                card: card.label(),
                kind: card.kind.clone(),
                channels,
            });
        }
    }
    out
}

/// PLC card overview: every card of every rack in the project ([`wc_core::plc_rack`]), slot by
/// slot and channel by channel, with the PLC input or output drawn at each channel's address:
/// the same device, description, connection and page as in the PLC I/O list. A channel with
/// nothing drawn is listed empty (a spare); two points on one address give two rows. Rows with a
/// drawn point link to its page. Cards whose channels cannot be worked out are left out
/// ([`plc_rack_warnings`] says why). [`plc_card_sheets`] gives the same as drawn cards.
pub fn plc_card_overview(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "PLC cards",
        &[
            "Rack",
            "Slot",
            "Card",
            "Type",
            "Channel",
            "Address",
            "Device",
            "Description",
            "Connected to",
            "Page",
        ],
    );
    for card in plc_card_sheets(i) {
        for ch in &card.channels {
            let head = vec![
                card.rack.clone(),
                card.slot.to_string(),
                card.card.clone(),
                card.kind.clone(),
                ch.channel.to_string(),
                ch.address.clone(),
            ];
            if ch.points.is_empty() {
                let mut r = head.clone();
                r.extend(std::iter::repeat_n(String::new(), 4));
                t.rows.push(r);
            }
            for point in &ch.points {
                let mut r = head.clone();
                r.extend([
                    point.device.clone(),
                    point.description.clone(),
                    point.connected.clone(),
                    point.page_number.clone(),
                ]);
                t.row_pages.resize(t.rows.len(), String::new());
                t.row_pages.push(point.page_id.clone());
                t.rows.push(r);
            }
        }
    }
    t
}

/// What is wrong with the project's PLC racks: a card whose channels cannot be worked out, two
/// cards in one slot, two cards sharing an address, and drawn PLC inputs and outputs that are on
/// no card: their address is on none, is not a PLC address, names the other direction, or names
/// bit 8 or higher. Points without an address are left to the PLC I/O list, which lists them
/// last. Empty when there are no racks.
///
/// Cards are compared by the bits their channels cover (a word covers two bytes, so `%IW64`
/// and `%IB65` share one). A byte or word card with its own `step` numbers its addresses its own
/// way, so it is compared by channel address only; a bit card ignores its step and is compared by
/// bits like any other.
pub fn plc_rack_warnings(i: &Inputs<'_>) -> Vec<String> {
    let racks = &i.project.meta.racks;
    let mut out = Vec::new();
    if racks.is_empty() {
        return out;
    }
    // Channel addresses and covered bits seen so far, with the card that has them.
    let mut owner: BTreeMap<PlcAddress, String> = BTreeMap::new();
    let mut covered: Vec<(bool, std::ops::Range<u64>, String)> = Vec::new();
    for rack in racks {
        let mut slots = BTreeSet::new();
        for card in &rack.cards {
            let here = format!("PLC rack \"{}\" slot {}", rack.name, card.slot);
            if !slots.insert(card.slot) {
                out.push(format!("{here}: a second card in this slot"));
            }
            let addresses = match card.addresses() {
                Err(e) => {
                    out.push(format!("{here}: {e}, left off the PLC card overview"));
                    continue;
                }
                Ok(a) => a,
            };
            // The step only moves byte, word and double word channels; bit cards ignore it.
            let own_step = card.step.is_some() && addresses.first().is_some_and(|a| a.bit.is_none());
            let mut clash: Option<(PlcAddress, String)> = None;
            for a in &addresses {
                let shared = owner.get(a).cloned().or_else(|| {
                    if own_step {
                        return None;
                    }
                    let bits = a.bits();
                    covered
                        .iter()
                        .find(|(o, r, _)| *o == a.output && r.start < bits.end && bits.start < r.end)
                        .map(|(_, _, c)| c.clone())
                });
                if let Some(first) = shared {
                    clash.get_or_insert((*a, first));
                }
            }
            if let Some((a, first)) = clash {
                out.push(format!("{here}: address {} is also on {first}", a.iec()));
            }
            for a in addresses {
                owner.entry(a).or_insert_with(|| here.clone());
                if !own_step {
                    covered.push((a.output, a.bits(), here.clone()));
                }
            }
        }
    }
    let points = plc_points(i);
    for point in &points {
        if point.address.is_empty() {
            continue;
        }
        let what = format!(
            "PLC {} {} (page {})",
            point.kind.to_ascii_lowercase(),
            point.device,
            point.page_number
        );
        let output = point.kind == "Output";
        match PlcAddress::parse_any_bit(&point.address) {
            None => out.push(format!(
                "{what}: address {:?} is not a PLC address, so it is on no card",
                point.address
            )),
            Some(a) if a.output != output => out.push(format!(
                "{what}: address {} is an {}, so it is on no card",
                a.iec(),
                if a.output { "output" } else { "input" }
            )),
            Some(a) if a.bit.is_some_and(|b| b > 7) => out.push(format!(
                "{what}: address {} names no bit of a byte, so it is on no card",
                a.iec()
            )),
            Some(a) if !owner.contains_key(&a) => {
                out.push(format!("{what} at {} is on no card of the PLC racks", a.iec()))
            }
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_in_the_common_spellings() {
        let at = |a: &str, out: bool| location(a, out).map(|(l, t)| format!("{l} {t}"));
        assert_eq!(at("%IX0.0", false).as_deref(), Some("%IX0.0 BOOL"));
        assert_eq!(at("%I0.2", false).as_deref(), Some("%IX0.2 BOOL"));
        assert_eq!(at("i 1.07", false).as_deref(), Some("%IX1.7 BOOL"));
        assert_eq!(at("E2.3", false).as_deref(), Some("%IX2.3 BOOL"));
        assert_eq!(at("A4.1", true).as_deref(), Some("%QX4.1 BOOL"));
        assert_eq!(at("IW64", false).as_deref(), Some("%IW64 WORD"));
        assert_eq!(at("%QD8", true).as_deref(), Some("%QD8 DWORD"));
        // Wrong direction, a bit number on a word, no number, or not an address at all.
        assert_eq!(at("%Q0.0", false), None);
        assert_eq!(at("IW64.1", false), None);
        assert_eq!(at("IX3", false), None);
        assert_eq!(at("I", false), None);
        assert_eq!(at("DI1", false), None);
        assert_eq!(at("Slot 2 / Ch 3", false), None);
        assert_eq!(at("", false), None);
        assert_eq!(at("%I0..1", false), None);
    }

    #[test]
    fn identifiers_from_tags() {
        assert_eq!(identifier("-A1.DI1"), "A1_DI1");
        assert_eq!(identifier("=PUMP+CAB1-K1"), "PUMP_CAB1_K1");
        assert_eq!(identifier("-1S2"), "io_1S2");
        assert_eq!(identifier("-AT"), "io_AT");
        assert_eq!(identifier("-DT"), "io_DT");
        assert_eq!(identifier("-LREAL"), "io_LREAL");
        assert_eq!(identifier("-tod"), "io_tod");
        assert_eq!(identifier("-SINT"), "io_SINT");
        assert_eq!(identifier("=PROGRAM"), "io_PROGRAM");
        assert_eq!(identifier("-STEP"), "io_STEP");
        assert_eq!(identifier("-DT1"), "DT1");
        assert!(RESERVED.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(identifier("--"), "io_");
        assert_eq!(identifier("Ø-ventil"), "ventil");
        assert_eq!(comment("a (*b*) c\nd"), "a ( *b* ) c d");
    }

    mod drawn {
        #![allow(clippy::unwrap_used)]

        use std::collections::BTreeMap;

        use wc_core::geom::{Point, Rotation};
        use wc_core::plc_rack::{PlcCard, PlcRack};
        use wc_core::symbol::{Pin, PinDir, SymbolRole};
        use wc_core::{Device, DeviceTag, Page, PlacedSymbol, Project, Symbol};

        use super::super::*;

        fn symbol(id: &str, pin: &str) -> Symbol {
            Symbol {
                source: String::new(),
                id: id.into(),
                name: id.into(),
                category: "plc".into(),
                letter_code: String::new(),
                role: SymbolRole::Standalone,
                graphics: vec![],
                pins: vec![Pin {
                    name: pin.into(),
                    at: Point::new(0.0, 0.0),
                    dir: PinDir::Down,
                    ..Default::default()
                }],
                tag_at: Point::default(),
                description: String::new(),
            }
        }

        fn symbols() -> BTreeMap<String, Symbol> {
            [symbol("in", "I"), symbol("out", "Q")]
                .into_iter()
                .map(|s| (s.id.clone(), s))
                .collect()
        }

        /// A project named `name` with one page holding a PLC point per entry:
        /// (symbol id, tag, address, description).
        fn project(name: &str, points: &[(&str, &str, &str, &str)]) -> Project {
            let mut p = Project::new(name);
            let mut page = Page::new("p1", "1", "PLC");
            for (n, (symbol, tag, address, description)) in points.iter().enumerate() {
                let tag = (!tag.is_empty()).then(|| DeviceTag::parse(tag).unwrap());
                page.symbols.push(PlacedSymbol {
                    id: format!("s{n}"),
                    symbol: (*symbol).into(),
                    at: Point::new(20.0 * n as f64, 50.0),
                    rotation: Rotation::R0,
                    mirror: false,
                    tag: tag.clone(),
                });
                if let Some(tag) = tag {
                    p.devices.insert(
                        tag,
                        Device {
                            description: (*description).into(),
                            properties: [("Address".to_string(), (*address).to_string())].into_iter().collect(),
                            ..Default::default()
                        },
                    );
                }
            }
            p.pages.push(page);
            p
        }

        fn variables(p: &Project) -> PlcVariables {
            let (s, parts) = (symbols(), BTreeMap::new());
            plc_variables(&Inputs {
                project: p,
                symbols: &s,
                parts: &parts,
            })
        }

        fn declarations(v: &PlcVariables) -> Vec<&str> {
            v.text.split("\r\n").filter(|l| l.starts_with("    ")).collect()
        }

        #[test]
        fn an_empty_project_declares_an_empty_list_with_an_escaped_name() {
            let v = variables(&project("Plant (*A*)\nnew", &[]));
            assert_eq!((v.variables, v.unlocated), (0, 0));
            assert_eq!(
                v.text,
                "(* PLC inputs and outputs of Plant ( *A* ) new, written by WiringCraft from the drawings. *)\r\nVAR_GLOBAL\r\nEND_VAR\r\n"
            );
        }

        #[test]
        fn tags_that_give_one_identifier_are_numbered_regardless_of_case() {
            let p = project(
                "t",
                &[
                    ("in", "-A1.I1", "%IX0.0", ""),
                    ("in", "+A1-I1", "%IX0.1", ""),
                    ("in", "-a1.i1", "%IX0.2", ""),
                ],
            );
            let v = variables(&p);
            assert_eq!((v.variables, v.unlocated), (3, 0));
            let names: Vec<&str> = declarations(&v)
                .iter()
                .filter_map(|l| l.split_whitespace().next())
                .collect();
            assert_eq!(names, ["A1_I1", "A1_I1_2", "a1_i1_3"]);
        }

        #[test]
        fn word_addresses_take_their_type_and_respelled_addresses_are_noted() {
            let p = project(
                "t",
                &[
                    ("in", "-B1", "IW64", "Level"),
                    ("in", "-B2", "%IX0.0", ""),
                    ("out", "-Y1", "%QD8", ""),
                ],
            );
            let v = variables(&p);
            assert_eq!((v.variables, v.unlocated), (3, 0));
            let d = declarations(&v);
            // Inputs before outputs, in address text order; written in the IEC spelling
            // exactly, no address note.
            assert_eq!(d[0], "    B1 AT %IW64 : WORD; (* -B1, Level, address IW64, page 1 *)");
            assert_eq!(d[1], "    B2 AT %IX0.0 : BOOL; (* -B2, page 1 *)");
            assert_eq!(d[2], "    Y1 AT %QD8 : DWORD; (* -Y1, page 1 *)");
        }

        #[test]
        fn points_without_a_usable_address_are_declared_unlocated_and_last() {
            let p = project(
                "t",
                &[
                    // An output at an input address, a non-address, no address and no tag.
                    ("out", "-Q1", "%I0.0", ""),
                    ("in", "-S1", "Slot 2", ""),
                    ("in", "-S2", "", ""),
                    ("in", "", "", ""),
                    ("in", "-S9", "%IX9.9", ""),
                ],
            );
            let v = variables(&p);
            assert_eq!((v.variables, v.unlocated), (5, 4));
            let d = declarations(&v);
            assert_eq!(d.len(), 5);
            assert!(d[0].starts_with("    S9 AT %IX9.9 : BOOL;"), "{d:?}");
            for l in &d[1..] {
                assert!(l.contains(" AT %I* : BOOL;") || l.contains(" AT %Q* : BOOL;"), "{l}");
            }
            let line = |var: &str| d.iter().find(|l| l.starts_with(&format!("    {var} "))).copied();
            assert!(line("Q1").is_some_and(|l| l.contains("AT %Q*") && l.contains("address %IX0.0 is an input")));
            assert!(line("S1").is_some_and(|l| l.contains("address \"Slot 2\" is not a PLC address")));
            assert!(line("S2").is_some_and(|l| !l.contains("address")));
            // The untagged point is named after its symbol id, without "(no tag)".
            assert!(
                line("s3").is_some_and(|l| l.contains("(* s3 (no tag), page 1 *)")),
                "{d:?}"
            );
        }

        #[test]
        fn descriptions_cannot_end_the_comment_or_the_line() {
            let p = project("t", &[("in", "-S1", "%IX0.0", "stop *) (* now\r\nplease")]);
            let v = variables(&p);
            let d = declarations(&v);
            assert_eq!(
                d,
                ["    S1 AT %IX0.0 : BOOL; (* -S1, stop * ) ( * now  please, page 1 *)"]
            );
        }

        fn card(slot: u32, tag: &str, channels: u32, start: &str) -> PlcCard {
            PlcCard {
                slot,
                tag: tag.into(),
                channels,
                start: start.into(),
                ..Default::default()
            }
        }

        #[test]
        fn card_sheets_share_an_address_between_points_and_skip_broken_cards() {
            let mut p = project(
                "t",
                &[
                    ("in", "-S1", "%IX0.1", "first"),
                    ("in", "-S2", "I0.1", "second"),
                    // Bit 9 is on no channel; an output address is not on an input card.
                    ("in", "-S3", "%I0.9", ""),
                    ("out", "-Q1", "%QX0.0", ""),
                ],
            );
            p.meta.racks.push(PlcRack {
                name: "R1".into(),
                cards: vec![card(4, "", 0, "%IX8.0"), card(2, "", 2, "%IX0.0")],
                ..Default::default()
            });
            let (s, parts) = (symbols(), BTreeMap::new());
            let i = Inputs {
                project: &p,
                symbols: &s,
                parts: &parts,
            };
            let sheets = plc_card_sheets(&i);
            // The card without channels is left out; the other is named by its slot.
            assert_eq!(sheets.len(), 1);
            let sheet = &sheets[0];
            assert_eq!(
                (sheet.rack.as_str(), sheet.slot, sheet.card.as_str()),
                ("R1", 2, "slot 2")
            );
            let addresses: Vec<&str> = sheet.channels.iter().map(|c| c.address.as_str()).collect();
            assert_eq!(addresses, ["%IX0.0", "%IX0.1"]);
            assert!(sheet.channels[0].points.is_empty());
            let devices: Vec<&str> = sheet.channels[1].points.iter().map(|p| p.device.as_str()).collect();
            assert_eq!(devices, ["-S1", "-S2"]);
            assert_eq!(sheet.channels[1].points[1].description, "second");
            assert_eq!(sheet.channels[1].points[1].page_id, "p1");

            // Two rows for the shared channel, each linked to its page; the spare links nowhere.
            let t = plc_card_overview(&i);
            assert_eq!(t.rows.len(), 3);
            assert_eq!(t.rows[0][5..], ["%IX0.0", "", "", "", ""]);
            assert_eq!(t.rows[1][4..7], ["1", "%IX0.1", "-S1"]);
            assert_eq!(t.rows[2][4..7], ["1", "%IX0.1", "-S2"]);
            assert_eq!(t.row_pages, ["", "p1", "p1"]);

            let w = plc_rack_warnings(&i);
            let has = |s: &str| w.iter().any(|w| w.contains(s));
            assert!(has("slot 4: no channels, left off the PLC card overview"), "{w:?}");
            assert!(has("-S3 (page 1): address %IX0.9 names no bit"), "{w:?}");
            assert!(has("-Q1 (page 1) at %QX0.0 is on no card"), "{w:?}");
            assert_eq!(w.len(), 3, "{w:?}");
        }

        #[test]
        fn without_racks_there_are_no_sheets_and_no_warnings() {
            let p = project("t", &[("in", "-S1", "Slot 2", ""), ("in", "-S2", "%IX0.0", "")]);
            let (s, parts) = (symbols(), BTreeMap::new());
            let i = Inputs {
                project: &p,
                symbols: &s,
                parts: &parts,
            };
            assert!(plc_card_sheets(&i).is_empty());
            assert!(plc_card_overview(&i).rows.is_empty());
            assert!(plc_rack_warnings(&i).is_empty());
        }

        #[test]
        fn cards_in_two_racks_at_one_address_clash_across_racks() {
            let mut p = project("t", &[]);
            for name in ["R1", "R2"] {
                p.meta.racks.push(PlcRack {
                    name: name.into(),
                    cards: vec![card(1, "", 8, "%QX2.0")],
                    ..Default::default()
                });
            }
            let (s, parts) = (symbols(), BTreeMap::new());
            let i = Inputs {
                project: &p,
                symbols: &s,
                parts: &parts,
            };
            assert_eq!(
                plc_rack_warnings(&i),
                ["PLC rack \"R2\" slot 1: address %QX2.0 is also on PLC rack \"R1\" slot 1"]
            );
            // Both are still laid out.
            assert_eq!(plc_card_sheets(&i).len(), 2);
        }
    }
}
