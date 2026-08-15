//! Prove exif-sooc is a drop-in for a real ExifTool command line.
//!
//! `compare` checks tag VALUES, one group-qualified tag at a time. This checks
//! the INTERFACE: the same argument list, through both programs, has to produce
//! the same JSON. Tag selection, `-Group:Tag`, the `#` numeric suffix and
//! `-json` all have to mean what they mean in ExifTool, and a difference in any
//! of them shows up here.
//!
//! The argument list is not a sample. It is the invocation from aadhar.sh's
//! www/scripts/extract-photo-metadata.sh, 36 selected tags, which is the
//! pipeline this tool has to slot into. The directory is handed to both
//! programs whole, so which files each one decides to read is part of what is
//! being compared.
//!
//! This direction is stricter than `compare`, on purpose. There, a tag ExifTool
//! reports and this does not is out of scope. Here, asking for 36 tags and
//! getting 35 back is the failure the check exists to catch, so a key ExifTool
//! emitted and this did not counts as a difference.
//!
//!     cargo xtask compare-pipeline <folder of photographs>

use crate::compare::scalar;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

/// The production invocation, verbatim.
const ARGS: &[&str] = &[
    "-json",
    "-q",
    "-FileName",
    "-Make",
    "-Model",
    "-LensModel",
    "-FNumber",
    "-ExposureTime",
    "-ISO",
    "-FocalLengthIn35mmFormat",
    "-ExposureCompensation",
    "-ExposureMode",
    "-ExposureProgram",
    "-MeteringMode",
    "-DateTimeOriginal",
    "-ImageWidth",
    "-ImageHeight",
    "-ColorSpace",
    "-WhiteBalance",
    "-ColorTemperature",
    "-WhiteBalanceFineTune",
    "-FlashMode",
    "-Flash",
    "-FilmMode",
    "-DynamicRange",
    "-FocusMode",
    "-DriveMode",
    "-FujiFilm:Sharpness",
    "-NoiseReduction",
    "-Clarity",
    "-DevelopmentDynamicRange",
    "-ColorChromeEffect",
    "-ColorChromeFXBlue",
    "-GrainEffectRoughness",
    "-GrainEffectSize",
    "-HighlightTone",
    "-ShadowTone",
    "-Saturation",
    "-Orientation#",
];

/// Run one program over the whole directory and key its objects by filename.
///
/// FileName is the key because the argument list asks for it, and SourceFile is
/// only the fallback: a run where this tool dropped FileName still lines up
/// file for file, and the missing key is then REPORTED rather than turned into
/// a directory that mysteriously compares against nothing.
fn collect(cmd: &str, dir: &str) -> Result<BTreeMap<String, Value>, String> {
    let out = Command::new(cmd)
        .args(ARGS)
        .arg(dir)
        .output()
        .map_err(|e| format!("could not run {cmd}: {e}"))?;

    // An empty stdout is a directory with nothing readable in it, not broken
    // JSON, and saying so is the difference between a five second answer and a
    // hunt through a parser.
    if out.stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(format!("{cmd} read no files under {dir}"));
    }

    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "{cmd} produced unreadable JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(200)
                .collect::<String>()
        )
    })?;

    let items = match v {
        Value::Array(a) => a,
        other => vec![other],
    };
    let mut map = BTreeMap::new();
    for it in items {
        let name = it
            .get("FileName")
            .and_then(|s| s.as_str())
            .map(str::to_string)
            .or_else(|| {
                let src = it.get("SourceFile")?.as_str()?;
                Some(
                    std::path::Path::new(src)
                        .file_name()?
                        .to_string_lossy()
                        .to_string(),
                )
            });
        if let Some(name) = name {
            map.insert(name, it);
        }
    }
    Ok(map)
}

pub fn run(args: &[String]) -> i32 {
    let Some(dir) = args.first() else {
        eprintln!("usage: cargo xtask compare-pipeline <folder of photographs>");
        return 2;
    };
    let ours_bin = std::env::var("EXIF_SOOC").unwrap_or_else(|_| "target/release/exif-sooc".into());

    let et = match collect("exiftool", dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("xtask: {e}");
            return 1;
        }
    };
    let ours = match collect(&ours_bin, dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("xtask: {e}");
            return 1;
        }
    };
    if et.is_empty() {
        eprintln!("xtask: exiftool read no files under {dir}");
        return 1;
    }

    let (mut same, mut absent) = (0u32, 0u32);
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut example: BTreeMap<String, (String, String, Option<String>)> = BTreeMap::new();

    for (name, a) in &et {
        let Some(a) = a.as_object() else { continue };
        let b = ours.get(name).and_then(|b| b.as_object());
        if b.is_none() {
            absent += 1;
        }
        for (k, v) in a {
            // SourceFile is the path each program was handed, so it differs by
            // nothing but how the directory was spelled.
            if k == "SourceFile" {
                continue;
            }
            match b.and_then(|b| b.get(k)) {
                None => {
                    *counts.entry(format!("{k} (missing)")).or_default() += 1;
                    example
                        .entry(k.clone())
                        .or_insert((name.clone(), scalar(v), None));
                }
                Some(mine) if scalar(mine) != scalar(v) => {
                    *counts.entry(format!("{k} (differs)")).or_default() += 1;
                    example.entry(k.clone()).or_insert((
                        name.clone(),
                        scalar(v),
                        Some(scalar(mine)),
                    ));
                }
                Some(_) => same += 1,
            }
        }
    }

    let total: u32 = counts.values().sum();
    println!(
        "{} files, {same} field values identical, {total} differences",
        et.len()
    );
    if absent > 0 {
        println!("{absent} of those files were not reported at all");
    }

    let mut ranked: Vec<_> = counts.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (label, n) in ranked.into_iter().take(10) {
        let tag = label.split(' ').next().unwrap_or(label);
        let (f, theirs, mine) = &example[tag];
        let mine = mine.clone().unwrap_or_else(|| "(absent)".into());
        println!("  {label:30} x{n:<4} {f:16} exiftool={theirs:?} ours={mine:?}");
    }

    if same == 0 {
        eprintln!("\ncompared nothing, so the harness is broken rather than the tool");
        return 1;
    }
    if total > 0 { 1 } else { 0 }
}
