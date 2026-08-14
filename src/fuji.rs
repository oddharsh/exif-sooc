//! Fujifilm MakerNotes.
//!
//! Two details decide whether this works, and both fail quietly.
//!
//! Byte 8 holds a POINTER to the directory rather than a count of bytes to
//! skip. It reads 12 on every X-series file measured, and it is not a constant.
//!
//! Value offsets inside are relative to the start of the MakerNotes block
//! rather than to the TIFF header (ExifTool: MakerNotes.pm:131, `Base =>
//! '$start'`). Inline values survive the wrong base, so getting this wrong
//! leaves the numbers correct and every string empty.

use crate::fuji_tags::FUJI;
use crate::tags;
use crate::tiff::Dir;
use crate::value::Value;

pub const SIGNATURES: [&[u8]; 2] = [b"FUJIFILM", b"GENERALE"];

/// True if this MakerNotes block is Fujifilm's.
///
/// The test is on the DATA and never on the camera make, because the same
/// header is written by some Leica, Minolta and Sharp bodies, and GE models
/// write "GENERALE" (ExifTool: MakerNotes.pm:122-124).
pub fn is_fuji(block: &[u8]) -> bool {
    SIGNATURES.iter().any(|s| block.starts_with(s))
}

pub fn parse(block: &[u8]) -> Vec<(&'static str, Value, String)> {
    let mut out = Vec::new();
    if block.len() < 12 || !is_fuji(block) {
        return out;
    }
    let ifd = u32::from_le_bytes([block[8], block[9], block[10], block[11]]) as usize;
    // Fujifilm forces little-endian regardless of the file's own byte order.
    let dir = Dir::new(block, true);
    for (i, e) in dir.entries(ifd).iter().enumerate() {
        let at = ifd + 2 + i * 12;
        let Some(v) = dir.value(e, at) else { continue };
        let Some(def) = tags::find(FUJI, e.tag) else {
            continue;
        };
        let print = match tags::fuji_override(e.tag) {
            Some(f) => f(&v).unwrap_or_else(|| def.print(&v)),
            None => def.print(&v),
        };
        out.push((def.name, v, print));
    }
    // 0x100b is NoiseReduction only when it is not 0x100, which is the value
    // every X-series body writes to mean "not this tag". ExifTool drops it with
    // a RawConv (FujiFilm.pm:237); keeping it would collide with 0x100e, whose
    // name is also NoiseReduction, and the surviving value would depend on map
    // ordering rather than on the file.
    let drop_100b = out
        .iter()
        .any(|(n, v, _)| *n == "NoiseReduction" && v.as_i64() == Some(0x100));
    if drop_100b {
        let mut seen = false;
        out.retain(|(n, v, _)| {
            if *n == "NoiseReduction" && v.as_i64() == Some(0x100) && !seen {
                seen = true;
                return false;
            }
            true
        });
    }
    out
}
