#!/usr/bin/env python3
"""Diff exif-sooc against exiftool over a folder of real files.

The contract is one-directional on purpose: every tag exif-sooc emits must
match exiftool. Tags exiftool has and we do not are out of scope, because this
tool never claimed to be exiftool.

    python3 tools/compare-exiftool.py "/path/to/photos"
"""
import json, subprocess, sys, os, glob, collections

BIN = os.environ.get("EXIF_SOOC", "target/release/exif-sooc")
root = sys.argv[1]
files = sorted(
    f for ext in ("HIF","JPG","jpg","heic","HEIC","raf","RAF")
    for f in glob.glob(os.path.join(root, f"*.{ext}"))
)
if not files:
    sys.exit(f"no files under {root}")

def batched(cmd, chunk=40):
    out = []
    for i in range(0, len(files), chunk):
        r = subprocess.run(cmd + files[i:i+chunk], capture_output=True, text=True)
        try:
            j = json.loads(r.stdout)
        except json.JSONDecodeError:
            print("parse failure:", r.stderr[:300]); continue
        out.extend(j if isinstance(j, list) else [j])
    return {os.path.abspath(d["SourceFile"]): d for d in out}

et = batched(["exiftool", "-j", "-G"])
ours = batched([BIN, "--json"])

# exiftool puts Fuji tags in group1 "FujiFilm" under -G1 but group0 "MakerNotes"
# under -G, so ours are matched by name within either.
ok = bad = 0
mismatch = collections.Counter()
examples = {}
for f in files:
    a, b = et.get(os.path.abspath(f)), ours.get(os.path.abspath(f))
    if not a or not b:
        print(f"MISSING OUTPUT {f}"); bad += 1; continue
    # Match within the same group. FujiFilm:Sharpness and EXIF:Sharpness are
    # different tags with different values, and comparing by bare name reports
    # a difference that is really two tags sharing a word.
    by_group = {}
    for k, v in a.items():
        if ":" in k:
            g, n = k.split(":", 1)
            by_group.setdefault((g, n), v)
    for k, v in b.items():
        if ":" not in k or k.startswith(("SourceFile", "Computed:", "File:FileType")):
            continue
        g, name = k.split(":", 1)
        # exiftool -G reports maker notes under group0 "MakerNotes"
        et_group = "MakerNotes" if g == "FujiFilm" else g
        if (et_group, name) not in by_group:
            continue  # exiftool does not report it here; nothing to check
        by_name = {name: by_group[(et_group, name)]}
        if str(by_name[name]) != str(v):
            bad += 1
            mismatch[name] += 1
            examples.setdefault(name, (os.path.basename(f), by_name[name], v))
        else:
            ok += 1

print(f"{len(files)} files: {ok} tags agree, {bad} disagree")
if mismatch:
    print("\nmismatches:")
    for name, n in mismatch.most_common(20):
        f, e, o = examples[name]
        print(f"  {name:26} x{n:<4} {f:16} exiftool={e!r:38} ours={o!r}")
sys.exit(1 if bad else 0)
