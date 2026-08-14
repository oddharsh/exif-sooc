//! A small JSON writer.
//!
//! Hand-rolled rather than pulled in, because the whole output is strings and
//! numbers and a serialiser would be the only dependency in the tree.

pub fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Numbers go out unquoted so a consumer can do arithmetic; everything else is
/// a string. A value that merely looks numeric ("0130", a Fujifilm version) is
/// deliberately left quoted, since dropping its leading zero would change it.
pub fn scalar(s: &str, out: &mut String) {
    // A value that merely looks numeric stays a string when quoting is the only
    // way to keep it intact: "0130" is a Fujifilm version, and unquoting it
    // would drop the leading zero.
    let looks_padded = s.len() > 1 && s.starts_with('0') && !s.starts_with("0.");
    let numeric = s.parse::<f64>().is_ok() && !looks_padded && !s.starts_with('+');
    if numeric {
        out.push_str(s);
    } else {
        escape(s, out);
    }
}
