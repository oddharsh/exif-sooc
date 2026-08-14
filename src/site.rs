//! The stem-keyed record shape, and a merge that preserves what it does not know.
//!
//! WHY THIS EXISTS, since it is the one output here that is not ExifTool's.
//!
//! aadhar.sh's photo pipeline ran `exiftool -json` into a 50-line `jq` filter
//! that reshaped an array of per-file records into `{stem: {…}}`, then merged
//! that into the accumulated `metadata.json` with jq's object `*`. Two costs
//! came with it. The reshape duplicated field knowledge in a shell script, and
//! the merge pinned the pipeline to jq specifically: `*` is a RECURSIVE merge
//! and the obvious substitute `+` is shallow, so every jq-compatible tool that
//! omits `*` (jaq, notably) silently drops keys on a re-run.
//!
//! Emitting the final shape here removes the reshape, and merging here removes
//! the operator. Both are ADDITIVE: `-json` stays byte-identical to ExifTool,
//! which is what the 160-file corpus diff actually tests, and nothing in this
//! module runs unless `--keyed` is asked for.
//!
//! THE MERGE CONTRACT, which is the whole reason this is not a one-liner.
//! A fresh read produces the 32 fields below. The file on disk may carry MORE
//! than that per photo: aadhar.sh adds a `recipe` card in a later step, and
//! histograms land in the per-photo files. So the rule is:
//!
//!   * a stem the fresh read did not see passes through untouched
//!   * a stem it did see keeps every key it already had, with the freshly read
//!     fields written over the top
//!
//! which is jq's `*` at this depth and NOT `+`. Getting this backwards costs
//! the recipe card on every re-merged photo, silently, and the failure looks
//! like a tooltip that has quietly stopped showing lines.
//!
//! Preserved values are copied as RAW TEXT rather than parsed and re-emitted,
//! so a key this tool has never heard of survives byte-exact. That is also why
//! the reader below is a structural scanner rather than a JSON parser: it needs
//! object boundaries and key names, and deliberately does not need to
//! understand values it is only going to hand back.

use crate::{json, Group, Photo};
use std::collections::BTreeMap;

/// The site's field order. This is a CONTRACT, not a preference: the consuming
/// pipeline diffs regenerated artifacts against committed ones and fails on
/// drift, so a reordering here reads downstream as every photo having changed.
const FIELDS: [&str; 32] = [
    "camera",
    "lens",
    "aperture",
    "shutter",
    "iso",
    "focal",
    "ev",
    "date",
    "width",
    "height",
    "color_space",
    "white_balance",
    "color_temp",
    "wb_shift",
    "flash",
    "exposure_mode",
    "meter",
    "focus_mode",
    "drive",
    "sharpness",
    "noise_reduction",
    "clarity",
    "dr_value",
    "film",
    "dr",
    "chrome",
    "chrome_blue",
    "grain",
    "grain_size",
    "highlight_tone",
    "shadow_tone",
    "saturation",
];

/// The filename with its extension removed, which is how every downstream
/// artifact keys a photo.
pub fn stem(p: &Photo) -> String {
    p.path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Pixel dimensions as a viewer sees them, from the tags the site asks for.
///
/// `Photo::dimensions()` reads ExifImageWidth/Height. The pipeline this feeds
/// asks ExifTool for ImageWidth/ImageHeight, and on some bodies they differ, so
/// this prefers the pair the existing records were built from and falls back
/// rather than introducing a silent one-off in stored dimensions.
fn dims(p: &Photo) -> (Option<i64>, Option<i64>) {
    let pick = |a: &str, b: &str| {
        p.get(a)
            .and_then(|t| t.value.as_i64())
            .or_else(|| p.get(b).and_then(|t| t.value.as_i64()))
    };
    let w = pick("ImageWidth", "ExifImageWidth");
    let h = pick("ImageHeight", "ExifImageHeight");
    let rotated = matches!(
        p.get("Orientation").and_then(|t| t.value.as_i64()),
        Some(5..=8)
    );
    if rotated {
        (h, w)
    } else {
        (w, h)
    }
}

/// One photo's 32 fields, in order, as raw JSON fragments ("null" included).
fn record(p: &Photo) -> Vec<(&'static str, String)> {
    let lit = |s: Option<String>| -> String {
        match s.filter(|v| !v.is_empty()) {
            Some(v) => {
                let mut o = String::new();
                json::scalar(&v, &mut o);
                o
            }
            None => "null".to_string(),
        }
    };
    let txt = |n: &str| p.get(n).map(|t| t.print.clone());
    // Fujifilm writes TWO Sharpness tags. The ExifIFD one is a coarse
    // Soft/Normal/Hard; FujiFilm:Sharpness carries the -4..+4 the recipe card
    // needs, so the group-qualified read is deliberate and not a tidy-up.
    let fuji = |n: &str| p.get_in(Group::Fuji, n).map(|t| t.print.clone());

    let (w, h) = dims(p);
    let num = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());

    let out = vec![
        ("camera", lit(p.camera())),
        ("lens", lit(p.lens())),
        ("aperture", lit(txt("FNumber").map(|v| format!("f/{v}")))),
        ("shutter", lit(txt("ExposureTime"))),
        ("iso", lit(txt("ISO"))),
        ("focal", lit(txt("FocalLengthIn35mmFormat"))),
        ("ev", lit(txt("ExposureCompensation"))),
        ("date", lit(txt("DateTimeOriginal"))),
        ("width", num(w)),
        ("height", num(h)),
        ("color_space", lit(txt("ColorSpace"))),
        ("white_balance", lit(txt("WhiteBalance"))),
        ("color_temp", lit(txt("ColorTemperature"))),
        ("wb_shift", lit(txt("WhiteBalanceFineTune"))),
        ("flash", lit(txt("Flash"))),
        (
            "exposure_mode",
            lit(txt("ExposureMode").or_else(|| txt("ExposureProgram"))),
        ),
        ("meter", lit(txt("MeteringMode"))),
        ("focus_mode", lit(txt("FocusMode"))),
        ("drive", lit(txt("DriveMode"))),
        (
            "sharpness",
            lit(fuji("Sharpness").or_else(|| txt("Sharpness"))),
        ),
        ("noise_reduction", lit(txt("NoiseReduction"))),
        ("clarity", lit(txt("Clarity"))),
        ("dr_value", lit(txt("DevelopmentDynamicRange"))),
        ("film", lit(fuji("FilmMode"))),
        ("dr", lit(fuji("DynamicRange"))),
        ("chrome", lit(fuji("ColorChromeEffect"))),
        ("chrome_blue", lit(fuji("ColorChromeFXBlue"))),
        ("grain", lit(fuji("GrainEffectRoughness"))),
        ("grain_size", lit(fuji("GrainEffectSize"))),
        ("highlight_tone", lit(fuji("HighlightTone"))),
        ("shadow_tone", lit(fuji("ShadowTone"))),
        ("saturation", lit(fuji("Saturation"))),
    ];
    // FIELDS is the downstream contract, so it is CHECKED here rather than
    // left beside the list as a comment that can quietly disagree with it.
    // Adding a field below without adding it there (or reordering either) is
    // the edit this catches, and downstream it would read as every photo
    // having changed at once.
    debug_assert!(
        out.iter().map(|(k, _)| *k).eq(FIELDS.iter().copied()),
        "record() no longer emits FIELDS in order"
    );
    out
}

/// `{stem: {…}}` for the photos read, with no merge.
pub fn keyed(photos: &[&Photo]) -> String {
    let mut m: BTreeMap<String, Vec<(&str, String)>> = BTreeMap::new();
    for p in photos {
        m.insert(stem(p), record(p));
    }
    let mut out = String::from("{\n");
    for (i, (k, rec)) in m.iter().enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        write_entry(k, rec.iter().map(|(a, b)| (*a, b.as_str())), &mut out);
    }
    out.push_str("\n}\n");
    out
}

fn write_entry<'a>(key: &str, fields: impl Iterator<Item = (&'a str, &'a str)>, out: &mut String) {
    out.push_str("  ");
    json::escape(key, out);
    out.push_str(": {\n");
    let mut first = true;
    for (k, v) in fields {
        if !first {
            out.push_str(",\n");
        }
        first = false;
        out.push_str("    ");
        json::escape(k, out);
        out.push_str(": ");
        out.push_str(v);
    }
    out.push_str("\n  }");
}

/// Merge freshly read photos into an existing keyed document.
///
/// See the module header for the contract. `existing` is the file's text; a
/// stem it holds that the read did not see is copied through untouched, and a
/// key within a touched stem that the read does not produce is preserved.
pub fn merge_into(existing: &str, photos: &[&Photo]) -> Result<String, String> {
    let fresh: Vec<(String, Vec<(&str, String)>)> =
        photos.iter().map(|p| (stem(p), record(p))).collect();
    merge_records(existing, &fresh)
}

/// The merge, with no Photo in sight.
///
/// PURE on purpose: the contract this has to keep (a preserved `recipe`, an
/// untouched stem, an unknown key surviving byte-exact) is expressible as text
/// in and text out, so the tests below exercise it directly instead of
/// assembling camera files to reach it.
pub fn merge_records(
    existing: &str,
    fresh_all: &[(String, Vec<(&str, String)>)],
) -> Result<String, String> {
    let mut doc = scan_object(existing)?;
    for (s, fresh) in fresh_all {
        let s = s.clone();
        let prior = doc.remove(&s).unwrap_or_default();
        // Prior key ORDER wins for keys that already existed, so a re-merge
        // does not reshuffle a file that is diffed downstream. New fields land
        // in FIELDS order after them.
        let mut merged: Vec<(String, String)> = Vec::new();
        for (k, v) in &prior {
            match fresh.iter().find(|(fk, _)| *fk == k.as_str()) {
                Some((_, nv)) => merged.push((k.clone(), nv.clone())),
                None => merged.push((k.clone(), v.clone())),
            }
        }
        for (k, v) in fresh.iter() {
            if !merged.iter().any(|(mk, _)| mk.as_str() == *k) {
                merged.push((k.to_string(), v.clone()));
            }
        }
        doc.insert(s, merged);
    }
    let mut out = String::from("{\n");
    for (i, (k, rec)) in doc.iter().enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        write_entry(
            k,
            rec.iter().map(|(a, b)| (a.as_str(), b.as_str())),
            &mut out,
        );
    }
    out.push_str("\n}\n");
    Ok(out)
}

/// A STRUCTURAL scan of `{ "key": { "k": <value>, … }, … }`.
///
/// Values come back as raw source text and are never interpreted, so a key this
/// tool has never heard of round-trips byte-exact. That is the point: the merge
/// has to preserve `recipe` (and anything added later) without this file
/// growing an opinion about it. Only enough JSON is understood to find where
/// each value starts and stops.
fn scan_object(src: &str) -> Result<BTreeMap<String, Vec<(String, String)>>, String> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut out = BTreeMap::new();
    skip_ws(b, &mut i);
    if b.get(i) != Some(&b'{') {
        return Err("expected a JSON object at the top level".into());
    }
    i += 1;
    loop {
        skip_ws(b, &mut i);
        match b.get(i) {
            Some(b'}') => return Ok(out),
            Some(b'"') => {}
            Some(c) => return Err(format!("expected a key, found {:?}", *c as char)),
            None => return Err("unterminated object".into()),
        }
        let key = read_string(b, &mut i)?;
        skip_ws(b, &mut i);
        if b.get(i) != Some(&b':') {
            return Err(format!("expected ':' after {key}"));
        }
        i += 1;
        skip_ws(b, &mut i);
        if b.get(i) != Some(&b'{') {
            return Err(format!("{key}: expected an object"));
        }
        i += 1;
        let mut fields = Vec::new();
        loop {
            skip_ws(b, &mut i);
            match b.get(i) {
                Some(b'}') => {
                    i += 1;
                    break;
                }
                Some(b'"') => {}
                Some(c) => return Err(format!("{key}: expected a field, found {:?}", *c as char)),
                None => return Err("unterminated record".into()),
            }
            let fk = read_string(b, &mut i)?;
            skip_ws(b, &mut i);
            if b.get(i) != Some(&b':') {
                return Err(format!("{key}.{fk}: expected ':'"));
            }
            i += 1;
            skip_ws(b, &mut i);
            let start = i;
            skip_value(b, &mut i)?;
            fields.push((fk, src[start..i].to_string()));
            skip_ws(b, &mut i);
            if b.get(i) == Some(&b',') {
                i += 1;
            }
        }
        out.insert(key, fields);
        skip_ws(b, &mut i);
        if b.get(i) == Some(&b',') {
            i += 1;
        }
    }
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while matches!(b.get(*i), Some(b' ' | b'\n' | b'\t' | b'\r')) {
        *i += 1;
    }
}

fn read_string(b: &[u8], i: &mut usize) -> Result<String, String> {
    *i += 1; // opening quote
    let mut s = String::new();
    loop {
        match b.get(*i) {
            None => return Err("unterminated string".into()),
            Some(b'"') => {
                *i += 1;
                return Ok(s);
            }
            Some(b'\\') => {
                // Enough of the escape grammar to find the end of the string;
                // \u is copied through rather than decoded, because keys here
                // are ASCII field names and a decoded key would not match one.
                let e = *b.get(*i + 1).ok_or("unterminated escape")?;
                s.push('\\');
                s.push(e as char);
                *i += 2;
            }
            Some(c) => {
                s.push(*c as char);
                *i += 1;
            }
        }
    }
}

/// Advance past one value of any type, tracking nesting. Strings are skipped
/// whole so a brace inside one cannot unbalance the count.
fn skip_value(b: &[u8], i: &mut usize) -> Result<(), String> {
    let mut depth = 0usize;
    loop {
        match b.get(*i) {
            None => return Err("unterminated value".into()),
            Some(b'"') => {
                let mut j = *i;
                read_string(b, &mut j)?;
                *i = j;
            }
            Some(c @ (b'{' | b'[')) => {
                let _ = c;
                depth += 1;
                *i += 1;
            }
            Some(b'}' | b']') if depth > 0 => {
                depth -= 1;
                *i += 1;
            }
            Some(b'}' | b']') => return Ok(()), // end of the record
            Some(b',') if depth == 0 => return Ok(()),
            Some(_) => *i += 1,
        }
        if depth == 0 && matches!(b.get(*i), Some(b',' | b'}' | b']')) {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fresh record the way `record()` would, without a camera file.
    fn fresh(pairs: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn the_field_order_is_the_declared_contract() {
        // FIELDS is the downstream contract, so it has to be checked against
        // what record() emits rather than sitting beside it as a comment. A
        // reordering reads downstream as every photo having changed.
        // record() carries a debug_assert that its keys ARE this list in this
        // order; this pins the list's own shape so a silent truncation of it
        // cannot make that assertion vacuous.
        assert_eq!(FIELDS.len(), 32);
        assert_eq!(FIELDS[0], "camera");
        assert_eq!(FIELDS[31], "saturation");
        let mut sorted = FIELDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 32, "a duplicated field name would shadow one");
    }

    #[test]
    fn a_merge_preserves_the_recipe_card() {
        // THE contract. jq's `*` keeps this key; the obvious `+` drops it, and
        // the failure is silent because the tooltip simply stops drawing lines.
        let existing = r#"{
  "A": { "camera": "old", "iso": 100, "recipe": { "Film Simulation": "Classic Chrome" } }
}"#;
        let out = merge_records(
            existing,
            &[("A".into(), fresh(&[("camera", "\"new\""), ("iso", "200")]))],
        )
        .unwrap();
        assert!(
            out.contains("\"recipe\""),
            "recipe must survive a merge:\n{out}"
        );
        assert!(
            out.contains("Classic Chrome"),
            "recipe CONTENT must survive:\n{out}"
        );
        assert!(out.contains("\"new\""), "a freshly read field must win");
        assert!(!out.contains("\"old\""), "the stale value must be gone");
        assert!(out.contains("200"), "iso must update");
    }

    #[test]
    fn an_unseen_stem_passes_through_untouched() {
        let existing = r#"{
  "SEEN": { "camera": "a" },
  "UNSEEN": { "camera": "b", "recipe": { "x": 1 } }
}"#;
        let out =
            merge_records(existing, &[("SEEN".into(), fresh(&[("camera", "\"z\"")]))]).unwrap();
        assert!(out.contains("\"UNSEEN\""), "an unseen stem must survive");
        assert!(out.contains("\"b\""), "its values must be untouched");
        assert!(out.contains("\"z\""), "the seen stem must update");
    }

    #[test]
    fn an_unknown_value_survives_byte_exact() {
        // Values are copied as raw text, so a shape this tool has never heard
        // of round-trips. The brace inside a string is the case that breaks a
        // scanner counting braces without tracking strings.
        let existing = r#"{
  "A": { "camera": "x", "future": [1, 2, {"nested": "brace } and quote \" inside"}] }
}"#;
        let out = merge_records(existing, &[("A".into(), fresh(&[("camera", "\"y\"")]))]).unwrap();
        assert!(
            out.contains(r#"[1, 2, {"nested": "brace } and quote \" inside"}]"#),
            "the unknown value must round-trip byte-exact:\n{out}"
        );
    }

    #[test]
    fn a_first_run_has_nothing_to_preserve() {
        let out = merge_records("{}", &[("A".into(), fresh(&[("camera", "\"x\"")]))]).unwrap();
        assert!(out.contains("\"A\"") && out.contains("\"x\""));
    }

    #[test]
    fn prior_key_order_is_kept_so_a_re_merge_does_not_reshuffle() {
        let existing = r#"{ "A": { "zzz": 1, "camera": "old" } }"#;
        let out = merge_records(
            existing,
            &[(
                "A".into(),
                fresh(&[("camera", "\"new\""), ("brand_new", "7")]),
            )],
        )
        .unwrap();
        let z = out.find("zzz").unwrap();
        let c = out.find("camera").unwrap();
        let b = out.find("brand_new").unwrap();
        assert!(z < c, "existing order must hold");
        assert!(c < b, "a genuinely new field lands after the existing ones");
    }

    #[test]
    fn malformed_input_is_refused_rather_than_half_read() {
        // A truncated file must not merge into a plausible-looking result: that
        // would overwrite a good metadata.json with a partial one.
        assert!(merge_records("{ \"A\": { \"camera\": ", &[]).is_err());
        assert!(
            merge_records("[]", &[]).is_err(),
            "the top level must be an object"
        );
        assert!(
            merge_records("{ \"A\": 3 }", &[]).is_err(),
            "a record must be an object"
        );
    }
}
