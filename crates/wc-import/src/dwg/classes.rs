//! The classes of a DWG drawing (`AcDb:Classes`): the object types numbered 500 and up, each
//! with its DXF name and whether it is an entity. The reader uses them to name and count the
//! custom entities it does not draw (WIPEOUT, IMAGE, proxy entities), and to tell them from
//! custom non-graphic objects, which are not counted.
//!
//! Layout as LibreDWG's `read_2004_section_classes` reads it: a 16-byte sentinel, the size of
//! the class data (RL), for 2010 drawings of maintenance release 4 and later a second size (RL),
//! from 2007 the data's size in bits (RL); then the highest class number (BS), two bytes and a
//! bit, and for each class its number (BS), proxy flags (BS), application, C++ and DXF names,
//! whether it was a zombie (B), its item class id (BS: 0x1F2 entity, 0x1F3 object), the number
//! of instances, two version numbers and two unknown numbers (BL each). Names are TV in the data
//! (2004) or TU in a string stream at the end of the data (2010).

use std::collections::HashMap;

use super::bits::Bits;
use super::objects::string_stream;
use super::{Version, bad};
use crate::Result;

/// The sentinel the class data starts with.
const SENTINEL: [u8; 16] = [
    0x8D, 0xA1, 0xC4, 0xB8, 0xC4, 0xA9, 0xF8, 0xC5, 0xC0, 0xDC, 0xF4, 0x5F, 0xE7, 0xCF, 0xB6, 0x8A,
];

/// The item class id of a class whose objects are entities.
const ENTITY: u16 = 0x1F2;

/// The most classes a drawing may define.
const MAX_CLASSES: usize = 10_000;

/// One class: its DXF name and whether its objects are entities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Class {
    pub(crate) dxf_name: String,
    pub(crate) entity: bool,
}

/// Check that a CRC (RS) and the closing sentinel, the opening one with every bit flipped,
/// follow the class data area ending at byte `end`.
fn closing_sentinel(data: &[u8], end: usize) -> std::result::Result<(), &'static str> {
    let closing = end.checked_add(2).and_then(|at| data.get(at..at + SENTINEL.len()));
    match closing {
        None => Err("its closing sentinel is missing"),
        Some(c) if c.iter().zip(SENTINEL).all(|(a, b)| *a == !b) => Ok(()),
        Some(_) => Err("its closing sentinel is wrong"),
    }
}

/// The classes by number, read from the `AcDb:Classes` section `data` of a drawing of `version`
/// and maintenance release `maint` (byte 0x0B of the file header).
pub(crate) fn read_classes(data: &[u8], version: Version, maint: u8) -> Result<HashMap<u16, Class>> {
    // The reasons below are errors of their own; their "DWG: " is not repeated.
    let damaged = |why: &str| {
        let why = why.strip_prefix("DWG: ").unwrap_or(why);
        bad(format!("the class section (AcDb:Classes) is damaged: {why}"))
    };
    if !data.starts_with(&SENTINEL) {
        return Err(damaged("it does not start with its sentinel"));
    }
    let r2010 = version == Version::R2010;
    let mut head = Bits::new(data, SENTINEL.len() * 8, data.len() * 8);
    let size = head.rl().map_err(|e| damaged(&e.to_string()))? as usize;
    let mut strings = None;
    let data_end;
    if r2010 {
        // From maintenance release 4 a second size follows, and everything after moves 4 bytes.
        let second = maint > 3;
        if second {
            head.rl().map_err(|e| damaged(&e.to_string()))?;
        }
        let bits = head.rl().map_err(|e| damaged(&e.to_string()))? as usize;
        // The class data area starts after the size fields before the bit size (the first, and
        // the second from release 4) and holds the bit size, the data and the texts; it must fit
        // in the section.
        let area_end = (SENTINEL.len() + if second { 8 } else { 4 })
            .checked_add(size)
            .filter(|e| *e <= data.len())
            .ok_or_else(|| damaged("its size is past its end"))?;
        // The flag bit of the string stream lies `bits` after bit 191 (159 without the second
        // size).
        let flag_at = bits
            .checked_add(if second { 191 } else { 159 })
            .filter(|f| *f < data.len() * 8)
            .ok_or_else(|| damaged("its texts lie past its end"))?;
        // A CRC and the closing sentinel follow the area, as in 2004.
        closing_sentinel(data, area_end).map_err(&damaged)?;
        // The streams lie in that area.
        if flag_at / 8 >= area_end {
            return Err(damaged("its texts lie past its class data"));
        }
        let s = string_stream(data, flag_at + 1).map_err(|e| damaged(&e.to_string()))?;
        data_end = s.as_ref().map_or(flag_at, Bits::pos);
        strings = s;
    } else {
        // The data, then a CRC (RS) and the closing sentinel (16 bytes) must fit in the section.
        let end = (SENTINEL.len() + 4)
            .checked_add(size)
            .filter(|e| e.saturating_add(2 + SENTINEL.len()) <= data.len())
            .ok_or_else(|| damaged("its size is past its end, or its closing sentinel is missing"))?;
        closing_sentinel(data, end).map_err(&damaged)?;
        data_end = end * 8;
    }
    let mut d = Bits::new(data, head.pos(), data_end);
    let mut read = || -> Result<HashMap<u16, Class>> {
        let max = d.bs()?;
        d.rc()?;
        d.rc()?;
        d.b()?;
        let count = usize::from(max).saturating_sub(499);
        // No classes is written as 499 (or 0); anything else below 500 is no class number.
        if (max != 0 && max < 499) || count > MAX_CLASSES {
            return Err(bad(format!("it says the highest class is {max}")));
        }
        let mut classes = HashMap::new();
        for _ in 0..count {
            let number = d.bs()?;
            d.bs()?;
            let dxf_name = match strings.as_mut() {
                Some(s) => {
                    s.tu()?;
                    s.tu()?;
                    s.tu()?
                }
                None if r2010 => return Err(bad("its texts are missing")),
                None => {
                    d.t8()?;
                    d.t8()?;
                    d.t8()?
                }
            };
            d.b()?;
            let id = d.bs()?;
            for _ in 0..5 {
                d.bl()?;
            }
            let class = Class {
                dxf_name,
                entity: id == ENTITY,
            };
            if number < 500 || number > max {
                return Err(bad(format!("class {number} is outside 500 to {max}")));
            }
            if classes.insert(number, class).is_some() {
                return Err(bad(format!("class {number} is listed twice")));
            }
        }
        Ok(classes)
    };
    read().map_err(|e| damaged(&e.to_string()))
}
