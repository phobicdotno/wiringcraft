//! The bit-packed values DWG objects are written in (#162).
//!
//! Object data is a stream of bits, read from the most significant bit of each byte down. Most
//! values are short codes: a two-bit prefix says whether a full value follows or a common one
//! (zero, one, 256) is meant. Every read is checked against the end of the stream, so damaged
//! data gives an error, never a panic.

use super::bad;
use crate::Result;

/// A cursor over a range of bits in a byte buffer.
#[derive(Debug, Clone)]
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// The next bit to read, counted from the start of `data`.
    pos: usize,
    /// One past the last bit this cursor may read.
    end: usize,
}

impl<'a> Bits<'a> {
    /// A cursor over bits `start..end` of `data`. The range is clamped to the buffer.
    pub(crate) fn new(data: &'a [u8], start: usize, end: usize) -> Self {
        let end = end.min(data.len().saturating_mul(8));
        Bits {
            data,
            pos: start.min(end),
            end,
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    fn bit(&mut self) -> Result<bool> {
        if self.pos >= self.end {
            return Err(bad("an object's data ends too early"));
        }
        let byte = self.data.get(self.pos / 8).copied().unwrap_or(0);
        let b = byte & (0x80 >> (self.pos % 8)) != 0;
        self.pos += 1;
        Ok(b)
    }

    /// `n` bits (at most 32) as a number, the first bit read the highest.
    fn bits(&mut self, n: u32) -> Result<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | u32::from(self.bit()?);
        }
        Ok(v)
    }

    /// B: one bit.
    pub(crate) fn b(&mut self) -> Result<bool> {
        self.bit()
    }

    /// BB: two bits.
    pub(crate) fn bb(&mut self) -> Result<u8> {
        Ok(self.bits(2)? as u8)
    }

    /// RC: a raw byte.
    pub(crate) fn rc(&mut self) -> Result<u8> {
        Ok(self.bits(8)? as u8)
    }

    /// RS: a raw little-endian 16-bit number.
    pub(crate) fn rs(&mut self) -> Result<u16> {
        let lo = u16::from(self.rc()?);
        let hi = u16::from(self.rc()?);
        Ok(lo | (hi << 8))
    }

    /// RL: a raw little-endian 32-bit number.
    pub(crate) fn rl(&mut self) -> Result<u32> {
        let lo = u32::from(self.rs()?);
        let hi = u32::from(self.rs()?);
        Ok(lo | (hi << 16))
    }

    fn raw_u64(&mut self) -> Result<u64> {
        let lo = u64::from(self.rl()?);
        let hi = u64::from(self.rl()?);
        Ok(lo | (hi << 32))
    }

    /// RD: a raw little-endian double.
    pub(crate) fn rd(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.raw_u64()?))
    }

    /// BS: a short with a two-bit prefix.
    pub(crate) fn bs(&mut self) -> Result<u16> {
        Ok(match self.bb()? {
            0 => self.rs()?,
            1 => u16::from(self.rc()?),
            2 => 0,
            _ => 256,
        })
    }

    /// BL: a long with a two-bit prefix.
    pub(crate) fn bl(&mut self) -> Result<u32> {
        match self.bb()? {
            0 => self.rl(),
            1 => Ok(u32::from(self.rc()?)),
            2 => Ok(0),
            _ => Err(bad("an object holds a long number in an unknown form")),
        }
    }

    /// BLL: a 64-bit number as a three-bit byte count and that many bytes, lowest first.
    pub(crate) fn bll(&mut self) -> Result<u64> {
        let n = self.bits(3)?;
        let mut v = 0u64;
        for i in 0..n {
            v |= u64::from(self.rc()?) << (8 * i);
        }
        Ok(v)
    }

    /// BD: a double with a two-bit prefix.
    pub(crate) fn bd(&mut self) -> Result<f64> {
        match self.bb()? {
            0 => self.rd(),
            1 => Ok(1.0),
            2 => Ok(0.0),
            _ => Err(bad("an object holds a number in an unknown form")),
        }
    }

    /// DD: a double written as the bytes that differ from `default`.
    pub(crate) fn dd(&mut self, default: f64) -> Result<f64> {
        let mut bytes = default.to_le_bytes();
        match self.bb()? {
            0 => {}
            1 => {
                for b in bytes.iter_mut().take(4) {
                    *b = self.rc()?;
                }
            }
            2 => {
                bytes[4] = self.rc()?;
                bytes[5] = self.rc()?;
                for b in bytes.iter_mut().take(4) {
                    *b = self.rc()?;
                }
            }
            _ => return self.rd(),
        }
        Ok(f64::from_le_bytes(bytes))
    }

    /// 2RD: two raw doubles.
    pub(crate) fn rd2(&mut self) -> Result<(f64, f64)> {
        Ok((self.rd()?, self.rd()?))
    }

    /// 3BD: three doubles with prefixes.
    pub(crate) fn bd3(&mut self) -> Result<(f64, f64, f64)> {
        Ok((self.bd()?, self.bd()?, self.bd()?))
    }

    /// BT: a thickness, zero when the first bit is set.
    pub(crate) fn bt(&mut self) -> Result<f64> {
        if self.b()? { Ok(0.0) } else { self.bd() }
    }

    /// BE: an extrusion direction, (0, 0, 1) when the first bit is set.
    pub(crate) fn be(&mut self) -> Result<(f64, f64, f64)> {
        if self.b()? { Ok((0.0, 0.0, 1.0)) } else { self.bd3() }
    }

    /// H: a handle reference, as its code (how it relates to the object it is in) and value.
    pub(crate) fn h(&mut self) -> Result<(u8, u64)> {
        let head = self.rc()?;
        let mut v = 0u64;
        for _ in 0..(head & 0x0F) {
            v = (v << 8) | u64::from(self.rc()?);
        }
        Ok((head >> 4, v))
    }

    /// A handle reference made absolute, for an object whose own handle is `own`.
    pub(crate) fn handle(&mut self, own: u64) -> Result<u64> {
        let (code, v) = self.h()?;
        Ok(match code {
            0x6 => own.wrapping_add(1),
            0x8 => own.wrapping_sub(1),
            0xA => own.wrapping_add(v),
            0xC => own.wrapping_sub(v),
            _ => v,
        })
    }

    /// Skip `n` bytes.
    pub(crate) fn skip_bytes(&mut self, n: u64) -> Result<()> {
        let bits = usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_mul(8))
            .filter(|b| self.pos.saturating_add(*b) <= self.end)
            .ok_or_else(|| bad("an object's data ends too early"))?;
        self.pos += bits;
        Ok(())
    }

    /// The bytes of a TV of the 2004 format, without trailing zeros, to decode in the drawing's
    /// code page.
    pub(crate) fn t8_bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.bs()?;
        let mut bytes = Vec::with_capacity(usize::from(n));
        for _ in 0..n {
            bytes.push(self.rc()?);
        }
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        Ok(bytes)
    }

    /// TU: a text of the 2007 and later format, a length and that many UTF-16 units.
    pub(crate) fn tu(&mut self) -> Result<String> {
        let n = self.bs()?;
        let mut units = Vec::with_capacity(usize::from(n));
        for _ in 0..n {
            units.push(self.rs()?);
        }
        while units.last() == Some(&0) {
            units.pop();
        }
        Ok(String::from_utf16_lossy(&units))
    }

    /// Read a raw 16-bit number at bit `at` without moving the cursor.
    pub(crate) fn rs_at(&self, at: usize) -> Result<u16> {
        let mut c = Bits {
            data: self.data,
            pos: at,
            end: self.end,
        };
        c.rs()
    }

    /// Read one bit at bit `at` without moving the cursor.
    pub(crate) fn bit_at(&self, at: usize) -> Result<bool> {
        let mut c = Bits {
            data: self.data,
            pos: at,
            end: self.end,
        };
        c.bit()
    }
}

/// MS: a modular short at the start of `bytes`: 15 bits per little-endian word, the top bit
/// saying another word follows. Returns the value and the number of bytes read.
pub(crate) fn modular_short(bytes: &[u8]) -> Result<(usize, usize)> {
    let mut v = 0usize;
    let mut at = 0;
    for shift in (0..60).step_by(15) {
        let word = bytes
            .get(at..at + 2)
            .map(|w| usize::from(u16::from_le_bytes([w[0], w[1]])))
            .ok_or_else(|| bad("an object's size is cut off"))?;
        at += 2;
        v |= (word & 0x7FFF) << shift;
        if word & 0x8000 == 0 {
            return Ok((v, at));
        }
    }
    Err(bad("an object's size is too large"))
}

/// MC: a modular char at the start of `bytes`: 7 bits per byte, the top bit saying another
/// byte follows. With `signed`, bit 6 of the last byte is the sign. Returns the value and the
/// number of bytes read.
pub(crate) fn modular_char(bytes: &[u8], signed: bool) -> Result<(i64, usize)> {
    let mut v = 0i64;
    for (i, &b) in bytes.iter().enumerate().take(9) {
        let shift = 7 * i as u32;
        if b & 0x80 != 0 {
            v |= i64::from(b & 0x7F) << shift;
            continue;
        }
        if signed {
            v |= i64::from(b & 0x3F) << shift;
            if b & 0x40 != 0 {
                v = -v;
            }
        } else {
            v |= i64::from(b & 0x7F) << shift;
        }
        return Ok((v, i + 1));
    }
    Err(bad("a number in the object map is cut off or too long"))
}
