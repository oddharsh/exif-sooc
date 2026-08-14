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
    // An EXIF APP1 can be 64 KB by spec, and the segments before it push it
    // further out. Grow once rather than walking off the end of the window.
    // ExifTool reads the same way: segments from the front, stopping at SOS.
    if src.front().len() < 192 * 1024 && src.len() > src.front().len() as u64 {
        let _ = src.ensure(192 * 1024);
    }
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

/// Image dimensions from the Start Of Frame marker.
///
/// This is where the size really lives: EXIF's ExifImageWidth is what the
/// camera SAID, and a body that writes no EXIF dimensions still has an SOF,
/// because a decoder cannot do without one. Two Leica JPEGs in the test corpus
/// are exactly that case.
pub fn dimensions(src: &Source) -> Option<(u32, u32)> {
    let d = src.front();
    if d.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    loop {
        while d.get(i) == Some(&0xFF) && d.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if d.get(i)? != &0xFF {
            return None;
        }
        let marker = *d.get(i + 1)?;
        if marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([*d.get(i + 2)?, *d.get(i + 3)?]) as usize;
        if len < 2 {
            return None;
        }
        // SOF0 through SOF15, minus the three markers in that range that are
        // not frame headers: DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([*d.get(i + 5)?, *d.get(i + 6)?]) as u32;
            let w = u16::from_be_bytes([*d.get(i + 7)?, *d.get(i + 8)?]) as u32;
            return Some((w, h));
        }
        // A frame header always precedes the scan, so there is nothing left
        // to find after SOS.
        if marker == 0xDA {
            return None;
        }
        i = i + 4 + len - 2;
        if i >= d.len() {
            return None;
        }
    }
}
