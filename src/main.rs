//! exif-sooc: read camera metadata, and Fujifilm recipes, out of SOOC files.

use exif_sooc::{json, read, Photo};
use std::path::{Path, PathBuf};

const USAGE: &str = "\
exif-sooc — camera metadata and Fujifilm film recipes

USAGE:
    exif-sooc [OPTIONS] <PATH>...

    A PATH may be a file or a directory. Directories are scanned one level
    deep for .jpg, .jpeg, .heif, .heic, .hif and .raf.

OPTIONS:
    -j, --json       one JSON object per file, group-qualified keys (default)
    -r, --recipe     the Fujifilm recipe, as a card
    -t, --tsv        one tab-separated row per file
    -h, --help       this
    -V, --version    version
";

fn main() {
    let mut args = std::env::args().skip(1);
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut mode = Mode::Json;

    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return;
            }
            "-V" | "--version" => {
                println!("exif-sooc {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-j" | "--json" => mode = Mode::Json,
            "-r" | "--recipe" => mode = Mode::Recipe,
            "-t" | "--tsv" => mode = Mode::Tsv,
            s if s.starts_with('-') => {
                eprintln!("exif-sooc: unknown option {s}");
                std::process::exit(2);
            }
            s => paths.push(PathBuf::from(s)),
        }
    }

    if paths.is_empty() {
        print!("{USAGE}");
        std::process::exit(2);
    }

    let files = expand(&paths);
    if files.is_empty() {
        eprintln!("exif-sooc: no readable image files");
        std::process::exit(1);
    }

    // One thread per core, each taking a contiguous slice. Reading metadata is
    // I/O plus a few thousand instructions, so the split does not need to be
    // clever, only present.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(files.len());
    let chunk = files.len().div_ceil(threads);
    let results: Vec<Vec<(PathBuf, Result<Photo, String>)>> = std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|p| {
                            let r = read(p).map_err(|e| e.to_string());
                            (p.clone(), r)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut out = String::new();
    let mut failed = 0;
    let flat: Vec<_> = results.into_iter().flatten().collect();

    match mode {
        Mode::Json => {
            out.push_str("[\n");
            let mut first = true;
            for (path, r) in &flat {
                match r {
                    Ok(p) => {
                        if !first {
                            out.push_str(",\n");
                        }
                        first = false;
                        emit_json(p, &mut out);
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("{}: {e}", path.display());
                    }
                }
            }
            out.push_str("\n]\n");
        }
        Mode::Recipe => {
            for (path, r) in &flat {
                match r {
                    Ok(p) => emit_recipe(p, &mut out),
                    Err(e) => {
                        failed += 1;
                        eprintln!("{}: {e}", path.display());
                    }
                }
            }
        }
        Mode::Tsv => {
            out.push_str("file\tcamera\tlens\tshutter\taperture\tiso\tfilm\n");
            for (path, r) in &flat {
                match r {
                    Ok(p) => {
                        let g = |n: &str| p.get(n).map(|t| t.print.as_str()).unwrap_or("");
                        out.push_str(&format!(
                            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                            p.path.file_name().unwrap_or_default().to_string_lossy(),
                            p.camera().unwrap_or_default(),
                            p.lens().unwrap_or_default(),
                            g("ExposureTime"),
                            g("FNumber"),
                            g("ISO"),
                            p.recipe().and_then(|r| r.film_simulation).unwrap_or_default(),
                        ));
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("{}: {e}", path.display());
                    }
                }
            }
        }
    }

    print!("{out}");
    if failed > 0 && failed == flat.len() {
        std::process::exit(1);
    }
}

enum Mode {
    Json,
    Recipe,
    Tsv,
}

fn emit_json(p: &Photo, out: &mut String) {
    out.push_str("  {\n    \"SourceFile\": ");
    json::escape(&p.path.display().to_string(), out);
    out.push_str(",\n    \"File:FileType\": ");
    json::escape(p.format.as_str(), out);
    for t in &p.tags {
        out.push_str(",\n    ");
        json::escape(&format!("{}:{}", t.group.as_str(), t.name), out);
        out.push_str(": ");
        json::scalar(&t.print, out);
    }
    if let Some((w, h)) = p.dimensions() {
        out.push_str(&format!(
            ",\n    \"Computed:DisplayWidth\": {w},\n    \"Computed:DisplayHeight\": {h}"
        ));
    }
    out.push_str("\n  }");
}

fn emit_recipe(p: &Photo, out: &mut String) {
    out.push_str(&format!(
        "{}\n",
        p.path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let line = |out: &mut String, k: &str, v: Option<String>| {
        if let Some(v) = v.filter(|s| !s.is_empty()) {
            out.push_str(&format!("  {k:<22} {v}\n"));
        }
    };
    line(out, "Camera", p.camera());
    line(out, "Lens", p.lens());
    let g = |n: &str| p.get(n).map(|t| t.print.clone());
    line(out, "Exposure", g("ExposureTime"));
    line(out, "Aperture", g("FNumber"));
    line(out, "ISO", g("ISO"));
    match p.recipe() {
        Some(r) => {
            line(out, "Film Simulation", r.film_simulation);
            line(out, "Dynamic Range", r.dynamic_range);
            line(out, "Grain", match (r.grain, r.grain_size) {
                (Some(a), Some(b)) => Some(format!("{a}, {b}")),
                (a, _) => a,
            });
            line(out, "Color Chrome", r.color_chrome);
            line(out, "Color Chrome FX Blue", r.color_chrome_fx_blue);
            line(out, "White Balance", match (r.white_balance, r.color_temperature) {
                (Some(w), Some(k)) => Some(format!("{w} ({k}K)")),
                (w, _) => w,
            });
            line(out, "WB Shift", r.white_balance_shift);
            line(out, "Highlight", r.highlight);
            line(out, "Shadow", r.shadow);
            line(out, "Color", r.color);
            line(out, "Sharpness", r.sharpness);
            line(out, "Noise Reduction", r.noise_reduction);
            line(out, "Clarity", r.clarity);
        }
        None => out.push_str("  (no Fujifilm recipe)\n"),
    }
    out.push('\n');
}

const EXTS: [&str; 6] = ["jpg", "jpeg", "heif", "heic", "hif", "raf"];

fn expand(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let Ok(rd) = std::fs::read_dir(p) else { continue };
            let mut found: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| is_image(p))
                .collect();
            found.sort();
            out.extend(found);
        } else {
            out.push(p.clone());
        }
    }
    out
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}
