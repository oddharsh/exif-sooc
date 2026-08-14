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
pub mod tags;
mod tiff;
pub mod value;

use std::path::{Path, PathBuf};

pub use value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Heif,
    Raf,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Heif => "HEIF",
            Format::Raf => "RAF",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Exif,
    /// Fujifilm MakerNotes.
    Fuji,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
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

/// Read one file.
pub fn read(path: impl AsRef<Path>) -> Result<Photo, Error> {
    let path = path.as_ref();
    let mut src = read::Source::open(path)?;
    let format = sniff(src.front()).ok_or(Error::Unsupported)?;

    let (tiff, _at) = match format {
        Format::Jpeg => jpeg::exif(&mut src).ok_or(Error::NoExif)?,
        Format::Heif => bmff::exif(&mut src).ok_or(Error::NoExif)?,
        Format::Raf => {
            let (mut inner, base) = raf::jpeg(&mut src).ok_or(Error::NoExif)?;
            let (t, off) = jpeg::exif(&mut inner).ok_or(Error::NoExif)?;
            (t, base + off)
        }
    };

    let mut tags = Vec::new();
    walk(&tiff, &mut tags);
    Ok(Photo {
        path: path.to_path_buf(),
        format,
        tags,
    })
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

    while let Some(at) = queue.pop() {
        // A malformed file can point a directory at itself.
        if seen.contains(&at) || seen.len() > 8 {
            continue;
        }
        seen.push(at);

        for (i, e) in dir.entries(at).iter().enumerate() {
            let entry_at = at + 2 + i * 12;
            if e.tag == tags::EXIF_IFD {
                queue.push(e.value as usize);
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
            out.push(Tag {
                group: Group::Exif,
                name: def.name,
                value,
                print,
            });
        }
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
