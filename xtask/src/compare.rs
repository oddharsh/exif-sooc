//! Diff exif-sooc against ExifTool over a folder of real photographs.
//!
//! The contract is one-directional on purpose: every tag exif-sooc emits has to
//! match ExifTool. Tags ExifTool reports that this does not are out of scope,
//! because this was never trying to be ExifTool.
//!
//! Two guards matter as much as the comparison itself. Tags are matched WITHIN
//! a group, since FujiFilm:Sharpness and EXIF:Sharpness are different tags that
//! share a word. And a run that compares nothing FAILS: this reported a pass
//! while checking zero tags once, after the CLI changed to unqualified keys and
//! the group filter stopped matching, which is the one thing a correctness gate
//! must never do.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const EXTS: &[&str] = &[
    "jpg", "jpeg", "heif", "heic", "hif", "raf", "dng", "tif", "tiff",
];

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}

/// Run a tool over the files in chunks and key the objects by SourceFile.
fn collect(cmd: &str, args: &[&str], files: &[PathBuf]) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for chunk in files.chunks(40) {
        let mut c = Command::new(cmd);
        c.args(args);
        for f in chunk {
            c.arg(f);
        }
        let Ok(o) = c.output() else {
            eprintln!("xtask: could not run {cmd}");
            return out;
        };
        let Ok(v) = serde_json::from_slice::<Value>(&o.stdout) else {
            eprintln!(
                "xtask: {cmd} produced unreadable JSON: {}",
                String::from_utf8_lossy(&o.stderr)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
            continue;
        };
        let items = match v {
            Value::Array(a) => a,
            other => vec![other],
        };
        for it in items {
            if let Some(src) = it.get("SourceFile").and_then(|s| s.as_str()) {
                let name = Path::new(src)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| src.to_string());
                out.insert(name, it);
            }
        }
    }
    out
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn run(args: &[String]) -> i32 {
    let Some(dir) = args.first() else {
        eprintln!("usage: cargo xtask compare <folder of photographs>");
        return 2;
    };
    let files = files_in(Path::new(dir));
    if files.is_empty() {
        eprintln!("xtask: no readable images under {dir}");
        return 1;
    }
    let ours_bin = std::env::var("EXIF_SOOC").unwrap_or_else(|_| "target/release/exif-sooc".into());

    let et = collect("exiftool", &["-j", "-G"], &files);
    let ours = collect(&ours_bin, &["-json", "-G"], &files);

    let (mut agree, mut disagree) = (0u32, 0u32);
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut example: BTreeMap<String, (String, String, String)> = BTreeMap::new();

    for f in &files {
        let name = f
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let (Some(a), Some(b)) = (et.get(&name), ours.get(&name)) else {
            continue;
        };
        let Some(b) = b.as_object() else { continue };
        for (k, v) in b {
            if !k.contains(':') || k.starts_with("File:FileType") || k.starts_with("Computed:") {
                continue;
            }
            let (g, tag) = k.split_once(':').unwrap();
            // ExifTool reports maker notes under group0 "MakerNotes".
            let et_group = if g == "FujiFilm" { "MakerNotes" } else { g };
            let Some(theirs) = a.get(format!("{et_group}:{tag}")) else {
                continue; // not reported there; nothing to check against
            };
            if scalar(theirs) == scalar(v) {
                agree += 1;
            } else {
                disagree += 1;
                *counts.entry(tag.to_string()).or_default() += 1;
                example
                    .entry(tag.to_string())
                    .or_insert((name.clone(), scalar(theirs), scalar(v)));
            }
        }
    }

    println!(
        "{} files: {agree} tags agree, {disagree} disagree",
        files.len()
    );
    if disagree > 0 {
        println!("\nmismatches:");
        let mut ranked: Vec<_> = counts.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1));
        for (tag, n) in ranked.into_iter().take(20) {
            let (f, a, b) = &example[tag];
            println!("  {tag:26} x{n:<4} {f:16} exiftool={a:?} ours={b:?}");
        }
    }
    if agree == 0 {
        eprintln!("\ncompared 0 tags, which means the harness is broken rather than the tool");
        return 1;
    }
    if disagree > 0 { 1 } else { 0 }
}
