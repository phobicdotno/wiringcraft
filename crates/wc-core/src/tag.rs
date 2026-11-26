//! Reference designations in the IEC 81346 style: `=FUNCTION+LOCATION-PRODUCT`.
//!
//! Each aspect is optional and may have several levels separated by `.` (for example
//! `=PUMPS.P1+CAB1.R2-K3`). The product aspect is what most people call the device tag.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Error, Result};

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
        let flush = |tag: &mut DeviceTag, aspect: Option<char>, buf: &mut String| -> Result<()> {
            let Some(a) = aspect else {
                return if buf.is_empty() {
                    Ok(())
                } else {
                    Err(Error::Tag(buf.clone(), "must start with =, + or -"))
                };
            };
            if buf.is_empty() {
                return Err(Error::Tag(format!("{a}"), "aspect without a name"));
            }
            let levels: Vec<String> = buf.split('.').map(str::to_owned).collect();
            if levels.iter().any(|l| l.is_empty()) {
                return Err(Error::Tag(buf.clone(), "empty level"));
            }
            let target = match a {
                '=' => &mut tag.function,
                '+' => &mut tag.location,
                _ => &mut tag.product,
            };
            if !target.is_empty() {
                return Err(Error::Tag(buf.clone(), "aspect given twice"));
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
    fn rejects_bad_tags() {
        for bad in ["", "K1", "=A1", "-K1-K2", "-K 1", "=-K1", "-K1."] {
            assert!(DeviceTag::parse(bad).is_err(), "{bad} should fail");
        }
    }
}
