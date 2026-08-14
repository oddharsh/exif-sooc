//! JPEG: find the EXIF APP1 segment.
//!
//! Segments are walked from the start and the walk stops at the first SOS,
//! because everything after that is compressed image data and nothing this
//! tool wants lives there.

use crate::read::Source;

/// Return the TIFF block inside the EXIF APP1 segment, and its absolute
/// position in the file.
///
/// The position matters because thumbnail offsets inside the EXIF are stored
/// TIFF-relative, so a caller that wants to extract one has to add it back.
pub fn exif(src: &mut Source) -> Option<(Vec<u8>, u64)> {
    let d = src.front();
    if d.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    loop {
        // Segments are 0xFF then a marker. Fill bytes of 0xFF are legal
        // between segments.
        while d.get(i) == Some(&0xFF) && d.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if d.get(i)? != &0xFF {
            return None;
        }
        let marker = *d.get(i + 1)?;
        // SOS (0xDA) starts image data; EOI (0xD9) ends the file.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([*d.get(i + 2)?, *d.get(i + 3)?]) as usize;
        if len < 2 {
            return None;
        }
        let body = i + 4;
        let end = body + len - 2;
        if marker == 0xE1 && d.get(body..body + 6) == Some(b"Exif\0\0") {
            let tiff = body + 6;
            return Some((d.get(tiff..end)?.to_vec(), tiff as u64));
        }
        i = end;
        if i >= d.len() {
            return None;
        }
    }
}
