//! JPEG XL: the ISO BMFF-style container.
//!
//! A container opens with a 12-byte signature box (`JXL ` and a fixed body),
//! then `ftyp`, then any mix of codestream boxes (`jxlc`, or `jxlp` parts) and
//! metadata boxes. EXIF is an `Exif` box whose body opens with a four-byte
//! big-endian offset to the TIFF header, the same shape as a HEIF's EXIF item.
//! XMP is an `xml ` box holding the packet itself.
//!
//! Two things a reader of this has to know, both measured on cjxl 0.12:
//!
//!   - cjxl BROTLI-COMPRESSES metadata by default. Each one becomes a `brob`
//!     box whose body is the inner box type followed by a Brotli stream, so a
//!     file straight out of `cjxl` carries no `Exif` box at all. This crate has
//!     no dependencies and no Brotli decoder, so a compressed box is reported
//!     as exactly that rather than as "no EXIF". `cjxl --compress_boxes=0`
//!     writes the boxes plain, for 5 KB on a 12.5 MB frame.
//!   - A lossless JPEG transcode carries a `jbrd` box, the data `djxl` needs to
//!     rebuild the original JPEG byte for byte, and that data covers the EXIF
//!     too. Editing the metadata of such a file would quietly break the
//!     rebuild, so the writer refuses it.
//!
//! A bare codestream (`FF 0A`, no container) has nowhere to keep metadata.
//!
//! The box walk reads headers through the bounded `Source`, so a 20 MB file
//! still costs a few small reads: the metadata cjxl writes sits before the
//! main codestream part.
//!
//! ExifTool: Jpeg2000.pm, the JXL container and its `brob` handling.

use crate::read::Source;

pub const SIGNATURE: [u8; 12] = [0, 0, 0, 12, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A];
pub const CODESTREAM: [u8; 2] = [0xFF, 0x0A];

/// One top-level box, located without reading its body.
#[derive(Clone, Copy)]
pub struct Bx {
    pub kind: [u8; 4],
    /// Where the box starts, header included.
    pub start: u64,
    /// Where its body starts.
    pub body: u64,
    pub end: u64,
}

/// Every top-level box, from headers alone.
pub fn boxes(src: &mut Source) -> Option<Vec<Bx>> {
    let len = src.len();
    let mut out = Vec::new();
    let mut at = 0u64;
    while at + 8 <= len {
        let head = src.range(at, 8).ok()?;
        let size = u32::from_be_bytes(head[0..4].try_into().ok()?) as u64;
        let kind: [u8; 4] = head[4..8].try_into().ok()?;
        let (size, hdr) = match size {
            1 => {
                let big = src.range(at + 8, 8).ok()?;
                (u64::from_be_bytes(big.try_into().ok()?), 16)
            }
            0 => (len - at, 8),
            n => (n, 8),
        };
        if size < hdr || at + size > len {
            return None;
        }
        out.push(Bx {
            kind,
            start: at,
            body: at + hdr,
            end: at + size,
        });
        at += size;
    }
    Some(out)
}

/// The same walk over bytes already in memory, for the writer.
pub fn boxes_in(d: &[u8]) -> Result<Vec<Bx>, String> {
    let mut src = Source::from_vec(d.to_vec());
    boxes(&mut src).ok_or_else(|| "a box runs past the end of the file".into())
}

/// The inner type of a `brob` box: the first four bytes of its body.
fn brob_inner(src: &mut Source, b: &Bx) -> Option<[u8; 4]> {
    (b.end >= b.body + 4).then_some(())?;
    src.range(b.body, 4).ok()?.try_into().ok()
}

/// What a container holds for one kind of metadata.
pub enum Found<T> {
    Plain(T),
    /// Present, but Brotli-compressed in a `brob` box.
    Compressed,
    Absent,
}

/// The TIFF block of the `Exif` box, and its absolute file position.
pub fn exif(src: &mut Source) -> Found<(Vec<u8>, u64)> {
    let Some(list) = boxes(src) else {
        return Found::Absent;
    };
    for b in &list {
        if &b.kind == b"Exif" {
            let Ok(payload) = src.range(b.body, (b.end - b.body) as usize) else {
                return Found::Absent;
            };
            if payload.len() < 4 {
                return Found::Absent;
            }
            let skip = u32::from_be_bytes(payload[0..4].try_into().unwrap()) as usize;
            let start = 4 + skip;
            if start > payload.len() {
                return Found::Absent;
            }
            return Found::Plain((payload[start..].to_vec(), b.body + start as u64));
        }
    }
    if list
        .iter()
        .any(|b| &b.kind == b"brob" && brob_inner(src, b) == Some(*b"Exif"))
    {
        return Found::Compressed;
    }
    Found::Absent
}

/// The XMP packet of the `xml ` box, verbatim.
pub fn xmp(src: &mut Source) -> Found<Vec<u8>> {
    let Some(list) = boxes(src) else {
        return Found::Absent;
    };
    for b in &list {
        if &b.kind == b"xml " {
            return match src.range(b.body, (b.end - b.body) as usize) {
                Ok(p) => Found::Plain(p),
                Err(_) => Found::Absent,
            };
        }
    }
    if list
        .iter()
        .any(|b| &b.kind == b"brob" && brob_inner(src, b) == Some(*b"xml "))
    {
        return Found::Compressed;
    }
    Found::Absent
}

/// Is this box metadata that `-all=` removes and `-TagsFromFile` replaces?
fn is_metadata(d: &[u8], b: &Bx) -> bool {
    match &b.kind {
        b"Exif" | b"xml " | b"jumb" => true,
        b"brob" => {
            let at = b.body as usize;
            matches!(d.get(at..at + 4), Some(t) if t == b"Exif" || t == b"xml " || t == b"jumb")
        }
        _ => false,
    }
}

/// A box with its 8-byte header.
fn boxed(kind: &[u8; 4], body: &[u8]) -> Result<Vec<u8>, String> {
    let size = u32::try_from(body.len() + 8).map_err(|_| "metadata box over 4 GB")?;
    let mut b = size.to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(body);
    Ok(b)
}

/// Rebuild a JXL container without its metadata boxes, then insert EXIF and
/// XMP where the first metadata box was (or after `ftyp`, and after `jxll`
/// when present, if there was none).
///
/// Every other box is copied byte for byte, so the codestream, and with it
/// every pixel, is untouched. Refuses a bare codestream, which has no box to
/// hold metadata, and a file carrying `jbrd`, whose JPEG rebuild depends on
/// the metadata it would change.
pub fn rewrite(d: &[u8], tiff: Option<&[u8]>, xmp: Option<&[u8]>) -> Result<Vec<u8>, String> {
    if d.starts_with(&CODESTREAM) {
        return Err("a bare JPEG XL codestream has no box to hold metadata".into());
    }
    if !d.starts_with(&SIGNATURE) {
        return Err("not a JPEG XL container".into());
    }
    let list = boxes_in(d)?;
    if list.iter().any(|b| &b.kind == b"jbrd") {
        return Err("carries JPEG reconstruction data (jbrd); changing its metadata would break djxl's byte-exact rebuild".into());
    }
    let mut insert = Vec::new();
    if let Some(t) = tiff {
        let mut body = 0u32.to_be_bytes().to_vec(); // TIFF header immediately follows
        body.extend_from_slice(t);
        insert.extend(boxed(b"Exif", &body)?);
    }
    if let Some(x) = xmp {
        insert.extend(boxed(b"xml ", x)?);
    }
    let first_meta = list.iter().position(|b| is_metadata(d, b));
    let after_header = list
        .iter()
        .rposition(|b| matches!(&b.kind, b"JXL " | b"ftyp" | b"jxll"))
        .map(|i| i + 1)
        .unwrap_or(0);
    let insert_at = first_meta.unwrap_or(after_header);

    let mut out = Vec::with_capacity(d.len() + insert.len());
    for (i, b) in list.iter().enumerate() {
        if i == insert_at {
            out.extend_from_slice(&insert);
        }
        if !is_metadata(d, b) {
            out.extend_from_slice(&d[b.start as usize..b.end as usize]);
        }
    }
    if insert_at >= list.len() {
        out.extend_from_slice(&insert);
    }
    Ok(out)
}
