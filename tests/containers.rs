//! The three containers, built byte by byte.
//!
//! Each test pins one way a container can be read wrongly WHILE STILL PARSING,
//! which is the failure mode that matters: a wrong base or a wrong field width
//! leaves the numbers looking fine and quietly empties every string.

use std::io::Write;

/// A TIFF with Make, Model and an ExifIFD holding one rational.
fn tiff() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&[0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]); // II*, IFD0 at 8
    d.extend_from_slice(&3u16.to_le_bytes());
    let entry = |d: &mut Vec<u8>, tag: u16, fmt: u16, count: u32, val: u32| {
        d.extend_from_slice(&tag.to_le_bytes());
        d.extend_from_slice(&fmt.to_le_bytes());
        d.extend_from_slice(&count.to_le_bytes());
        d.extend_from_slice(&val.to_le_bytes());
    };
    // IFD0 is 8 + 2 + 3*12 + 4 = 50 bytes, so the values start there.
    entry(&mut d, 0x010F, 2, 9, 50); // Make
    entry(&mut d, 0x0110, 2, 7, 59); // Model
    entry(&mut d, 0x8769, 4, 1, 66); // ExifIFD
    d.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(d.len(), 50);
    d.extend_from_slice(b"FUJIFILM\0");
    d.extend_from_slice(b"X-T50\0\0");
    assert_eq!(d.len(), 66);
    // ExifIFD: one entry, ExposureTime = 1/500, its rational at 84
    d.extend_from_slice(&1u16.to_le_bytes());
    entry(&mut d, 0x829A, 5, 1, 84);
    d.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(d.len(), 84);
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&500u32.to_le_bytes());
    d
}

fn jpeg_with(tiff: &[u8]) -> Vec<u8> {
    let mut d = vec![0xFF, 0xD8];
    let payload_len = tiff.len() + 6 + 2;
    d.extend_from_slice(&[0xFF, 0xE1]);
    d.extend_from_slice(&(payload_len as u16).to_be_bytes());
    d.extend_from_slice(b"Exif\0\0");
    d.extend_from_slice(tiff);
    d.extend_from_slice(&[0xFF, 0xDA]); // SOS, where the walk must stop
    d
}

fn write(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("exif-sooc-{name}"));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(bytes).unwrap();
    p
}

#[test]
fn jpeg_exif_is_read() {
    let p = write("t.jpg", &jpeg_with(&tiff()));
    let photo = exif_sooc::read(&p).unwrap();
    assert_eq!(photo.camera().as_deref(), Some("FUJIFILM X-T50"));
    assert_eq!(photo.get("ExposureTime").unwrap().print, "1/500");
}

#[test]
fn heif_exif_is_read() {
    // meta -> iinf (one infe, version 2, type Exif) -> iloc -> payload
    let boxed = |kind: &[u8; 4], body: &[u8]| {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(body);
        b
    };
    const ID: u16 = 768;
    let mut infe = vec![2u8, 0, 0, 0];
    infe.extend_from_slice(&ID.to_be_bytes());
    infe.extend_from_slice(&0u16.to_be_bytes());
    infe.extend_from_slice(b"Exif");
    infe.push(0);
    let infe = boxed(b"infe", &infe);

    let mut iinf = vec![0u8, 0, 0, 0];
    iinf.extend_from_slice(&1u16.to_be_bytes());
    iinf.extend_from_slice(&infe);
    let iinf = boxed(b"iinf", &iinf);

    let mut iloc = vec![1u8, 0, 0, 0];
    iloc.extend_from_slice(&0x4400u16.to_be_bytes()); // offset 4B, length 4B
    iloc.extend_from_slice(&1u16.to_be_bytes());
    iloc.extend_from_slice(&ID.to_be_bytes());
    iloc.extend_from_slice(&0u16.to_be_bytes()); // construction method 0
    iloc.extend_from_slice(&0u16.to_be_bytes()); // data reference
    iloc.extend_from_slice(&1u16.to_be_bytes()); // one extent
    let patch = iloc.len();
    iloc.extend_from_slice(&0u32.to_be_bytes());
    let tiff = tiff();
    let payload_len = 4 + 6 + tiff.len();
    iloc.extend_from_slice(&(payload_len as u32).to_be_bytes());
    let iloc = boxed(b"iloc", &iloc);

    let mut meta = vec![0u8, 0, 0, 0];
    meta.extend_from_slice(&iinf);
    let iloc_in_meta = meta.len();
    meta.extend_from_slice(&iloc);
    let meta = boxed(b"meta", &meta);

    let mut ftyp = b"heix".to_vec();
    ftyp.extend_from_slice(&0u32.to_be_bytes());
    ftyp.extend_from_slice(b"mif1heix");
    let ftyp = boxed(b"ftyp", &ftyp);

    let mut file = ftyp.clone();
    let meta_at = file.len();
    file.extend_from_slice(&meta);
    let payload_at = file.len() as u32;
    file.extend_from_slice(&6u32.to_be_bytes());
    file.extend_from_slice(b"Exif\0\0");
    file.extend_from_slice(&tiff);

    let at = meta_at + 8 + iloc_in_meta + 8 + patch;
    file[at..at + 4].copy_from_slice(&payload_at.to_be_bytes());

    let p = write("t.hif", &file);
    let photo = exif_sooc::read(&p).unwrap();
    assert_eq!(photo.format, exif_sooc::Format::Heif);
    assert_eq!(photo.camera().as_deref(), Some("FUJIFILM X-T50"));
}

#[test]
fn raf_reduces_to_its_embedded_jpeg() {
    let jpeg = jpeg_with(&tiff());
    let mut d = vec![0u8; 112];
    d[..8].copy_from_slice(b"FUJIFILM");
    // ExifTool: FujiFilm.pm:1940 — position and length at byte 84
    d[84..88].copy_from_slice(&112u32.to_be_bytes());
    d[88..92].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
    d.extend_from_slice(&jpeg);
    let p = write("t.raf", &d);
    let photo = exif_sooc::read(&p).unwrap();
    assert_eq!(photo.format, exif_sooc::Format::Raf);
    assert_eq!(photo.get("ExposureTime").unwrap().print, "1/500");
}

#[test]
fn portrait_dimensions_are_orientation_corrected() {
    // A camera stores sensor-native landscape pixels plus an Orientation tag.
    // Reporting the stored pair puts every vertical shot in a photo grid at
    // the wrong aspect ratio.
    let mut t = tiff();
    // Append an IFD0 entry set: rebuild a small TIFF with the three tags.
    t.clear();
    t.extend_from_slice(&[0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]);
    t.extend_from_slice(&2u16.to_le_bytes());
    let mut e = |tag: u16, fmt: u16, count: u32, val: u32| {
        t.extend_from_slice(&tag.to_le_bytes());
        t.extend_from_slice(&fmt.to_le_bytes());
        t.extend_from_slice(&count.to_le_bytes());
        t.extend_from_slice(&val.to_le_bytes());
    };
    e(0x0112, 3, 1, 8); // Orientation: Rotate 270 CW
    e(0x8769, 4, 1, 38); // ExifIFD, right after this one (8 + 2 + 2*12 + 4)
    t.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(t.len(), 38);
    t.extend_from_slice(&2u16.to_le_bytes());
    let mut e2 = |tag: u16, fmt: u16, count: u32, val: u32| {
        t.extend_from_slice(&tag.to_le_bytes());
        t.extend_from_slice(&fmt.to_le_bytes());
        t.extend_from_slice(&count.to_le_bytes());
        t.extend_from_slice(&val.to_le_bytes());
    };
    e2(0xA002, 4, 1, 7728);
    e2(0xA003, 4, 1, 5152);
    t.extend_from_slice(&0u32.to_le_bytes());

    let p = write("portrait.jpg", &jpeg_with(&t));
    let photo = exif_sooc::read(&p).unwrap();
    assert_eq!(photo.dimensions(), Some((5152, 7728)));
}

#[test]
fn a_file_that_is_none_of_the_three_is_refused() {
    let p = write("t.png", &[0x89, b'P', b'N', b'G', 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(matches!(
        exif_sooc::read(&p),
        Err(exif_sooc::Error::Unsupported)
    ));
}

#[test]
fn truncated_files_do_not_panic() {
    let full = jpeg_with(&tiff());
    for cut in [2, 8, 20, 40, full.len() - 3] {
        let p = write(&format!("cut{cut}.jpg"), &full[..cut]);
        let _ = exif_sooc::read(&p);
    }
}
