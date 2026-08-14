#!/usr/bin/env bash
# Prove exif-sooc is a drop-in for a real ExifTool command line.
#
# The comparison harness checks tag values. This checks the INTERFACE: the same
# argument list, through both programs, has to produce the same JSON. Tag
# selection, -Group:Tag, the # numeric suffix and -json all have to mean what
# they mean in ExifTool, and a difference in any of them shows up here.
#
#   tools/compare-pipeline.sh /path/to/photos
set -euo pipefail

DIR="${1:?usage: compare-pipeline.sh <dir>}"
BIN="${EXIF_SOOC:-target/release/exif-sooc}"

# The invocation from aadhar.sh's www/scripts/extract-photo-metadata.sh.
ARGS=(-json -q
  -FileName -Make -Model -LensModel -FNumber -ExposureTime -ISO
  -FocalLengthIn35mmFormat -ExposureCompensation -ExposureMode -ExposureProgram
  -MeteringMode -DateTimeOriginal -ImageWidth -ImageHeight -ColorSpace
  -WhiteBalance -ColorTemperature -WhiteBalanceFineTune -FlashMode -Flash
  -FilmMode -DynamicRange -FocusMode -DriveMode -FujiFilm:Sharpness
  -NoiseReduction -Clarity -DevelopmentDynamicRange -ColorChromeEffect
  -ColorChromeFXBlue -GrainEffectRoughness -GrainEffectSize -HighlightTone
  -ShadowTone -Saturation -Orientation#)

exiftool "${ARGS[@]}" "$DIR" > /tmp/pipeline-exiftool.json
"$BIN"   "${ARGS[@]}" "$DIR" > /tmp/pipeline-sooc.json

python3 - <<'PY'
import json, collections, sys
et = {x["FileName"]: x for x in json.load(open("/tmp/pipeline-exiftool.json"))}
so = {x["FileName"]: x for x in json.load(open("/tmp/pipeline-sooc.json"))}
same = 0
diff = collections.Counter()
example = {}
for name, a in et.items():
    b = so.get(name, {})
    for k, v in a.items():
        if k == "SourceFile":
            continue
        if k not in b:
            diff[f"{k} (missing)"] += 1; example.setdefault(k, (name, v, None))
        elif str(b[k]) != str(v):
            diff[f"{k} (differs)"] += 1; example.setdefault(k, (name, v, b[k]))
        else:
            same += 1
print(f"{len(et)} files, {same} field values identical, {sum(diff.values())} differences")
for k, n in diff.most_common(10):
    f, a, b = example[k.split(" ")[0]]
    print(f"  {k:30} x{n:<4} {f:16} exiftool={a!r:24} ours={b!r}")
if not same:
    sys.exit("compared nothing, so the harness is broken rather than the tool")
sys.exit(1 if diff else 0)
PY
