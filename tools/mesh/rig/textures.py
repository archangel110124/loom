# tools/mesh/rig/textures.py
"""Tiling PBR maps for the rig, generated from parameters.

**Not baked in Cycles, deliberately.** Cycles is available here (OPTIX and CUDA
on the 4090) and would give richer nodes, but it is not bit-reproducible across
machines or driver versions, and this project's entire verification story is
that a result reproduces. A numpy field is the same bytes on any box, costs no
GPU while somebody is at the machine, and keeps the asset a pure function of its
parameters — the property that let `build_boat.py` survive a migration intact.

ponytail: value-noise fBm, not a Perlin/Worley library. If the timber needs
真 grain anisotropy that this cannot reach, the upgrade path is a Cycles AO +
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
        out += amp * _value_noise(size, freq, max(1, freq // stretch), rng)
        norm += amp
        amp *= gain
        freq *= 2
    return out / norm


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
    # function came out milky — mean RGB [113, 107, 97] against a target of
    # [48, 43, 38], with a standard deviation of 8. Measured, then fixed.
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
    return (np.clip(rgb, 0.0, 1.0) * 255.0 + 0.5).astype(np.uint8), height


def normal_from_height(height, strength=2.0):
    """Tangent-space normal map, +Y green (OpenGL). Wraps, like its source."""
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
