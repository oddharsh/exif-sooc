//! exif-sooc: camera metadata and Fujifilm film recipes.
//!
//! The command line is deliberately shaped like ExifTool's, because the point
//! is to be swappable into a pipeline that already calls ExifTool. Tag
//! selection, `-Group:Tag`, the `#` numeric suffix and `-json` all mean what
//! they mean there.

use exif_sooc::{json, read, site, Group, Photo};
use std::path::{Path, PathBuf};

const USAGE: &str = "\
exif-sooc — camera metadata and Fujifilm film recipes

USAGE:
    exif-sooc [OPTIONS] [-TAG...] <PATH>...

    A PATH may be a file or a directory. Directories are scanned for
    .jpg .jpeg .heif .heic .hif .raf .dng .tif .tiff

TAG SELECTION (as in ExifTool)
    -Make -Model          only these tags, keys unqualified
    -FujiFilm:Sharpness   disambiguate by group when two tags share a name
    -Orientation#         the raw value rather than the readable one

OPTIONS
    -json, -j     JSON, one object per file (the default)
    -G, -G0       qualify keys with the group: \"EXIF:Make\"
    -n            raw values for every tag
    -q            stay quiet about files that could not be read
    -r            recurse into directories
    -t            tab-separated, one row per file
    --recipe      the Fujifilm recipe as a card
    --keyed       one object keyed by filename stem, site record shape
    --merge-into <FILE>
                  merge --keyed output into an existing keyed FILE and print
                  the result. A stem the read did not see is passed through
                  untouched; a key within a touched stem that this tool does
                  not produce (a recipe card, say) is PRESERVED. Implies
                  --keyed. Writes to stdout, never to FILE.
    -h, --help    this
    -V, --version version
";

#[derive(Default)]
struct Sel {
    group: Option<String>,
    name: String,
    numeric: bool,
}

#[derive(PartialEq)]
enum Mode {
    Json,
    Tsv,
    Recipe,
    /// The stem-keyed record shape aadhar.sh stores. Additive: `-json` stays
    /// byte-identical to ExifTool, which is what the corpus diff tests.
    Keyed,
}

fn main() {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut select: Vec<Sel> = Vec::new();
    let mut mode = Mode::Json;
    let (mut quiet, mut groups, mut numeric_all, mut recurse) = (false, false, false, false);
    let mut merge_into: Option<PathBuf> = None;
    let mut want_merge_path = false;

    for a in std::env::args().skip(1) {
        if want_merge_path {
            merge_into = Some(PathBuf::from(&a));
            want_merge_path = false;
            continue;
        }
        match a.as_str() {
            "--merge-into" => {
                want_merge_path = true;
                mode = Mode::Keyed;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return;
            }
            "-V" | "--version" => {
                println!("exif-sooc {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-json" | "-j" | "--json" => mode = Mode::Json,
            "-t" | "--tsv" => mode = Mode::Tsv,
            "--recipe" => mode = Mode::Recipe,
            "--keyed" => mode = Mode::Keyed,
            "-q" | "-q -q" => quiet = true,
            "-G" | "-G0" | "-G1" => groups = true,
            "-n" => numeric_all = true,
            "-r" => recurse = true,
            // Accepted and ignored: ExifTool's short-name switches, which this
            // tool's output already satisfies since it never prints
            // descriptions.
            "-s" | "-s2" | "-s3" | "-S" => {}
            s if s.starts_with("--") => {
                eprintln!("exif-sooc: unknown option {s}");
                std::process::exit(2);
            }
            // A leading dash followed by a letter is a tag, which is how
            // ExifTool spells selection.
            s if s.len() > 1
                && s.starts_with('-')
                && s[1..].starts_with(|c: char| c.is_ascii_alphabetic()) =>
            {
                let body = &s[1..];
                let numeric = body.ends_with('#');
                let body = body.trim_end_matches('#');
                let (group, name) = match body.split_once(':') {
                    Some((g, n)) => (Some(g.to_string()), n.to_string()),
                    None => (None, body.to_string()),
                };
                select.push(Sel {
                    group,
                    name,
                    numeric,
                });
            }
            s if s.starts_with('-') => {
                eprintln!("exif-sooc: unknown option {s}");
                std::process::exit(2);
            }
            s => paths.push(PathBuf::from(s)),
        }
    }

    if want_merge_path {
        eprintln!("exif-sooc: --merge-into needs a file path");
        std::process::exit(2);
    }
    if paths.is_empty() {
        print!("{USAGE}");
        std::process::exit(2);
    }

    let files = expand(&paths, recurse);
    if files.is_empty() {
        if !quiet {
            eprintln!("exif-sooc: no readable image files");
        }
        std::process::exit(1);
    }

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
                        .map(|p| (p.clone(), read(p).map_err(|e| e.to_string())))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let flat: Vec<_> = results.into_iter().flatten().collect();

    let mut out = String::new();
    let mut failed = 0;
    let mut ok = Vec::new();
    for (path, r) in &flat {
        match r {
            Ok(p) => ok.push(p),
            Err(e) => {
                failed += 1;
                if !quiet {
                    eprintln!("{}: {e}", path.display());
                }
            }
        }
    }

    match mode {
        Mode::Keyed => {
            // A merge READS the file and prints the result. It never writes in
            // place: the caller redirects, so a crash mid-write cannot leave a
            // truncated metadata.json where a complete one was.
            match &merge_into {
                None => out.push_str(&site::keyed(&ok)),
                Some(path) => match std::fs::read_to_string(path) {
                    Ok(existing) => match site::merge_into(&existing, &ok) {
                        Ok(merged) => out.push_str(&merged),
                        Err(e) => {
                            eprintln!("exif-sooc: {}: {e}", path.display());
                            std::process::exit(1);
                        }
                    },
                    // A missing file is the FIRST run, which is a normal state
                    // rather than an error: there is nothing to preserve yet.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        out.push_str(&site::keyed(&ok))
                    }
                    Err(e) => {
                        eprintln!("exif-sooc: {}: {e}", path.display());
                        std::process::exit(1);
                    }
                },
            }
        }
        Mode::Json => {
            out.push_str("[\n");
            for (i, p) in ok.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                emit_json(p, &select, groups, numeric_all, &mut out);
            }
            out.push_str("\n]\n");
        }
        Mode::Tsv => {
            out.push_str("file\tcamera\tlens\tshutter\taperture\tiso\tfilm\n");
            for p in &ok {
                let g = |n: &str| p.get(n).map(|t| t.print.as_str()).unwrap_or("");
                out.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    p.path.file_name().unwrap_or_default().to_string_lossy(),
                    p.camera().unwrap_or_default(),
                    p.lens().unwrap_or_default(),
                    g("ExposureTime"),
                    g("FNumber"),
                    g("ISO"),
                    p.recipe()
                        .and_then(|r| r.film_simulation)
                        .unwrap_or_default(),
                ));
            }
        }
        Mode::Recipe => {
            for p in &ok {
                emit_recipe(p, &mut out);
            }
        }
    }

    print!("{out}");
    if failed > 0 && ok.is_empty() {
        std::process::exit(1);
    }
}

/// Does this tag satisfy the selection?
fn matches(t: &exif_sooc::Tag, s: &Sel) -> bool {
    if !t.name.eq_ignore_ascii_case(&s.name) {
        return false;
    }
    match &s.group {
        None => true,
        Some(g) => t.group.as_str().eq_ignore_ascii_case(g),
    }
}

fn emit_json(p: &Photo, select: &[Sel], groups: bool, numeric_all: bool, out: &mut String) {
    out.push_str("  {\n    \"SourceFile\": ");
    json::escape(&p.path.display().to_string(), out);

    let write = |t: &exif_sooc::Tag, numeric: bool, out: &mut String| {
        out.push_str(",\n    ");
        if groups {
            json::escape(&format!("{}:{}", t.group.as_str(), t.name), out);
        } else {
            json::escape(t.name, out);
        }
        out.push_str(": ");
        if numeric {
            json::scalar(&t.value.to_display(), out);
        } else {
            json::scalar(&t.print, out);
        }
    };

    if select.is_empty() {
        for t in &p.tags {
            write(t, numeric_all, out);
        }
        if let Some((w, h)) = p.dimensions() {
            out.push_str(&format!(
                ",\n    \"DisplayWidth\": {w},\n    \"DisplayHeight\": {h}"
            ));
        }
    } else {
        // Selection order is the caller's, and a tag that is not present is
        // simply absent, which is what ExifTool does.
        for s in select {
            if let Some(t) = p.tags.iter().find(|t| matches(t, s)) {
                write(t, s.numeric || numeric_all, out);
            }
        }
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
    let g = |n: &str| p.get_in(Group::Exif, n).map(|t| t.print.clone());
    line(out, "Exposure", g("ExposureTime"));
    line(out, "Aperture", g("FNumber"));
    line(out, "ISO", g("ISO"));
    match p.recipe() {
        Some(r) => {
            line(out, "Film Simulation", r.film_simulation);
            line(out, "Dynamic Range", r.dynamic_range);
            line(
                out,
                "Grain",
                match (r.grain, r.grain_size) {
                    (Some(a), Some(b)) => Some(format!("{a}, {b}")),
                    (a, _) => a,
                },
            );
            line(out, "Color Chrome", r.color_chrome);
            line(out, "Color Chrome FX Blue", r.color_chrome_fx_blue);
            line(
                out,
                "White Balance",
                match (r.white_balance, r.color_temperature) {
                    (Some(w), Some(k)) => Some(format!("{w} ({k}K)")),
                    (w, _) => w,
                },
            );
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

const EXTS: [&str; 9] = [
    "jpg", "jpeg", "heif", "heic", "hif", "raf", "dng", "tif", "tiff",
];

fn expand(paths: &[PathBuf], recurse: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            collect(p, recurse, &mut out);
        } else {
            out.push(p.clone());
        }
    }
    out
}

fn collect(dir: &Path, recurse: bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut here: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            dirs.push(p);
        } else if is_image(&p) {
            here.push(p);
        }
    }
    here.sort();
    out.extend(here);
    if recurse {
        dirs.sort();
        for d in dirs {
            collect(&d, true, out);
        }
    }
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}
