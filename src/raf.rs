//! Fujifilm RAF.
//!
//! The container is a thin wrapper: a 112-byte header, then an embedded JPEG
//! that carries the whole EXIF including the MakerNotes. So RAF costs one
//! 112-byte read plus whatever the JPEG path needs, and reduces to it.
//!
//! ExifTool: FujiFilm.pm ProcessRAF

use crate::read::Source;

pub const MAGIC: &[u8; 8] = b"FUJIFILM";

/// The camera model string in the RAF header, which is written there directly
/// rather than only in the embedded EXIF.
pub fn model(src: &Source) -> Option<String> {
    let d = src.front().get(0x1c..0x1c + 32)?;
    let end = d.iter().position(|&c| c == 0).unwrap_or(d.len());
    Some(String::from_utf8_lossy(&d[..end]).trim().to_string())
}

/// The embedded JPEG, as its own source.
pub fn jpeg(src: &mut Source) -> Option<(Source, u64)> {
    let head = src.front();
    if !head.starts_with(MAGIC) {
        return None;
    }
    // ExifTool: FujiFilm.pm:1940, `unpack('x84NN', $buff)`
    let pos = u32::from_be_bytes(head.get(84..88)?.try_into().ok()?) as u64;
    let len = u32::from_be_bytes(head.get(88..92)?.try_into().ok()?) as usize;
    if pos == 0 || len == 0 || pos & 0x8000 != 0 {
        return None;
    }
    let bytes = src.range(pos, len).ok()?;
    Some((Source::from_vec(bytes), pos))
}
