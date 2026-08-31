#!/usr/bin/env python3
"""Generate `assets/textures/water_foam.png` — the sea-foam detail texture.

    python3 tools/texture/foam.py [out.png]

Stage 3 of Sea of Thieves' foam pipeline is "blended with texture": the
progressively-blurred coverage mask is a soft density with no detail of its
own, and a foam texture is what carves bubbles back into it.  In
`scene.slang` that texture is the *threshold* the coverage is eroded against
(`foam = smoothstep(T - b, T + b, mask)`), which is what the procedural
`waterFoamNoise` ladder used to be.

Three properties this file has to have, and each of them is load-bearing:

**Tileable.**  The sea is drawn in a frame that slides with the wind and
stretches 6x across it, so the texture is sampled over an unbounded plane and
any seam would draw a grid on the ocean.  Every octave here is a *periodic*
lattice whose period divides `SIDE`, so the wrap is exact rather than blended.

**Histogram-equalised to uniform on [0, 1).**  This is what makes it a
drop-in for the noise it replaces.  `loom_value_noise` is very nearly uniform,
and `smoothstep(T ± b, mask)` averaged over an area is the CDF of `T` at
`mask` — so foam coverage is a function of `T`'s *distribution*, not of its
shape.  Equalising means swapping the pattern changes which pixels are foam
and never how many, and the calibrated whitecap fractions upstream survive
untouched.  A raw fBm is bell-shaped and would have quietly moved every one.

**Filamentary, not blobby.**  Value noise has no structure at any scale but
its own lattice, which is the "lattice / facets / stringiness" this texture
exists to replace.  Foam is torn into filaments by shear, so the field here is
a domain-warped ridge (`1 - |2n - 1|`) rather than plain fBm: the warp does
the tearing and the ridge turns fBm's smooth hills into creases.

Deterministic — an integer hash, no `random`, no seed argument — and stdlib
only, so re-running it reproduces the committed PNG byte for byte.  Checked by
`--check`, which is what keeps the asset honest rather than a blob somebody
once made.
"""

import struct
import sys
import zlib

SIDE = 512

# Periods of the fBm octaves, in texels.  Each divides SIDE, which is what
# makes the wrap exact.  Five is where the sum stops gaining detail the
# 512-texel grid can hold.
OCTAVES = (8, 16, 32, 64, 128)

# How the octave amplitudes fall off with frequency — see `field`.
PERSISTENCE = 0.55

# How far the warp drags a sample, in texels.  Zero is plain fBm — round
# blobs.  Past about 40 the field folds over itself and reads as marble.
WARP = 26.0

# Periods of the two warp octaves.  The coarse one drags whole filaments; the
# fine one is what breaks the lattice's axis alignment — see `warp_at`.
WARP_PERIODS = (128, 32)


def hash01(x: int, y: int, salt: int) -> float:
    """A lattice value in [0, 1). Integer, so it is exact on any machine."""
    h = (x * 0x1F1F_1F1F ^ y * 0x8DA6_B343 ^ salt * 0xD8163841) & 0xFFFF_FFFF
    h = (h ^ (h >> 15)) * 0x2C1B_3C6D & 0xFFFF_FFFF
    h = (h ^ (h >> 12)) * 0x2974_5B15 & 0xFFFF_FFFF
    h ^= h >> 16
    return h / 4294967296.0


def lattice(period: int, salt: int) -> list:
    """One periodic octave, as a `period x period` grid of values."""
    n = SIDE // period
    return [[hash01(x, y, salt) for x in range(n)] for y in range(n)]


def sample(grid: list, period: int, x: float, y: float) -> float:
    """Smoothstep-interpolated bilinear lookup, periodic in both axes."""
    n = len(grid)
    u, v = x / period, y / period
    x0, y0 = int(u) % n, int(v) % n
    fx, fy = u - int(u), v - int(v)
    fx = fx * fx * (3.0 - 2.0 * fx)
    fy = fy * fy * (3.0 - 2.0 * fy)
    x1, y1 = (x0 + 1) % n, (y0 + 1) % n
    top = grid[y0][x0] + (grid[y0][x1] - grid[y0][x0]) * fx
    bot = grid[y1][x0] + (grid[y1][x1] - grid[y1][x0]) * fx
    return top + (bot - top) * fy


def field() -> list:
    """The foam field, before equalisation: a domain-warped ridged fBm."""
    warp_x = [lattice(p, 0x51ED + i) for i, p in enumerate(WARP_PERIODS)]
    warp_y = [lattice(p, 0x7A11 + i) for i, p in enumerate(WARP_PERIODS)]

    def warp_at(grids, x, y):
        """Two octaves of displacement.

        One octave drags whole filaments and leaves their *lattice* aligned to
        the axes — visible as horizontal and vertical combing.  The second,
        finer octave breaks that alignment, which is the only thing here that
        does.
        """
        acc = 0.0
        for grid, period, weight in zip(grids, WARP_PERIODS, (1.0, 0.45)):
            acc += (sample(grid, period, x, y) - 0.5) * 2.0 * weight
        return acc * WARP
    octaves = [(p, lattice(p, 0x100 + i)) for i, p in enumerate(OCTAVES)]
    # `period ** PERSISTENCE`, and both ends of that exponent were rendered.
    # At 1.0 this is textbook fBm — amplitude proportional to wavelength — and
    # the coarsest octave carries half the spread, so the result is clouds with
    # the filaments buried.  At 0 it is white noise and reads as static.  This
    # is a *detail* texture sampled under a mask that already carries the
    # large-scale shape, so it wants the high-frequency end.
    weights = [p ** PERSISTENCE for p, _ in octaves]
    total = sum(weights)

    out = []
    for y in range(SIDE):
        row = []
        for x in range(SIDE):
            dx = warp_at(warp_x, x, y)
            dy = warp_at(warp_y, x, y)
            wx, wy = x + dx, y + dy
            acc = 0.0
            for (period, grid), weight in zip(octaves, weights):
                n = sample(grid, period, wx, wy)
                # The ridge. `1 - |2n - 1|` folds the octave about its own
                # mean, so a smooth hill becomes a crease — which is the
                # filament, and the one thing plain fBm cannot produce.
                acc += (1.0 - abs(n + n - 1.0)) * weight
            row.append(acc / total)
        out.append(row)
    return out


def equalise(f: list) -> bytes:
    """Rank every texel and rewrite it as its own quantile.

    Exactly uniform on [0, 1) by construction, which is the property the
    shader's threshold depends on.  Ties are broken by index, so this is a
    total order and therefore deterministic.
    """
    flat = [(f[y][x], y * SIDE + x) for y in range(SIDE) for x in range(SIDE)]
    flat.sort()
    out = bytearray(SIDE * SIDE)
    last = len(flat) - 1
    for rank, (_, i) in enumerate(flat):
        out[i] = round(rank * 255 / last)
    return bytes(out)


def png(gray: bytes) -> bytes:
    """An 8-bit greyscale PNG. `loom_asset::texture::load` widens it to RGBA."""

    def chunk(tag: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body))
            + tag
            + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFF_FFFF)
        )

    raw = b"".join(
        b"\x00" + gray[y * SIDE : (y + 1) * SIDE] for y in range(SIDE)
    )
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", SIDE, SIDE, 8, 0, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    out = args[0] if args else "assets/textures/water_foam.png"
    blob = png(equalise(field()))
    if "--check" in sys.argv:
        with open(out, "rb") as fh:
            same = fh.read() == blob
        print(f"{out}: {'reproduces' if same else 'DIFFERS'}")
        return 0 if same else 1
    with open(out, "wb") as fh:
        fh.write(blob)
    print(f"{out}: {SIDE}x{SIDE}, {len(blob)} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
