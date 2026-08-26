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
    """Taste is not gateable, but *brightness* is — and it must be gated in the
    space the value actually lives in.

    **Assert on LINEAR reflectance, never on raw bytes.** The albedo PNG is
    loaded as `ColorSpace::Srgb` (`crates/loom_cli/src/materials.rs:194`), so
    the engine gamma-DECODES every byte before lighting it. The deck this
    replaces is authored `albedo = [0.19, 0.17, 0.15]`
    (`deeper_demo.loom:1059`), and that is a LINEAR triple — comparing it to a
    byte is comparing two different quantities. This test used to derive
    "i.e. [48, 43, 38]" from it and assert on bytes, which is not a check of
    the texture, it is the sRGB bug written down as a target. Decode first,
    then judge.

    A texture is allowed to sit above the flat colour it replaces — it has
    variation the flat one did not — but not by double, and not below half.
    """
    alb, _ = textures.weathered_timber(size=256, seed=7)
    b = alb.reshape(-1, 3).astype(np.float64) / 255.0
    lin = np.where(b <= 0.04045, b / 12.92, ((b + 0.055) / 1.055) ** 2.4)
    mean = lin.mean(axis=0)
    std = lin.std(axis=0)
    target = np.array([0.19, 0.17, 0.15])

    assert (mean < target * 2.0).all(), \
        "too pale: linear mean %s, authored target %s" % (mean.round(4), target)
    assert (mean > target * 0.5).all(), \
        "too dark: linear mean %s, authored target %s" % (mean.round(4), target)
    assert (std > 0.015).all(), \
        "too flat: linear std %s — the grain is not reaching the colour" % std.round(4)
    assert mean[0] > mean[2], \
        "timber is warm; linear R should exceed B, got %s" % mean.round(4)
    print("  palette ok  linear mean=%s std=%s (bytes mean=%s)"
          % (mean.round(4), std.round(4),
             alb.reshape(-1, 3).mean(axis=0).round(1)))


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
