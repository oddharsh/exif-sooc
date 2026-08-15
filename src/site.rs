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
        match s {
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
        // A body with no electronic contacts (an adapted manual lens) reports
        // an EMPTY LensModel, and empty is not a lens name. It is recorded as
        // null so the field means "not recorded" rather than "recorded as
        // nothing", which is the same discipline the rest of this record keeps.
        //
        // It was "" until 2026-08-15, inherited from the jq filter this replaced:
        // jq's `//` substitutes null alone and "" is truthy there, so an empty
        // tag passed straight through. 11 of 158 frames were that case.
        // Deliberately kept during the port so that change stayed a no-op, and
        // changed here on its own.
        ("lens", lit(txt("LensModel").filter(|v| !v.is_empty()))),
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

/// The 32 fields plus the derived `recipe` card, when the frame has one.
///
/// Kept separate from `record()` so FIELDS stays the exact contract of the
/// flat part and its debug_assert keeps meaning something.
fn record_full(p: &Photo) -> Vec<(&'static str, String)> {
    let mut r = record(p);
    if let Some(card) = recipe_card(p) {
        let mut o = String::from("{\n");
        for (i, (k, v)) in card.iter().enumerate() {
            if i > 0 {
                o.push_str(",\n");
            }
            o.push_str("      ");
            json::escape(k, &mut o);
            o.push_str(": ");
            // ALWAYS a string, never scalar(). A card value is a formatted
            // display string: "0", "+2", "-1", "DR400", "-1/3". scalar() would
            // unquote whichever of them happen to parse as numbers, so the type
            // would depend on the SIGN ("-1" a number, "+2" a string) and a
            // consumer would have to handle both for one column.
            json::escape(v, &mut o);
        }
        o.push_str("\n    }");
        r.push(("recipe", o));
    }
    r
}

/// `{stem: {…}}` for the photos read, with no merge.
pub fn keyed(photos: &[&Photo]) -> String {
    let mut m: BTreeMap<String, Vec<(&str, String)>> = BTreeMap::new();
    for p in photos {
        m.insert(stem(p), record_full(p));
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
        photos.iter().map(|p| (stem(p), record_full(p))).collect();
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

// ── the recipe card ──────────────────────────────────────────────────────────
//
// The fujixweekly idiom: what the photographer SET, not what the sensor wrote.
// "Standard" is not a dynamic range, "Soft" is not a sharpness, and
// "Red +40, Blue -100" is not how anyone writes a WB shift.
//
// Ported from aadhar.sh's build-recipes.py, which derived this from a flattened
// 32-field record. This reads the TAGS instead, which removes that hop.
//
// WHERE "FROM RAW" STOPS, because it is not where you would guess. Fuji stores
// the tone knobs as internal codes: Sharpness 130, ShadowTone -32,
// NoiseReduction 736. The code-to-setting mapping already lives in this crate's
// Fuji print logic, so reading `.value` for those and re-deriving the mapping
// would be a SECOND copy of a lookup table that already exists. The setting
// number is the leading token of `.print` ("-1 (medium soft)"), so that is what
// is read. Raw values ARE used where they are genuinely numbers and the print
// form is a lossy rendering of them: the WB fine-tune pair, the development
// dynamic range, and exposure compensation.

/// The leading signed integer of a friendly Fuji string: "-1 (medium soft)" -> -1.
fn leading_num(s: &str) -> Option<i64> {
    let t = s.trim();
    let (sign, rest) = match t.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i64>().ok().map(|n| sign * n)
    }
}

/// Recipe cards always show the sign: 0 -> "0", 2 -> "+2".
fn signed(n: i64) -> String {
    if n > 0 {
        format!("+{n}")
    } else {
        n.to_string()
    }
}

/// EV as the fraction a card prints. -0.33 -> "-1/3", 1.0 -> "+1".
fn thirds(ev: f64) -> String {
    let steps = (ev * 3.0).round() as i64; // EV is set in 1/3-stop clicks
    if steps == 0 {
        return "0".to_string();
    }
    let (whole, rem) = (steps.abs() / 3, steps.abs() % 3);
    let sign = if steps > 0 { "+" } else { "-" };
    match (whole, rem) {
        (w, 0) => format!("{sign}{w}"),
        (0, r) => format!("{sign}{r}/3"),
        (w, r) => format!("{sign}{w} {r}/3"),
    }
}

/// "F0/Standard (Provia)" -> "Provia/Standard"; anything else passes through.
fn film_name(raw: &str) -> String {
    if let (Some(open), true) = (raw.find('('), raw.contains('/')) {
        if let Some(close) = raw[open..].find(')') {
            let inner = &raw[open + 1..open + close];
            let after_slash = raw.split('/').nth(1).unwrap_or("");
            let name = after_slash.split(" (").next().unwrap_or(after_slash);
            return format!("{inner}/{name}");
        }
    }
    raw.to_string()
}

/// Fuji's WB fine-tune is stored in units of 20 per on-camera step, so the raw
/// pair `40 -100` is what the photographer set as "+2 Red & -5 Blue".
const WB_STEP: i64 = 20;

fn is_bw(sat: Option<&str>) -> bool {
    let s = sat.unwrap_or("").to_ascii_lowercase();
    ["acros", "monochrome", "b&w", "b & w", "bw", "sepia"]
        .iter()
        .any(|k| s.contains(k))
}

/// The card, ordered, or None when the frame is not a Fuji one.
pub fn recipe_card(p: &Photo) -> Option<Vec<(String, String)>> {
    let fuji = |n: &str| p.get_in(Group::Fuji, n).map(|t| t.print.clone());
    let any = |n: &str| p.get(n).map(|t| t.print.clone());

    let sat = fuji("Saturation");
    let bw = is_bw(sat.as_deref());
    // A B&W sim is the FILM, not a colour setting: Fuji leaves FilmMode blank
    // and records the choice in Saturation.
    let film = fuji("FilmMode")
        .map(|f| film_name(&f))
        .or_else(|| if bw { sat.clone() } else { None });

    let mut card: Vec<(String, String)> = Vec::new();
    let mut put = |k: &str, v: Option<String>| {
        if let Some(v) = v.filter(|s| !s.is_empty()) {
            card.push((k.to_string(), v));
        }
    };

    put("Film Simulation", film.clone());
    // The RAW value, because the print form of DynamicRange says "Standard"
    // while DevelopmentDynamicRange carries the 100/200/400 that prints as DR.
    put(
        "Dynamic Range",
        p.get_in(Group::Fuji, "DevelopmentDynamicRange")
            .and_then(|t| t.value.as_i64())
            .map(|n| format!("DR{n}")),
    );
    put("Grain Effect", grain_card(p));
    put("Color Chrome Effect", fuji("ColorChromeEffect"));
    put("Color Chrome FX Blue", fuji("ColorChromeFXBlue"));
    put("White Balance", wb_card(p));

    // The tone knobs print as bare signed numbers on a card.
    for (key, tag) in [("Highlight", "HighlightTone"), ("Shadow", "ShadowTone")] {
        put(key, fuji(tag).and_then(|v| leading_num(&v)).map(signed));
    }
    put(
        "Color",
        if bw {
            None
        } else {
            sat.as_deref().and_then(leading_num).map(signed)
        },
    );
    for (key, tag) in [
        ("Sharpness", "Sharpness"),
        ("High ISO NR", "NoiseReduction"),
        ("Clarity", "Clarity"),
    ] {
        put(key, fuji(tag).and_then(|v| leading_num(&v)).map(signed));
    }

    // Exposure rides along: it is what the card's last two lines carry.
    put("ISO", any("ISO"));
    put(
        "Exposure Compensation",
        p.get("ExposureCompensation")
            .and_then(|t| t.value.as_f64())
            .map(thirds),
    );

    // No film sim and no recipe knobs means it is not a Fuji frame.
    let has_knob = card.iter().any(|(k, _)| {
        matches!(
            k.as_str(),
            "Dynamic Range" | "Grain Effect" | "Color Chrome Effect"
        )
    });
    if film.is_none() && !has_knob {
        return None;
    }
    Some(card)
}

/// "Weak, Small" — roughness then size; "Off" collapses to one word.
fn grain_card(p: &Photo) -> Option<String> {
    let rough = p.get_in(Group::Fuji, "GrainEffectRoughness")?.print.clone();
    if rough.is_empty() {
        return None;
    }
    if rough.eq_ignore_ascii_case("off") {
        return Some("Off".into());
    }
    match p
        .get_in(Group::Fuji, "GrainEffectSize")
        .map(|t| t.print.clone())
    {
        Some(size) if !size.is_empty() && !size.eq_ignore_ascii_case("off") => {
            Some(format!("{rough}, {size}"))
        }
        _ => Some(rough),
    }
}

/// "Kelvin (4500K), +2 Red & -5 Blue" — base mode, then the fine-tune.
fn wb_card(p: &Photo) -> Option<String> {
    let mut base = p
        .get_in(Group::Fuji, "WhiteBalance")
        .or_else(|| p.get("WhiteBalance"))?
        .print
        .clone();
    if base.is_empty() {
        return None;
    }
    if base == "Kelvin" {
        if let Some(k) = p.get("ColorTemperature").and_then(|t| t.value.as_i64()) {
            base = format!("Kelvin ({k}K)");
        }
    }
    // The RAW pair ("40 -100") rather than the print form ("Red +40, Blue
    // -100"), so this reads two integers instead of a rendering of them.
    let shift = match p.get_in(Group::Fuji, "WhiteBalanceFineTune") {
        Some(t) => t,
        None => return Some(base),
    };
    let nums: Vec<i64> = match &shift.value {
        crate::Value::I32s(v) => v.iter().map(|n| *n as i64).collect(),
        crate::Value::U32s(v) => v.iter().map(|n| *n as i64).collect(),
        // A single-valued or textual encoding still has to yield the pair, so
        // fall back to the decoded print form rather than silently dropping the
        // shift line off the card.
        _ => shift
            .print
            .split(|c: char| !c.is_ascii_digit() && c != '-')
            .filter(|w| !w.is_empty() && *w != "-")
            .filter_map(|w| w.parse::<i64>().ok())
            .collect(),
    };
    if nums.len() < 2 {
        return Some(base);
    }
    let (r, b) = (nums[0] / WB_STEP, nums[1] / WB_STEP);
    if r == 0 && b == 0 {
        return Some(format!("{base}, 0 shift"));
    }
    Some(format!("{base}, {} Red & {} Blue", signed(r), signed(b)))
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
    fn an_empty_tag_is_null_rather_than_an_empty_string() {
        // `lit` keeps "" for a tag that genuinely holds one, because that is a
        // real recorded value for most fields. `lens` is the exception and
        // filters at the call site: an empty LensModel means no lens was
        // reported, not that the lens is named "".
        let keep = |s: Option<String>| match s {
            Some(v) => format!("{v:?}"),
            None => "null".to_string(),
        };
        assert_eq!(
            keep(Some(String::new())),
            "\"\"",
            "lit itself does not filter"
        );
        assert_eq!(keep(Some("x".into())), "\"x\"");
        // what the lens call site does with it
        assert_eq!(keep(Some(String::new()).filter(|v| !v.is_empty())), "null");
        assert_eq!(
            keep(Some("Summilux".to_string()).filter(|v| !v.is_empty())),
            "\"Summilux\""
        );
    }

    #[test]
    fn a_card_prints_the_setting_not_the_stored_code() {
        // Fuji stores the tone knobs as internal codes (Sharpness 130,
        // ShadowTone -32, NoiseReduction 736). The setting number is the
        // leading token of the decoded string, which is what a card shows.
        assert_eq!(leading_num("-1 (medium soft)"), Some(-1));
        assert_eq!(leading_num("+3 (very high)"), Some(3));
        assert_eq!(leading_num("0"), Some(0));
        assert_eq!(leading_num("Strong"), None);
        assert_eq!(signed(0), "0");
        assert_eq!(signed(2), "+2");
        assert_eq!(signed(-2), "-2");
    }

    #[test]
    fn exposure_compensation_prints_as_thirds() {
        // EV is set in 1/3-stop clicks and a card prints the fraction, so a
        // stored -0.33 has to come back as the -1/3 that was dialled in.
        assert_eq!(thirds(-0.33), "-1/3");
        assert_eq!(thirds(0.0), "0");
        assert_eq!(thirds(1.0), "+1");
        assert_eq!(thirds(-0.67), "-2/3");
        assert_eq!(thirds(1.33), "+1 1/3");
        assert_eq!(thirds(-2.0), "-2");
    }

    #[test]
    fn a_film_name_unwraps_fujis_double_barrelled_form() {
        assert_eq!(film_name("F0/Standard (Provia)"), "Provia/Standard");
        assert_eq!(film_name("Nostalgic Neg"), "Nostalgic Neg");
        // No slash means nothing to unwrap, even with a paren present.
        assert_eq!(film_name("Acros (Red)"), "Acros (Red)");
    }

    #[test]
    fn a_bw_sim_is_the_film_and_drops_the_colour_line() {
        // Fuji leaves FilmMode blank for Acros/Monochrome and records the
        // choice in Saturation, so it routes to Film Simulation instead.
        assert!(is_bw(Some("Acros Green Filter")));
        assert!(is_bw(Some("Monochrome")));
        assert!(is_bw(Some("Sepia")));
        assert!(!is_bw(Some("+3 (very high)")));
        assert!(!is_bw(None));
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
