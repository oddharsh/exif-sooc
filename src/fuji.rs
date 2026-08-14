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
    let mut drive_settings: Option<u32> = None;
    if block.len() < 12 || !is_fuji(block) {
        return out;
    }
    let ifd = u32::from_le_bytes([block[8], block[9], block[10], block[11]]) as usize;
    // Fujifilm forces little-endian regardless of the file's own byte order.
    let dir = Dir::maker(block, true);
    for (i, e) in dir.entries(ifd).iter().enumerate() {
        let at = ifd + 2 + i * 12;
        let Some(v) = dir.value(e, at) else { continue };
        if e.tag == 0x1103 {
            drive_settings = match &v {
                Value::U32(n) => Some(*n),
                Value::Bytes(b) if b.len() >= 4 => {
                    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                }
                _ => None,
            };
            continue;
        }
        let Some(def) = tags::find(FUJI, e.tag) else {
            continue;
        };
        let print = match tags::fuji_override(e.tag) {
            Some(f) => f(&v).unwrap_or_else(|| def.print(&v)),
            None => def.print(&v),
        };
        out.push((def.name, v, print));
    }
    // 0x1103 DriveSettings is a ProcessBinaryData table: one int32u carrying
    // two masked fields. ExifTool: FujiFilm.pm:1159-1188.
    //
    // It is decoded here rather than through the tag table because the table
    // is keyed by tag id and these two share one, which is exactly what
    // ExifTool's fractional keys (0.1, 0.2) express.
    if let Some(bits) = drive_settings {
        let mode = bits & 0x0000_00ff;
        let speed = (bits >> 24) & 0x0000_00ff;
        out.push((
            "DriveMode",
            Value::U32(mode),
            match mode {
                0 => "Single".to_string(),
                1 => "Continuous Low".to_string(),
                2 => "Continuous High".to_string(),
                n => n.to_string(),
            },
        ));
        out.push((
            "DriveSpeed",
            Value::U32(speed),
            if speed == 0 {
                "n/a".to_string()
            } else {
                format!("{speed} fps")
            },
        ));
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
