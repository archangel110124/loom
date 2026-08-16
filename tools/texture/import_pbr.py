#!/usr/bin/env python3
"""Import a downloaded PBR texture set into `assets/textures/`.

The engine takes exactly two maps per material — `albedo_map` and `normal_map`,
both PNG (`loom_asset` depends on the `png` crate and no JPEG decoder). Every
vendor ships six to ten maps under a different naming scheme, most of them JPEG.
This is the one place that mismatch is resolved, so a new material is one
command rather than a fresh round of guessing which file is the normal.

Three schemes are recognised, which is every one in the current download set:

    Poliigon      2K/Poliigon_<Name>_<id>_BaseColor.jpg   _Normal.png
    COL/NRM       <Name>_COL_2K.jpg                       _NRM_2K.png
    ambientCG     <Name>_2K-PNG_Color.png                 _NormalGL.png

**`NormalGL`, never `NormalDX`.** `scene.slang` builds a `cotangentFrame` and
applies the sampled normal in the usual tangent space, which is the OpenGL
convention with +Y up. A DirectX normal map has its green channel inverted, and
the failure is subtle in the worst way: lighting looks plausible but every
bump reads as a dent.

**1024x1024, matching every texture already in `assets/textures/`.** These sets
are 2K and 4K; a 4K albedo is 17 MB of JPEG that becomes far more as PNG, and
this engine's textures are tiled across terrain rather than inspected close up.

The maps that are thrown away are thrown away because there is nowhere to put
them: `Material` has `metallic` and `roughness` as scalars and no map slots at
all, so Roughness, Metallic, AO and Displacement have no home. That is a real
gap and the reason this script prints what it discarded rather than silently
dropping it.

Usage:
    import_pbr.py <zip> <name> [<zip> <name> ...]
    import_pbr.py --list <zip>          # show what a set contains

Writes `assets/textures/<name>_albedo.png` and `<name>_normal.png`.
"""
import io
import sys
import zipfile
from pathlib import Path

from PIL import Image

SIZE = 1024
OUT = Path("assets/textures")

# Ordered: the first pattern that matches wins, so `NormalGL` is picked over
# `NormalDX` and a PNG normal over the JPEG beside it.
ALBEDO = ("_basecolor", "_col_", "_color", "_diff")
NORMAL_PREFERRED = ("_normalgl", "_nrm_", "_normal", "_nor_gl")
NORMAL_REJECT = ("_normaldx",)
DISCARD = ("rough", "metal", "ambientocclusion", "_ao_", "disp", "bump",
           "gloss", "refl", "preview", "emission", "transmission", "alphamask")


def pick(names, wanted, reject=()):
    """Best file for a map: prefer PNG, then the earliest matching pattern."""
    best = None
    for pattern_rank, pattern in enumerate(wanted):
        for n in names:
            low = n.lower()
            if pattern not in low or any(r in low for r in reject):
                continue
            if not low.endswith((".png", ".jpg", ".jpeg")):
                continue
            # PNG first: the vendors' normals are often PNG next to a JPEG
            # copy, and a normal map through JPEG is blocky where it matters.
            rank = (pattern_rank, 0 if low.endswith(".png") else 1, len(n))
            if best is None or rank < best[0]:
                best = (rank, n)
    return best[1] if best else None


def convert(zf, member, out_path, grey_ok=False):
    with zf.open(member) as fh:
        img = Image.open(io.BytesIO(fh.read()))
        img = img.convert("L" if grey_ok else "RGB")
        if img.size != (SIZE, SIZE):
            img = img.resize((SIZE, SIZE), Image.LANCZOS)
        img.save(out_path, "PNG", optimize=True)
    return out_path.stat().st_size


def main(argv):
    if not argv:
        print(__doc__)
        return 1
    if argv[0] == "--list":
        with zipfile.ZipFile(argv[1]) as zf:
            for n in sorted(zf.namelist()):
                print(" ", n)
        return 0

    if len(argv) % 2:
        print("need <zip> <name> pairs")
        return 1

    OUT.mkdir(parents=True, exist_ok=True)
    for zip_path, name in zip(argv[0::2], argv[1::2]):
        with zipfile.ZipFile(zip_path) as zf:
            names = [n for n in zf.namelist() if not n.endswith("/")]
            albedo = pick(names, ALBEDO)
            normal = pick(names, NORMAL_PREFERRED, NORMAL_REJECT)
            if not albedo or not normal:
                print(f"{name}: SKIPPED — albedo={albedo} normal={normal}")
                continue
            a = convert(zf, albedo, OUT / f"{name}_albedo.png")
            n = convert(zf, normal, OUT / f"{name}_normal.png")
            used = {albedo, normal}
            dropped = sorted({
                kind for x in names if x not in used
                for kind in DISCARD if kind.strip("_") in x.lower()
            })
            print(f"{name}:")
            print(f"    albedo  <- {Path(albedo).name}  ({a/1024:.0f} KiB)")
            print(f"    normal  <- {Path(normal).name}  ({n/1024:.0f} KiB)")
            # Named, not counted: which PBR channels this engine has nowhere to
            # put is the useful half of the message.
            print(f"    dropped   no slot in `Material`: {', '.join(dropped) or 'none'}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
