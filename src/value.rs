//! Tag values, and how they print.
//!
//! Every value is kept twice, the way ExifTool keeps them: the number the file
//! actually contains, and the string a human reads. Callers that want to do
//! arithmetic take the first; callers writing a caption take the second. A tool
//! that only kept the printed form would make `ISO 640` impossible to compare
//! against 640.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    U32(u32),
    I32(i32),
    F64(f64),
    Text(String),
    Bytes(Vec<u8>),
    U32s(Vec<u32>),
    I32s(Vec<i32>),
    /// numerator, denominator, kept unreduced so a 0/0 stays distinguishable
    /// from a 0/1. Fujifilm writes 0/0 for the aperture of an adapted manual
    /// lens, and "undef" is the honest reading of that.
    Ratio(i64, i64),
    Ratios(Vec<(i64, i64)>),
}

impl Value {
    /// The number, when there is a single one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::U32(v) => Some(*v as f64),
            Value::I32(v) => Some(*v as f64),
            Value::F64(v) => Some(*v),
            Value::Ratio(_, 0) => None,
            Value::Ratio(n, d) => Some(*n as f64 / *d as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::U32(v) => Some(*v as i64),
            Value::I32(v) => Some(*v as i64),
            Value::F64(v) => Some(*v as i64),
            Value::Ratio(n, d) if *d != 0 => Some(n / d),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// The default rendering, used when a tag has no conversion of its own.
    ///
    /// Integral rationals print without a decimal point, because a focal length
    /// of 35 mm is written `35/1` and reads wrong as `35.0`.
    pub fn to_display(&self) -> String {
        match self {
            Value::U32(v) => v.to_string(),
            Value::I32(v) => v.to_string(),
            Value::F64(v) => fmt_f64(*v),
            Value::Text(s) => s.clone(),
            Value::Bytes(b) => b.iter().map(|c| format!("{c:02x}")).collect(),
            Value::U32s(v) => join(v.iter().map(|x| x.to_string())),
            Value::I32s(v) => join(v.iter().map(|x| x.to_string())),
            Value::Ratio(_, 0) => "undef".to_string(),
            Value::Ratio(n, d) => fmt_f64(*n as f64 / *d as f64),
            Value::Ratios(v) => join(v.iter().map(|(n, d)| {
                if *d == 0 {
                    "undef".to_string()
                } else {
                    fmt_f64(*n as f64 / *d as f64)
                }
            })),
        }
    }
}

fn join(parts: impl Iterator<Item = String>) -> String {
    let mut out = String::new();
    for (i, p) in parts.enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&p);
    }
    out
}

/// Print a float the way a person writes it: no trailing `.0`, and no long
/// binary tail on a value that came from a small fraction.
pub fn fmt_f64(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let mut s = String::new();
    let _ = write!(s, "{:.4}", v);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}
