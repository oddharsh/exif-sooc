//! ISO base media files: HEIF, HEIC and AVIF.
//!
//! The EXIF is an *item* rather than a segment. `iinf` says which item is the
//! EXIF, `iloc` says where its bytes are, and the bytes themselves open with a
//! header before the TIFF starts.
//!
//! ExifTool: QuickTime.pm ParseItemLocation and HandleItemInfo.

use crate::read::Source;

struct Bx<'a> {
    kind: [u8; 4],
    body: &'a [u8],
    /// Absolute position of `body` in the file.
    at: u64,
    /// Where the next box starts, relative to the slice being walked.
    next: usize,
}

fn box_at(d: &[u8], off: usize, abs: u64) -> Option<Bx<'_>> {
    let size = u32::from_be_bytes(d.get(off..off + 4)?.try_into().ok()?) as u64;
    let kind: [u8; 4] = d.get(off + 4..off + 8)?.try_into().ok()?;
    let (size, head) = match size {
        1 => (
            u64::from_be_bytes(d.get(off + 8..off + 16)?.try_into().ok()?),
            16,
        ),
        0 => ((d.len() - off) as u64, 8),
        n => (n, 8),
    };
    if size < head as u64 {
        return None;
    }
    let end = off + size as usize;
    let body = d.get(off + head..end.min(d.len()))?;
    Some(Bx {
        kind,
        body,
        at: abs + (off + head) as u64,
        next: end,
    })
}

fn find<'a>(d: &'a [u8], abs: u64, kind: &[u8; 4]) -> Option<Bx<'a>> {
    let mut off = 0usize;
    while off + 8 <= d.len() {
        let b = box_at(d, off, abs)?;
        if &b.kind == kind {
            return Some(b);
        }
        if b.next <= off {
            return None;
        }
        off = b.next;
    }
    None
}

/// Where the `meta` box ends, read from the top-level box headers alone.
fn meta_end(d: &[u8]) -> Option<usize> {
    let mut off = 0usize;
    while off + 8 <= d.len() {
        let size = u32::from_be_bytes(d.get(off..off + 4)?.try_into().ok()?) as u64;
        let kind = d.get(off + 4..off + 8)?;
        let size = match size {
            1 => u64::from_be_bytes(d.get(off + 8..off + 16)?.try_into().ok()?),
            0 => return None,
            n => n,
        };
        if kind == b"meta" {
            return Some(off + size as usize);
        }
        let next = off.checked_add(size as usize)?;
        if next <= off {
            return None;
        }
        off = next;
    }
    None
}

/// Return the TIFF block of the EXIF item, and its absolute file position.
pub fn exif(src: &mut Source) -> Option<(Vec<u8>, u64)> {
    // The `meta` box declares its own size, so one look at the box headers says
    // whether the window holds all of it.
    if let Some(need) = meta_end(src.front()) {
        if need > src.front().len() {
            let _ = src.ensure(need);
        }
    }
    let front = src.front();
    let meta = find(front, 0, b"meta")?;
    // meta is a FullBox: four bytes of version and flags before its children.
    let kids = meta.body.get(4..)?;
    let kids_at = meta.at + 4;

    let iinf = find(kids, kids_at, b"iinf")?;
    let item = exif_item_id(iinf.body)?;
    let iloc = find(kids, kids_at, b"iloc")?;
    let (offset, len) = locate(iloc.body, item)?;

    let payload = src.range(offset, len).ok()?;
    let start = tiff_start(&payload)?;
    Some((payload[start..].to_vec(), offset + start as u64))
}

/// The item ID whose type is `Exif`.
///
/// The item-ID width is the detail that catches people: versions 0, 1 and 2 of
/// an `infe` box store 16 bits and only version 3 stores 32. Every camera
/// measured writes version 2, so reading it as 32-bit rejects the entry for
/// being too short and finds no EXIF at all.
/// ExifTool: QuickTime.pm ParseItemInfoEntry
fn exif_item_id(iinf: &[u8]) -> Option<u32> {
    let version = *iinf.first()?;
    let (count, mut off) = if version == 0 {
        (
            u16::from_be_bytes(iinf.get(4..6)?.try_into().ok()?) as u32,
            6,
        )
    } else {
        (u32::from_be_bytes(iinf.get(4..8)?.try_into().ok()?), 8)
    };
    for _ in 0..count {
        let b = box_at(iinf, off, 0)?;
        if &b.kind == b"infe" {
            let v = *b.body.first()?;
            let (id, type_at) = if v <= 2 {
                (
                    u16::from_be_bytes(b.body.get(4..6)?.try_into().ok()?) as u32,
                    8,
                )
            } else {
                (u32::from_be_bytes(b.body.get(4..8)?.try_into().ok()?), 10)
            };
            if b.body.get(type_at..type_at + 4)? == b"Exif" {
                return Some(id);
            }
        }
        if b.next <= off {
            return None;
        }
        off = b.next;
    }
    None
}

/// Where item `want` lives, as an absolute offset and length.
///
/// The field widths are stored in the box: one packed u16 carries four nibbles
/// giving the byte size of the offset, length, base-offset and index fields, so
/// the entry stride is per-file rather than fixed.
/// ExifTool: QuickTime.pm ParseItemLocation
fn locate(iloc: &[u8], want: u32) -> Option<(u64, usize)> {
    let version = *iloc.first()?;
    let sizes = u16::from_be_bytes(iloc.get(4..6)?.try_into().ok()?);
    let (osz, lsz, bsz, isz) = (
        (sizes >> 12) as usize,
        ((sizes >> 8) & 0xf) as usize,
        ((sizes >> 4) & 0xf) as usize,
        (sizes & 0xf) as usize,
    );
    let (count, mut p) = if version < 2 {
        (
            u16::from_be_bytes(iloc.get(6..8)?.try_into().ok()?) as u32,
            8,
        )
    } else {
        (u32::from_be_bytes(iloc.get(6..10)?.try_into().ok()?), 10)
    };

    let var = |d: &[u8], p: &mut usize, n: usize| -> Option<u64> {
        if n == 0 {
            return Some(0);
        }
        let b = d.get(*p..*p + n)?;
        *p += n;
        Some(b.iter().fold(0u64, |a, &c| (a << 8) | c as u64))
    };

    for _ in 0..count {
        let id = if version < 2 {
            let v = u16::from_be_bytes(iloc.get(p..p + 2)?.try_into().ok()?) as u32;
            p += 2;
            v
        } else {
            let v = u32::from_be_bytes(iloc.get(p..p + 4)?.try_into().ok()?);
            p += 4;
            v
        };
        let construction = if version == 1 || version == 2 {
            let v = u16::from_be_bytes(iloc.get(p..p + 2)?.try_into().ok()?) & 0xf;
            p += 2;
            v
        } else {
            0
        };
        // A non-zero data reference index means the bytes are in another file.
        let dref = u16::from_be_bytes(iloc.get(p..p + 2)?.try_into().ok()?);
        p += 2;
        let base = var(iloc, &mut p, bsz)?;
        let extents = u16::from_be_bytes(iloc.get(p..p + 2)?.try_into().ok()?);
        p += 2;

        let mut first: Option<(u64, usize)> = None;
        for _ in 0..extents {
            if version == 1 || version == 2 {
                var(iloc, &mut p, isz)?;
            }
            let off = var(iloc, &mut p, osz)?;
            let len = var(iloc, &mut p, lsz)?;
            if first.is_none() {
                first = Some((base.checked_add(off)?, len as usize));
            }
        }
        // Construction method 1 puts the bytes in an `idat` box, which this
        // does not read; reporting nothing beats reporting the wrong bytes.
        if id == want && construction == 0 && dref == 0 {
            return first;
        }
    }
    None
}

/// Skip the EXIF payload header and return where the TIFF begins.
///
/// The payload normally opens with a four-byte big-endian count of bytes to
/// skip, which is 6 because an `Exif\0\0` header follows it. Two malformed
/// shapes are common enough that ExifTool names both.
/// ExifTool: QuickTime.pm HandleItemInfo
fn tiff_start(payload: &[u8]) -> Option<usize> {
    if payload.len() < 4 {
        return None;
    }
    if payload.starts_with(b"MM\0\x2a") || payload.starts_with(b"II\x2a\0") {
        return Some(0); // missing Exif header
    }
    if payload.starts_with(b"Exif\0\0") {
        return Some(6); // missing Exif header size
    }
    let skip = u32::from_be_bytes(payload[0..4].try_into().ok()?) as usize;
    let start = 4 + skip;
    (start <= payload.len()).then_some(start)
}
