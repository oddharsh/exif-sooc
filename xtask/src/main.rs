//! The project's own tools.
//!
//! These were Python until 2026-08-14. Nothing was wrong with the Python; one
//! language in the tree is worth more than the few lines it cost. They live in
//! a workspace member so the crate a consumer installs stays dependency-free:
//! parsing Perl wants a regex engine and comparing against ExifTool wants a
//! JSON reader, and neither has any business in the library.
//!
//!     cargo xtask fuji-tags <path/to/ExifTool/lib/Image/ExifTool/FujiFilm.pm>
//!     cargo xtask compare <folder of photographs>
//!     cargo xtask compare-pipeline <folder of photographs>

mod compare;
mod fuji;
mod pipeline;

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    let code = match cmd.as_str() {
        "fuji-tags" => fuji::run(&rest),
        "compare" => compare::run(&rest),
        "compare-pipeline" => pipeline::run(&rest),
        "-h" | "--help" | "" => {
            println!(
                "cargo xtask fuji-tags <FujiFilm.pm>   regenerate src/fuji_tags.rs\n\
                 cargo xtask compare <DIR>             diff every tag against exiftool\n\
                 cargo xtask compare-pipeline <DIR>    diff a real ExifTool command line"
            );
            0
        }
        other => {
            eprintln!("xtask: unknown command {other}");
            2
        }
    };
    std::process::exit(code);
}
