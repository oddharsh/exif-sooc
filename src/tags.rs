//! Tag definitions and how a raw value becomes a readable one.

use crate::value::{fmt_f64, Value};

pub enum Conv {
    /// Print the value as it is.
    None,
    /// Look the number up. Sorted by key, so this binary searches.
    Map(&'static [(i64, &'static str)]),
    Fn(fn(&Value) -> Option<String>),
}

pub struct TagDef {
    pub id: u16,
    pub name: &'static str,
    pub conv: Conv,
}

pub fn find(table: &'static [TagDef], id: u16) -> Option<&'static TagDef> {
    table
        .binary_search_by_key(&id, |t| t.id)
        .ok()
        .map(|i| &table[i])
}

impl TagDef {
    pub fn print(&self, v: &Value) -> String {
        match &self.conv {
            Conv::None => v.to_display(),
            Conv::Map(m) => v
                .as_i64()
                .and_then(|k| m.binary_search_by_key(&k, |e| e.0).ok().map(|i| m[i].1))
                .map(str::to_string)
                // An unknown code prints as a number rather than as a guess.
                // ExifTool renders these as "Unknown (N)"; the bare number is
                // easier to act on and never pretends to be a name.
                .unwrap_or_else(|| v.to_display()),
            Conv::Fn(f) => f(v).unwrap_or_else(|| v.to_display()),
        }
    }
}

// ---------------------------------------------------------------- formatters

/// Shutter speed, as a photographer writes it.
/// ExifTool: Exif.pm PrintExposureTime
fn exposure_time(v: &Value) -> Option<String> {
    let s = v.as_f64()?;
    if s >= 0.3 {
        Some(fmt_f64(s))
    } else {
        Some(format!("1/{}", (1.0 / s).round() as i64))
    }
}

/// Aperture.
///
/// One decimal above f/1, two below it, and anything that is not a positive
/// number prints as it is. An adapted manual lens writes 0/0, which stays
/// "undef" rather than becoming f/0.
/// ExifTool: Exif.pm PrintFNumber
fn fnumber(v: &Value) -> Option<String> {
    if let Value::Ratio(_, 0) = v {
        return Some("undef".into());
    }
    let n = v.as_f64()?;
    Some(if n <= 0.0 {
        fmt_f64(n)
    } else if n < 1.0 {
        format!("{n:.2}")
    } else {
        format!("{n:.1}")
    })
}

/// Focal length as stored in a rational: one decimal.
fn focal_mm(v: &Value) -> Option<String> {
    Some(format!("{:.1} mm", v.as_f64()?))
}

/// The 35 mm equivalent is a plain integer in the file, and prints as one.
/// The two focal tags therefore render differently on purpose: 35.0 mm and
/// 53 mm, which is what ExifTool does.
fn focal_mm_int(v: &Value) -> Option<String> {
    Some(format!("{} mm", fmt_f64(v.as_f64()?)))
}

/// Exposure compensation carries its sign, because "0.33" and "+0.33" read the
/// same to a machine and opposite to a person scanning a column of them.
fn ev(v: &Value) -> Option<String> {
    let n = v.as_f64()?;
    if n == 0.0 {
        return Some("0".into());
    }
    let mut s = format!("{n:+.2}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    Some(s)
}

/// Fujifilm writes the white balance shift as two signed longs.
///
/// Printed undivided, which is what ExifTool does
/// (FujiFilm.pm:230, `sprintf("Red %+d, Blue %+d", ...)`). Its own note says
/// newer bodies should divide by 20 to get the number the camera menu shows,
/// and it does not do that, so neither does this: `Photo::recipe()` exposes
/// the raw pair and a caller who wants menu units can divide.
fn wb_fine_tune(v: &Value) -> Option<String> {
    let (r, b) = match v {
        Value::I32s(a) if a.len() == 2 => (a[0], a[1]),
        Value::U32s(a) if a.len() == 2 => (a[0] as i32, a[1] as i32),
        _ => return None,
    };
    Some(format!("Red {r:+}, Blue {b:+}"))
}

// ------------------------------------------------------------------- lookups

static ORIENTATION: &[(i64, &str)] = &[
    (1, "Horizontal (normal)"),
    (2, "Mirror horizontal"),
    (3, "Rotate 180"),
    (4, "Mirror vertical"),
    (5, "Mirror horizontal and rotate 270 CW"),
    (6, "Rotate 90 CW"),
    (7, "Mirror horizontal and rotate 90 CW"),
    (8, "Rotate 270 CW"),
];

static EXPOSURE_PROGRAM: &[(i64, &str)] = &[
    (0, "Not Defined"),
    (1, "Manual"),
    (2, "Program AE"),
    (3, "Aperture-priority AE"),
    (4, "Shutter speed priority AE"),
    (5, "Creative (Slow speed)"),
    (6, "Action (High speed)"),
    (7, "Portrait"),
    (8, "Landscape"),
    (9, "Bulb"),
];

static METERING: &[(i64, &str)] = &[
    (0, "Unknown"),
    (1, "Average"),
    (2, "Center-weighted average"),
    (3, "Spot"),
    (4, "Multi-spot"),
    (5, "Multi-segment"),
    (6, "Partial"),
    (255, "Other"),
];

static COLOR_SPACE: &[(i64, &str)] = &[(1, "sRGB"), (2, "Adobe RGB"), (65535, "Uncalibrated")];

static EXPOSURE_MODE: &[(i64, &str)] = &[(0, "Auto"), (1, "Manual"), (2, "Auto bracket")];

static WHITE_BALANCE: &[(i64, &str)] = &[(0, "Auto"), (1, "Manual")];

/// A DNG marks its full-resolution frame here, which is how a raw file says
/// which of its several images is the photograph.
static SUBFILE_TYPE: &[(i64, &str)] = &[
    (0, "Full-resolution image"),
    (1, "Reduced-resolution image"),
    (2, "Single page of multi-page image"),
];

static RESOLUTION_UNIT: &[(i64, &str)] = &[(1, "None"), (2, "inches"), (3, "cm")];

static SCENE_CAPTURE: &[(i64, &str)] = &[
    (0, "Standard"),
    (1, "Landscape"),
    (2, "Portrait"),
    (3, "Night"),
];

/// Flash is a bit field rather than an enum, and the combinations that matter
/// are the ones cameras actually write.
/// ExifTool: Exif.pm %flash
static FLASH: &[(i64, &str)] = &[
    (0x0, "No Flash"),
    (0x1, "Fired"),
    (0x5, "Fired, Return not detected"),
    (0x7, "Fired, Return detected"),
    (0x8, "On, Did not fire"),
    (0x9, "On, Fired"),
    (0xd, "On, Return not detected"),
    (0xf, "On, Return detected"),
    (0x10, "Off, Did not fire"),
    (0x14, "Off, Did not fire, Return not detected"),
    (0x18, "Auto, Did not fire"),
    (0x19, "Auto, Fired"),
    (0x1d, "Auto, Fired, Return not detected"),
    (0x1f, "Auto, Fired, Return detected"),
    (0x20, "No flash function"),
    (0x30, "Off, No flash function"),
    (0x41, "Fired, Red-eye reduction"),
    (0x45, "Fired, Red-eye reduction, Return not detected"),
    (0x47, "Fired, Red-eye reduction, Return detected"),
    (0x49, "On, Red-eye reduction"),
    (0x4d, "On, Red-eye reduction, Return not detected"),
    (0x4f, "On, Red-eye reduction, Return detected"),
    (0x50, "Off, Red-eye reduction"),
    (0x58, "Auto, Did not fire, Red-eye reduction"),
    (0x59, "Auto, Fired, Red-eye reduction"),
    (0x5d, "Auto, Fired, Red-eye reduction, Return not detected"),
    (0x5f, "Auto, Fired, Red-eye reduction, Return detected"),
];

// --------------------------------------------------------------- the tables

/// Standard EXIF, sorted by tag id.
pub static EXIF: &[TagDef] = &[
    TagDef {
        id: 0x00fe,
        name: "SubfileType",
        conv: Conv::Map(SUBFILE_TYPE),
    },
    TagDef {
        id: 0x0100,
        name: "ImageWidth",
        conv: Conv::None,
    },
    TagDef {
        id: 0x0101,
        name: "ImageHeight",
        conv: Conv::None,
    },
    TagDef {
        id: 0x010f,
        name: "Make",
        conv: Conv::None,
    },
    TagDef {
        id: 0x0110,
        name: "Model",
        conv: Conv::None,
    },
    TagDef {
        id: 0x0112,
        name: "Orientation",
        conv: Conv::Map(ORIENTATION),
    },
    TagDef {
        id: 0x011a,
        name: "XResolution",
        conv: Conv::None,
    },
    TagDef {
        id: 0x011b,
        name: "YResolution",
        conv: Conv::None,
    },
    TagDef {
        id: 0x0128,
        name: "ResolutionUnit",
        conv: Conv::Map(RESOLUTION_UNIT),
    },
    TagDef {
        id: 0x0131,
        name: "Software",
        conv: Conv::None,
    },
    TagDef {
        id: 0x0132,
        name: "ModifyDate",
        conv: Conv::None,
    },
    TagDef {
        id: 0x013b,
        name: "Artist",
        conv: Conv::None,
    },
    TagDef {
        id: 0x8298,
        name: "Copyright",
        conv: Conv::None,
    },
    TagDef {
        id: 0x829a,
        name: "ExposureTime",
        conv: Conv::Fn(exposure_time),
    },
    TagDef {
        id: 0x829d,
        name: "FNumber",
        conv: Conv::Fn(fnumber),
    },
    TagDef {
        id: 0x8822,
        name: "ExposureProgram",
        conv: Conv::Map(EXPOSURE_PROGRAM),
    },
    TagDef {
        id: 0x8827,
        name: "ISO",
        conv: Conv::None,
    },
    TagDef {
        id: 0x9003,
        name: "DateTimeOriginal",
        conv: Conv::None,
    },
    TagDef {
        id: 0x9004,
        name: "CreateDate",
        conv: Conv::None,
    },
    TagDef {
        id: 0x9204,
        name: "ExposureCompensation",
        conv: Conv::Fn(ev),
    },
    TagDef {
        id: 0x9207,
        name: "MeteringMode",
        conv: Conv::Map(METERING),
    },
    TagDef {
        id: 0x9209,
        name: "Flash",
        conv: Conv::Map(FLASH),
    },
    TagDef {
        id: 0x920a,
        name: "FocalLength",
        conv: Conv::Fn(focal_mm),
    },
    TagDef {
        id: 0x9291,
        name: "SubSecTimeOriginal",
        conv: Conv::None,
    },
    TagDef {
        id: 0xa001,
        name: "ColorSpace",
        conv: Conv::Map(COLOR_SPACE),
    },
    TagDef {
        id: 0xa002,
        name: "ExifImageWidth",
        conv: Conv::None,
    },
    TagDef {
        id: 0xa003,
        name: "ExifImageHeight",
        conv: Conv::None,
    },
    TagDef {
        id: 0xa402,
        name: "ExposureMode",
        conv: Conv::Map(EXPOSURE_MODE),
    },
    TagDef {
        id: 0xa403,
        name: "WhiteBalance",
        conv: Conv::Map(WHITE_BALANCE),
    },
    TagDef {
        id: 0xa405,
        name: "FocalLengthIn35mmFormat",
        conv: Conv::Fn(focal_mm_int),
    },
    TagDef {
        id: 0xa406,
        name: "SceneCaptureType",
        conv: Conv::Map(SCENE_CAPTURE),
    },
    TagDef {
        id: 0xa433,
        name: "LensMake",
        conv: Conv::None,
    },
    TagDef {
        id: 0xa434,
        name: "LensModel",
        conv: Conv::None,
    },
];

/// Tags whose value is a pointer to another directory.
pub const EXIF_IFD: u16 = 0x8769;
/// A DNG keeps its real image in a SubIFD and leaves IFD0 holding a thumbnail,
/// so a raw file that is not walked into reports an 8x8 frame.
pub const SUB_IFD: u16 = 0x014a;
pub const SUBFILE_TYPE_TAG: u16 = 0x00fe;
pub const MAKER_NOTE: u16 = 0x927c;

/// Fujifilm tags needing a formatter rather than a lookup. Applied on top of
/// the generated table, which carries the lookups.
pub fn fuji_override(id: u16) -> Option<fn(&Value) -> Option<String>> {
    match id {
        0x100a => Some(wb_fine_tune),
        _ => None,
    }
}
