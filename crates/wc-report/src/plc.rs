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

/// Words a variable may not be called.
const RESERVED: &[&str] = &[
    "ABS",
    "ACTION",
    "AND",
    "ARRAY",
    "AT",
    "BOOL",
    "BY",
    "BYTE",
    "CASE",
    "CONSTANT",
    "DATE",
    "DINT",
    "DO",
    "DWORD",
    "ELSE",
    "ELSIF",
    "END_CASE",
    "END_FOR",
    "END_IF",
    "END_VAR",
    "END_WHILE",
    "EXIT",
    "FALSE",
    "FOR",
    "FUNCTION",
    "IF",
    "INT",
    "MOD",
    "NOT",
    "OF",
    "OR",
    "REAL",
    "REPEAT",
    "RETURN",
    "STRING",
    "THEN",
    "TIME",
    "TO",
    "TRUE",
    "TYPE",
    "UNTIL",
    "VAR",
    "VAR_GLOBAL",
    "WHILE",
    "WORD",
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
            note.push(format!("address {:?} is not a PLC address", address.trim()));
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
/// and `%IB65` share one). A card with its own `step` numbers its addresses its own way, so it
/// is compared by channel address only.
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
            let mut clash: Option<(PlcAddress, String)> = None;
            for a in &addresses {
                let shared = owner.get(a).cloned().or_else(|| {
                    if card.step.is_some() {
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
                if card.step.is_none() {
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
        assert_eq!(identifier("--"), "io_");
        assert_eq!(identifier("Ø-ventil"), "ventil");
        assert_eq!(comment("a (*b*) c\nd"), "a ( *b* ) c d");
    }
}
