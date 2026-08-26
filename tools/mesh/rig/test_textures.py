# tools/mesh/rig/test_textures.py
"""Run: python3 tools/mesh/rig/test_textures.py"""
import os, sys, hashlib, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
import textures


def test_tiles_seamlessly():
    """A texture that does not tile shows a seam on every plank butt joint.

    **Each axis is judged against its own interior baseline.** The grain is 8:1
    anisotropic, so neighbouring ROWS differ several times as much as
    neighbouring COLUMNS; comparing a horizontal seam against the column-wise
    baseline fails a texture that tiles perfectly well. That bug was in this
    test first, and it is the reason the two baselines are computed separately.
    """
    alb, h = textures.weathered_timber(size=256, seed=3)
    chan = alb[:, :, 0].astype(int)
    interior_x = np.abs(np.diff(chan, axis=1)).mean()   # column to column
    interior_y = np.abs(np.diff(chan, axis=0)).mean()   # row to row
    seam_x = np.abs(chan[:, 0] - chan[:, -1]).mean()
    seam_y = np.abs(chan[0, :] - chan[-1, :]).mean()
    assert seam_x <= interior_x * 2.0 + 1.0, \
        "vertical seam: %.2f vs column baseline %.2f" % (seam_x, interior_x)
    assert seam_y <= interior_y * 2.0 + 1.0, \
        "horizontal seam: %.2f vs row baseline %.2f" % (seam_y, interior_y)
    print("  tiling ok  seam_x=%.2f/%.2f  seam_y=%.2f/%.2f"
          % (seam_x, interior_x, seam_y, interior_y))


def test_is_deterministic():
    """The whole reason for generating rather than painting."""
    a1, _ = textures.weathered_timber(size=128, seed=11)
    a2, _ = textures.weathered_timber(size=128, seed=11)
    a3, _ = textures.weathered_timber(size=128, seed=12)
    assert hashlib.sha256(a1.tobytes()).digest() == hashlib.sha256(a2.tobytes()).digest(), \
        "same seed gave different bytes"
    assert hashlib.sha256(a1.tobytes()).digest() != hashlib.sha256(a3.tobytes()).digest(), \
        "different seeds gave identical bytes — the seed is not wired up"
    print("  determinism ok")


def test_normal_map_is_a_unit_field():
    _, h = textures.weathered_timber(size=128, seed=5)
    n = textures.normal_from_height(h)
    v = (n.astype(np.float32) / 255.0) * 2.0 - 1.0
    length = np.sqrt((v ** 2).sum(axis=2))
    assert abs(length.mean() - 1.0) < 0.02, "mean normal length %.4f" % length.mean()
    # A flat-blue normal map means the height field never reached it.
    assert v[:, :, 0].std() > 0.01, "normal X is flat — height is not being read"
    print("  normal ok  |n|=%.4f  Xstd=%.4f" % (length.mean(), v[:, :, 0].std()))


def test_reads_as_dark_weathered_timber():
    """Taste is not gateable, but *brightness* is, and the first version of this
    generator failed on brightness: mean RGB [113, 107, 97] and std 8, a milky
    beige. The deck it replaces is authored `albedo = [0.19, 0.17, 0.15]`, i.e.
    [48, 43, 38]. A texture is allowed to sit above a flat colour it replaces —
    it has variation the flat one did not — but not by double.
    """
    alb, _ = textures.weathered_timber(size=256, seed=7)
    mean = alb.reshape(-1, 3).mean(axis=0)
    std = alb.reshape(-1, 3).std(axis=0)
    assert (mean < 80).all(), \
        "too pale: mean %s, target is near [48 43 38]" % mean.round(1)
    assert (mean > 30).all(), "too dark: mean %s" % mean.round(1)
    assert (std > 10).all(), \
        "too flat: std %s — the grain is not reaching the colour" % std.round(1)
    assert mean[0] > mean[2], "timber is warm; R should exceed B, got %s" % mean.round(1)
    print("  palette ok  mean=%s std=%s" % (mean.round(1), std.round(1)))


def test_png_round_trip():
    alb, _ = textures.weathered_timber(size=64, seed=1)
    p = os.path.join(tempfile.mkdtemp(), "t.png")
    textures.write_png(p, alb)
    assert os.path.getsize(p) > 100
    assert open(p, "rb").read(8) == b"\x89PNG\r\n\x1a\n", "not a PNG"
    print("  png ok  %d bytes" % os.path.getsize(p))


test_tiles_seamlessly()
test_is_deterministic()
test_normal_map_is_a_unit_field()
test_reads_as_dark_weathered_timber()
test_png_round_trip()
print("textures: all checks pass")
