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

use crate::{Inputs, Table, natural_key, plc_io_list};

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

/// A located address and the elementary type that goes with its size.
fn location(address: &str, output: bool) -> Option<(String, &'static str)> {
    let a: String = address
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let a = a.strip_prefix('%').unwrap_or(&a);
    let mut chars = a.chars();
    let area = match chars.next()? {
        'I' | 'E' => 'I',
        'Q' | 'A' => 'Q',
        _ => return None,
    };
    if (area == 'Q') != output {
        return None;
    }
    let rest = chars.as_str();
    let (size, number) = match rest.chars().next()? {
        c @ ('X' | 'B' | 'W' | 'D') => (Some(c), rest.get(1..)?),
        _ => (None, rest),
    };
    let parts: Vec<&str> = number.split('.').collect();
    if parts.is_empty()
        || parts.len() > 2
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let bit = parts.len() == 2;
    let size = match size {
        Some(s) => s,
        None if bit => 'X',
        None => return None,
    };
    // A bit address names a bit, a byte or word address does not.
    if bit != (size == 'X') {
        return None;
    }
    let ty = match size {
        'X' => "BOOL",
        'B' => "BYTE",
        'W' => "WORD",
        _ => "DWORD",
    };
    let number: Vec<String> = parts.iter().map(|p| p.trim_start_matches('0').to_owned()).collect();
    let number: Vec<&str> = number
        .iter()
        .map(|p| if p.is_empty() { "0" } else { p.as_str() })
        .collect();
    Some((format!("%{area}{size}{}", number.join(".")), ty))
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

/// The drawn PLC inputs and outputs by address, from the PLC I/O list: (I/O list row, page id).
fn points_by_address(i: &Inputs<'_>) -> BTreeMap<PlcAddress, Vec<(Vec<String>, String)>> {
    let page_id: BTreeMap<&str, &str> = i
        .project
        .pages
        .iter()
        .map(|p| (p.number.as_str(), p.id.as_str()))
        .collect();
    let mut out: BTreeMap<PlcAddress, Vec<(Vec<String>, String)>> = BTreeMap::new();
    for row in plc_io_list(i).rows {
        let [address, kind, ..] = row.as_slice() else { continue };
        let Some(a) = PlcAddress::parse(address) else { continue };
        if a.output != (kind == "Output") {
            continue;
        }
        let page = page_id.get(row[5].as_str()).copied().unwrap_or_default().to_owned();
        out.entry(a).or_default().push((row, page));
    }
    out
}

/// PLC card overview: every card of every rack in the project ([`wc_core::plc_rack`]), slot by
/// slot and channel by channel, with the PLC input or output drawn at each channel's address:
/// the same device, description, connection and page as in the PLC I/O list. A channel with
/// nothing drawn is listed empty (a spare); two points on one address give two rows. Rows with a
/// drawn point link to its page. Cards whose channels cannot be worked out are left out
/// ([`plc_rack_warnings`] says why).
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
    if i.project.meta.racks.is_empty() {
        return t;
    }
    let points = points_by_address(i);
    for rack in &i.project.meta.racks {
        let mut cards: Vec<_> = rack.cards.iter().collect();
        cards.sort_by_key(|c| c.slot);
        for card in cards {
            let Ok(addresses) = card.addresses() else { continue };
            for (ch, a) in addresses.iter().enumerate() {
                let head = vec![
                    rack.name.clone(),
                    card.slot.to_string(),
                    card.label(),
                    card.kind.clone(),
                    ch.to_string(),
                    a.iec(),
                ];
                match points.get(a) {
                    Some(drawn) => {
                        for (row, page) in drawn {
                            let mut r = head.clone();
                            r.extend([row[2].clone(), row[3].clone(), row[4].clone(), row[5].clone()]);
                            t.row_pages.resize(t.rows.len(), String::new());
                            t.row_pages.push(page.clone());
                            t.rows.push(r);
                        }
                    }
                    None => {
                        let mut r = head;
                        r.extend(std::iter::repeat_n(String::new(), 4));
                        t.rows.push(r);
                    }
                }
            }
        }
    }
    t
}

/// What is wrong with the project's PLC racks: a card whose channels cannot be worked out, two
/// cards in one slot, an address on two cards, and drawn PLC inputs and outputs whose address is
/// on no card. Empty when there are no racks or they are all fine.
pub fn plc_rack_warnings(i: &Inputs<'_>) -> Vec<String> {
    let racks = &i.project.meta.racks;
    let mut out = Vec::new();
    if racks.is_empty() {
        return out;
    }
    // address -> the first card that has it, as "rack slot n"
    let mut owner: BTreeMap<PlcAddress, String> = BTreeMap::new();
    for rack in racks {
        let mut slots = BTreeSet::new();
        for card in &rack.cards {
            let here = format!("PLC rack \"{}\" slot {}", rack.name, card.slot);
            if !slots.insert(card.slot) {
                out.push(format!("{here}: a second card in this slot"));
            }
            match card.addresses() {
                Err(e) => out.push(format!("{here}: {e}, left off the PLC card overview")),
                Ok(addresses) => {
                    for a in addresses {
                        match owner.get(&a) {
                            Some(first) => out.push(format!("{here}: address {} is also on {first}", a.iec())),
                            None => {
                                owner.insert(a, here.clone());
                            }
                        }
                    }
                }
            }
        }
    }
    for (a, drawn) in points_by_address(i) {
        if owner.contains_key(&a) {
            continue;
        }
        for (row, _) in drawn {
            out.push(format!(
                "PLC {} {} at {} (page {}) is on no card of the PLC racks",
                row[1].to_ascii_lowercase(),
                row[2],
                a.iec(),
                row[5]
            ));
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
