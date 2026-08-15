#!/usr/bin/env python3
"""Is this texture actually seamless? Answer with a number, not an eyeball.

The test: a tiling texture's wrap boundary must be statistically
indistinguishable from its own interior. So compare the mean absolute
difference across the wrap seam against the distribution of the same quantity
taken between every adjacent interior column (and row).

Reported as a z-score against the interior distribution. A seam that is
invisible scores near 0; a seam that is a hard edge scores in the tens.

Threshold: |z| <= 3.0 passes. That is deliberately strict — three standard
deviations of the texture's own gradient statistics — because "approximately
hidden" seams are exactly what shows up as a grid across a hillside once the
texture is tiled fifty times.

Usage: seamless.py FILE [FILE...]
"""
import sys

import numpy as np
from PIL import Image


def axis_score(a: np.ndarray) -> tuple[float, float, float]:
    """Score the wrap seam along axis 1 (columns).

    Returns (seam_mad, interior_mean, z).
    """
    # Mean absolute difference between each adjacent pair of columns.
    interior = np.abs(np.diff(a.astype(np.float64), axis=1)).mean(axis=(0, 2))
    # And across the wrap, last column against first.
    seam = np.abs(a[:, 0].astype(np.float64) - a[:, -1].astype(np.float64)).mean()
    mu, sd = interior.mean(), interior.std()
    # A perfectly flat texture has sd 0 and no meaningful z; call it a pass,
    # since a constant image genuinely does tile.
    z = 0.0 if sd < 1e-9 else (seam - mu) / sd
    return seam, mu, z


def report(path: str) -> bool:
    a = np.asarray(Image.open(path).convert("RGB"))
    seam_x, mu_x, z_x = axis_score(a)
    # Transpose to reuse the same code for the vertical seam.
    seam_y, mu_y, z_y = axis_score(np.transpose(a, (1, 0, 2)))
    worst = max(abs(z_x), abs(z_y))
    ok = worst <= 3.0
    print(
        f"{'PASS' if ok else 'FAIL'}  {path}\n"
        f"      horizontal seam {seam_x:6.2f} vs interior {mu_x:6.2f}   z = {z_x:+8.2f}\n"
        f"      vertical   seam {seam_y:6.2f} vs interior {mu_y:6.2f}   z = {z_y:+8.2f}"
    )
    return ok


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        raise SystemExit(2)
    raise SystemExit(0 if all([report(p) for p in sys.argv[1:]]) else 1)
