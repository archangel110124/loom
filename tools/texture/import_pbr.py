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

**Default 1024x1024, and `--size` when a material earns more.** The deciding
quantity is not resolution, it is **texels per metre = resolution x uv_scale**,
against roughly 400/m for a sharp near field at 1280 px.

Measured on a 40 m floor, high-frequency energy in the near field:

    uv_scale 1.0 (one tile per metre)   1K  9.32   4K  9.14   8K  9.12
    uv_scale 0.1 (one tile per 10 m)    1K  1.14   4K 14.28   8K 18.23

At one tile per metre a 1K texture already supplies 1024 texels/m against a
screen wanting ~320, so all three are **identical** — the near field is screen
limited and the extra texels cannot be shown. Stretched ten times, 1K collapses
to mush and the ranking inverts completely.

**So the mip chain is what makes excess resolution free and useless at the same
time**: the sampler picks the level matching screen density and discards the
rest, which is why 8K beats 4K only where 4K is itself below the screen's
appetite.

Cost is not free, though — for one material, both maps:

    1K   4.3 MiB   0.9 s to load and render
    4K  79.9 MiB   2.0 s
    8K 285.4 MiB   5.9 s

Every `uv_scale` in this project is between 0.35 and 1.1, where 4K supplies
1400-4500 texels/m and 8K's advantage never appears. 8K is therefore not worth
285 MB in git per material; if a scene ever authors `uv_scale <= 0.1`, import
that one material at `--size 8192` and say why in the scene.

The maps that are thrown away are thrown away because there is nowhere to put
them: `Material` has `metallic` and `roughness` as scalars and no map slots at
all, so Roughness, Metallic, AO and Displacement have no home. That is a real
gap and the reason this script prints what it discarded rather than silently
dropping it.

Usage:
    import_pbr.py [--size N] <zip> <name> [<zip> <name> ...]
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


def parse_size(argv):
    """`--size N` anywhere in the arguments, removed from the list."""
    global SIZE
    if "--size" in argv:
        i = argv.index("--size")
        SIZE = int(argv[i + 1])
        del argv[i:i + 2]
    return argv

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
    argv = parse_size(list(argv))
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
