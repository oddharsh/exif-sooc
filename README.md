# exif-sooc

Camera metadata, and the whole Fujifilm film recipe, out of a folder of
straight-out-of-camera files. JPEG, HEIF/HEIC and RAF.

```
$ exif-sooc --recipe DSCF1234.HIF
DSCF1234.HIF
  Camera                 FUJIFILM X-T50
  Lens                   XF35mmF1.4 R
  Exposure               1/500
  Aperture               8.0
  ISO                    640
  Film Simulation        Nostalgic Neg
  Dynamic Range          Standard
  Grain                  Strong, Large
  Color Chrome           Strong
  Color Chrome FX Blue   Strong
  White Balance          Kelvin (4500K)
  WB Shift               Red +40, Blue -100
  Highlight              -1 (medium soft)
  Shadow                 +2 (hard)
  Color                  +3 (very high)
  Sharpness              -1 (medium soft)
  Noise Reduction        -4 (weakest)
  Clarity                0
```

## Why it is fast

It does not read your files.

Every field lives near the front. On a 16 MB Fujifilm HIF the box that points
at the EXIF sits at byte 859, and the EXIF itself is under 8 KB. So one 128 KB
window off the front answers almost everything, and the rare read outside it
seeks. Over a folder of 160 photos that is about 20 MB read instead of 3 GB.

Measured on an M-series Mac over 160 real files, 114 HIF and 46 JPEG, 3.0 GB
on disk, warm page cache, best of three:

| | total | per file |
|---|--:|--:|
| **exif-sooc** | **0.009s** | **0.1 ms** |
| exiftool 13 | 1.04s | 6.5 ms |
| exif-oxide (git) | 1.64s | 10.2 ms |

The gap is I/O and process startup rather than parsing cleverness. ExifTool
pays for Perl once per run; exif-oxide reads whole files.

## Is it right?

That is the part worth being suspicious of, so it is the part with a gate.

Over those same 160 files, **every one of the 9,784 tags exif-sooc emits agrees
with `exiftool -j -G`, byte for byte**. The contract is one-directional on
purpose: anything this tool prints has to match, and tags ExifTool reports that
this does not are out of scope. It was never trying to be ExifTool.

Point the suite at your own photos:

```sh
EXIF_SOOC_CORPUS=~/Pictures/sooc cargo test --release
```

There are synthetic tests for the containers too, each pinning a way a file can
be read wrongly while still parsing, which is the failure mode that actually
happens: a wrong base offset or a wrong field width leaves the numbers looking
right and quietly empties every string.

## Install

```sh
cargo install exif-sooc
```

Zero dependencies, and it stays that way. The whole crate is byte layouts and
lookup tables, so a dependency would buy nothing and cost build time, binary
size and a supply chain. The release binary is about 420 KB.

## Use it

```sh
exif-sooc --recipe  photo.HIF        # the recipe card above
exif-sooc --json    ~/Pictures/sooc  # one object per file, exiftool-shaped keys
exif-sooc --tsv     ~/Pictures/sooc  # a row per file, for a spreadsheet
```

A directory is scanned one level deep for `.jpg`, `.jpeg`, `.heif`, `.heic`,
`.hif` and `.raf`. Files are read in parallel.

As a library:

```rust
let photo = exif_sooc::read("DSCF1234.HIF")?;

photo.camera();        // "FUJIFILM X-T50"
photo.lens();          // "XF35mmF1.4 R"
photo.dimensions();    // (5152, 7728), corrected for Orientation

if let Some(r) = photo.recipe() {
    println!("{:?}", r.film_simulation);   // Some("Nostalgic Neg")
}

for tag in &photo.tags {
    println!("{}:{} = {}", tag.group.as_str(), tag.name, tag.print);
}
```

Every tag keeps both forms, the way ExifTool does: `tag.value` is the number
the file contains and `tag.print` is the string a person reads. A tool that
kept only the printed form would make `ISO 640` impossible to compare against
640.

`dimensions()` is orientation-corrected, which is the one piece of arithmetic
worth doing for you. Cameras store a portrait frame as landscape pixels plus a
rotation tag, so the stored pair puts every vertical shot in a photo grid at
the wrong aspect ratio.

## What it will not do

- **Every camera brand.** Fujifilm MakerNotes are decoded in full. Every other
  body gets core EXIF, which is most of what anyone reads. Leica, Canon, Nikon
  and Sony maker notes are not touched.
- **Every tag.** About 70 standard EXIF tags and 42 Fujifilm ones, chosen
  because a photographer reads them.
- **Writing.** This only reads.
- **RAW image data.** RAF is read for its metadata by way of the embedded JPEG.

For any of those, use [ExifTool](https://exiftool.org). It is the real thing
and this is not trying to replace it.

## Credit

ExifTool is 25 years of camera-specific knowledge and this would be guesswork
without it. Every layout here was checked against Phil Harvey's source, the
Fujifilm table is generated from it, and the tests use it as the oracle. See
[NOTICE](NOTICE).

MIT licensed.
