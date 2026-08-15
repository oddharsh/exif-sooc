//! Read camera metadata out of straight-out-of-camera files.
//!
//! Built for a specific job: pull the fields a photographer cares about, and
//! the whole Fujifilm film recipe, out of a folder of JPEG, HEIF and RAF files
//! without reading gigabytes to do it.
//!
//! ```no_run
//! let photo = exif_sooc::read("DSCF1234.HIF").unwrap();
//! println!("{} on {}", photo.camera().unwrap_or_default(), photo.lens().unwrap_or_default());
//! if let Some(r) = photo.recipe() {
//!     println!("{}", r.film_simulation.unwrap_or_default());
//! }
//! ```
//!
//! What it deliberately does not do: every camera brand, every tag, or writing.
//! ExifTool is 25 years of camera quirks and is the right tool for all three.

mod bmff;
mod fuji;
mod fuji_tags;
mod jpeg;
pub mod json;
mod raf;
mod read;
pub mod site;
pub mod tags;
mod tiff;
pub mod value;
pub mod write;

use std::path::{Path, PathBuf};

pub use value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Heif,
    Raf,
    /// A TIFF-family file, which is what a Leica DNG is.
    Dng,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Heif => "HEIF",
            Format::Raf => "RAF",
            Format::Dng => "DNG",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Facts about the file rather than about the exposure.
    File,
    Exif,
    /// Fujifilm MakerNotes.
    Fuji,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
            Group::File => "File",
            Group::Exif => "EXIF",
            Group::Fuji => "FujiFilm",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tag {
    pub group: Group,
    pub name: &'static str,
    /// The value as the file stores it.
    pub value: Value,
    /// The value as a person reads it.
    pub print: String,
}

#[derive(Debug, Clone)]
pub struct Photo {
    pub path: PathBuf,
    pub format: Format,
    pub tags: Vec<Tag>,
}

/// The in-camera settings that make a Fujifilm JPEG look the way it does.
///
/// Every field is optional and nothing is invented: a body that does not write
/// grain leaves grain `None` rather than reporting "Off".
#[derive(Debug, Clone, Default)]
pub struct Recipe {
    pub film_simulation: Option<String>,
    pub dynamic_range: Option<String>,
    pub grain: Option<String>,
    pub grain_size: Option<String>,
    pub color_chrome: Option<String>,
    pub color_chrome_fx_blue: Option<String>,
    pub white_balance: Option<String>,
    pub white_balance_shift: Option<String>,
    pub color_temperature: Option<u32>,
    pub highlight: Option<String>,
    pub shadow: Option<String>,
    pub color: Option<String>,
    pub sharpness: Option<String>,
    pub noise_reduction: Option<String>,
    pub clarity: Option<String>,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The file is not one of the three containers this reads.
    Unsupported,
    /// The container parsed but carries no EXIF.
    NoExif,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Unsupported => write!(f, "not a JPEG, HEIF or RAF"),
            Error::NoExif => write!(f, "no EXIF found"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// The raw TIFF block holding a file's EXIF, whatever container it came in.
///
/// Exposed because copying metadata between files needs the bytes rather than
/// the parsed tags: a HEIF keeps its EXIF as an item and a JPEG keeps it in a
/// segment, and re-serialising parsed tags would lose everything this crate
/// does not have a table for.
pub fn exif_tiff(path: impl AsRef<Path>) -> Result<Vec<u8>, Error> {
    let path = path.as_ref();
    let mut src = read::Source::open(path)?;
    let format = sniff(src.front()).ok_or(Error::Unsupported)?;
    let (tiff, _) = match format {
        Format::Jpeg => jpeg::exif(&mut src).ok_or(Error::NoExif)?,
        Format::Heif => bmff::exif(&mut src).ok_or(Error::NoExif)?,
        Format::Raf => {
            let (mut inner, base) = raf::jpeg(&mut src).ok_or(Error::NoExif)?;
            let (t, off) = jpeg::exif(&mut inner).ok_or(Error::NoExif)?;
            (t, base + off)
        }
        Format::Dng => (src.front().to_vec(), 0),
    };
    Ok(tiff)
}

/// Read one file.
pub fn read(path: impl AsRef<Path>) -> Result<Photo, Error> {
    let path = path.as_ref();
    let mut src = read::Source::open(path)?;
    let format = sniff(src.front()).ok_or(Error::Unsupported)?;
    let mut raf_model = None;

    let (tiff, _at) = match format {
        Format::Jpeg => jpeg::exif(&mut src).ok_or(Error::NoExif)?,
        Format::Heif => bmff::exif(&mut src).ok_or(Error::NoExif)?,
        Format::Raf => {
            raf_model = raf::model(&src);
            let (mut inner, base) = raf::jpeg(&mut src).ok_or(Error::NoExif)?;
            let (t, off) = jpeg::exif(&mut inner).ok_or(Error::NoExif)?;
            (t, base + off)
        }
        // The file IS the TIFF, so there is nothing to extract. The window has
        // to be grown to cover the directories first, since a DNG addresses
        // its values across the whole file rather than inside a small block.
        Format::Dng => {
            let want = tiff_span(&mut src)?;
            src.ensure(want)?;
            (src.front().to_vec(), 0)
        }
    };

    let mut tags = Vec::new();
    file_tags(path, format, &src, &mut tags);
    walk(&tiff, &mut tags);
    // The SOF is authoritative for a JPEG and is the only source when the file
    // carries no EXIF dimensions.
    if format == Format::Jpeg {
        if let Some((w, h)) = jpeg::dimensions(&src) {
            push_size(&mut tags, w, h);
        }
    }
    // Not for RAF. ExifTool reports the RAW frame size there, read out of the
    // RAF header's CFA block, while the EXIF inside describes the embedded
    // JPEG preview. Mirroring the preview's size would put a confidently wrong
    // number under the name a caller trusts. The header parse needs a real RAF
    // to verify against, and ExifTool's own sample is truncated to 38 KB.
    if format != Format::Raf {
        add_image_size(&mut tags);
    }
    // A RAF writes its model into the container header as well as into the
    // embedded EXIF, which is the only source if the embedded JPEG is short.
    if let Some(m) = raf_model {
        if !m.is_empty() && !tags.iter().any(|t| t.name == "Model") {
            tags.push(Tag {
                group: Group::Exif,
                name: "Model",
                value: Value::Text(m.clone()),
                print: m,
            });
        }
    }
    Ok(Photo {
        path: path.to_path_buf(),
        format,
        tags,
    })
}

/// How much of a TIFF has to be resident to walk IFD0 and its EXIF directory.
///
/// Two passes over a handful of entries, which is cheaper than reading a
/// 40 MB raw file to find a lens name 900 bytes in.
fn tiff_span(src: &mut read::Source) -> Result<usize, Error> {
    const STEP: usize = 128 * 1024;
    let mut want = STEP;
    // Two rounds: the first sizes IFD0, the second sizes whatever it points at.
    for _ in 0..2 {
        src.ensure(want)?;
        let data = src.front();
        let Some((first, le)) = tiff::header(data) else {
            return Err(Error::NoExif);
        };
        let dir = tiff::Dir::new(data, le);
        let mut need = first + 6;
        for at in [first] {
            for e in dir.entries(at) {
                if e.tag == tags::EXIF_IFD {
                    need = need.max(e.value as usize + STEP.min(64 * 1024));
                }
                if let Some(end) = dir.value_end(&e) {
                    need = need.max(end);
                }
            }
        }
        if need <= want {
            break;
        }
        want = need.next_multiple_of(STEP);
    }
    Ok(want)
}

/// The File group: what the filesystem and the container say, rather than what
/// the camera wrote. ExifTool reports these, and a caller asking for -FileName
/// is asking for one of them.
fn file_tags(path: &Path, format: Format, src: &read::Source, out: &mut Vec<Tag>) {
    fn push(out: &mut Vec<Tag>, name: &'static str, v: Value) {
        let print = v.to_display();
        out.push(Tag {
            group: Group::File,
            name,
            value: v,
            print,
        });
    }
    if let Some(n) = path.file_name().and_then(|s| s.to_str()) {
        push(out, "FileName", Value::Text(n.to_string()));
    }
    if let Some(d) = path.parent().and_then(|s| s.to_str()) {
        push(
            out,
            "Directory",
            Value::Text(if d.is_empty() {
                ".".into()
            } else {
                d.to_string()
            }),
        );
    }
    // ExifTool prints a readable size and keeps the byte count as the value,
    // so `-FileSize` reads "10 MB" and `-FileSize#` reads 10438475.
    let bytes = src.len();
    out.push(Tag {
        group: Group::File,
        name: "FileSize",
        value: Value::U32(bytes as u32),
        print: file_size(bytes),
    });
    push(out, "FileType", Value::Text(format.as_str().to_string()));
    push(out, "MIMEType", Value::Text(mime(format).to_string()));
}

fn push_size(tags: &mut Vec<Tag>, w: u32, h: u32) {
    for (name, v) in [("ImageWidth", w), ("ImageHeight", h)] {
        if tags.iter().any(|t| t.name == name) {
            continue;
        }
        tags.push(Tag {
            group: Group::File,
            name,
            value: Value::U32(v),
            print: v.to_string(),
        });
    }
}

/// ExifTool reports ImageWidth and ImageHeight in the File group, taken from
/// the image data. For a JPEG that is the SOF marker, which is read directly.
/// For the rest, the EXIF pair agrees on every file measured and is mirrored;
/// the comparison harness is what keeps that honest, since a file where the two
/// disagree fails the suite.
fn add_image_size(tags: &mut Vec<Tag>) {
    let mut add = Vec::new();
    for (from, to) in [
        ("ExifImageWidth", "ImageWidth"),
        ("ExifImageHeight", "ImageHeight"),
    ] {
        if tags.iter().any(|t| t.name == to) {
            continue;
        }
        if let Some(t) = tags.iter().find(|t| t.name == from) {
            add.push(Tag {
                group: Group::File,
                name: to,
                value: t.value.clone(),
                print: t.print.clone(),
            });
        }
    }
    tags.extend(add);
}

/// ExifTool: ExifTool.pm:6863-6870 (decimal units, its default)
fn file_size(n: u64) -> String {
    let f = n as f64;
    if n < 2000 {
        format!("{n} bytes")
    } else if n < 10_000 {
        format!("{:.1} kB", f / 1000.0)
    } else if n < 2_000_000 {
        format!("{:.0} kB", f / 1000.0)
    } else if n < 10_000_000 {
        format!("{:.1} MB", f / 1_000_000.0)
    } else if n < 2_000_000_000 {
        format!("{:.0} MB", f / 1_000_000.0)
    } else if n < 10_000_000_000 {
        format!("{:.1} GB", f / 1_000_000_000.0)
    } else {
        format!("{:.0} GB", f / 1_000_000_000.0)
    }
}

fn mime(f: Format) -> &'static str {
    match f {
        Format::Jpeg => "image/jpeg",
        Format::Heif => "image/heif",
        Format::Raf => "image/x-fujifilm-raf",
        Format::Dng => "image/x-adobe-dng",
    }
}

fn sniff(head: &[u8]) -> Option<Format> {
    if head.starts_with(&[0xFF, 0xD8]) {
        return Some(Format::Jpeg);
    }
    if head.starts_with(raf::MAGIC) {
        return Some(Format::Raf);
    }
    if head.get(4..8)? == b"ftyp" {
        return Some(Format::Heif);
    }
    // TIFF magic. A DNG is a TIFF, which is why Leica raw needs no container
    // handling of its own.
    if head.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || head.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        return Some(Format::Dng);
    }
    None
}

/// Walk IFD0, the EXIF sub-directory it points at, and the MakerNotes.
fn walk(tiff: &[u8], out: &mut Vec<Tag>) {
    let Some((first, le)) = tiff::header(tiff) else {
        return;
    };
    let dir = tiff::Dir::new(tiff, le);
    let mut queue = vec![first];
    let mut seen = Vec::new();

    let mut primary: Option<(Value, String, Value, String)> = None;

    // Breadth-first from IFD0, so that when a tag appears in several
    // directories the one a reader means is reached first. A DNG carries
    // SubfileType in every one of its four.
    let mut cursor = 0usize;
    while cursor < queue.len() {
        let at = queue[cursor];
        cursor += 1;
        // A malformed file can point a directory at itself.
        if seen.contains(&at) || seen.len() > 8 {
            continue;
        }
        seen.push(at);

        let entries = dir.entries(at);
        // A DNG puts a thumbnail in IFD0 and the photograph in a SubIFD, so the
        // frame a person means is the one that declares itself full-resolution
        // rather than the first one or the biggest one. ExifTool resolves the
        // same collision the same way. A file with no SubfileType, which is
        // every JPEG and every RAF, is unaffected.
        let full_res = entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.tag == tags::SUBFILE_TYPE_TAG)
            .and_then(|(i, e)| dir.value(e, at + 2 + i * 12))
            .and_then(|v| v.as_i64())
            == Some(0);
        let mut here: Vec<(&'static str, Value, String)> = Vec::new();

        for (i, e) in entries.iter().enumerate() {
            let entry_at = at + 2 + i * 12;
            if e.tag == tags::EXIF_IFD {
                queue.push(e.value as usize);
                continue;
            }
            // SubIFDs are a pointer ARRAY: a DNG commonly has three, holding
            // the full raw frame, a preview and a reduced copy.
            if e.tag == tags::SUB_IFD {
                match dir.value(e, entry_at) {
                    Some(Value::U32(v)) => queue.push(v as usize),
                    Some(Value::U32s(vs)) => queue.extend(vs.iter().map(|v| *v as usize)),
                    _ => {}
                }
                continue;
            }
            if e.tag == tags::MAKER_NOTE {
                let start = e.value as usize;
                let len = e.count as usize;
                if let Some(block) = tiff.get(start..start.saturating_add(len)) {
                    if fuji::is_fuji(block) {
                        for (name, value, print) in fuji::parse(block) {
                            out.push(Tag {
                                group: Group::Fuji,
                                name,
                                value,
                                print,
                            });
                        }
                    }
                }
                continue;
            }
            let Some(def) = tags::find(tags::EXIF, e.tag) else {
                continue;
            };
            let Some(value) = dir.value(e, entry_at) else {
                continue;
            };
            let print = def.print(&value);
            here.push((def.name, value.clone(), print.clone()));
            out.push(Tag {
                group: Group::Exif,
                name: def.name,
                value,
                print,
            });
        }

        if full_res {
            let get = |n: &str| here.iter().find(|(name, _, _)| *name == n).cloned();
            if let (Some(w), Some(h)) = (get("ImageWidth"), get("ImageHeight")) {
                primary = Some((w.1, w.2, h.1, h.2));
            }
        }
    }

    // One entry per name. A JSON object cannot hold a key twice anyway, so
    // emitting duplicates only decides which one silently disappears.
    let mut seen: Vec<(Group, &str)> = Vec::new();
    out.retain(|t| {
        let key = (t.group, t.name);
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });

    if let Some((wv, wp, hv, hp)) = primary {
        out.retain(|t| t.name != "ImageWidth" && t.name != "ImageHeight");
        out.push(Tag {
            group: Group::Exif,
            name: "ImageWidth",
            value: wv,
            print: wp,
        });
        out.push(Tag {
            group: Group::Exif,
            name: "ImageHeight",
            value: hv,
            print: hp,
        });
    }
}

impl Photo {
    pub fn get(&self, name: &str) -> Option<&Tag> {
        self.tags.iter().find(|t| t.name == name)
    }

    pub fn get_in(&self, group: Group, name: &str) -> Option<&Tag> {
        self.tags
            .iter()
            .find(|t| t.group == group && t.name == name)
    }

    fn text(&self, name: &str) -> Option<String> {
        self.get(name).map(|t| t.print.clone())
    }

    fn fuji(&self, name: &str) -> Option<String> {
        self.get_in(Group::Fuji, name).map(|t| t.print.clone())
    }

    /// "FUJIFILM X-T50", or whatever the two tags say together.
    pub fn camera(&self) -> Option<String> {
        let make = self.get_in(Group::Exif, "Make")?.print.clone();
        match self.get_in(Group::Exif, "Model") {
            Some(m) => Some(format!("{make} {}", m.print)),
            None => Some(make),
        }
    }

    pub fn lens(&self) -> Option<String> {
        self.text("LensModel").filter(|s| !s.is_empty())
    }

    /// Pixel dimensions as a viewer shows them.
    ///
    /// Cameras write sensor-native landscape dimensions plus an Orientation
    /// tag, so a portrait frame is stored 7728x5152 and displayed 5152x7728.
    /// Reporting the stored pair is how a photo grid ends up with the wrong
    /// aspect ratio on every vertical shot.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        let w = self.get("ExifImageWidth")?.value.as_i64()? as u32;
        let h = self.get("ExifImageHeight")?.value.as_i64()? as u32;
        let rotated = matches!(
            self.get("Orientation").and_then(|t| t.value.as_i64()),
            Some(5..=8)
        );
        Some(if rotated { (h, w) } else { (w, h) })
    }

    /// The Fujifilm recipe, when the file has one.
    pub fn recipe(&self) -> Option<Recipe> {
        if !self.tags.iter().any(|t| t.group == Group::Fuji) {
            return None;
        }
        Some(Recipe {
            film_simulation: self.fuji("FilmMode"),
            dynamic_range: self.fuji("DynamicRange"),
            grain: self.fuji("GrainEffectRoughness"),
            grain_size: self.fuji("GrainEffectSize"),
            color_chrome: self.fuji("ColorChromeEffect"),
            color_chrome_fx_blue: self.fuji("ColorChromeFXBlue"),
            white_balance: self.fuji("WhiteBalance"),
            white_balance_shift: self.fuji("WhiteBalanceFineTune"),
            color_temperature: self
                .get_in(Group::Fuji, "ColorTemperature")
                .and_then(|t| t.value.as_i64())
                .map(|v| v as u32),
            highlight: self.fuji("HighlightTone"),
            shadow: self.fuji("ShadowTone"),
            color: self.fuji("Saturation"),
            sharpness: self.fuji("Sharpness"),
            noise_reduction: self.fuji("NoiseReduction"),
            clarity: self.fuji("Clarity"),
        })
    }
}
