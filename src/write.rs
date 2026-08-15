//! Editing a JPEG's metadata segments.
//!
//! Not a general metadata writer, and deliberately so. Both jobs this needs to
//! cover are SEGMENT surgery rather than tag editing: dropping the APP markers,
//! or moving them from one file to another. Neither one parses or rebuilds a
//! single EXIF tag, so neither can corrupt one.
//!
//! What ExifTool does, measured rather than assumed:
//!
//!   -all=                     drops every APPn (JFIF, EXIF, XMP, ICC, IPTC)
//!                             and COM, leaving the coding segments and scan
//!   -TagsFromFile SRC -all:all  strips the destination the same way, then
//!                             inserts the source's APP1 segments (EXIF and
//!                             XMP) directly after SOI
//!
//! The entropy-coded scan is never touched, so both operations are lossless in
//! the strict sense: the pixels are the same bytes afterwards.

/// Markers that carry metadata rather than coding tables.
fn is_metadata(marker: u8) -> bool {
    (0xE0..=0xEF).contains(&marker) || marker == 0xFE
}

/// Walk the segments before the scan, calling `f` with (marker, start, end).
///
/// Offsets rather than slices, so a caller can collect them without borrowing
/// the buffer for the closure's whole life.
///
/// Stops at SOS, because everything after it is entropy-coded data in which an
/// 0xFF byte is data rather than a marker. Walking past it is how a JPEG parser
/// starts inventing segments.
fn walk(d: &[u8], mut f: impl FnMut(u8, usize, usize)) -> Result<usize, String> {
    if d.len() < 2 || d[0] != 0xFF || d[1] != 0xD8 {
        return Err("not a JPEG".into());
    }
    let mut i = 2;
    loop {
        if i + 1 >= d.len() {
            return Err("ran out of file before the scan".into());
        }
        if d[i] != 0xFF {
            return Err(format!("expected a marker at {i:#x}"));
        }
        let marker = d[i + 1];
        // SOS begins the scan; EOI ends an image with none.
        if marker == 0xDA || marker == 0xD9 {
            return Ok(i);
        }
        // Standalone markers carry no length.
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        if i + 3 >= d.len() {
            return Err("truncated segment header".into());
        }
        let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        if len < 2 || i + 2 + len > d.len() {
            return Err("segment length past the end of the file".into());
        }
        f(marker, i, i + 2 + len);
        i += 2 + len;
    }
}

/// Every APP1 segment, whole, in file order.
pub fn app1_segments(jpeg: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut spans = Vec::new();
    walk(jpeg, |m, a, b| {
        if m == 0xE1 {
            spans.push((a, b));
        }
    })?;
    Ok(spans
        .into_iter()
        .map(|(a, b)| jpeg[a..b].to_vec())
        .collect())
}

/// Wrap a bare TIFF block as an APP1 EXIF segment.
///
/// This is the path for a source that is not a JPEG. A HEIF keeps its EXIF as
/// an item rather than a segment, so there is nothing to copy across verbatim
/// and the envelope has to be built.
pub fn app1_from_tiff(tiff: &[u8]) -> Result<Vec<u8>, String> {
    let len = tiff.len() + 6 + 2;
    if len > 0xFFFF {
        // ExifTool splits across segments here. Refusing is honest; writing a
        // truncated EXIF block would be a file that parses and lies.
        return Err(format!(
            "EXIF is {} bytes, too large for one APP1 segment",
            tiff.len()
        ));
    }
    let mut seg = vec![0xFF, 0xE1];
    seg.extend_from_slice(&(len as u16).to_be_bytes());
    seg.extend_from_slice(b"Exif\0\0");
    seg.extend_from_slice(tiff);
    Ok(seg)
}

/// Rebuild the JPEG without metadata, optionally inserting segments after SOI.
pub fn rewrite(jpeg: &[u8], insert: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let mut kept: Vec<(usize, usize)> = Vec::new();
    let scan_at = walk(jpeg, |m, a, b| {
        if !is_metadata(m) {
            kept.push((a, b));
        }
    })?;

    let mut out = Vec::with_capacity(jpeg.len());
    out.extend_from_slice(&jpeg[..2]); // SOI
    for seg in insert {
        out.extend_from_slice(seg);
    }
    for (a, b) in kept {
        out.extend_from_slice(&jpeg[a..b]);
    }
    // The scan and everything after it, verbatim.
    out.extend_from_slice(&jpeg[scan_at..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SOI, an APP1, a DQT, then SOS and some scan data containing an 0xFF
    /// byte that must not be read as a marker.
    fn sample() -> Vec<u8> {
        let mut d = vec![0xFF, 0xD8];
        d.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08]);
        d.extend_from_slice(b"Exif\0\0");
        d.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x04, 0x11, 0x22]);
        d.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x03, 0x00]);
        d.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD9]);
        d
    }

    #[test]
    fn stripping_keeps_the_coding_segments_and_the_scan() {
        let out = rewrite(&sample(), &[]).unwrap();
        assert_eq!(&out[..2], &[0xFF, 0xD8]);
        assert!(app1_segments(&out).unwrap().is_empty(), "APP1 is gone");
        // DQT survives, and so does the scan including its 0xFF byte.
        assert!(out.windows(2).any(|w| w == [0xFF, 0xDB]));
        assert!(out.ends_with(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD9]));
    }

    #[test]
    fn grafting_puts_the_source_segment_first() {
        let src = sample();
        let app1 = app1_segments(&src).unwrap();
        assert_eq!(app1.len(), 1);
        let bare = rewrite(&sample(), &[]).unwrap();
        let out = rewrite(&bare, &app1).unwrap();
        assert_eq!(&out[2..4], &[0xFF, 0xE1], "APP1 sits directly after SOI");
        assert_eq!(app1_segments(&out).unwrap().len(), 1);
    }

    #[test]
    fn the_scan_is_never_walked() {
        // A parser that keeps reading past SOS finds 0xFF 0x00 and treats it
        // as a marker, then runs off the end. Reaching this assertion at all
        // is the test.
        let out = rewrite(&sample(), &[]).unwrap();
        assert!(out.len() > 8);
    }

    #[test]
    fn a_tiff_block_becomes_a_well_formed_app1() {
        let seg = app1_from_tiff(&[0x49, 0x49, 0x2A, 0x00]).unwrap();
        assert_eq!(&seg[..2], &[0xFF, 0xE1]);
        assert_eq!(u16::from_be_bytes([seg[2], seg[3]]) as usize, seg.len() - 2);
        assert_eq!(&seg[4..10], b"Exif\0\0");
    }

    #[test]
    fn an_oversized_exif_is_refused_rather_than_truncated() {
        assert!(app1_from_tiff(&vec![0u8; 70_000]).is_err());
    }

    #[test]
    fn a_file_that_is_not_a_jpeg_is_refused() {
        assert!(rewrite(&[0x89, b'P', b'N', b'G'], &[]).is_err());
    }
}
