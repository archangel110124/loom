#!/usr/bin/env python3
"""What a tone-map change does to the golden gate, computed from the existing
reference PNGs alone — no GPU, no shader edit, no build.

**Why this exists.** A change to the tone map moves every pixel of all 54
golden references, so `cargo xtask image --bless` is a 54-line manifest diff
that nobody can review after the fact. The shipped shoulder is invertible
below its clamp, so a reference PNG can be un-tonemapped back to the linear
HDR value the forward pass produced, a candidate operator applied, and the
result re-encoded. **The whole re-bless is therefore reviewable before the
change is written**, and a candidate that looks wrong costs no build.

`selftest` is the load-bearing part: applying the *shipped* operator to the
recovered HDR must reproduce the reference **bit for bit**. Anything above
zero means the inverse is wrong and every number this prints is void.

Blind spot, stated up front: a channel that reached 255 came from any HDR
value at or above the shoulder's clamp, so the recovered value there is a
floor rather than the value. That is a highlight-only error, and it is why the
acceptance criterion quotes the `worst` column with a tolerance rather than
exactly.

    python3 tools/postpredict/predict.py selftest
    python3 tools/postpredict/predict.py table
    python3 tools/postpredict/predict.py sheets <outdir>
"""

import os
import sys

import numpy as np
from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
REF = os.path.join(ROOT, "tests", "references")

# --- the shipped operator, `assets/shaders/tonemap.slang` ---------------------
KNEE = 0.76
D = 1.0 - KNEE

# --- the proposed operator ---------------------------------------------------
# Kept in step with `tonemap.slang` by hand rather than parsed out of it,
# because this file has to be able to model a candidate that is *not* the one
# currently in the tree — that is the entire point of running it first.
PIVOT = 0.30
CONTRAST = 1.30


def srgb_to_linear(u):
    return np.where(u <= 0.04045, u / 12.92, ((u + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(u):
    u = np.clip(u, 0.0, 1.0)
    return np.where(u <= 0.0031308, u * 12.92, 1.055 * u ** (1 / 2.4) - 0.055)


def shoulder(c):
    """`shoulder()` from the shader, in float64."""
    p = c.max(axis=-1, keepdims=True)
    top = 1.0 - D * D / np.maximum(p + D - KNEE, 1e-9)
    return np.where(p < KNEE, c, c * (top / np.maximum(p, 1e-9)))


def unshoulder(c):
    """Display-referred linear -> the HDR value the forward pass wrote.

    The shoulder scales the whole triple by one factor derived from its own
    peak, so inverting the peak inverts the triple. Identity below the knee.
    """
    top = c.max(axis=-1, keepdims=True)
    peak = (KNEE - D) + D * D / np.maximum(1.0 - top, 1e-9)
    return c * np.where(top < KNEE, 1.0, peak / np.maximum(top, 1e-9))


def operator(c):
    """The proposed curve: a contrast power about a mid-grey pivot, then the
    shipped shoulder untouched."""
    c = PIVOT * np.power(np.maximum(c / PIVOT, 1e-8), CONTRAST)
    return shoulder(c)


def encode(c):
    return np.round(linear_to_srgb(c) * 255.0).astype(np.uint8)


def recover(name):
    """The reference's uint8 pixels, and the HDR the forward pass wrote."""
    ref = np.asarray(Image.open(os.path.join(REF, name + ".png")).convert("RGB"))
    return ref, unshoulder(srgb_to_linear(ref.astype(np.float64) / 255.0))


def gate(a, b):
    """`loom compare`'s rule: a channel differing by more than 2 counts the
    pixel. Returns (fraction, worst channel, mean absolute)."""
    d = np.abs(a.astype(np.int16) - b.astype(np.int16))
    return (d.max(axis=-1) > 2).sum() / (a.shape[0] * a.shape[1]), int(d.max()), float(d.mean())


def names():
    return sorted(f[:-4] for f in os.listdir(REF) if f.endswith(".png"))


def _roundtrip(inverse=unshoulder):
    """Worst channel and mean over every row of forward(inverse(ref)) vs ref."""
    worst, total = 0, 0.0
    rows = names()
    for n in rows:
        ref = np.asarray(Image.open(os.path.join(REF, n + ".png")).convert("RGB"))
        hdr = inverse(srgb_to_linear(ref.astype(np.float64) / 255.0))
        d = np.abs(encode(shoulder(hdr)).astype(np.int16) - ref.astype(np.int16))
        worst, total = max(worst, int(d.max())), total + float(d.mean())
    return len(rows), worst, total / len(rows)


def selftest():
    """The inverse must round-trip bit-exact under the SHIPPED operator.

    **What this does and does not prove.** It proves `unshoulder` is a true
    inverse of `shoulder` through the 8-bit sRGB quantisation, which is the
    property every number below depends on. It does **not** prove `shoulder`
    matches the shader: `unshoulder` inverts whatever `shoulder` is, so a
    wrong pair of constants round-trips just as exactly as a right one —
    checked, by perturbing `KNEE` and watching this stay at zero. The check
    that closes that gap is A1 at commit 1, where the real render is compared
    against this prediction with a tolerance of two codes.

    The fault injection below is the runnable proof that a zero here is a
    measurement and not a tautology. **Its floor is ~1%**: an inverse error
    smaller than that is under one 8-bit code at mid grey and this cannot see
    it, which is well inside what A1 then catches.
    """
    rows, worst, mean = _roundtrip()
    print(f"{rows} rows, max worst channel {worst}, mean {mean:.5f}")
    if worst != 0:
        return 1
    # Fault injection: a 1% error in the inverse alone must be visible.
    _, hurt, _ = _roundtrip(lambda c: unshoulder(c) * 1.01)
    print(f"fault injection (inverse x1.01): worst channel {hurt} — must be > 0")
    return 0 if hurt > 0 else 1


def table():
    print(f"PIVOT {PIVOT}  CONTRAST {CONTRAST}")
    rows = []
    for n in names():
        ref, hdr = recover(n)
        rows.append((n,) + gate(encode(operator(hdr)), ref))
    rows.sort(key=lambda r: -r[3])
    print(f"{'row':<24}{'fraction':>10}{'worst':>7}{'mean':>8}")
    for n, f, w, m in rows:
        print(f"{n:<24}{f:>10.4f}{w:>7}{m:>8.2f}")
    print(f"\nmax worst channel over all {len(rows)} rows: {max(r[2] for r in rows)}")
    print(f"rows the gate would fail: {sum(1 for r in rows if r[1] > 0.001 or r[2] > 72)}")
    return 0


def sheets(out):
    os.makedirs(out, exist_ok=True)
    for n in names():
        _, hdr = recover(n)
        Image.fromarray(encode(operator(hdr))).save(os.path.join(out, n + ".png"))
    print(f"wrote {len(names())} predicted frames to {out}")
    return 0


if __name__ == "__main__":
    what = sys.argv[1] if len(sys.argv) > 1 else "selftest"
    sys.exit({"selftest": selftest, "table": table}.get(what, lambda: sheets(sys.argv[2]))())
