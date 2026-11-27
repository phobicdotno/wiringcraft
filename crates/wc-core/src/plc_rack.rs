//! PLC racks: the head and the I/O cards in its slots, with how many channels each card has and
//! the address of its first channel. The PLC card overview report lays every card out channel by
//! channel and shows the drawn PLC input or output at each address.
//!
//! They are kept in `project.toml` as `[[rack]]` tables, each with its `[[rack.card]]`s:
//!
//! ```toml
//! [[rack]]
//! name = "-A1"
//! description = "Main PLC"
//!
//! [[rack.card]]
//! slot = 1
//! tag = "-A1.DI1"
//! type = "DI 16x24 V DC"
//! channels = 16
//! start = "%IX0.0"
//!
//! [[rack.card]]
//! slot = 2
//! type = "AI 4x 4-20 mA"
//! channels = 4
//! start = "%IW64"
//! ```
//!
//! The start address says whether the card has inputs (`%I`, `I`, `E`) or outputs (`%Q`, `Q`,
//! `A`), and its size: a bit address (`%IX0.0`) counts channels bit by bit, a byte, word or
//! double word address (`%IB4`, `%IW64`, `%QD8`) moves 1, 2 or 4 bytes per channel. `step`
//! replaces that distance for PLCs that number words one by one (`step = 1` for `%IW0`, `%IW1`).
//! Unknown keys are kept when the project is saved.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One PLC rack (or station): a head and its cards.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PlcRack {
    /// What the rack is called on the overview pages, usually the PLC's device tag.
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The cards, in any order; the overview sorts them by slot.
    #[serde(default, rename = "card", skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<PlcCard>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: BTreeMap<String, toml::Value>,
}

/// One I/O card in a rack slot.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PlcCard {
    #[serde(default)]
    pub slot: u32,
    /// The card's device tag, when it has one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tag: String,
    /// What the card is (`DI 16x24 V DC`, an order number).
    #[serde(default, rename = "type", skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default)]
    pub channels: u32,
    /// The address of channel 0.
    #[serde(default)]
    pub start: String,
    /// Bytes from one channel to the next for byte, word and double word cards; `None` is the
    /// size of the address (1, 2 or 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: BTreeMap<String, toml::Value>,
}

/// Most channels a card may have: more is a typing error, and would fill pages with rows.
pub const MAX_CHANNELS: u32 = 256;

/// The highest slot number a card may have in the PLC racks form and the command line.
pub const MAX_SLOT: u32 = 999;

/// The largest channel step the PLC racks form and the command line take.
pub const MAX_STEP: u32 = 64;

/// Why `racks` holds a number the PLC racks form would not let the user type (a slot past
/// [`MAX_SLOT`], more than [`MAX_CHANNELS`] channels, a step outside 1 to [`MAX_STEP`]);
/// `None` when every number is in range. The assistant and the command line check racks they
/// are given by this.
pub fn problem(racks: &[PlcRack]) -> Option<String> {
    racks.iter().find_map(|rack| {
        rack.cards.iter().find_map(|c| {
            let at = || format!("rack {:?}, card in slot {}", rack.name, c.slot);
            if c.slot > MAX_SLOT {
                Some(format!("{}: the slot must be from 0 to {MAX_SLOT}", at()))
            } else if c.channels > MAX_CHANNELS {
                Some(format!("{}: a card has at most {MAX_CHANNELS} channels", at()))
            } else if c.step.is_some_and(|s| !(1..=MAX_STEP).contains(&s)) {
                Some(format!("{}: the step must be from 1 to {MAX_STEP}", at()))
            } else {
                None
            }
        })
    })
}

/// A PLC address read from one of its common spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlcAddress {
    /// `true` for an output (`%Q`, `Q`, `A`).
    pub output: bool,
    /// `X` (bit), `B`, `W` or `D`.
    pub size: char,
    /// The byte (or word) number.
    pub number: u64,
    /// The bit number, for a bit address.
    pub bit: Option<u64>,
}

impl PlcAddress {
    /// Reads `%IX0.0`, `%I0.0`, `I0.0`, `E0.0`, `IW64`, `A4.1`, `%QD8` and the like (spaces and
    /// case do not matter). `None` when it is not a PLC address, a bit number is past 7 or a
    /// bit address has no bit (or a word address has one).
    pub fn parse(address: &str) -> Option<Self> {
        Self::parse_any_bit(address).filter(|a| a.bit.is_none_or(|b| b <= 7))
    }

    /// As [`PlcAddress::parse`], but any bit number is taken (`%IX0.10`), as some lists number
    /// bits that way; such an address names no channel of a card.
    pub fn parse_any_bit(address: &str) -> Option<Self> {
        let a: String = address
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        let a = a.strip_prefix('%').unwrap_or(&a);
        let mut chars = a.chars();
        let output = match chars.next()? {
            'I' | 'E' => false,
            'Q' | 'A' => true,
            _ => return None,
        };
        let rest = chars.as_str();
        let (size, number) = match rest.chars().next()? {
            c @ ('X' | 'B' | 'W' | 'D') => (Some(c), rest.get(1..)?),
            _ => (None, rest),
        };
        let digits = |p: &str| -> Option<u64> {
            (!p.is_empty() && p.len() <= 18 && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse().ok())
                .flatten()
        };
        let mut parts = number.split('.');
        let number = digits(parts.next()?)?;
        let bit = match parts.next() {
            Some(b) => Some(digits(b)?),
            None => None,
        };
        if parts.next().is_some() {
            return None;
        }
        let size = match size {
            Some(s) => s,
            None if bit.is_some() => 'X',
            None => return None,
        };
        if bit.is_some() != (size == 'X') {
            return None;
        }
        Some(Self {
            output,
            size,
            number,
            bit,
        })
    }

    /// The IEC 61131-3 elementary type of a variable at this address: `BOOL`, `BYTE`, `WORD`
    /// or `DWORD`.
    pub fn elementary_type(&self) -> &'static str {
        match self.size {
            'X' => "BOOL",
            'B' => "BYTE",
            'W' => "WORD",
            _ => "DWORD",
        }
    }

    /// The bits it covers, counted from bit 0 of byte 0 of its area: one bit for a bit
    /// address, 8, 16 or 32 for a byte, word or double word (byte numbering).
    pub fn bits(&self) -> std::ops::Range<u64> {
        let start = self.number.saturating_mul(8).saturating_add(self.bit.unwrap_or(0));
        let len = match self.bit {
            Some(_) => 1,
            None => self.bytes() * 8,
        };
        start..start.saturating_add(len)
    }

    /// In the IEC 61131-3 spelling: `%IX0.2`, `%QW64`.
    pub fn iec(&self) -> String {
        let area = if self.output { 'Q' } else { 'I' };
        match self.bit {
            Some(b) => format!("%{area}{}{}.{b}", self.size, self.number),
            None => format!("%{area}{}{}", self.size, self.number),
        }
    }

    /// Bytes a channel of this size takes.
    fn bytes(&self) -> u64 {
        match self.size {
            'W' => 2,
            'D' => 4,
            _ => 1,
        }
    }
}

impl PlcCard {
    /// The address of each channel, from channel 0. An error says why there are none: a start
    /// that is not a PLC address, no channels, too many, or a step of 0.
    pub fn addresses(&self) -> Result<Vec<PlcAddress>, String> {
        let start = PlcAddress::parse(&self.start)
            .ok_or_else(|| format!("start address {:?} is not a PLC address", self.start))?;
        if self.channels == 0 {
            return Err("no channels".into());
        }
        if self.channels > MAX_CHANNELS {
            return Err(format!("{} channels, at most {MAX_CHANNELS}", self.channels));
        }
        let n = u64::from(self.channels);
        Ok(match start.bit {
            Some(bit) => {
                let first = start.number.checked_mul(8).and_then(|b| b.checked_add(bit));
                let first = first.ok_or("start address too large")?;
                (0..n)
                    .map(|k| PlcAddress {
                        number: (first + k) / 8,
                        bit: Some((first + k) % 8),
                        ..start
                    })
                    .collect()
            }
            None => {
                let step = self.step.map_or(start.bytes(), u64::from);
                if step == 0 {
                    return Err("step is 0".into());
                }
                if start.number.checked_add(step * n).is_none() {
                    return Err("start address too large".into());
                }
                (0..n)
                    .map(|k| PlcAddress {
                        number: start.number + step * k,
                        ..start
                    })
                    .collect()
            }
        })
    }

    /// What the card is called on the overview: its tag, else its type, else its slot.
    pub fn label(&self) -> String {
        if !self.tag.is_empty() {
            self.tag.clone()
        } else if !self.kind.is_empty() {
            self.kind.clone()
        } else {
            format!("slot {}", self.slot)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn addresses_in_the_common_spellings() {
        let iec = |a: &str| PlcAddress::parse(a).map(|a| a.iec());
        assert_eq!(iec("%IX0.0").as_deref(), Some("%IX0.0"));
        assert_eq!(iec("i 1.07").as_deref(), Some("%IX1.7"));
        assert_eq!(iec("E2.3").as_deref(), Some("%IX2.3"));
        assert_eq!(iec("A4.1").as_deref(), Some("%QX4.1"));
        assert_eq!(iec("IW064").as_deref(), Some("%IW64"));
        assert_eq!(iec("%qd8").as_deref(), Some("%QD8"));
        for bad in ["", "I", "IX3", "IW64.1", "%I0.8", "DI1", "%I0..1", "%I0.1.2", "Slot 2"] {
            assert_eq!(PlcAddress::parse(bad), None, "{bad}");
        }
        // Bit 10 is read only when asked for.
        assert_eq!(
            PlcAddress::parse_any_bit("%I0.10").map(|a| a.iec()).as_deref(),
            Some("%IX0.10")
        );
        assert_eq!(PlcAddress::parse_any_bit("%I0..1"), None);
        // The bits an address covers.
        let bits = |a: &str| PlcAddress::parse(a).map(|a| a.bits());
        assert_eq!(bits("%IX1.2"), Some(10..11));
        assert_eq!(bits("%IW64"), Some(512..528));
        assert_eq!(bits("%QB65"), Some(520..528));
    }

    #[test]
    fn channel_addresses_count_bits_and_words() {
        let card = |start: &str, channels: u32, step: Option<u32>| PlcCard {
            start: start.into(),
            channels,
            step,
            ..Default::default()
        };
        let list = |c: PlcCard| c.addresses().map(|a| a.iter().map(PlcAddress::iec).collect::<Vec<_>>());
        assert_eq!(
            list(card("%IX0.6", 4, None)).unwrap(),
            ["%IX0.6", "%IX0.7", "%IX1.0", "%IX1.1"]
        );
        assert_eq!(list(card("QW64", 3, None)).unwrap(), ["%QW64", "%QW66", "%QW68"]);
        assert_eq!(list(card("%IW0", 2, Some(1))).unwrap(), ["%IW0", "%IW1"]);
        assert!(list(card("Slot 1", 4, None)).is_err());
        assert!(list(card("%IX0.0", 0, None)).is_err());
        assert!(list(card("%IX0.0", MAX_CHANNELS + 1, None)).is_err());
        assert!(list(card("%IW0", 2, Some(0))).is_err());
    }

    #[test]
    fn problem_names_the_first_number_out_of_range() {
        let rack = |card: PlcCard| PlcRack {
            name: "-A1".into(),
            cards: vec![card],
            ..PlcRack::default()
        };
        let ok = PlcCard {
            slot: MAX_SLOT,
            channels: MAX_CHANNELS,
            step: Some(MAX_STEP),
            ..PlcCard::default()
        };
        assert_eq!(problem(&[rack(ok.clone())]), None);
        for (bad, why) in [
            (
                PlcCard {
                    slot: MAX_SLOT + 1,
                    ..ok.clone()
                },
                "the slot must be from 0 to 999",
            ),
            (
                PlcCard {
                    channels: MAX_CHANNELS + 1,
                    ..ok.clone()
                },
                "at most 256 channels",
            ),
            (
                PlcCard {
                    step: Some(0),
                    ..ok.clone()
                },
                "the step must be from 1 to 64",
            ),
            (
                PlcCard {
                    step: Some(MAX_STEP + 1),
                    ..ok.clone()
                },
                "the step must be from 1 to 64",
            ),
        ] {
            let got = problem(&[rack(bad)]).unwrap_or_default();
            assert!(got.contains(why) && got.starts_with("rack \"-A1\""), "{got}");
        }
    }

    #[test]
    fn racks_round_trip_through_toml_with_unknown_keys() {
        let text = r#"
name = "-A1"
description = "Main PLC"
colour = "grey"

[[card]]
slot = 1
tag = "-A1.DI1"
type = "DI 16"
channels = 16
start = "%IX0.0"
"#;
        let rack: PlcRack = toml::from_str(text).unwrap();
        assert_eq!(rack.cards[0].kind, "DI 16");
        assert_eq!(rack.cards[0].label(), "-A1.DI1");
        assert!(rack.unknown.contains_key("colour"));
        let back: PlcRack = toml::from_str(&toml::to_string(&rack).unwrap()).unwrap();
        assert_eq!(back, rack);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    fn card(start: &str, channels: u32, step: Option<u32>) -> PlcCard {
        PlcCard {
            start: start.into(),
            channels,
            step,
            ..PlcCard::default()
        }
    }

    fn iecs(c: &PlcCard) -> Vec<String> {
        let Ok(list) = c.addresses() else {
            unreachable!("{:?} should have addresses", c.start)
        };
        list.iter().map(PlcAddress::iec).collect()
    }

    #[test]
    fn german_and_byte_spellings_read_as_their_iec_area() {
        let parsed = |a: &str| PlcAddress::parse(a);
        assert_eq!(
            parsed("EB4"),
            Some(PlcAddress {
                output: false,
                size: 'B',
                number: 4,
                bit: None
            })
        );
        assert_eq!(parsed("aw 2").map(|a| a.iec()).as_deref(), Some("%QW2"));
        assert_eq!(parsed(" % q x 3 . 5 ").map(|a| a.iec()).as_deref(), Some("%QX3.5"));
        // Only one leading percent sign, and a size letter needs a number after it.
        for bad in [
            "%", "%%I0.0", "ID", "QB", "I-1.0", "I+1.0", "IX.1", "I1.", "MW10", "IZ4",
        ] {
            assert_eq!(parsed(bad), None, "{bad}");
        }
    }

    #[test]
    fn numbers_take_up_to_eighteen_digits() {
        let eighteen = "9".repeat(18);
        let Some(a) = PlcAddress::parse(&format!("IW{eighteen}")) else {
            unreachable!("18 digits is a number")
        };
        assert_eq!(a.number, 999_999_999_999_999_999);
        assert_eq!(PlcAddress::parse(&format!("IW{eighteen}9")), None);
        assert_eq!(PlcAddress::parse_any_bit(&format!("I0.{eighteen}9")), None);
    }

    #[test]
    fn elementary_types_follow_the_size() {
        let ty = |a: &str| PlcAddress::parse(a).map(|a| a.elementary_type());
        assert_eq!(ty("%IX0.0"), Some("BOOL"));
        assert_eq!(ty("%IB0"), Some("BYTE"));
        assert_eq!(ty("%QW0"), Some("WORD"));
        assert_eq!(ty("%QD0"), Some("DWORD"));
    }

    #[test]
    fn bits_of_a_double_word_and_of_a_wide_bit_number() {
        assert_eq!(PlcAddress::parse("%ID8").map(|a| a.bits()), Some(64..96));
        // A bit past 7 still counts from bit 0 of its byte.
        assert_eq!(PlcAddress::parse_any_bit("%IX1.10").map(|a| a.bits()), Some(18..19));
    }

    #[test]
    fn channels_follow_the_size_or_the_step() {
        assert_eq!(iecs(&card("%QD8", 3, None)), ["%QD8", "%QD12", "%QD16"]);
        assert_eq!(iecs(&card("%IB4", 2, None)), ["%IB4", "%IB5"]);
        assert_eq!(iecs(&card("%IB4", 3, Some(4))), ["%IB4", "%IB8", "%IB12"]);
        // A bit card counts bit by bit whatever the step says.
        assert_eq!(iecs(&card("%IX0.7", 2, Some(4))), ["%IX0.7", "%IX1.0"]);
        // Exactly the most channels is taken, and keeps the area.
        let Ok(full) = card("A0.0", MAX_CHANNELS, None).addresses() else {
            unreachable!()
        };
        assert_eq!(full.len(), MAX_CHANNELS as usize);
        assert!(full.iter().all(|a| a.output));
        assert_eq!(full.last().map(PlcAddress::iec).as_deref(), Some("%QX31.7"));
    }

    #[test]
    fn a_start_past_bit_seven_names_no_channel() {
        let Err(why) = card("%IX0.8", 4, None).addresses() else {
            unreachable!("bit 8 is no channel")
        };
        assert!(why.contains("\"%IX0.8\""), "{why}");
        let Err(why) = card("%IW0", 0, Some(1)).addresses() else {
            unreachable!()
        };
        assert_eq!(why, "no channels");
    }

    #[test]
    fn labels_fall_back_from_tag_to_type_to_slot() {
        let mut c = PlcCard {
            slot: 7,
            tag: "-A1.DO2".into(),
            kind: "DO 8".into(),
            ..PlcCard::default()
        };
        assert_eq!(c.label(), "-A1.DO2");
        c.tag.clear();
        assert_eq!(c.label(), "DO 8");
        c.kind.clear();
        assert_eq!(c.label(), "slot 7");
    }

    #[test]
    fn problem_looks_through_every_rack_and_checks_the_slot_first() {
        assert_eq!(problem(&[]), None);
        let good = PlcRack {
            name: "-A1".into(),
            cards: vec![PlcCard::default()],
            ..PlcRack::default()
        };
        let bad = PlcRack {
            name: "-A2".into(),
            cards: vec![
                PlcCard::default(),
                PlcCard {
                    slot: MAX_SLOT + 1,
                    channels: MAX_CHANNELS + 1,
                    ..PlcCard::default()
                },
            ],
            ..PlcRack::default()
        };
        let got = problem(&[good, bad]).unwrap_or_default();
        assert!(got.starts_with("rack \"-A2\", card in slot 1000"), "{got}");
        assert!(got.contains("the slot must be"), "{got}");
        // A step of 1, the least, is in range.
        let one = PlcRack {
            cards: vec![PlcCard {
                step: Some(1),
                ..PlcCard::default()
            }],
            ..PlcRack::default()
        };
        assert_eq!(problem(&[one]), None);
    }

    #[test]
    fn empty_fields_are_left_out_when_saved() {
        let rack = PlcRack {
            name: "-A1".into(),
            cards: vec![card("%IW0", 2, None)],
            ..PlcRack::default()
        };
        let Ok(text) = toml::to_string(&rack) else {
            unreachable!()
        };
        for absent in ["description", "tag", "type", "step"] {
            assert!(!text.contains(absent), "{absent} in {text}");
        }
        let Ok(back) = toml::from_str::<PlcRack>(&text) else {
            unreachable!()
        };
        assert_eq!(back, rack);
        // A rack with no keys at all reads as the default.
        assert_eq!(toml::from_str::<PlcRack>("").ok(), Some(PlcRack::default()));
    }
}
