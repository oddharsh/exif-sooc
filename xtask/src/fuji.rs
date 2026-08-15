//! Generate src/fuji_tags.rs from ExifTool's FujiFilm.pm.
//!
//! Transcribing lookup tables by hand is how a film simulation ends up reading
//! "Velvia" when the camera said "Provia", so the tables are extracted rather
//! than typed. Only the tags in WANT are emitted: the goal is the film recipe
//! and the fields a photographer reads, not all 200 in the table.
//!
//! Two are deliberately absent. InternalSerialNumber needs a ValueConv that
//! decodes a date out of embedded hex, and it publishes a camera's serial
//! number, which is not a thing this should print by default. FlickerReduction
//! prints in hex through an expression. Absent beats wrong.

use regex::Regex;
use std::collections::BTreeMap;
use std::process::Command;

/// 0x1103 (DriveSettings) is deliberately absent: it is a ProcessBinaryData
/// subdirectory that fuji.rs decodes itself, masked fields and all, so a table
/// entry for it would never be read.
const WANT: &[u16] = &[
    0x1000, 0x1001, 0x1002, 0x1003, 0x1004, 0x1005, 0x1006, 0x100a, 0x100b, 0x100e, 0x100f, 0x1010,
    0x1011, 0x1020, 0x1021, 0x1023, 0x1030, 0x1031, 0x1040, 0x1041, 0x1044, 0x1045, 0x1047, 0x1048,
    0x104c, 0x104d, 0x104e, 0x1050, 0x1100, 0x1101, 0x1153, 0x1400, 0x1401, 0x1402, 0x1403, 0x1404,
    0x1405, 0x1406, 0x1407, 0x140b, 0x1443, 0x1444, 0x1445,
];

/// Walk forward from an opening brace to its match.
fn brace_match(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0usize;
    for (i, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn run(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: cargo xtask fuji-tags <path/to/FujiFilm.pm>");
        return 2;
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("xtask: {path}: {e}");
            return 1;
        }
    };

    // The Main table runs from its declaration to the next top-level one.
    let Some(start) = src.find("%Image::ExifTool::FujiFilm::Main = (") else {
        eprintln!("xtask: no FujiFilm::Main table in {path}");
        return 1;
    };
    let end = src[start + 10..]
        .find("%Image::ExifTool::FujiFilm::")
        .map(|i| start + 10 + i)
        .unwrap_or(src.len());
    let body = &src[start..end];

    let entry = Regex::new(r"\n    (0x[0-9a-fA-F]+) => \{").unwrap();
    // ExifTool quotes with either mark, and 0x104c uses the double one. The
    // Python this replaced matched single quotes only and fell back to a
    // hardcoded name, so GrainEffectSize looked correct while being a guess.
    let name_re = Regex::new(r#"Name\s*=>\s*['"]([^'"]+)['"]"#).unwrap();
    let pc_re = Regex::new(r"PrintConv\s*=>\s*\{").unwrap();
    let kv_re = Regex::new(r#"(0x[0-9a-fA-F]+|-?\d+)\s*=>\s*['"]((?:[^'"\\]|\\.)*)['"]"#).unwrap();

    let mut out: BTreeMap<u16, (String, Vec<(i64, String)>)> = BTreeMap::new();
    let mut unnamed = 0u32;
    for m in entry.captures_iter(body) {
        let id = i64::from_str_radix(m[1].trim_start_matches("0x"), 16).unwrap_or(-1);
        if id < 0 || !WANT.contains(&(id as u16)) {
            continue;
        }
        let open = m.get(0).unwrap().end() - 1;
        let Some(close) = brace_match(body, open) else {
            continue;
        };
        let blob = &body[open..=close];

        // A wanted tag whose name cannot be read is reported rather than
        // quietly emitted as Tag_XXXX, which is how the previous generator
        // shipped a hardcoded guess for a year.
        let name = match name_re.captures(blob) {
            Some(c) => c[1].to_string(),
            None => {
                eprintln!("xtask: no Name for {:#06x}, emitting Tag_{id:04X}", id);
                unnamed += 1;
                format!("Tag_{id:04X}")
            }
        };

        let mut conv: Vec<(i64, String)> = Vec::new();
        if let Some(pm) = pc_re.find(blob) {
            let o = pm.end() - 1;
            if let Some(c) = brace_match(blob, o) {
                for kv in kv_re.captures_iter(&blob[o..=c]) {
                    let k = &kv[1];
                    let key = if let Some(hex) = k.strip_prefix("0x") {
                        i64::from_str_radix(hex, 16).ok()
                    } else {
                        k.parse::<i64>().ok()
                    };
                    if let Some(key) = key {
                        conv.push((key, kv[2].replace("\\'", "'").replace("\\\\", "\\")));
                    }
                }
                conv.sort_by_key(|(k, _)| *k);
            }
        }
        out.insert(id as u16, (name, conv));
    }

    // A generator that silently produces nothing is worse than one that fails:
    // the table would go empty and every Fujifilm tag would print as a number.
    if out.len() < WANT.len() / 2 {
        eprintln!(
            "xtask: only {} of {} tags matched, refusing to write a gutted table",
            out.len(),
            WANT.len()
        );
        return 1;
    }

    let mut rs = String::from(
        "//! Fujifilm MakerNotes tags.\n\
         //!\n\
         //! GENERATED by `cargo xtask fuji-tags` from ExifTool's FujiFilm.pm. Do not\n\
         //! edit by hand: the point of generating it is that a mistyped film\n\
         //! simulation is invisible until somebody reads the wrong recipe off a\n\
         //! photo. Re-run it to pick up a new camera's values.\n\
         //!\n\
         //! Tag meanings are ExifTool's, 25 years of them. See NOTICE.\n\
         \n\
         use crate::tags::{Conv, TagDef};\n\
         \n\
         pub static FUJI: &[TagDef] = &[\n",
    );
    for (id, (name, conv)) in &out {
        if conv.is_empty() {
            rs.push_str(&format!(
                "    TagDef {{ id: {id:#06x}, name: \"{name}\", conv: Conv::None }},\n"
            ));
        } else {
            let pairs: Vec<String> = conv
                .iter()
                .map(|(k, v)| format!("({k}, \"{}\")", v.replace('"', "\\\"")))
                .collect();
            rs.push_str(&format!(
                "    TagDef {{ id: {id:#06x}, name: \"{name}\", conv: Conv::Map(&[{}]) }},\n",
                pairs.join(", ")
            ));
        }
    }
    rs.push_str("];\n");

    if unnamed > 0 {
        eprintln!("xtask: {unnamed} wanted tag(s) had no readable Name");
    }
    let dest = "src/fuji_tags.rs";
    if let Err(e) = std::fs::write(dest, rs) {
        eprintln!("xtask: {dest}: {e}");
        return 1;
    }
    // Format it here rather than leaving a file that fails `cargo fmt --check`,
    // which would make every regeneration look like a change to the whole table.
    let _ = Command::new("rustfmt")
        .args(["--edition", "2024", dest])
        .status();
    println!("wrote {dest} with {} tags", out.len());
    0
}
