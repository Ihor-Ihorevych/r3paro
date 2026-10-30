//! Little-endian byte reader with the package format's primitive types.

use crate::{Error, Result};

#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    /// File offset of `data[0]`, for error messages and lazy-array skip offsets.
    base: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, base: 0 }
    }

    pub fn with_base(data: &'a [u8], base: usize) -> Self {
        Self { data, pos: 0, base }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn file_pos(&self) -> usize {
        self.base + self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(self.eof(pos - self.pos));
        }
        self.pos = pos;
        Ok(())
    }

    /// Moves to an absolute file offset (lazy-array skip offsets are file offsets).
    pub fn seek_file(&mut self, file_pos: usize) -> Result<()> {
        if file_pos < self.base {
            return Err(Error::Format(format!("seek to {file_pos:#x} before object start {:#x}", self.base)));
        }
        self.seek(file_pos - self.base)
    }

    fn eof(&self, need: usize) -> Error {
        Error::Eof { at: self.file_pos(), need }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(self.eof(n));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.data[self.pos..];
        self.pos = self.data.len();
        s
    }

    pub fn peek_u8(&self) -> Result<u8> {
        self.data.get(self.pos).copied().ok_or_else(|| self.eof(1))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn guid(&mut self) -> Result<[u8; 16]> {
        Ok(self.bytes(16)?.try_into().unwrap())
    }

    /// Compact index: sign and continuation in the first byte (6 value bits), then 7 bits per byte.
    pub fn ci(&mut self) -> Result<i32> {
        let b0 = self.u8()?;
        let negative = b0 & 0x80 != 0;
        let mut value = (b0 & 0x3F) as u32;
        if b0 & 0x40 != 0 {
            let mut shift = 6;
            loop {
                let b = self.u8()?;
                value |= ((b & 0x7F) as u32) << shift;
                if b & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift > 27 {
                    return Err(Error::Format(format!("compact index too long at {:#x}", self.file_pos())));
                }
            }
        }
        Ok(if negative { -(value as i32) } else { value as i32 })
    }

    /// Length-prefixed string: a positive length is Latin-1 bytes, a negative one UTF-16 units; both include the NUL.
    pub fn fstring(&mut self) -> Result<String> {
        let len = self.ci()?;
        if len == 0 {
            return Ok(String::new());
        }
        if len > 0 {
            let raw = self.bytes(len as usize)?;
            let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
            Ok(raw.iter().map(|&b| b as char).collect())
        } else {
            let n = len.unsigned_abs() as usize;
            let raw = self.bytes(n * 2)?;
            let mut units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            if units.last() == Some(&0) {
                units.pop();
            }
            Ok(String::from_utf16_lossy(&units))
        }
    }

    /// Dynamic array count (a compact index).
    pub fn count(&mut self) -> Result<usize> {
        let n = self.ci()?;
        if n < 0 || n as usize > self.remaining().max(16) * 8 {
            return Err(Error::Format(format!("bad array count {n} at {:#x}", self.file_pos())));
        }
        Ok(n as usize)
    }

    pub fn array<T>(&mut self, mut f: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let n = self.count()?;
        let mut v = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            v.push(f(self)?);
        }
        Ok(v)
    }

    /// Lazy array: an absolute file offset just past the data, then the array.
    pub fn lazy_array<T>(&mut self, f: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let end = self.i32()? as usize;
        let v = self.array(f)?;
        if self.file_pos() != end {
            return Err(Error::Format(format!(
                "lazy array ends at {:#x}, skip offset says {end:#x}",
                self.file_pos()
            )));
        }
        Ok(v)
    }

    /// Lazy byte array, returned as a slice without copying.
    pub fn lazy_bytes(&mut self) -> Result<&'a [u8]> {
        let end = self.i32()? as usize;
        let n = self.count()?;
        let s = self.bytes(n)?;
        if self.file_pos() != end {
            return Err(Error::Format(format!(
                "lazy byte array ends at {:#x}, skip offset says {end:#x}",
                self.file_pos()
            )));
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_ci(v: i32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut a = v.unsigned_abs();
        let mut b0 = (a & 0x3F) as u8;
        if v < 0 {
            b0 |= 0x80;
        }
        a >>= 6;
        if a > 0 {
            b0 |= 0x40;
        }
        out.push(b0);
        while a > 0 {
            let mut b = (a & 0x7F) as u8;
            a >>= 7;
            if a > 0 {
                b |= 0x80;
            }
            out.push(b);
        }
        out
    }

    #[test]
    fn compact_index_round_trip() {
        for v in [0, 1, -1, 63, 64, -64, 127, 128, 8191, 8192, 1 << 20, -(1 << 26), i32::MAX >> 2] {
            let bytes = write_ci(v);
            let mut r = Reader::new(&bytes);
            assert_eq!(r.ci().unwrap(), v, "value {v}");
            assert!(r.at_end());
        }
    }

    #[test]
    fn strings() {
        let mut ansi = vec![4u8];
        ansi.extend_from_slice(b"abc\0");
        assert_eq!(Reader::new(&ansi).fstring().unwrap(), "abc");
        let mut wide = write_ci(-3);
        for u in ['П' as u16, 'я' as u16, 0] {
            wide.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(Reader::new(&wide).fstring().unwrap(), "Пя");
    }
}
