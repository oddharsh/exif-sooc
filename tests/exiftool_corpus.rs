//! Agreement with ExifTool over a folder of real files.
//!
//! Synthetic tests prove the containers parse. This proves the ANSWERS are
//! right, which is the only claim that matters, and it is the reason this tool
//! can be small: it never has to guess whether a value is correct.
//!
//! The contract is one-directional. Every tag exif-sooc emits must match
//! ExifTool. Tags ExifTool reports and this does not are out of scope, because
//! this was never trying to be ExifTool.
//!
//! Opt in by pointing it at your own photos, since none ship with the crate:
//!
//!     EXIF_SOOC_CORPUS=~/Pictures/sooc cargo test --release -- --nocapture

use std::process::Command;

#[test]
fn agrees_with_exiftool() {
    let Ok(dir) = std::env::var("EXIF_SOOC_CORPUS") else {
        eprintln!("EXIF_SOOC_CORPUS not set, skipping");
        return;
    };
    if Command::new("exiftool").arg("-ver").output().is_err() {
        eprintln!("exiftool not installed, skipping");
        return;
    }

    let out = Command::new("python3")
        .arg("tools/compare-exiftool.py")
        .arg(&dir)
        .env("EXIF_SOOC", env!("CARGO_BIN_EXE_exif-sooc"))
        .output()
        .expect("run the comparison");
    let report = String::from_utf8_lossy(&out.stdout);
    eprintln!("{report}");
    assert!(
        out.status.success(),
        "exif-sooc disagreed with exiftool:\n{report}"
    );
}
