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

/// Overwrite EXIF Orientation inside an APP1 segment.
///
/// The one tag worth being able to set. When metadata is copied onto an export
/// whose rotation is already baked into its pixels, carrying the source's
/// Orientation across tells every viewer that honours EXIF to turn the frame a
/// second time, and a portrait lands on its side.
///
/// Orientation is a SHORT stored inline in its directory entry, so this
/// overwrites two bytes and touches nothing else. A file that has no
/// Orientation entry is left alone rather than grown: inserting an entry means
/// rewriting every offset in the directory, which is a different and much
/// riskier operation than this needs.
pub fn set_orientation(app1: &[u8], value: u16) -> Result<Vec<u8>, String> {
    if app1.len() < 20 || app1[0] != 0xFF || app1[1] != 0xE1 || &app1[4..10] != b"Exif\0\0" {
        return Err("not an APP1 EXIF segment".into());
    }
    let tiff_at = 10;
    let t = &app1[tiff_at..];
    let le = match t.get(0..4) {
        Some([0x49, 0x49, 0x2A, 0x00]) => true,
        Some([0x4D, 0x4D, 0x00, 0x2A]) => false,
        _ => return Err("no TIFF header".into()),
    };
    let rd32 = |b: &[u8]| {
        if le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        }
    };
    let rd16 = |b: &[u8]| {
        if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        }
    };
    let ifd0 = rd32(t.get(4..8).ok_or("truncated header")?) as usize;
    let n = rd16(t.get(ifd0..ifd0 + 2).ok_or("truncated IFD")?) as usize;

    let mut out = app1.to_vec();
    for i in 0..n {
        let e = ifd0 + 2 + i * 12;
        let tag = rd16(t.get(e..e + 2).ok_or("truncated entry")?);
        if tag == 0x0112 {
            let at = tiff_at + e + 8;
            let bytes = if le {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            };
            out[at..at + 2].copy_from_slice(&bytes);
            return Ok(out);
        }
    }
    Ok(out)
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
    // The scan, up to and including EOI. Anything past that is a TRAILER:
    // some cameras append a second image or a preview there, and ExifTool
    // treats it as metadata and drops it. Leaving it behind means `-all=`
    // returning a file that still carries 98 KB of payload on a Leica.
    let end = scan_end(jpeg, scan_at);
    out.extend_from_slice(&jpeg[scan_at..end]);
    Ok(out)
}

/// Where the image really ends: the EOI that closes the last scan.
///
/// A BASELINE JPEG has one scan, so the first marker after SOS is the end. A
/// PROGRESSIVE one has many, separated by DHT and further SOS segments, and
/// stopping at the first marker truncates it to the first scan. That is not a
/// degraded image, it is a broken file: a 27 KB thumbnail came out 4.5 KB
/// ending on a DHT marker.
///
/// So this walks: inside entropy-coded data a literal 0xFF is stuffed as
/// `FF 00` and restart markers `FF D0`..`FF D7` are part of the stream, while
/// anything else is a real marker. EOI ends the image; any other marker starts
/// a segment whose length is skipped before scanning resumes.
fn scan_end(d: &[u8], scan_at: usize) -> usize {
    let mut i = scan_at;
    loop {
        // Skip this segment's header (SOS and friends carry a length).
        if i + 3 >= d.len() {
            return d.len();
        }
        let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        i += 2 + len.max(2);

        // Then the entropy-coded data, up to the next real marker.
        while i + 1 < d.len() {
            if d[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = d[i + 1];
            if m == 0x00 || m == 0xFF || (0xD0..=0xD7).contains(&m) {
                i += 2;
                continue;
            }
            if m == 0xD9 {
                return (i + 2).min(d.len()); // EOI, inclusive
            }
            break; // another segment: DHT, SOS, DQT on the way to more scans
        }
        if i + 1 >= d.len() {
            return d.len();
        }
    }
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

    /// A progressive JPEG: two scans separated by a DHT, then EOI. Stopping at
    /// the first marker after the first scan cuts the image in half, which is
    /// exactly what shipped and destroyed a corpus of thumbnails.
    fn progressive() -> Vec<u8> {
        let mut d = vec![0xFF, 0xD8];
        d.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08]);
        d.extend_from_slice(b"Exif\0\0");
        d.extend_from_slice(&[0xFF, 0xC2, 0x00, 0x04, 0x11, 0x22]); // SOF2
        d.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x03, 0x00]); // first SOS
        d.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34]); // entropy, stuffed FF
        d.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x04, 0xAA, 0xBB]); // DHT
        d.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x03, 0x01]); // second SOS
        d.extend_from_slice(&[0x56, 0xFF, 0x00, 0x78]); // more entropy
        d.extend_from_slice(&[0xFF, 0xD9]);
        d
    }

    #[test]
    fn a_progressive_jpeg_keeps_every_scan() {
        let full = progressive();
        let out = rewrite(&full, &[]).unwrap();
        assert!(out.ends_with(&[0xFF, 0xD9]), "ends at EOI");
        // Both scans and the DHT between them survive.
        assert_eq!(out.windows(2).filter(|w| *w == [0xFF, 0xDA]).count(), 2);
        assert!(out.windows(2).any(|w| w == [0xFF, 0xC4]));
        assert!(out.windows(4).any(|w| w == [0x56, 0xFF, 0x00, 0x78]));
        assert!(
            app1_segments(&out).unwrap().is_empty(),
            "metadata still gone"
        );
    }

    #[test]
    fn a_trailer_after_a_progressive_image_is_still_dropped() {
        let mut d = progressive();
        d.extend_from_slice(b"TRAILING");
        let out = rewrite(&d, &[]).unwrap();
        assert!(out.ends_with(&[0xFF, 0xD9]));
        assert!(!out.windows(8).any(|w| w == b"TRAILING"));
    }

    #[test]
    fn a_trailer_after_eoi_is_dropped() {
        let mut d = sample();
        d.extend_from_slice(b"TRAILING PAYLOAD");
        let out = rewrite(&d, &[]).unwrap();
        assert!(out.ends_with(&[0xFF, 0xD9]), "the file ends at EOI");
        assert!(!out.windows(8).any(|w| w == b"TRAILING"));
    }

    #[test]
    fn a_stuffed_ff_in_the_scan_is_not_the_end() {
        // FF 00 is a literal 0xFF in the entropy stream. Treating it as a
        // marker truncates the photograph.
        let out = rewrite(&sample(), &[]).unwrap();
        assert!(out.ends_with(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD9]));
    }

    #[test]
    fn a_tiff_block_becomes_a_well_formed_app1() {
        let seg = app1_from_tiff(&[0x49, 0x49, 0x2A, 0x00]).unwrap();
        assert_eq!(&seg[..2], &[0xFF, 0xE1]);
        assert_eq!(u16::from_be_bytes([seg[2], seg[3]]) as usize, seg.len() - 2);
        assert_eq!(&seg[4..10], b"Exif\0\0");
    }

    #[test]
    fn orientation_can_be_forced_to_upright() {
        // A minimal APP1 with one IFD0 entry: Orientation = 8.
        let mut tiff = vec![0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0];
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let seg = app1_from_tiff(&tiff).unwrap();
        let out = set_orientation(&seg, 1).unwrap();
        // The value sits inline in the entry, so only those bytes move.
        assert_eq!(out.len(), seg.len());
        assert_ne!(out, seg);
        let at = 10 + 8 + 2 + 8;
        assert_eq!(u16::from_le_bytes([out[at], out[at + 1]]), 1);
    }

    #[test]
    fn a_segment_without_orientation_is_left_alone() {
        let tiff = vec![0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let seg = app1_from_tiff(&tiff).unwrap();
        assert_eq!(set_orientation(&seg, 1).unwrap(), seg);
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
