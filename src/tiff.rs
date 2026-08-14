//! TIFF/EXIF directory walking.
//!
//! One rule decides the whole design: **offsets in an IFD are relative to the
//! block the IFD belongs to**, and different containers hand you a different
//! block. Standard EXIF counts from the TIFF header. A Fujifilm MakerNotes
//! directory counts from the start of the MakerNotes value instead
//! (ExifTool: MakerNotes.pm:131, `Base => '$start'`).
//!
//! So `Dir` holds the block, and every offset indexes straight into it. Getting
//! this wrong is quiet rather than loud: inline values are unaffected, so the
//! numbers keep looking right while every string comes back empty.

use crate::value::Value;

#[derive(Clone, Copy)]
pub struct Dir<'a> {
    /// The block offsets are relative to.
    pub data: &'a [u8],
    pub le: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub tag: u16,
    pub format: u16,
    pub count: u32,
    pub value: u32,
}

/// Byte width of each TIFF format code, indexed by the code itself.
/// ExifTool: Exif.pm @formatSize
const FORMAT_SIZE: [usize; 14] = [0, 1, 1, 2, 4, 8, 1, 1, 2, 4, 8, 4, 8, 4];

impl<'a> Dir<'a> {
    pub fn new(data: &'a [u8], le: bool) -> Self {
        Self { data, le }
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        let b = self.data.get(at..at + 2)?;
        Some(if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b = self.data.get(at..at + 4)?;
        Some(if self.le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    }

    /// Every entry of the IFD starting at `at`.
    ///
    /// A malformed count is bounded rather than trusted: the entry count is two
    /// attacker-controlled bytes and 65535 entries is 786 KB of claimed
    /// directory.
    pub fn entries(&self, at: usize) -> Vec<Entry> {
        let Some(count) = self.u16_at(at) else {
            return Vec::new();
        };
        let count = count as usize;
        let end = at + 2 + count * 12;
        if end > self.data.len() {
            return Vec::new();
        }
        (0..count)
            .filter_map(|i| {
                let o = at + 2 + i * 12;
                Some(Entry {
                    tag: self.u16_at(o)?,
                    format: self.u16_at(o + 2)?,
                    count: self.u32_at(o + 4)?,
                    value: self.u32_at(o + 8)?,
                })
            })
            .collect()
    }

    /// Offset of the next IFD after the one at `at`, if any.
    pub fn next_ifd(&self, at: usize) -> Option<usize> {
        let count = self.u16_at(at)? as usize;
        let n = self.u32_at(at + 2 + count * 12)?;
        if n == 0 {
            None
        } else {
            Some(n as usize)
        }
    }

    /// The bytes an entry's value occupies.
    ///
    /// Four bytes or fewer live inline in the entry itself; anything larger is
    /// addressed by an offset into the block.
    fn raw(&self, e: &Entry, at_entry: usize) -> Option<&'a [u8]> {
        let size = *FORMAT_SIZE.get(e.format as usize)?;
        if size == 0 {
            return None;
        }
        let total = size.checked_mul(e.count as usize)?;
        if total <= 4 {
            self.data.get(at_entry + 8..at_entry + 8 + total)
        } else {
            let start = e.value as usize;
            self.data.get(start..start.checked_add(total)?)
        }
    }

    /// Decode one entry. `at_entry` is where its 12 bytes begin.
    pub fn value(&self, e: &Entry, at_entry: usize) -> Option<Value> {
        let raw = self.raw(e, at_entry)?;
        let n = e.count as usize;
        let rd16 = |b: &[u8]| {
            if self.le {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        };
        let rd32 = |b: &[u8]| {
            if self.le {
                u32::from_le_bytes([b[0], b[1], b[2], b[3]])
            } else {
                u32::from_be_bytes([b[0], b[1], b[2], b[3]])
            }
        };

        Some(match e.format {
            // ASCII, NUL-terminated. Trailing SPACES are kept: Fujifilm pads
            // Quality to "FINE   " and ExifTool reports the padding, so
            // trimming it here would be a difference with no reason behind it.
            2 => {
                let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                let s = String::from_utf8_lossy(&raw[..end]);
                // Trailing SPACES are content-adjacent and are kept, because
                // Fujifilm pads Quality to "FINE   " and ExifTool reports the
                // padding. A field that is ENTIRELY blank is a different
                // thing: cameras write 300 spaces into an unset Artist or
                // Copyright, and that is an empty field rather than a string
                // of spaces.
                Value::Text(if s.trim().is_empty() {
                    String::new()
                } else {
                    s.to_string()
                })
            }
            1 | 6 | 7 => {
                if n == 1 && e.format == 1 {
                    Value::U32(raw[0] as u32)
                } else {
                    Value::Bytes(raw.to_vec())
                }
            }
            3 => {
                if n == 1 {
                    Value::U32(rd16(raw) as u32)
                } else {
                    Value::U32s(raw.chunks_exact(2).map(rd16).map(u32::from).collect())
                }
            }
            8 => {
                if n == 1 {
                    Value::I32(rd16(raw) as i16 as i32)
                } else {
                    Value::I32s(raw.chunks_exact(2).map(|b| rd16(b) as i16 as i32).collect())
                }
            }
            4 => {
                if n == 1 {
                    Value::U32(rd32(raw))
                } else {
                    Value::U32s(raw.chunks_exact(4).map(rd32).collect())
                }
            }
            9 => {
                if n == 1 {
                    Value::I32(rd32(raw) as i32)
                } else {
                    Value::I32s(raw.chunks_exact(4).map(|b| rd32(b) as i32).collect())
                }
            }
            5 | 10 => {
                let signed = e.format == 10;
                let mut out = Vec::with_capacity(n);
                for c in raw.chunks_exact(8) {
                    let a = rd32(&c[0..4]);
                    let b = rd32(&c[4..8]);
                    if signed {
                        out.push((a as i32 as i64, b as i32 as i64));
                    } else {
                        out.push((a as i64, b as i64));
                    }
                }
                if out.len() == 1 {
                    Value::Ratio(out[0].0, out[0].1)
                } else {
                    Value::Ratios(out)
                }
            }
            11 => Value::F64(f32::from_bits(rd32(raw)) as f64),
            12 => {
                let mut b8 = [0u8; 8];
                b8.copy_from_slice(&raw[..8]);
                Value::F64(if self.le {
                    f64::from_le_bytes(b8)
                } else {
                    f64::from_be_bytes(b8)
                })
            }
            _ => Value::Bytes(raw.to_vec()),
        })
    }
}

/// Read a TIFF header and return (first IFD offset, little-endian).
///
/// ExifTool: Exif.pm ProcessTIFF
pub fn header(data: &[u8]) -> Option<(usize, bool)> {
    let le = match data.get(0..4)? {
        [0x49, 0x49, 0x2a, 0x00] => true,
        [0x4d, 0x4d, 0x00, 0x2a] => false,
        _ => return None,
    };
    let b = data.get(4..8)?;
    let off = if le {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    } else {
        u32::from_be_bytes([b[0], b[1], b[2], b[3]])
    };
    Some((off as usize, le))
}
