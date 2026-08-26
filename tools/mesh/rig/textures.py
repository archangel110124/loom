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
    rot = np.clip((rot_field - 0.58) / 0.22, 0.0, 1.0)

    height = np.clip(grain * (1.0 - 0.55 * rot), 0.0, 1.0).astype(np.float32)

    # Colour. Three states blended by two masks: wet-dark timber, lifted toward
    # grey where the weather has silvered it, dropped toward black-green where
    # it has gone. The grain drives colour as hard as it drives height, which is
    # what the milky first version was missing.
    g = grain[:, :, None]
    dry = np.array([0.150, 0.124, 0.099]) + g * np.array([0.135, 0.115, 0.092])
    silver = np.array([0.245, 0.240, 0.228]) + g * np.array([0.130, 0.128, 0.120])
    rotten = np.array([0.045, 0.052, 0.038]) + g * np.array([0.040, 0.045, 0.030])

    s = np.clip(silvering * 1.30 - 0.30, 0.0, 1.0)[:, :, None]
    r = rot[:, :, None]
    rgb = dry * (1.0 - s) + silver * s
    rgb = rgb * (1.0 - r) + rotten * r
    # The three palettes above are LINEAR reflectance and are composed linearly
    # — which is the only space in which blending two materials is meaningful.
    # The bytes that leave here are sRGB, because that is what the engine
    # decodes them as. See `_srgb_encode`.
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height


def normal_from_height(height, strength=2.0):
    """Tangent-space normal map, +Y green (OpenGL). Wraps, like its source.

    **NOT sRGB-encoded, and must never be.** A normal map is not colour — it is
    three signed vector components packed into bytes, and `materials.rs:195`
    loads it as `ColorSpace::Linear` precisely so nothing touches them. Running
    the transfer function over a set of vectors tilts every surface toward the
    map's brighter channels. The albedo path got an sRGB encode (see
    `_srgb_encode`); this one is asymmetric ON PURPOSE. Do not "fix" it to
    match."""
    dx = (np.roll(height, -1, axis=1) - np.roll(height, 1, axis=1)) * strength
    dy = (np.roll(height, -1, axis=0) - np.roll(height, 1, axis=0)) * strength
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
