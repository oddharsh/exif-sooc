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

/// A HEIF carrying EXIF (item 768) and, optionally, a `mime` item (769) whose
/// payload is `xmp` split across `splits` extents, the shape a Fujifilm HIF
/// has. `mime_tail` is what follows the item type: name, content type and
/// content encoding, NUL-terminated.
fn heif_with_xmp(xmp: &[u8], splits: usize, mime_tail: &[u8]) -> Vec<u8> {
    let boxed = |kind: &[u8; 4], body: &[u8]| {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(body);
        b
    };
    let infe = |id: u16, kind: &[u8; 4], tail: &[u8]| {
        let mut e = vec![2u8, 0, 0, 0];
        e.extend_from_slice(&id.to_be_bytes());
        e.extend_from_slice(&0u16.to_be_bytes());
        e.extend_from_slice(kind);
        e.extend_from_slice(tail);
        boxed(b"infe", &e)
    };
    let mut iinf = vec![0u8, 0, 0, 0];
    iinf.extend_from_slice(&2u16.to_be_bytes());
    iinf.extend_from_slice(&infe(768, b"Exif", b"\0"));
    iinf.extend_from_slice(&infe(769, b"mime", mime_tail));
    let iinf = boxed(b"iinf", &iinf);

    let mut exif = 6u32.to_be_bytes().to_vec();
    exif.extend_from_slice(b"Exif\0\0");
    exif.extend_from_slice(&tiff());
    let chunk = xmp.len().div_ceil(splits);
    let parts: Vec<&[u8]> = xmp.chunks(chunk).collect();

    // iloc v1, 4-byte offsets and lengths. Offsets are relative to the file,
    // so they are patched once the meta box's size is known.
    let iloc_len = 8 + 4 + 2 + 2 + (2 + 2 + 2 + 2 + 8) + (2 + 2 + 2 + 2 + 8 * parts.len());
    let meta_len = 8 + 4 + iinf.len() + iloc_len;
    let mut ftyp = b"heix".to_vec();
    ftyp.extend_from_slice(&0u32.to_be_bytes());
    ftyp.extend_from_slice(b"mif1heix");
    let ftyp = boxed(b"ftyp", &ftyp);
    let data_at = (ftyp.len() + meta_len) as u32;

    let mut iloc = vec![1u8, 0, 0, 0];
    iloc.extend_from_slice(&0x4400u16.to_be_bytes());
    iloc.extend_from_slice(&2u16.to_be_bytes());
    let item = |iloc: &mut Vec<u8>, id: u16, spans: &[(u32, u32)]| {
        iloc.extend_from_slice(&id.to_be_bytes());
        iloc.extend_from_slice(&0u16.to_be_bytes()); // construction method 0
        iloc.extend_from_slice(&0u16.to_be_bytes()); // data reference
        iloc.extend_from_slice(&(spans.len() as u16).to_be_bytes());
        for (o, l) in spans {
            iloc.extend_from_slice(&o.to_be_bytes());
            iloc.extend_from_slice(&l.to_be_bytes());
        }
    };
    item(&mut iloc, 768, &[(data_at, exif.len() as u32)]);
    // Lay the extents out in REVERSE file order, so a reader that sorts by
    // offset, or reads one span past the other, assembles the wrong packet.
    let mut spans = Vec::new();
    let mut at = data_at + exif.len() as u32 + xmp.len() as u32;
    for p in &parts {
        at -= p.len() as u32;
        spans.push((at, p.len() as u32));
    }
    item(&mut iloc, 769, &spans);
    let iloc = boxed(b"iloc", &iloc);
    assert_eq!(iloc.len(), iloc_len);

    let mut meta = vec![0u8, 0, 0, 0];
    meta.extend_from_slice(&iinf);
    meta.extend_from_slice(&iloc);
    let meta = boxed(b"meta", &meta);
    assert_eq!(meta.len(), meta_len);

    let mut file = ftyp;
    file.extend_from_slice(&meta);
    file.extend_from_slice(&exif);
    for p in parts.iter().rev() {
        file.extend_from_slice(p);
    }
    file
}

const FUJI_XMP: &[u8] = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"><xmp:Rating>3</xmp:Rating></rdf:Description></rdf:RDF></x:xmpmeta>";
const RDF: &[u8] = b"\0application/rdf+xml\0";

#[test]
fn heif_xmp_is_read_verbatim() {
    let p = write("x1.hif", &heif_with_xmp(FUJI_XMP, 1, RDF));
    assert_eq!(exif_sooc::heif_xmp(&p).unwrap().as_deref(), Some(FUJI_XMP));
    // Reading the XMP item must not disturb finding the EXIF item beside it.
    let photo = exif_sooc::read(&p).unwrap();
    assert_eq!(photo.camera().as_deref(), Some("FUJIFILM X-T50"));
}

#[test]
fn a_split_xmp_item_is_joined_in_extent_order() {
    let p = write("x3.hif", &heif_with_xmp(FUJI_XMP, 3, RDF));
    assert_eq!(exif_sooc::heif_xmp(&p).unwrap().as_deref(), Some(FUJI_XMP));
}

#[test]
fn a_mime_item_that_is_not_xmp_or_is_encoded_is_not_xmp() {
    // Another content type, then XMP that is deflated rather than a packet.
    // A writer may also omit content_encoding entirely, which is the shape
    // heif_xmp_is_read_verbatim already covers.
    let other = write(
        "x-other.hif",
        &heif_with_xmp(FUJI_XMP, 1, b"\0text/plain\0"),
    );
    assert_eq!(exif_sooc::heif_xmp(&other).unwrap(), None);
    let deflated = write(
        "x-deflate.hif",
        &heif_with_xmp(FUJI_XMP, 1, b"\0application/rdf+xml\0deflate\0"),
    );
    assert_eq!(exif_sooc::heif_xmp(&deflated).unwrap(), None);
}

/// Every APP1 in a JPEG, as (marker payload) slices.
fn app1s(jpeg: &[u8]) -> Vec<Vec<u8>> {
    exif_sooc::write::app1_segments(jpeg).unwrap()
}

#[test]
fn tags_from_a_heif_carry_its_xmp_after_its_exif() {
    // The regression this pins: a HIF's XMP never reached the JPEG, so an
    // in-camera star rating vanished from every archive copy.
    let src = write("tff.hif", &heif_with_xmp(FUJI_XMP, 2, RDF));
    let dst = write(
        "tff.jpg",
        &jpeg_with(&[0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0, 0, 0]),
    );
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_exif-sooc"))
        .arg("-TagsFromFile")
        .arg(&src)
        .args(["-all:all", "-overwrite_original"])
        .arg(&dst)
        .status()
        .unwrap();
    assert!(status.success());

    let out = std::fs::read(&dst).unwrap();
    let segs = app1s(&out);
    assert_eq!(segs.len(), 2, "one EXIF and one XMP, nothing left over");
    assert_eq!(
        &segs[0][4..10],
        b"Exif\0\0",
        "EXIF first, as ExifTool orders it"
    );
    let xmp = &segs[1][4..];
    assert!(xmp.starts_with(exif_sooc::write::XMP_HEADER));
    assert_eq!(&xmp[exif_sooc::write::XMP_HEADER.len()..], FUJI_XMP);
    // The destination's own EXIF was stripped, and the source's made it over.
    assert_eq!(
        exif_sooc::read(&dst).unwrap().camera().as_deref(),
        Some("FUJIFILM X-T50")
    );
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

// ── JPEG XL ─────────────────────────────────────────────────────────

fn jbox(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(body);
    b
}

/// A container: signature, ftyp, then the given boxes in order.
fn jxl_with(boxes: &[Vec<u8>]) -> Vec<u8> {
    let mut d = vec![0, 0, 0, 12, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A];
    d.extend(jbox(b"ftyp", b"jxl \0\0\0\0jxl "));
    for b in boxes {
        d.extend_from_slice(b);
    }
    d
}

/// An Exif box body: a zero offset, then the TIFF header.
fn exif_body(tiff: &[u8]) -> Vec<u8> {
    let mut b = 0u32.to_be_bytes().to_vec();
    b.extend_from_slice(tiff);
    b
}

/// tiff() naming a different body, so a copy that kept the old EXIF shows.
fn old_tiff() -> Vec<u8> {
    let mut t = tiff();
    t[59..64].copy_from_slice(b"X-H2S");
    t
}

/// The two codestream parts cjxl writes around the metadata, as stand-ins.
fn parts() -> (Vec<u8>, Vec<u8>) {
    (
        jbox(b"jxlp", b"\0\0\0\0head"),
        jbox(b"jxlp", b"\x80\0\0\x01the rest of the image"),
    )
}

fn run(args: &[&std::ffi::OsStr]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_exif-sooc"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn jxl_exif_is_read_from_a_plain_box() {
    let (head, rest) = parts();
    let p = write(
        "t.jxl",
        &jxl_with(&[head, jbox(b"Exif", &exif_body(&tiff())), rest]),
    );
    let photo = exif_sooc::read(&p).unwrap();
    assert_eq!(photo.format, exif_sooc::Format::Jxl);
    assert_eq!(photo.camera().as_deref(), Some("FUJIFILM X-T50"));
    assert_eq!(photo.get("ExposureTime").unwrap().print, "1/500");
    assert_eq!(photo.get("MIMEType").unwrap().print, "image/jxl");
}

#[test]
fn a_brotli_compressed_exif_box_is_named_rather_than_missing() {
    // cjxl's default: the box is `brob`, its body the inner type then Brotli.
    let mut brob = b"Exif".to_vec();
    brob.extend_from_slice(&[0x1b, 0x2a, 0x00, 0x00]);
    let (head, rest) = parts();
    let p = write("brob.jxl", &jxl_with(&[head, jbox(b"brob", &brob), rest]));
    assert!(matches!(
        exif_sooc::read(&p),
        Err(exif_sooc::Error::Compressed)
    ));
    // Control: the same file with no metadata at all is plain NoExif.
    let (head, rest) = parts();
    let none = write("none.jxl", &jxl_with(&[head, rest]));
    assert!(matches!(
        exif_sooc::read(&none),
        Err(exif_sooc::Error::NoExif)
    ));
}

#[test]
fn a_bare_codestream_is_a_jxl_with_no_exif() {
    let p = write(
        "bare.jxl",
        &[0xFF, 0x0A, 0xFA, 0x7A, 0x00, 0x00, 0x00, 0x00],
    );
    assert!(matches!(exif_sooc::read(&p), Err(exif_sooc::Error::NoExif)));
}

#[test]
fn tags_from_a_heif_replace_only_a_jxl_s_metadata_boxes() {
    // The job this exists for: an archive encoded from a HIF's pixels carries
    // whatever EXIF the decoder kept (sips drops every Fujifilm maker note),
    // and the HIF's own EXIF and XMP have to replace it without touching a
    // codestream byte.
    let src = write("tfj.hif", &heif_with_xmp(FUJI_XMP, 1, RDF));
    let (head, rest) = parts();
    let mut stale_xmp = b"xml ".to_vec();
    stale_xmp.extend_from_slice(&[0x1b, 0x00]);
    let before = jxl_with(&[
        head.clone(),
        jbox(b"Exif", &exif_body(&old_tiff())),
        jbox(b"brob", &stale_xmp),
        rest.clone(),
    ]);
    let dst = write("tfj.jxl", &before);
    let out = run(&[
        "-TagsFromFile".as_ref(),
        src.as_os_str(),
        "-all:all".as_ref(),
        "-overwrite_original".as_ref(),
        dst.as_os_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let after = std::fs::read(&dst).unwrap();
    assert_eq!(
        after,
        jxl_with(&[
            head,
            jbox(b"Exif", &exif_body(&tiff())),
            jbox(b"xml ", FUJI_XMP),
            rest
        ]),
        "one Exif and one xml box where the old metadata was, every other box byte for byte"
    );
    assert_eq!(
        exif_sooc::read(&dst).unwrap().camera().as_deref(),
        Some("FUJIFILM X-T50")
    );
    assert_eq!(
        exif_sooc::heif_xmp(&dst).unwrap().as_deref(),
        Some(FUJI_XMP)
    );
}

#[test]
fn a_jxl_that_can_rebuild_a_jpeg_is_never_edited() {
    // jbrd is how djxl rebuilds the original JPEG byte for byte, and it covers
    // the EXIF. An edit would leave a file that decodes but no longer rebuilds.
    let src = write("jb.hif", &heif_with_xmp(FUJI_XMP, 1, RDF));
    let (head, rest) = parts();
    let before = jxl_with(&[
        head,
        jbox(b"jbrd", b"reconstruction"),
        jbox(b"Exif", &exif_body(&old_tiff())),
        rest,
    ]);
    let dst = write("jb.jxl", &before);
    for args in [
        vec![
            "-TagsFromFile".as_ref(),
            src.as_os_str(),
            "-all:all".as_ref(),
        ],
        vec!["-all=".as_ref()],
    ] {
        let mut args = args;
        args.extend(["-overwrite_original".as_ref(), dst.as_os_str()]);
        let out = run(&args);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("jbrd"));
        assert_eq!(
            std::fs::read(&dst).unwrap(),
            before,
            "refused means untouched"
        );
    }
}

#[test]
fn strip_removes_a_jxl_s_metadata_and_keeps_its_codestream() {
    let (head, rest) = parts();
    let dst = write(
        "strip.jxl",
        &jxl_with(&[
            head.clone(),
            jbox(b"Exif", &exif_body(&tiff())),
            jbox(b"xml ", FUJI_XMP),
            rest.clone(),
        ]),
    );
    let out = run(&[
        "-all=".as_ref(),
        "-overwrite_original".as_ref(),
        dst.as_os_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(&dst).unwrap(), jxl_with(&[head, rest]));
}

#[test]
fn a_jxl_s_orientation_is_not_rewritten_through_exif() {
    // A decoder obeys the codestream's orientation field and ignores EXIF's,
    // so changing only EXIF would make the two disagree.
    let src = write("or.hif", &heif_with_xmp(FUJI_XMP, 1, RDF));
    let (head, rest) = parts();
    let before = jxl_with(&[head, rest]);
    let dst = write("or.jxl", &before);
    let out = run(&[
        "-TagsFromFile".as_ref(),
        src.as_os_str(),
        "-all:all".as_ref(),
        "-Orientation#=6".as_ref(),
        "-overwrite_original".as_ref(),
        dst.as_os_str(),
    ]);
    assert!(!out.status.success());
    assert_eq!(std::fs::read(&dst).unwrap(), before);
}
