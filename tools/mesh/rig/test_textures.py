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
    """**Run at the size that SHIPS, not at a convenient 128.**

    The old version of this test generated at `size=128` and asserted
    `Xstd > 0.01`. `normal_from_height` took a per-texel central difference, so
    the relief it produced scaled as 1/size and the statistic went with it:

        128  0.0549 PASS      1024  0.0084 FAIL
        512  0.0157 PASS      2048  0.0047 FAIL   <- the size the rig ships at

    So the test passed on a map nobody uses while the deck's actual normal map
    tilted its surfaces by an average of 0.037 deg. Raising `TEX_SIZE` to 4096
    would have halved it again with the test still green.

    Two ways out were available: normalise the statistic so it is
    size-invariant, or assert at the production size. **This asserts at the
    production size**, because a size-invariant statistic measured at 128 still
    would not have been measuring the bytes on disk — the fault was not that
    the number was unnormalised, it was that the test and the shipped asset
    were two different images. `textures.TEX_SIZE` is the single source both
    builders and this test read, so a change to the shipped size moves the
    test with it. It costs ~1 s.

    The threshold is UNCHANGED at 0.01. It was not the threshold that was
    wrong, and lowering it to admit today's output is the failure this finding
    is about: on today's shipped map (per-texel `strength`, 2048) this test
    fails at 0.0047. `normal_from_height` was fixed instead, so `strength` is
    a slope across the tile rather than a nudge per texel.
    """
    _, h = textures.weathered_timber(size=textures.TEX_SIZE, seed=5)
    n = textures.normal_from_height(h)
    v = (n.astype(np.float32) / 255.0) * 2.0 - 1.0
    length = np.sqrt((v ** 2).sum(axis=2))
    assert abs(length.mean() - 1.0) < 0.02, "mean normal length %.4f" % length.mean()
    # A flat-blue normal map means the height field never reached it.
    assert v[:, :, 0].std() > 0.01, \
        "normal X is flat at the shipped size %d — Xstd %.4f. Height is either " \
        "not being read or `strength` is being applied per texel again" \
        % (textures.TEX_SIZE, v[:, :, 0].std())
    tilt = np.degrees(np.arccos(np.clip(v[:, :, 2], -1.0, 1.0)))
    print("  normal ok  %d²  |n|=%.4f  Xstd=%.4f  tilt mean=%.2f° p99=%.2f°"
          % (textures.TEX_SIZE, length.mean(), v[:, :, 0].std(),
             tilt.mean(), np.percentile(tilt, 99)))


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

    # Palette D, approved 2026-08-26. Calibrated against three measured
    # points, not chosen for roundness:
    #   palette D          0.1009   must pass
    #   silver-only revert 0.1113   must fail  <- the ceiling sits between these
    #   full Phase 0       0.2316   must fail
    # 0.106 leaves Palette D ~5.1% headroom and no more -- tight for a
    # regression bound, but affordable because this generator is
    # deterministic (same seed, same bytes, verified across independent
    # rebuilds): there is no run-to-run variance for the band to absorb, so
    # the only thing that can move this number is somebody editing the
    # palette, which is exactly what the band exists to catch. 0.16 and 0.125
    # were both tried first and both left the silver-only regression passing.
    assert (mean < 0.106).all(), \
        "too pale for palette D: linear mean %s" % mean.round(4)
    assert (mean > 0.045).all(), \
        "too dark: linear mean %s" % mean.round(4)
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


def test_steel_is_darker_and_ruster_than_timber():
    """Rust is red-shifted and the plate under it is near-neutral. If R does not
    lead, this is grey paint, not rust."""
    alb, _ = textures.weathered_steel(size=256, seed=11)
    lin = np.where(alb / 255.0 <= 0.04045, (alb / 255.0) / 12.92,
                   (((alb / 255.0) + 0.055) / 1.055) ** 2.4)
    m = lin.reshape(-1, 3).mean(axis=0)
    assert m[0] > m[2] * 1.25, "not rust-shifted: linear mean %s" % m.round(4)
    # **Calibrated, not vacuous.** The old bound was 0.14 against a measured
    # 0.0786 — 78% of headroom, which no single-constant edit to this palette
    # can cross. Measured 2026-08-26, linear mean of the brightest channel:
    #   palette as shipped              0.0786   must pass
    #   `wet` reverted to plate-grey    0.0820   must fail  <- band sits here
    #   `rusty` doubled                 0.1100   must fail
    # 0.080 is the midpoint of the first two. That is 1.8% of headroom, which
    # is affordable for the same reason the deck's 0.106 is: this generator is
    # deterministic, so there is no run-to-run variance for a band to absorb
    # and the only thing that can move the number is somebody editing the
    # palette — which is what the band exists to catch. Both injections above
    # PASSED the old 0.14 and FAIL this.
    assert (m < 0.080).all(), "too bright for wet steel: %s" % m.round(4)
    # Measured 0.0157 at the time of writing; the bound sits below it so this
    # is a regression check rather than a restatement of today's number.
    assert lin.reshape(-1, 3).std(axis=0).mean() > 0.012, \
        "too flat — the rust is not varying"
    print("  steel ok  linear mean=%s" % m.round(4))


def test_steel_tiles_and_is_deterministic():
    a1, _ = textures.weathered_steel(size=128, seed=4)
    a2, _ = textures.weathered_steel(size=128, seed=4)
    assert hashlib.sha256(a1.tobytes()).digest() == hashlib.sha256(a2.tobytes()).digest()
    chan = a1[:, :, 0].astype(int)
    ix = np.abs(np.diff(chan, axis=1)).mean(); iy = np.abs(np.diff(chan, axis=0)).mean()
    sx = np.abs(chan[:, 0] - chan[:, -1]).mean(); sy = np.abs(chan[0, :] - chan[-1, :]).mean()
    assert sx <= ix * 2.0 + 1.0, "vertical seam %.2f vs %.2f" % (sx, ix)
    assert sy <= iy * 2.0 + 1.0, "horizontal seam %.2f vs %.2f" % (sy, iy)
    print("  steel tiling ok  seam %.2f/%.2f  %.2f/%.2f" % (sx, ix, sy, iy))


def test_tidal_growth_is_green_black_and_varied():
    """The seam is the one place in the rig that is not brown or grey. If G does
    not lead, this is more rust and the seam does not read."""
    alb, _ = textures.tidal_growth(size=256, seed=13)
    lin = np.where(alb / 255.0 <= 0.04045, (alb / 255.0) / 12.92,
                   (((alb / 255.0) + 0.055) / 1.055) ** 2.4)
    m = lin.reshape(-1, 3).mean(axis=0)
    assert m[1] > m[0] * 1.15, "not green-shifted: %s" % m.round(4)
    # **Calibrated, not vacuous** — same story as the steel ceiling above. The
    # old bound was 0.10 against a measured 0.0411. Measured 2026-08-26, linear
    # mean of the brightest channel:
    #   palette as shipped        0.0411   must pass
    #   `shell` doubled           0.0481   must fail  <- band sits here
    #   `weed_col` doubled        0.0558   must fail
    #   `wet_steel` tripled       0.0797   must fail  <- and this one is the
    #       regression the assertion's own message names: at 0.0797 the seam is
    #       no longer the darkest thing on the rig, it is level with the steel
    #       (0.0786) it is supposed to be a seam against. The old 0.10 let it
    #       through, so the message was a claim the check did not make.
    # 0.045 is the midpoint of the first two.
    assert (m < 0.045).all(), "the tide zone must be the darkest thing on the rig: %s" % m.round(4)
    # Measured 0.0152; bound set below it, as a regression check.
    assert lin.reshape(-1, 3).std(axis=0).mean() > 0.012, "too flat"
    print("  tidal ok  linear mean=%s" % m.round(4))


test_tiles_seamlessly()
test_is_deterministic()
test_normal_map_is_a_unit_field()
test_reads_as_dark_weathered_timber()
test_png_round_trip()
test_steel_is_darker_and_ruster_than_timber()
test_steel_tiles_and_is_deterministic()
test_tidal_growth_is_green_black_and_varied()
print("textures: all checks pass")
