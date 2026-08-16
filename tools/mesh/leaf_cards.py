"""Emit a canopy of alpha-tested foliage cards as one OBJ.

    python3 tools/mesh/leaf_cards.py <out.obj> [--preset spruce|broadleaf] [...]

**One sprig texture, drawn a thousand times.** This is the half of a tree that
generated geometry cannot be. A photogrammetry or diffusion mesh spends
thousands of triangles on every needle clump, gives each one a unique patch of
a shared atlas, and still reads as a lumpy solid — because it *is* a lumpy
solid. Real foliage is thin, and the way to draw thin things is to cut their
shape out of a rectangle. That is what `Material::alpha_cutoff` exists for and
what `assets/test/alpha_cutout.loom` proves.

**Every card is emitted twice, with the winding reversed the second time.**
The mesh pipeline culls back faces, so a single quad vanishes when seen from
behind — a canopy you can see through from one side. Two coplanar quads facing
opposite ways cost four triangles and cannot z-fight, because for any view
direction exactly one of them survives culling.

**Placement is a seeded pure function.** Same arguments, same OBJ, byte for
byte, so a scene's geometry does not drift under it and a diff is readable.
"""
import argparse
import math
import random


def cone_sites(rng, count, height, radius, base, taper, droop, jitter):
    """Positions and outward directions on a conifer's branch envelope.

    Placed on the *surface* of the envelope rather than through its volume:
    the inside of a spruce is bare wood and shadow, and cards buried there are
    invisible from every angle while still costing their triangles.
    """
    sites = []
    for _ in range(count):
        # Biased toward the top so the crown stays dense as it narrows; a
        # uniform draw leaves the tip bald and the skirt matted.
        t = rng.random() ** 0.75
        y = base + t * (height - base)
        r = radius * (1.0 - t) ** taper
        if r <= 1e-4:
            continue
        a = rng.random() * math.tau
        rr = r * (1.0 - jitter * rng.random())
        pos = (math.cos(a) * rr, y + rng.uniform(-jitter, jitter) * height * 0.05,
               math.sin(a) * rr)
        # Outward and downward: a spruce's branchlets hang.
        out = (math.cos(a), -droop, math.sin(a))
        sites.append((pos, out, t))
    return sites


def ellipsoid_sites(rng, count, height, radius, base, taper, droop, jitter):
    """Positions on a broadleaf crown -- a squashed ellipsoid shell."""
    sites = []
    span = height - base
    for _ in range(count):
        # Cosine-weighted in elevation so the crown is fullest at its equator.
        u = rng.uniform(-1.0, 1.0)
        a = rng.random() * math.tau
        s = math.sqrt(max(0.0, 1.0 - u * u))
        rr = radius * s * (1.0 - jitter * rng.random())
        y = base + span * (0.5 + 0.5 * u)
        pos = (math.cos(a) * rr, y, math.sin(a) * rr)
        out = (math.cos(a) * s, max(u, -droop), math.sin(a) * s)
        sites.append((pos, out, 0.5 + 0.5 * u))
    return sites


def normalise(v):
    n = math.sqrt(sum(c * c for c in v))
    return (v[0] / n, v[1] / n, v[2] / n) if n > 1e-9 else (1.0, 0.0, 0.0)


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0])


def card(pos, out, size, roll):
    """Four corners of one quad, facing `out`, rolled about it."""
    f = normalise(out)
    up = (0.0, 1.0, 0.0) if abs(f[1]) < 0.95 else (1.0, 0.0, 0.0)
    right = normalise(cross(up, f))
    real_up = cross(f, right)
    c, s = math.cos(roll), math.sin(roll)
    u = tuple(right[i] * c + real_up[i] * s for i in range(3))
    v = tuple(real_up[i] * c - right[i] * s for i in range(3))
    h = size * 0.5
    return [
        tuple(pos[i] - u[i] * h - v[i] * h for i in range(3)),
        tuple(pos[i] + u[i] * h - v[i] * h for i in range(3)),
        tuple(pos[i] + u[i] * h + v[i] * h for i in range(3)),
        tuple(pos[i] - u[i] * h + v[i] * h for i in range(3)),
    ]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--preset", choices=["spruce", "broadleaf"], default="spruce")
    ap.add_argument("--count", type=int, default=600)
    ap.add_argument("--height", type=float, default=1.0,
                    help="canopy top, in the same units the scene scales")
    ap.add_argument("--base", type=float, default=0.12, help="lowest foliage")
    ap.add_argument("--radius", type=float, default=0.42)
    ap.add_argument("--taper", type=float, default=1.0, help="cone sharpness")
    ap.add_argument("--size", type=float, default=0.20, help="card edge length")
    ap.add_argument("--size-jitter", type=float, default=0.35)
    ap.add_argument("--droop", type=float, default=0.35)
    ap.add_argument("--jitter", type=float, default=0.30)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--name", default="Canopy")
    args = ap.parse_args()

    rng = random.Random(args.seed)
    place = cone_sites if args.preset == "spruce" else ellipsoid_sites
    sites = place(rng, args.count, args.height, args.radius, args.base,
                  args.taper, args.droop, args.jitter)

    verts, uvs, faces = [], [], []
    # One shared UV square: every card samples the whole sprite. Reusing the
    # same four texture coordinates is the point -- the sprig is one texture
    # drawn many times, not an atlas of unique patches.
    uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]

    for pos, out, t in sites:
        size = args.size * (1.0 - args.size_jitter * rng.random())
        # Smaller toward the tip, which is what makes a crown read as tapering
        # rather than as the same clump scaled down bodily.
        size *= 0.55 + 0.45 * (1.0 - t)
        quad = card(pos, out, size, rng.random() * math.tau)
        n = len(verts)
        verts.extend(quad)
        faces.append((n + 1, n + 2, n + 3))
        faces.append((n + 1, n + 3, n + 4))
        # The same quad wound the other way: back faces are culled, so without
        # this the canopy is transparent from one side.
        faces.append((n + 1, n + 3, n + 2))
        faces.append((n + 1, n + 4, n + 3))

    with open(args.out, "w", encoding="utf-8") as fh:
        fh.write(f"# {args.count} foliage cards, preset {args.preset}, "
                 f"seed {args.seed}. Generated by tools/mesh/leaf_cards.py\n")
        fh.write(f"o {args.name}\n")
        for v in verts:
            fh.write(f"v {v[0]:.5f} {v[1]:.5f} {v[2]:.5f}\n")
        for u in uvs:
            fh.write(f"vt {u[0]:.4f} {u[1]:.4f}\n")
        for f in faces:
            # UV corners cycle with the quad corners; both triangles of a quad
            # take the corners they actually own.
            a, b, c = f
            ta = (a - 1) % 4 + 1
            tb = (b - 1) % 4 + 1
            tc = (c - 1) % 4 + 1
            fh.write(f"f {a}/{ta} {b}/{tb} {c}/{tc}\n")

    print(f"LEAF-CARDS {args.out} cards={len(sites)} verts={len(verts)} "
          f"tris={len(faces)} preset={args.preset} seed={args.seed}")


if __name__ == "__main__":
    main()
