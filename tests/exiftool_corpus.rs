//! Agreement with ExifTool over a folder of real photographs.
//!
//! Synthetic tests prove the containers parse. This proves the ANSWERS are
//! right, which is the only claim that matters and the reason this tool can be
//! small: it never has to guess whether a value is correct.
//!
//! Two claims, so two tests. `compare` checks every tag this reads, one
//! group-qualified tag at a time. `compare-pipeline` checks a whole ExifTool
//! command line, the 36 selected tags a production pipeline actually asks for,
//! which is the drop-in claim the README makes.
//!
//! Both comparisons live in xtask, so they can be run by hand as easily as by
//! the suite. Opt in by pointing them at your own photos, since none ship with
//! the crate:
//!
//!     cargo build --release
//!     EXIF_SOOC_CORPUS=~/Pictures/sooc cargo test --release -- --nocapture

use std::path::Path;
use std::process::Command;

/// Run one xtask comparison, or explain why it was skipped.
fn compare(subcommand: &str) {
    let Ok(dir) = std::env::var("EXIF_SOOC_CORPUS") else {
        eprintln!("EXIF_SOOC_CORPUS not set, skipping");
        return;
    };
    if Command::new("exiftool").arg("-ver").output().is_err() {
        eprintln!("exiftool not installed, skipping");
        return;
    }
    // The xtask binary rather than `cargo run`, which would want the build lock
    // this test is already holding.
    let xtask = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/xtask");
    if !xtask.exists() {
        eprintln!("target/release/xtask missing; run `cargo build --release` first. Skipping.");
        return;
    }

    let out = Command::new(&xtask)
        .args([subcommand, &dir])
        .env("EXIF_SOOC", env!("CARGO_BIN_EXE_exif-sooc"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run the comparison");
    let report = String::from_utf8_lossy(&out.stdout);
    eprintln!("{report}");
    assert!(
        out.status.success(),
        "exif-sooc disagreed with exiftool ({subcommand}):\n{report}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn agrees_with_exiftool() {
    compare("compare");
}

/// The drop-in claim: one real ExifTool argument list, the same JSON out.
#[test]
fn agrees_through_a_real_command_line() {
    compare("compare-pipeline");
}
