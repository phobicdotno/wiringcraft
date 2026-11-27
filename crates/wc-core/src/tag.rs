//! Reference designations in the IEC 81346 style: `=FUNCTION+LOCATION-PRODUCT`.
//!
//! Each aspect is optional and may have several levels separated by `.` (for example
//! `=PUMPS.P1+CAB1.R2-K3`). The product aspect is what most people call the device tag.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Error, Result};

/// Hyphens, dashes and minus signs that a tag pasted from a document may carry for `-`.
const LOOKS_LIKE_DASH: [char; 9] = [
    '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}', '\u{2212}', '\u{FE63}', '\u{FF0D}',
];
/// Small and full-width plus signs, for `+`.
const LOOKS_LIKE_PLUS: [char; 2] = ['\u{FE62}', '\u{FF0B}'];
/// Small and full-width equals signs, for `=`.
const LOOKS_LIKE_EQUALS: [char; 2] = ['\u{FE66}', '\u{FF1D}'];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DeviceTag {
    pub function: Vec<String>,
    pub location: Vec<String>,
    pub product: Vec<String>,
}

impl DeviceTag {
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::Tag(s.into(), "empty"));
        }
        let mut tag = DeviceTag::default();
        let mut current: Option<char> = None;
        let mut buf = String::new();
        // Errors quote the whole tag as typed, so they name what the user entered.
        let flush = |tag: &mut DeviceTag, aspect: Option<char>, buf: &mut String| -> Result<()> {
            let Some(a) = aspect else {
                return if buf.is_empty() {
                    Ok(())
                } else {
                    Err(Error::Tag(s.into(), "must start with =, + or -"))
                };
            };
            let (target, why_unnamed, why_empty_level, why_twice) = match a {
                '=' => (
                    &mut tag.function,
                    "the function aspect (=) has no name",
                    "the function aspect (=) has an empty level",
                    "the function aspect (=) is given twice",
                ),
                '+' => (
                    &mut tag.location,
                    "the location aspect (+) has no name",
                    "the location aspect (+) has an empty level",
                    "the location aspect (+) is given twice",
                ),
                _ => (
                    &mut tag.product,
                    "the product aspect (-) has no name",
                    "the product aspect (-) has an empty level",
                    "the product aspect (-) is given twice",
                ),
            };
            if buf.is_empty() {
                return Err(Error::Tag(s.into(), why_unnamed));
            }
            let levels: Vec<String> = buf.split('.').map(str::to_owned).collect();
            if levels.iter().any(|l| l.is_empty()) {
                return Err(Error::Tag(s.into(), why_empty_level));
            }
            if !target.is_empty() {
                return Err(Error::Tag(s.into(), why_twice));
            }
            *target = levels;
            buf.clear();
            Ok(())
        };
        for c in s.chars() {
            match c {
                '=' | '+' | '-' => {
                    flush(&mut tag, current, &mut buf)?;
                    current = Some(c);
                }
                c if c.is_alphanumeric() || c == '.' || c == '_' => buf.push(c),
                // Pasted from a word processor or a PDF, a sign is often one that only looks
                // like the plain one.
                c if LOOKS_LIKE_DASH.contains(&c) => {
                    return Err(Error::Tag(s.into(), "a sign that only looks like -: type a plain -"));
                }
                c if LOOKS_LIKE_PLUS.contains(&c) => {
                    return Err(Error::Tag(s.into(), "a sign that only looks like +: type a plain +"));
                }
                c if LOOKS_LIKE_EQUALS.contains(&c) => {
                    return Err(Error::Tag(s.into(), "a sign that only looks like =: type a plain ="));
                }
                _ => return Err(Error::Tag(s.into(), "only letters, digits, '.', '_' allowed")),
            }
        }
        flush(&mut tag, current, &mut buf)?;
        if tag.product.is_empty() {
            return Err(Error::Tag(s.into(), "needs a product aspect such as -K1"));
        }
        Ok(tag)
    }

    /// A terminal's strip and number: `-X1.3` is terminal `3` of strip `-X1`. A tag with one
    /// product level (`-X1`) is the strip itself, without a number (one is given in drawing
    /// order). Every place that names a terminal's strip uses this, so they agree.
    pub fn terminal_parts(&self) -> (DeviceTag, Option<String>) {
        let mut strip = self.clone();
        let number = if strip.product.len() > 1 {
            strip.product.pop()
        } else {
            None
        };
        (strip, number)
    }

    /// The letter code of the product aspect: the leading letters of its last level, e.g. `K`
    /// for `-K12` and `XD` for `-XD3`. Empty when that level starts with a digit (as the
    /// terminal number in `-X1.3` does).
    pub fn letter_code(&self) -> String {
        self.product
            .last()
            .map(|p| p.chars().take_while(|c| c.is_alphabetic()).collect())
            .unwrap_or_default()
    }
}

impl fmt::Display for DeviceTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.function.is_empty() {
            write!(f, "={}", self.function.join("."))?;
        }
        if !self.location.is_empty() {
            write!(f, "+{}", self.location.join("."))?;
        }
        write!(f, "-{}", self.product.join("."))
    }
}

impl std::str::FromStr for DeviceTag {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl Serialize for DeviceTag {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DeviceTag {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        DeviceTag::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_tag_splits_into_its_strip_and_number() -> Result<()> {
        let (strip, n) = DeviceTag::parse("=A+C1-X1.3")?.terminal_parts();
        assert_eq!((strip.to_string(), n.as_deref()), ("=A+C1-X1".to_owned(), Some("3")));
        let (strip, n) = DeviceTag::parse("-X1")?.terminal_parts();
        assert_eq!((strip.to_string(), n), ("-X1".to_owned(), None));
        Ok(())
    }

    #[test]
    fn parses_full_designation() -> Result<()> {
        let t = DeviceTag::parse("=PUMPS.P1+CAB1-K3")?;
        assert_eq!(t.function, vec!["PUMPS", "P1"]);
        assert_eq!(t.location, vec!["CAB1"]);
        assert_eq!(t.product, vec!["K3"]);
        assert_eq!(t.to_string(), "=PUMPS.P1+CAB1-K3");
        assert_eq!(t.letter_code(), "K");
        Ok(())
    }

    #[test]
    fn letter_code_is_the_leading_letters_of_the_last_product_level() -> Result<()> {
        assert_eq!(DeviceTag::parse("-K12")?.letter_code(), "K");
        assert_eq!(DeviceTag::parse("=A+C1-XD3")?.letter_code(), "XD");
        assert_eq!(DeviceTag::parse("-X1.3")?.letter_code(), "");
        Ok(())
    }

    #[test]
    fn product_only() -> Result<()> {
        assert_eq!(DeviceTag::parse("-Q1")?.to_string(), "-Q1");
        Ok(())
    }

    #[test]
    fn errors_quote_the_whole_tag_and_name_the_aspect() {
        for (bad, why) in [
            ("-K1-K2", "the product aspect (-) is given twice"),
            ("=A+B=C-K1", "the function aspect (=) is given twice"),
            ("-K1.", "the product aspect (-) has an empty level"),
            ("+C1..R2-K1", "the location aspect (+) has an empty level"),
            ("=-K1", "the function aspect (=) has no name"),
            ("K1", "must start with =, + or -"),
        ] {
            let Err(e) = DeviceTag::parse(bad) else {
                unreachable!("{bad} should fail");
            };
            assert_eq!(e.to_string(), format!("invalid device tag {bad:?}: {why}"));
        }
    }

    #[test]
    fn a_look_alike_sign_names_the_plain_one_to_type() {
        let cases = [
            (&LOOKS_LIKE_DASH[..], "=A+C1{}K1", '-'),
            (&LOOKS_LIKE_PLUS[..], "=A{}C1-K1", '+'),
            (&LOOKS_LIKE_EQUALS[..], "{}A+C1-K1", '='),
        ];
        for (signs, shape, plain) in cases {
            for &c in signs {
                let bad = shape.replace("{}", &c.to_string());
                let Err(e) = DeviceTag::parse(&bad) else {
                    unreachable!("{bad} should fail");
                };
                let why = format!("a sign that only looks like {plain}: type a plain {plain}");
                assert_eq!(e.to_string(), format!("invalid device tag {bad:?}: {why}"));
            }
        }
        // Every look-alike is caught, not only the common en dash.
        let all = [LOOKS_LIKE_DASH.len(), LOOKS_LIKE_PLUS.len(), LOOKS_LIKE_EQUALS.len()];
        assert_eq!(all, [9, 2, 2]);
    }

    #[test]
    fn rejects_bad_tags() {
        for bad in ["", "K1", "=A1", "-K1-K2", "-K 1", "=-K1", "-K1."] {
            assert!(DeviceTag::parse(bad).is_err(), "{bad} should fail");
        }
    }
}
