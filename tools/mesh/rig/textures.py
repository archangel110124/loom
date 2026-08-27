# tools/mesh/rig/textures.py
"""Tiling PBR maps for the rig, generated from parameters.

**Not baked in Cycles, deliberately.** Cycles is available here (OPTIX and CUDA
on the 4090) and would give richer nodes, but it is not bit-reproducible across
machines or driver versions, and this project's entire verification story is
that a result reproduces. A numpy field is the same bytes on any box, costs no
GPU while somebody is at the machine, and keeps the asset a pure function of its
parameters — the property that let `build_boat.py` survive a migration intact.

ponytail: value-noise fBm, not a Perlin/Worley library. If the timber needs
true grain anisotropy that this cannot reach, the upgrade path is a Cycles AO +
curvature bake composited on top of these, NOT a new dependency.

Everything wraps: `np.roll`-based interpolation on a periodic lattice, so every
octave is seamless and therefore so is the sum. A seam on a tiling deck texture
appears at every plank butt joint at once, which is the most visible way for
this to be wrong.
"""
import struct
import zlib

import numpy as np

# The size every rig map SHIPS at. Named here rather than in each builder so
# there is one number to change and so `test_textures.py` can assert at the
# size the generators actually produce -- see `normal_from_height`, whose
# output used to be measured at 128 and shipped at 2048, two maps with a
# 12x difference in relief and one PASS between them.
TEX_SIZE = 2048


def _axis(size, freq):
    """Wrapped bilinear sample positions for one axis, at `freq` cells."""
    t = (np.arange(size, dtype=np.float32) + 0.5) * freq / size - 0.5
    i0 = np.floor(t).astype(np.int32)
    frac = (t - i0).astype(np.float32)
    smooth = frac * frac * (3.0 - 2.0 * frac)      # smoothstep, C1 at the cells
    return i0 % freq, (i0 + 1) % freq, smooth


def _value_noise(size, freq_y, freq_x, rng):
    """One octave of periodic value noise, ANISOTROPIC.

    Separate frequencies per axis is what makes timber read as timber: grain is
    a low frequency across the board and a high one along it. Averaging shifted
    isotropic copies — the obvious alternative — introduces its own periodicity
    at the shift interval and reads as corduroy.

    Index arithmetic is modulo the frequency, which is what makes each octave
    periodic and therefore the whole sum tileable. A seam on a decking texture
    shows at every butt joint at once.
    """
    lattice = rng.random((freq_y, freq_x)).astype(np.float32)
    y0, y1, sy = _axis(size, freq_y)
    x0, x1, sx = _axis(size, freq_x)

    a = lattice[np.ix_(y0, x0)]
    b = lattice[np.ix_(y0, x1)]
    c = lattice[np.ix_(y1, x0)]
    d = lattice[np.ix_(y1, x1)]
    top = a + (b - a) * sx[None, :]
    bot = c + (d - c) * sx[None, :]
    return top + (bot - top) * sy[:, None]


def _fbm(size, rng, octaves=5, base=4, gain=0.5, stretch=1):
    """`stretch` divides the ALONG-grain frequency: 8 gives 8:1 timber grain."""
    out = np.zeros((size, size), dtype=np.float32)
    amp, freq, norm = 1.0, base, 0.0
    for _ in range(octaves):
        # Floor is 2, NOT 1. freq=1 is degenerate, not merely low-frequency:
        # `_axis` returns i0 % 1 == 0 and (i0 + 1) % 1 == 0, so both bilinear
        # taps read the same lattice cell and the interpolation along that axis
        # is a no-op — the octave comes out exactly flat, std 0.000000. Measured
        # 2026-08-26: with a floor of 1, 85.7% of the silvering fBm's amplitude
        # sat in such octaves, giving ~10.7:1 banding where this docstring
        # promises 8:1. Two cells is the smallest frequency that still
        # interpolates.
        out += amp * _value_noise(size, freq, max(2, freq // stretch), rng)
        norm += amp
        amp *= gain
        freq *= 2
    return out / norm


def _srgb_encode(linear):
    """Linear reflectance 0..1 -> sRGB-encoded 0..1.

    **This exists because the engine gamma-DECODES this map.**
    `crates/loom_cli/src/materials.rs:194` loads `albedo_map` as
    `ColorSpace::Srgb`, while the palette below is authored in LINEAR
    reflectance (the same space `deeper_demo.loom:1059`'s flat
    `albedo = [0.19, 0.17, 0.15]` is read in). The first version of this file
    wrote `linear * 255` straight out as bytes and conflated the two, so a
    linear 0.19 shipped as byte 48, the engine decoded byte 48 back to linear
    0.031, and the deck drew 4-4.9x too dark. Encoded properly, linear 0.19 is
    byte ~123.

    The real piecewise transfer function, not a 1/2.2 approximation: 1/2.2 is
    off by up to 4 byte levels and it is wrong exactly where this texture lives
    — in the dark end, where the linear segment matters most.
    """
    c = np.clip(linear, 0.0, 1.0)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1.0 / 2.4) - 0.055)


def weathered_timber(size=2048, seed=7):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    The plank runs along +U. Grain is fBm stretched 8:1 along the grain, which
    is what makes it read as timber rather than as noise. On top of that: a
    low-frequency silvering that greys the exposed surface, and sparse dark rot
    blooms that eat both the colour and the height.
    """
    rng = np.random.default_rng(seed)

    # Grain runs ALONG the board, which is +U. 8:1 anisotropy: fine detail
    # across the grain, long smooth runs along it.
    grain = (_fbm(size, rng, octaves=6, base=32, stretch=8) * 0.70
             + _fbm(size, rng, octaves=4, base=4, stretch=4) * 0.30)
    # **Normalise to the full range.** Without this the contrast depends on how
    # many octaves happened to land near the mean, and the first version of this
    # function came out milky — a flat, near-uniform field with a byte standard
    # deviation of 8. Measured, then fixed. (The "target of [48, 43, 38]" this
    # comment used to name was the sRGB bug, not a target: see `_srgb_encode`.
    # The target is LINEAR [0.19, 0.17, 0.15], which encodes near byte 123.)
    grain = (grain - grain.min()) / (grain.max() - grain.min())

    silvering = _fbm(size, rng, octaves=3, base=2, stretch=3)
    silvering = (silvering - silvering.min()) / (silvering.max() - silvering.min())
    rot_field = _fbm(size, rng, octaves=5, base=6, stretch=5)
    rot = np.clip((rot_field - 0.46) / 0.22, 0.0, 1.0)

    height = np.clip(grain * (1.0 - 0.55 * rot), 0.0, 1.0).astype(np.float32)

    # Colour. Three states blended by two masks: wet-dark timber, lifted toward
    # grey where the weather has silvered it, dropped toward black-green where
    # it has gone. The grain drives colour as hard as it drives height, which is
    # what the milky first version was missing.
    g = grain[:, :, None]
    # **Palette D**, chosen by the human from four in-engine variants rendered at
    # standing eye height on 2026-08-26. Darker, wetter, rot spread wider — a
    # jetty over cold water rather than a maintained boardwalk. `silver` came
    # down hardest because it was what dominated the frame; the tide zone below
    # this deck needs somewhere darker to go, and the shipped palette left it
    # none. LINEAR reflectance; `_srgb_encode` handles the transfer on the way out.
    dry = np.array([0.070, 0.055, 0.040]) + g * np.array([0.090, 0.072, 0.052])
    silver = np.array([0.125, 0.120, 0.108]) + g * np.array([0.080, 0.077, 0.068])
    rotten = np.array([0.022, 0.026, 0.016]) + g * np.array([0.026, 0.029, 0.017])

    s = np.clip(silvering * 1.10 - 0.45, 0.0, 1.0)[:, :, None]
    r = rot[:, :, None]
    rgb = dry * (1.0 - s) + silver * s
    rgb = rgb * (1.0 - r) + rotten * r
    # The three palettes above are LINEAR reflectance and are composed linearly
    # — which is the only space in which blending two materials is meaningful.
    # The bytes that leave here are sRGB, because that is what the engine
    # decodes them as. See `_srgb_encode`.
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height


def weathered_steel(size=2048, seed=11):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    Rusted steel plate. Three states, like the timber: sound paint, rust bloom,
    and the dark wet streak that runs DOWN from every bloom because water does.
    The streaking is vertical here where the timber's was horizontal — steel
    weathers along gravity, not along a grain — so `stretch` is applied to the
    other axis by transposing the field.

    LINEAR reflectance in, sRGB bytes out. See `_srgb_encode`.
    """
    rng = np.random.default_rng(seed)

    # Blooms are isotropic; the streaks below them are not. Build the streak
    # field stretched along Y by generating stretched-along-X and transposing,
    # which reuses `_fbm` rather than adding a second anisotropy path.
    bloom = _fbm(size, rng, octaves=5, base=5)
    bloom = (bloom - bloom.min()) / (bloom.max() - bloom.min())
    streak = _fbm(size, rng, octaves=5, base=24, stretch=6).T
    streak = (streak - streak.min()) / (streak.max() - streak.min())
    pit = _fbm(size, rng, octaves=6, base=40)

    rust = np.clip((bloom - 0.45) / 0.30, 0.0, 1.0)
    # A streak only exists below a bloom, so the bloom field is smeared
    # DOWNWARD and used to gate the streak field.
    #
    # **The smear must WRAP.** `np.maximum.accumulate(rust, axis=0)` is the
    # obvious way to write "anything above this has already rusted" and it does
    # not tile: it starts fresh at row 0, so the top of the image carries no
    # accumulated rust and the bottom carries all of it. Measured seam 8.72
    # against a 1.12 interior baseline — a visible band across every pile at the
    # same height. `np.roll` wraps, and a bounded smear is the better model
    # anyway: a streak fades with distance below the bloom that fed it.
    REACH = size // 6
    smear = rust.copy()
    for k in range(1, REACH):
        smear = np.maximum(smear, np.roll(rust, k, axis=0) * (1.0 - k / REACH))
    run = np.clip(smear * streak * 1.4 - 0.25, 0.0, 1.0)

    height = np.clip(0.35 + 0.45 * rust + 0.20 * pit, 0.0, 1.0).astype(np.float32)

    p = pit[:, :, None]
    plate = np.array([0.048, 0.050, 0.052]) + p * np.array([0.030, 0.031, 0.032])
    rusty = np.array([0.115, 0.052, 0.024]) + p * np.array([0.085, 0.040, 0.016])
    wet = np.array([0.030, 0.022, 0.017]) + p * np.array([0.022, 0.016, 0.012])

    r = rust[:, :, None]
    w = run[:, :, None]
    rgb = plate * (1.0 - r) + rusty * r
    rgb = rgb * (1.0 - w) + wet * w
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height


def tidal_growth(size=2048, seed=13):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    The band where the piles cross the water. Three things live here: black
    weed that hangs in vertical fronds, barnacle crust that is pale and hard and
    clusters, and the wet steel showing through between them.

    It is the darkest and the only green thing on the rig, deliberately — the
    spec calls this the horror seam, and a seam that is the same colour as what
    it joins is not a seam. LINEAR in, sRGB bytes out.
    """
    rng = np.random.default_rng(seed)

    # Weed hangs, so its field is stretched along Y — generated stretched along
    # X and transposed, the same trick `weathered_steel` uses.
    weed_f = _fbm(size, rng, octaves=5, base=20, stretch=5).T
    weed_f = (weed_f - weed_f.min()) / (weed_f.max() - weed_f.min())
    # Barnacles cluster, so a low frequency gates a high one.
    cluster = _fbm(size, rng, octaves=3, base=4)
    cluster = (cluster - cluster.min()) / (cluster.max() - cluster.min())
    grit = _fbm(size, rng, octaves=6, base=64)

    weed = np.clip((weed_f - 0.42) / 0.30, 0.0, 1.0)
    barnacle = np.clip((grit - 0.60) / 0.14, 0.0, 1.0) * np.clip(cluster * 1.6 - 0.35, 0.0, 1.0)

    # Barnacles stand proud; weed lies flat and wet.
    height = np.clip(0.30 + 0.55 * barnacle - 0.10 * weed + 0.15 * grit, 0.0, 1.0).astype(np.float32)

    g = grit[:, :, None]
    wet_steel = np.array([0.022, 0.024, 0.023]) + g * np.array([0.016, 0.017, 0.016])
    weed_col = np.array([0.014, 0.030, 0.016]) + g * np.array([0.012, 0.026, 0.013])
    shell = np.array([0.090, 0.086, 0.076]) + g * np.array([0.055, 0.052, 0.045])

    w = weed[:, :, None]
    b = barnacle[:, :, None]
    rgb = wet_steel * (1.0 - w) + weed_col * w
    rgb = rgb * (1.0 - b) + shell * b
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height


def weathered_boards(size=1024, seed=17):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    A painted timber wall that has been in salt air for years: the paint is
    mostly gone, what is left is chalky, and the grain shows through.

    **The grain runs VERTICALLY here** — down the image — because a wall board
    stands on end. The deck's grain runs along +U because its planks lie flat.
    Same generator, transposed anisotropy; getting it wrong makes the shed read
    as a floor stood on its edge, which is the one thing a viewer notices
    immediately and cannot name.

    LINEAR reflectance in, sRGB bytes out. See `_srgb_encode`.
    """
    rng = np.random.default_rng(seed)

    # Stretched along X and transposed, so the long runs end up vertical.
    grain = (_fbm(size, rng, octaves=6, base=32, stretch=8).T * 0.70
             + _fbm(size, rng, octaves=4, base=4, stretch=4).T * 0.30)
    grain = (grain - grain.min()) / (grain.max() - grain.min())

    # Paint survives in patches, not evenly. Low frequency, isotropic.
    paint = _fbm(size, rng, octaves=4, base=3)
    paint = (paint - paint.min()) / (paint.max() - paint.min())
    kept = np.clip(paint * 1.45 - 0.35, 0.0, 1.0)

    # Rot creeps up from the bottom of a board, so it is gated by a vertical
    # ramp as well as by its own field. **The ramp has to WRAP.** A
    # `np.linspace` down the image is the obvious ramp and it does not tile:
    # measured seam_y 12.10 against a 0.24 interior baseline, a hard band across
    # the wall wherever V passes 1. The shed is 2.4 m tall and the UV maps 2 m
    # per tile, so this texture IS tiled vertically — an earlier draft of this
    # comment claimed otherwise and was wrong. Seam_y 0.32 against 0.29.
    #
    # **What the cosine does, measured, not assumed.** `0.5 - 0.5*cos(2*pi*t)`
    # has ONE maximum, at t = 0.5, and is zero at t = 0 and t = 1 — the rot
    # sits in the MIDDLE of the tile, not at its ends. An earlier draft of this
    # comment said "both ends"; the shipped 1024² albedo's darkest row is 509,
    # t = 0.497. What still puts rot at both the sill and the eaves is the
    # WALL's V range, not the ramp's shape: `build_shed.py` maps V = 1 - y/2
    # about the shed's own centre, so the wall runs V 0.400 at its top (world
    # y 3.80) to V 1.600 at its sill (1.40) — 1.2 tiles, both values read back
    # off `rig_shed.obj` — and t = 0.5 falls inside that twice, at V = 0.5
    # (world y 3.60, 200 mm under the eaves) and V = 1.5 (world y 1.60, 200 mm
    # above the sill). Do not claim more than that from a picture: in
    # `rig_shed.loom`'s frame neither band is legible on its own. The upper one
    # lands inside the roof soffit's shadow (measured 36-55 luminance across
    # those rows against the lit wall's 99), and the lower one sits in a smooth
    # top-to-bottom falloff — 99.7 at the wall's brightest row down to 74.7 at
    # the sill — with no local minimum at it. The band is visible in the
    # TEXTURE (row-mean luminance 80.4 at t = 0.5 against 94.4 at t = 0), which
    # is where it was measured.
    rot_f = _fbm(size, rng, octaves=5, base=6, stretch=4).T
    rot_f = (rot_f - rot_f.min()) / (rot_f.max() - rot_f.min())
    t = np.arange(size, dtype=np.float32) / size
    rise = (0.5 - 0.5 * np.cos(2.0 * np.pi * t))[:, None]
    rot = np.clip((rot_f * 0.6 + rise * 0.6) - 0.62, 0.0, 1.0)

    height = np.clip(0.30 + 0.45 * grain - 0.25 * rot, 0.0, 1.0).astype(np.float32)

    g = grain[:, :, None]
    bare = np.array([0.088, 0.066, 0.047]) + g * np.array([0.080, 0.060, 0.042])
    painted = np.array([0.160, 0.098, 0.064]) + g * np.array([0.068, 0.042, 0.027])
    rotten = np.array([0.030, 0.027, 0.020]) + g * np.array([0.024, 0.022, 0.016])

    k = kept[:, :, None]
    r = rot[:, :, None]
    rgb = bare * (1.0 - k) + painted * k
    rgb = rgb * (1.0 - r) + rotten * r
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height


def corrugated_metal(size=1024, seed=19):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    Galvanised sheet, weathered. The spangle has gone chalky, rust has taken the
    laps and the fixings, and dirt has run down from every one.

    **The corrugation itself is GEOMETRY, not this texture.** `build_roof.py`
    folds the sheet; this supplies the surface on top of it. Baking ridges into
    the height map as well would double them and read as a moire.

    LINEAR reflectance in, sRGB bytes out.
    """
    rng = np.random.default_rng(seed)

    chalk = _fbm(size, rng, octaves=5, base=6)
    chalk = (chalk - chalk.min()) / (chalk.max() - chalk.min())
    # Rust starts at fixings and laps -- sparse points, not a field.
    spot = _fbm(size, rng, octaves=6, base=14)
    spot = (spot - spot.min()) / (spot.max() - spot.min())
    # 0.62 leaves so little rust that the mean comes back R/B 1.05 — visually
    # bare galvanise, and sitting on its own assertion's boundary. 0.52 gives
    # 1.24, which reads as a rusting roof and has headroom both ways.
    rust = np.clip((spot - 0.52) / 0.22, 0.0, 1.0)
    # And runs DOWN from each one. Same wrapping smear as `weathered_steel`:
    # `np.maximum.accumulate` does not tile and puts a band across the sheet.
    REACH = size // 8
    smear = rust.copy()
    for k in range(1, REACH):
        smear = np.maximum(smear, np.roll(rust, k, axis=0) * (1.0 - k / REACH))
    dirt = np.clip(smear * 0.55 - 0.15, 0.0, 1.0)

    height = np.clip(0.40 + 0.35 * rust + 0.15 * chalk, 0.0, 1.0).astype(np.float32)

    c = chalk[:, :, None]
    zinc = np.array([0.088, 0.092, 0.096]) + c * np.array([0.046, 0.048, 0.050])
    rusty = np.array([0.150, 0.072, 0.034]) + c * np.array([0.080, 0.038, 0.016])
    grime = np.array([0.042, 0.041, 0.038]) + c * np.array([0.028, 0.027, 0.024])

    r = rust[:, :, None]
    d = dirt[:, :, None]
    rgb = zinc * (1.0 - r) + rusty * r
    rgb = rgb * (1.0 - d) + grime * d
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height


def painted_iron(size=1024, seed=23):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    Bollards and a mast: cast iron that was painted once. The paint survives in
    the hollows and has been rubbed off every edge and every place a rope has
    passed, which is most of a bollard.

    Isotropic on purpose -- unlike the timber and unlike the piles' vertical
    streaking, wear on a bollard has no grain and no gravity direction. It
    follows the rope, and the rope goes everywhere.

    LINEAR reflectance in, sRGB bytes out.
    """
    rng = np.random.default_rng(seed)

    wear = _fbm(size, rng, octaves=5, base=7)
    wear = (wear - wear.min()) / (wear.max() - wear.min())
    pit = _fbm(size, rng, octaves=6, base=30)
    pit = (pit - pit.min()) / (pit.max() - pit.min())
    bare = np.clip((wear - 0.45) / 0.28, 0.0, 1.0)
    rust = np.clip((pit - 0.68) / 0.18, 0.0, 1.0) * bare

    height = np.clip(0.45 + 0.25 * pit - 0.20 * bare, 0.0, 1.0).astype(np.float32)

    p = pit[:, :, None]
    paint = np.array([0.052, 0.048, 0.044]) + p * np.array([0.030, 0.028, 0.026])
    iron = np.array([0.070, 0.068, 0.070]) + p * np.array([0.040, 0.039, 0.040])
    rusty = np.array([0.098, 0.050, 0.028]) + p * np.array([0.050, 0.026, 0.014])

    b = bare[:, :, None]
    r = rust[:, :, None]
    rgb = paint * (1.0 - b) + iron * b
    rgb = rgb * (1.0 - r) + rusty * r
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height


def weathered_paint(size=1024, seed=29, paint=(0.022, 0.055, 0.125)):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    Painted timber, salt-worn. Three states: paint that has held, bare grey-warm
    timber where it has worn through, and white salt bloom drifted over both.

    **`paint` is a knob, not a constant.** Sampled off the reference's sunlit
    wall the colour reads linear [0.036, 0.108, 0.311] — blue at 8.6x red — but
    that sample has the SUN in it. What transfers is the hue ratio; the level
    does not. The default keeps the ratio and drops the level to what a painted
    board actually reflects. Retune it from the scene, not from the photograph.

    Grain runs horizontally here because the boards are laid horizontally — the
    opposite of `weathered_boards`, which was written for vertical siding.

    LINEAR reflectance in, sRGB bytes out. See `_srgb_encode`.
    """
    rng = np.random.default_rng(seed)

    # Grain along the board, which is +U for horizontal siding.
    grain = (_fbm(size, rng, octaves=6, base=32, stretch=8) * 0.70
             + _fbm(size, rng, octaves=4, base=4, stretch=4) * 0.30)
    grain = (grain - grain.min()) / (grain.max() - grain.min())

    # Where the paint has gone. **Wear follows the GRAIN, not the weather.**
    # The first version read `base=5`, isotropic, and weighted the weather
    # field 0.75 against the grain's 0.25 through a 0.16-wide transition. In
    # the render that came out as lichen: tan blotches ~0.4 m across (base 5
    # over a 2 m tile) with hard edges, scattered over the wall like
    # camouflage, floating free of the boards they sit on. The photograph has
    # nothing of the kind — its paint goes along the grain and at the edges,
    # in streaks a hand wide.
    #
    # Three changes, each doing one thing:
    #   base 5 -> 18       blobs from ~0.40 m to ~0.11 m, so they read as
    #                      surface rather than as objects on it;
    #   stretch 1 -> 4     the wear field is itself elongated along +U, the
    #                      direction the board runs, so a patch is a streak;
    #   0.25 -> 0.55 grain the grain is now the LARGER term, so where the
    #                      paint goes is decided mostly by the timber under it.
    # The transition widens 0.16 -> 0.28 so a patch fades out instead of
    # ending at a contour line, and the threshold drops 0.60 -> 0.48 to keep
    # the fully-bare fraction inside `test_textures.py`'s 0.04..0.40 band
    # (measured 0.070 at 256²) — a wider transition alone would have starved
    # it to 0.001 and failed the floor.
    #
    # Measured on the wear field, anisotropy as mean |d/dy| over mean |d/dx|:
    # 1.66 before, 5.77 after. Higher means flatter, longer streaks.
    wear_f = _fbm(size, rng, octaves=5, base=18, stretch=4)
    wear_f = (wear_f - wear_f.min()) / (wear_f.max() - wear_f.min())
    # Grain sits proud of the softer wood between it, so the grain wears first.
    worn = np.clip((wear_f * 0.45 + grain * 0.55 - 0.48) / 0.28, 0.0, 1.0)

    # Salt dries white in the sheltered parts, over paint and bare wood alike.
    #
    # **This field was half the blotching, and fixing only the wear left it.**
    # At `base=9`, isotropic and blending to a full 1.0, it put ~0.22 m puffs
    # of near-white (`bloom` is 0.21 linear against the paint's 0.125, and
    # neutral, so it takes the blue out where it lands) over the wall — read
    # off the first corrected render, where the wear had gone streaky and
    # these had not. Same three changes as the wear above: finer (24), run
    # along the board (stretch 4), and a 0.5 CEILING so salt tints the paint
    # rather than replacing it. Measured at 256²: peak blend 0.58 -> 0.35,
    # anisotropy 0.96 -> 3.90, B/R 2.46 -> 2.51.
    salt = _fbm(size, rng, octaves=4, base=24, stretch=4)
    salt = np.clip((salt - salt.min()) / (salt.max() - salt.min()) * 1.3 - 0.60,
                   0.0, 1.0) * 0.5

    height = np.clip(0.35 + 0.35 * grain - 0.20 * worn, 0.0, 1.0).astype(np.float32)

    g = grain[:, :, None]
    p = np.array(paint)
    painted = p + g * (p * 0.55)
    # Bare timber under it: warm, mid-grey, the same family as the deck.
    bare = np.array([0.085, 0.068, 0.050]) + g * np.array([0.070, 0.056, 0.040])
    bloom = np.array([0.210, 0.212, 0.208]) + g * np.array([0.060, 0.060, 0.058])

    w = worn[:, :, None]
    s = salt[:, :, None]
    rgb = painted * (1.0 - w) + bare * w
    rgb = rgb * (1.0 - s) + bloom * s
    return (_srgb_encode(np.clip(rgb, 0.0, 1.0)) * 255.0 + 0.5).astype(np.uint8), height


def normal_from_height(height, strength=0.008):
    """Tangent-space normal map, +Y green (OpenGL). Wraps, like its source.

    **NOT sRGB-encoded, and must never be.** A normal map is not colour — it is
    three signed vector components packed into bytes, and `materials.rs:195`
    loads it as `ColorSpace::Linear` precisely so nothing touches them. Running
    the transfer function over a set of vectors tilts every surface toward the
    map's brighter channels. The albedo path got an sRGB encode (see
    `_srgb_encode`); this one is asymmetric ON PURPOSE. Do not "fix" it to
    match.

    **`strength` is a SLOPE, not a per-texel nudge, and the difference shipped
    a flat map.** The central difference below is taken per TEXEL, so for a
    height field whose features live in UV space it measures `dh` across
    `2/size` of a tile: double the resolution and every gradient halves. With
    the old `strength=2.0` applied straight to that difference, the same
    generator produced (weathered_timber, seed 5, normal X std / mean surface
    tilt):

        128   0.0549  11.29 deg      1024  0.0084   0.75 deg
        512   0.0157   2.85 deg      2048  0.0047   0.04 deg   <- what shipped

    The test asserted `>0.01` while running at 128 and printed PASS; the deck
    normal map on disk tilted its surfaces by an average of 0.037 deg, which is
    flat. Multiplying by `size` removes the resolution dependence: what is left
    is `dh/du`, the gradient across the TILE, and `strength` becomes the one
    thing it should have been all along — **relief amplitude as a fraction of
    the tile's width.** 0.008 is ~8 mm of grain, pitting and barnacle crust
    across the ~1-2 m tiles this rig uses (`build_deck.py:120` is one tile per
    2 m along a board; `build_substructure.py`'s tubes are one per metre of
    length), and it now measures the same at 512 as at 2048 (0.0161 / 0.0162).
    `test_textures.py` asserts at `TEX_SIZE`, so the number under test is the
    number on disk."""
    ky = 0.5 * height.shape[0] * strength
    kx = 0.5 * height.shape[1] * strength
    dx = (np.roll(height, -1, axis=1) - np.roll(height, 1, axis=1)) * kx
    dy = (np.roll(height, -1, axis=0) - np.roll(height, 1, axis=0)) * ky
    nx, ny, nz = -dx, -dy, np.ones_like(height)
    length = np.sqrt(nx * nx + ny * ny + nz * nz)
    v = np.stack([nx / length, ny / length, nz / length], axis=2)
    return ((v * 0.5 + 0.5) * 255.0 + 0.5).astype(np.uint8)


def write_png(path, rgb):
    """Minimal RGB8 PNG. No pillow — this is thirty lines and one less dep."""
    h, w = rgb.shape[:2]
    raw = np.concatenate(
        [np.zeros((h, 1), dtype=np.uint8), rgb.reshape(h, w * 3)], axis=1)

    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff))

    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(raw.tobytes(), 6))
           + chunk(b"IEND", b""))
    open(path, "wb").write(png)
