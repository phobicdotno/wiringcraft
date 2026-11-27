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

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    /// Writes bits most significant first, the way `Bits` reads them.
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        len: usize,
    }

    impl Writer {
        fn bit(&mut self, b: bool) -> &mut Self {
            if self.len.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if b && let Some(last) = self.bytes.last_mut() {
                *last |= 0x80 >> (self.len % 8);
            }
            self.len += 1;
            self
        }
        fn bits(&mut self, v: u64, n: u32) -> &mut Self {
            for i in (0..n).rev() {
                self.bit((v >> i) & 1 != 0);
            }
            self
        }
        fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
            for &b in bytes {
                self.bits(u64::from(b), 8);
            }
            self
        }
        fn reader(&self) -> Bits<'_> {
            Bits::new(&self.bytes, 0, self.len)
        }
    }

    #[test]
    fn raw_values_read_little_endian_off_byte_boundaries() -> TestResult {
        let mut w = Writer::default();
        w.bit(true)
            .bytes(&[0xAB])
            .bytes(&0x1234u16.to_le_bytes())
            .bytes(&0xDEAD_BEEFu32.to_le_bytes())
            .bytes(&(-2.5f64).to_le_bytes())
            .bytes(&1.5f64.to_le_bytes())
            .bytes(&3.0f64.to_le_bytes());
        let mut r = w.reader();
        assert!(r.b()?);
        assert_eq!(r.rc()?, 0xAB);
        assert_eq!(r.rs()?, 0x1234);
        assert_eq!(r.rl()?, 0xDEAD_BEEF);
        assert_eq!(r.rd()?, -2.5);
        assert_eq!(r.rd2()?, (1.5, 3.0));
        assert_eq!(r.pos(), w.len);
        assert!(r.b().is_err());
        Ok(())
    }

    #[test]
    fn bs_and_bl_cover_every_prefix() -> TestResult {
        let mut w = Writer::default();
        w.bits(0, 2).bytes(&0x0302u16.to_le_bytes());
        w.bits(1, 2).bytes(&[0xFE]);
        w.bits(2, 2);
        w.bits(3, 2);
        w.bits(0, 2).bytes(&0x0102_0304u32.to_le_bytes());
        w.bits(1, 2).bytes(&[0x7F]);
        w.bits(2, 2);
        let mut r = w.reader();
        assert_eq!(r.bs()?, 0x0302);
        assert_eq!(r.bs()?, 0xFE);
        assert_eq!(r.bs()?, 0);
        assert_eq!(r.bs()?, 256);
        assert_eq!(r.bl()?, 0x0102_0304);
        assert_eq!(r.bl()?, 0x7F);
        assert_eq!(r.bl()?, 0);
        // Prefix 3 is not a long.
        let mut w = Writer::default();
        w.bits(3, 2);
        assert!(w.reader().bl().is_err());
        Ok(())
    }

    #[test]
    fn bd_bt_be_and_bd3_cover_their_short_forms() -> TestResult {
        let mut w = Writer::default();
        w.bits(0, 2).bytes(&7.25f64.to_le_bytes());
        w.bits(1, 2).bits(2, 2);
        w.bit(true); // BT: zero
        w.bit(false).bits(1, 2); // BT: a BD of 1.0
        w.bit(true); // BE: default extrusion
        w.bit(false).bits(2, 2).bits(2, 2).bits(1, 2); // BE: (0, 0, 1) spelled out
        let mut r = w.reader();
        assert_eq!(r.bd()?, 7.25);
        assert_eq!(r.bd()?, 1.0);
        assert_eq!(r.bd()?, 0.0);
        assert_eq!(r.bt()?, 0.0);
        assert_eq!(r.bt()?, 1.0);
        assert_eq!(r.be()?, (0.0, 0.0, 1.0));
        assert_eq!(r.be()?, (0.0, 0.0, 1.0));
        assert_eq!(r.pos(), w.len);
        let mut w = Writer::default();
        w.bits(3, 2);
        assert!(w.reader().bd().is_err());
        Ok(())
    }

    #[test]
    fn dd_patches_the_default_bytes() -> TestResult {
        let default = 123.456f64;
        let new = -9.875f64;
        let nb = new.to_le_bytes();
        let db = default.to_le_bytes();
        let mut w = Writer::default();
        w.bits(0, 2); // the default itself
        w.bits(1, 2).bytes(&nb[..4]); // low four bytes replaced
        w.bits(2, 2).bytes(&nb[4..6]).bytes(&nb[..4]); // six bytes replaced
        w.bits(3, 2).bytes(&nb); // a full double
        let mut r = w.reader();
        assert_eq!(r.dd(default)?, default);
        let mut low = db;
        low[..4].copy_from_slice(&nb[..4]);
        assert_eq!(r.dd(default)?.to_bits(), f64::from_le_bytes(low).to_bits());
        let mut six = db;
        six[..6].copy_from_slice(&nb[..6]);
        assert_eq!(r.dd(default)?.to_bits(), f64::from_le_bytes(six).to_bits());
        assert_eq!(r.dd(default)?, new);
        assert_eq!(r.pos(), w.len);
        Ok(())
    }

    #[test]
    fn bll_reads_a_counted_little_endian_number() -> TestResult {
        let mut w = Writer::default();
        w.bits(0, 3);
        w.bits(3, 3).bytes(&[0x01, 0x02, 0x03]);
        w.bits(7, 3).bytes(&[0xFF; 7]);
        let mut r = w.reader();
        assert_eq!(r.bll()?, 0);
        assert_eq!(r.bll()?, 0x03_0201);
        assert_eq!(r.bll()?, 0x00FF_FFFF_FFFF_FFFF);
        Ok(())
    }

    #[test]
    fn handles_resolve_against_the_owner() -> TestResult {
        let own = 0x100;
        let mut w = Writer::default();
        w.bytes(&[0x52, 0x12, 0x34]); // code 5, absolute 0x1234
        w.bytes(&[0x60]); // own + 1
        w.bytes(&[0x80]); // own - 1
        w.bytes(&[0xA1, 0x10]); // own + 0x10
        w.bytes(&[0xC1, 0x10]); // own - 0x10
        w.bytes(&[0x30]); // code 3, no bytes: handle 0
        let mut r = w.reader();
        assert_eq!(r.clone().h()?, (5, 0x1234));
        assert_eq!(r.handle(own)?, 0x1234);
        assert_eq!(r.handle(own)?, 0x101);
        assert_eq!(r.handle(own)?, 0xFF);
        assert_eq!(r.handle(own)?, 0x110);
        assert_eq!(r.handle(own)?, 0xF0);
        assert_eq!(r.handle(own)?, 0);
        // Offsets wrap instead of overflowing.
        let mut w = Writer::default();
        w.bytes(&[0x80, 0xC1, 0x05]);
        let mut r = w.reader();
        assert_eq!(r.handle(0)?, u64::MAX);
        assert_eq!(r.handle(2)?, u64::MAX - 2);
        // A count of 15 bytes keeps the low eight, without panicking.
        let mut w = Writer::default();
        w.bytes(&[0x0F]).bytes(&[0xAA; 15]);
        assert_eq!(w.reader().h()?, (0, 0xAAAA_AAAA_AAAA_AAAA));
        Ok(())
    }

    #[test]
    fn text_reads_strip_trailing_zeros() -> TestResult {
        let mut w = Writer::default();
        w.bits(1, 2).bytes(&[4]).bytes(b"ab\0\0");
        w.bits(1, 2).bytes(&[3]);
        for u in [u16::from(b'h'), 0x00E9, 0] {
            w.bytes(&u.to_le_bytes());
        }
        w.bits(2, 2); // an empty TU
        let mut r = w.reader();
        assert_eq!(r.t8_bytes()?, b"ab");
        assert_eq!(r.tu()?, "h\u{e9}");
        assert_eq!(r.tu()?, "");
        Ok(())
    }

    #[test]
    fn every_read_errors_on_truncated_input() {
        // A long run of full-form values, cut at every bit: no read may panic, and the cut one
        // must fail.
        let mut w = Writer::default();
        w.bits(0, 2).bytes(&[1, 2]); // BS
        w.bits(0, 2).bytes(&[1, 2, 3, 4]); // BL
        w.bits(0, 2).bytes(&[0x11; 8]); // BD
        w.bits(3, 2).bytes(&[0x22; 8]); // DD
        w.bits(7, 3).bytes(&[0x33; 7]); // BLL
        w.bytes(&[0x54, 1, 2, 3, 4]); // H
        w.bits(1, 2).bytes(&[2, b'x', b'y']); // T8
        w.bits(1, 2).bytes(&[1, b'z', 0]); // TU
        let full = w.len;
        let read_all = |r: &mut Bits<'_>| -> Result<()> {
            r.bs()?;
            r.bl()?;
            r.bd()?;
            r.dd(0.0)?;
            r.bll()?;
            r.handle(1)?;
            r.t8_bytes()?;
            r.tu()?;
            Ok(())
        };
        assert!(read_all(&mut Bits::new(&w.bytes, 0, full)).is_ok());
        for cut in 0..full {
            let mut r = Bits::new(&w.bytes, 0, cut);
            assert!(read_all(&mut r).is_err(), "cut at bit {cut} read through");
            assert!(r.pos() <= cut);
        }
        // Each primitive on its own, on an empty cursor.
        let empty = Bits::new(&[], 0, 0);
        assert!(empty.clone().b().is_err());
        assert!(empty.clone().bb().is_err());
        assert!(empty.clone().rc().is_err());
        assert!(empty.clone().rs().is_err());
        assert!(empty.clone().rl().is_err());
        assert!(empty.clone().rd().is_err());
        assert!(empty.clone().rd2().is_err());
        assert!(empty.clone().bd3().is_err());
        assert!(empty.clone().bt().is_err());
        assert!(empty.clone().be().is_err());
        assert!(empty.clone().h().is_err());
        assert!(empty.clone().t8_bytes().is_err());
        assert!(empty.clone().tu().is_err());
    }

    #[test]
    fn range_is_clamped_and_peeks_stay_in_range() -> TestResult {
        let data = [0xFF, 0x00, 0x34, 0x12];
        // An end past the buffer is clamped to it, and a start past the end to the end.
        let mut r = Bits::new(&data, 16, 1000);
        assert_eq!(r.rs()?, 0x1234);
        assert!(r.b().is_err());
        let mut r = Bits::new(&data, 50, 10);
        assert_eq!(r.pos(), 10);
        assert!(r.b().is_err());
        // Peeks read without moving and respect the end.
        let r = Bits::new(&data, 0, 24);
        assert!(r.bit_at(0)?);
        assert!(!r.bit_at(8)?);
        assert!(r.bit_at(24).is_err());
        assert!(r.bit_at(usize::MAX).is_err());
        assert_eq!(r.rs_at(0)?, 0x00FF);
        assert!(r.rs_at(16).is_err());
        assert_eq!(r.pos(), 0);
        // Skips stop at the end and survive absurd counts.
        let mut r = Bits::new(&data, 0, 32);
        r.skip_bytes(2)?;
        assert_eq!(r.pos(), 16);
        assert!(r.skip_bytes(3).is_err());
        assert_eq!(r.pos(), 16);
        assert!(r.skip_bytes(u64::MAX).is_err());
        assert!(r.skip_bytes(u64::MAX / 8).is_err());
        r.skip_bytes(2)?;
        assert!(r.skip_bytes(1).is_err());
        r.skip_bytes(0)?;
        Ok(())
    }

    #[test]
    fn modular_short_reads_chained_words() -> TestResult {
        assert_eq!(modular_short(&[0x34, 0x12])?, (0x1234, 2));
        // 0x7FFF with the continue bit, then 1: 0x7FFF | 1 << 15.
        assert_eq!(modular_short(&[0xFF, 0xFF, 0x01, 0x00, 0xEE])?, (0xFFFF, 4));
        assert!(modular_short(&[]).is_err());
        assert!(modular_short(&[0x01]).is_err());
        assert!(modular_short(&[0x00, 0x80, 0x00]).is_err());
        // Four words that all say another follows is too large.
        assert!(modular_short(&[0xFF; 8]).is_err());
        assert!(modular_short(&[0xFF; 64]).is_err());
        Ok(())
    }

    #[test]
    fn modular_char_reads_unsigned_and_signed() -> TestResult {
        assert_eq!(modular_char(&[0x05], false)?, (5, 1));
        assert_eq!(modular_char(&[0x7F], false)?, (127, 1));
        assert_eq!(modular_char(&[0x7F], true)?, (-63, 1));
        assert_eq!(modular_char(&[0x45], true)?, (-5, 1));
        assert_eq!(modular_char(&[0x05, 0xAA], true)?, (5, 1));
        // 0x82 0x01: 2 | 1 << 7.
        assert_eq!(modular_char(&[0x82, 0x01], false)?, (130, 2));
        assert_eq!(modular_char(&[0x82, 0x41], true)?, (-130, 2));
        assert!(modular_char(&[], false).is_err());
        assert!(modular_char(&[0x80, 0x80], true).is_err());
        // Nine continuation bytes is too long; the ninth may end it without overflowing.
        assert!(modular_char(&[0xFF; 9], false).is_err());
        let mut long = [0xFF; 9];
        long[8] = 0x7F;
        assert!(modular_char(&long, false).is_ok());
        assert!(modular_char(&long, true).is_ok());
        Ok(())
    }
}
