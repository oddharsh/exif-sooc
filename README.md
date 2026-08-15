# exif-sooc

Camera metadata, and the whole Fujifilm film recipe, out of a folder of
straight-out-of-camera files. JPEG, HEIF/HEIC, RAF and DNG.

Reads a 16 KB window instead of the file, speaks ExifTool's command line, and
proves itself by agreeing with ExifTool on every tag it prints.

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
| **exif-sooc** | **0.009s** | **0.06 ms** |
| exiftool 13 | 0.99s | 6.21 ms |
| exif-oxide (git) | 1.54s | 9.63 ms |

The gap is I/O and process startup rather than parsing cleverness. ExifTool
pays for Perl once per run; exif-oxide reads whole files.

Worth being precise about where the remaining time goes, because it decides
what is worth optimising next: **process startup is about 4 ms of that 9**, and
the actual work is 25 to 35 microseconds per file, most of it the `open` and
`read` themselves. There is no bulk data here for SIMD to touch, and the files
are already read in parallel across cores. It is syscall-bound, which is a
polite way of saying it is finished.

## Is it right?

That is the part worth being suspicious of, so it is the part with a gate.

Over those same 160 files, **every one of the 11,050 tags exif-sooc emits agrees
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

## Drop-in for ExifTool

The command line is shaped like ExifTool's, because the point is to be
swappable into a pipeline that already calls it. Tag selection, `-Group:Tag`,
the `#` numeric suffix and `-json` mean what they mean there.

```sh
exif-sooc -json -q -Make -Model -FilmMode -FujiFilm:Sharpness -Orientation# ~/Pictures
```

That exact invocation, taken from a production photo pipeline with 36 selected
tags, produces **byte-identical JSON to ExifTool across 160 files**, which
`tools/compare-pipeline.sh` checks.

| | |
|---|---|
| `-json`, `-j` | JSON, one object per file (the default, and the only structured format) |
| `-TagName` | select tags; keys come back unqualified, as ExifTool does |
| `-Group:TagName` | disambiguate when two tags share a name |
| `-TagName#` | the raw value rather than the readable one |
| `-n` | raw values for everything |
| `-G`, `-G0` | qualify keys with the group |
| `-q`, `-r`, `-t`, `-s` | quiet, recurse, tab-separated, short names |

Tags this does not know are absent rather than guessed at, which is the same
result as ExifTool's default of hiding unknown tags.

## Writing

Two operations, both segment surgery rather than tag editing. Neither one
parses or rebuilds a single EXIF tag, so neither can corrupt one, and the
entropy-coded scan is copied across untouched, so the pixels are the same bytes
afterwards.

```sh
exif-sooc -all= -overwrite_original out.jpg              # remove all metadata
exif-sooc -TagsFromFile src.HIF -all:all out.jpg         # copy metadata across
```

`-all=` is **byte-identical to ExifTool's on all 20 files** of a mixed Fujifilm
and Leica test set, 452 MB of JPEG. It drops every APPn segment (JFIF, EXIF,
XMP, ICC, IPTC), COM, and any trailer after EOI, which is what ExifTool does,
measured rather than assumed.

That trailer matters more than it sounds. Some cameras append a preview or a
second image after the image ends, and a Leica M here carried 669 KB of it.
Finding the real end means honouring the entropy stream's rules: a literal 0xFF
is stuffed as `FF 00` and restart markers are part of the data, so a plain
search for `FF D9` lands on an embedded preview's EOI 11 KB in, with 10 MB of
photograph still to come.

Speed, on those 20 files, best of three:

| | per process, per file | one process for all 20 |
|---|--:|--:|
| **exif-sooc `-all=`** | **25.5 ms** | **20.2 ms** |
| exiftool `-all=` | 135.7 ms | 43.2 ms |
| **exif-sooc `-TagsFromFile`** | **25.6 ms** | |
| exiftool `-TagsFromFile` | 282.4 ms | |

The per-process column is the one that matters for a shell pipeline calling it
once per photo: 5.3x on the strip and **11x on the copy**. Copying is where the
gap widens, because ExifTool rebuilds the metadata while this moves the segments
across, and neither one has to touch the scan.

`-TagsFromFile` differs from ExifTool deliberately, and the difference is worth
knowing. ExifTool REBUILDS the metadata; this copies the source's APP1 segments
verbatim. On one Fujifilm frame that means 165 tags where ExifTool's rewrite
keeps 163, since a verbatim copy also carries the tags it has no table for
(`CompressedBitsPerPixel`, `InteropIndex`, `InteropVersion` were the three), and
it costs about 63 KB of the source's original padding. Same embedded thumbnail
either way. More faithful, larger; for an archive copy that trade is the right
way round.

The source does not have to be a JPEG. A HEIF keeps its EXIF as an item rather
than a segment, so there is nothing to copy across and the APP1 envelope is
built around the extracted TIFF block. Copying a `.HIF`'s metadata onto an
encoded JPEG works and keeps the Fujifilm recipe.

Without `-overwrite_original` a `_original` backup is left beside each file, as
ExifTool does. A tool that edits photographs in place by default is one bad flag
away from an unrecoverable afternoon.

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
exif-sooc --keyed   ~/Pictures/sooc  # one object keyed by filename stem, with recipe cards
```

### Keeping an index up to date

`--keyed` emits `{stem: {…}}` rather than ExifTool's array, and `--merge-into`
folds a fresh read into an index you already have:

```sh
exif-sooc --merge-into metadata.json -q new-photos/ > tmp && mv tmp metadata.json
```

The merge exists because the obvious shell version is wrong in a way that does
not announce itself. `jq -s '.[0] * .[1]'` is a RECURSIVE merge; the reflexive
substitute `+` is shallow and silently drops any key the fresh read does not
produce. So the rule here is explicit:

- a stem the read did not see passes through untouched
- a stem it did see keeps every key it already had, with the freshly read
  fields written over the top

That second line is what preserves a field some later step owns, such as a
derived recipe card or a baked histogram. Values this tool does not recognise
are copied as raw text, so they round-trip byte-exact.

It prints to stdout and never writes to the file it read, so a crash cannot
leave a truncated index where a complete one was. A missing file is treated as
a first run rather than an error. Malformed input is refused outright, since
half-reading an index and writing the result is how a good file becomes a bad
one.

### The recipe card in `--keyed`

Each Fujifilm frame carries a `recipe` object in the fujixweekly idiom: what the
photographer SET, rather than what the sensor wrote. "Standard" is not a dynamic
range, "Soft" is not a sharpness, and `Red +40, Blue -100` is not how anyone
writes a WB shift.

```json
"recipe": {
  "Film Simulation": "Nostalgic Neg",
  "Dynamic Range": "DR400",
  "Grain Effect": "Strong, Large",
  "White Balance": "Kelvin (4500K), +2 Red & -5 Blue",
  "Highlight": "-1",
  "Exposure Compensation": "-1/3"
}
```

Every value is a formatted string, including `"0"` and `"-1"`. Unquoting the
ones that happen to parse as numbers would make the type depend on the sign.

A line is omitted when the camera did not record it, and a non-Fuji frame gets
no `recipe` key at all rather than an empty one.

`--keyed` and `--merge-into` are ADDITIVE. `-json` and `--recipe` are byte-identical
before and after they were added, which is what the 160-file ExifTool corpus diff
actually tests.

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
  body gets core EXIF, which is most of what anyone reads and is all a Leica M
  writes anyway. Canon, Nikon and Sony maker notes are not touched.
- **RAF raw dimensions.** A RAF's EXIF describes its embedded preview, and the
  raw frame size lives in the RAF header's CFA block, which is not parsed. Those
  two tags are absent rather than filled with the preview's size.
- **Every tag.** About 70 standard EXIF tags and 42 Fujifilm ones, chosen
  because a photographer reads them.
- **Writing.** This only reads.
- **RAW image data.** RAF is read by way of its embedded JPEG, and a DNG is a
  TIFF, so both give up their metadata without decoding a single pixel.

For any of those, use [ExifTool](https://exiftool.org). It is the real thing
and this is not trying to replace it.

## Credit

ExifTool is 25 years of camera-specific knowledge and this would be guesswork
without it. Every layout here was checked against Phil Harvey's source, the
Fujifilm table is generated from it, and the tests use it as the oracle. See
[NOTICE](NOTICE).

MIT licensed.
