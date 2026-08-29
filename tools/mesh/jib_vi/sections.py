"""JIB VI's hull, measured: watertightness, displacement, and the section curve.

Read-only. This tool changes nothing — it prints a table and a verdict, and the
verdict is the point. `SEA-REBUILD.md` §2.1 records two irreconcilable
displacements for one hull (57.636 m^3 from `jib_vi_wide.obj`, 60.340 from
summing the shipped per-material shells) and says the second is *an upper bound
rather than a measurement*, because a sum of shells assumes their union is
closed and non-overlapping. `mass` gets set from whichever is real, so the
guess propagates straight into the physics.

Three independent measurements, deliberately overlapping so they can disagree:

1. **Exact, per mesh.** Volume below a plane by clipping every triangle at it
   and summing tetrahedra about a point ON the plane. The plane cap is then
   coplanar with the apex and contributes exactly zero, so no cap has to be
   triangulated. Exact for a closed shell; meaningless for an open one, which
   is why (2) runs first.
2. **Watertightness, per mesh.** Every undirected edge of a closed,
   consistently-wound surface carries exactly two half-edges in opposite
   directions. Counted on POSITION-WELDED indices, not raw OBJ indices: an
   exporter may split a vertex for normals and leave the geometry closed, and
   the raw count would call that a hole.
3. **The union, by stabbing.** A vertical ray per column of a fine (x, z) grid,
   crossings sorted, parity from y = -inf giving each mesh's solid intervals in
   that column. Union the intervals ACROSS meshes and the double-count cannot
   survive; sum them per mesh instead and it is reproduced. The gap between
   those two totals is the overlap, measured rather than assumed — and no mesh
   boolean is needed to get it. Discretisation error is bounded by calibrating
   the same pass against (1) on a mesh (1) is entitled to measure.

The section curve falls out of the same clip as (1): cumulative volume forward
of station x, taken about the point (x, h, 0), which lies on BOTH cut planes
(y = h and x = x) so both caps vanish together. Differencing consecutive
stations gives each station's mean immersed section, and the areas therefore
sum back to the displacement exactly, by construction rather than by luck.

    python3 tools/mesh/jib_vi/sections.py            # the report
    python3 tools/mesh/jib_vi/sections.py --self     # the self-check only
"""
import math
import os
import sys
from collections import Counter, defaultdict

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "rig"))
import rigkit  # noqa: E402

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                     "..", "..", ".."))
MESHDIR = os.path.join(ROOT, "assets", "meshes")

# The scene's own numbers, quoted so the report can be read without the file
# open. Every one of them is re-read from disk below and asserted, because a
# number quoted from a configuration that has since been replaced is this
# branch's most common defect.
SCENE = os.path.join(ROOT, "assets", "test", "jib_vi_float.loom")
DESIGNED_Y = 0.0        # the designed waterline: the model's own origin
WELD = 1e-4             # position weld tolerance, metres (OBJ ships 6 dp)

# Apexes for the origin-independence test of section 2. Spread out, and none of
# them at the model origin, because a degenerate apex can be accidentally right.
ORIGINS = [(0.0, 0.0, 0.0), (200.0, 0.0, 0.0), (0.0, 0.0, 200.0),
           (5000.0, 0.0, -5000.0)]


# ---------------------------------------------------------------- geometry

def tri_coords(d):
    v = d["verts"]
    return [(v[a], v[b], v[c]) for a, b, c in d["tris"]]


def clip_poly(poly, axis, limit):
    """Sutherland-Hodgman against the half-space `coord[axis] <= limit`."""
    out = []
    n = len(poly)
    for i in range(n):
        a, b = poly[i], poly[(i + 1) % n]
        av, bv = a[axis] - limit, b[axis] - limit
        if av <= 0.0:
            out.append(a)
        if (av <= 0.0) != (bv <= 0.0):
            t = av / (av - bv)
            out.append(tuple(a[k] + t * (b[k] - a[k]) for k in range(3)))
    return out


def clipped_volume(tris, planes, origin):
    """Volume of the clipped solid, as tetrahedra about `origin`.

    `planes` is a list of (axis, limit) half-spaces `coord[axis] <= limit`.
    **`origin` must lie on every cut plane** or the answer is wrong: what makes
    the missing caps free is that a cap coplanar with the apex has zero height.
    """
    ox, oy, oz = origin
    total = 0.0
    for t in tris:
        poly = list(t)
        for axis, limit in planes:
            poly = clip_poly(poly, axis, limit)
            if len(poly) < 3:
                break
        if len(poly) < 3:
            continue
        p0 = poly[0]
        ax, ay, az = p0[0] - ox, p0[1] - oy, p0[2] - oz
        for k in range(1, len(poly) - 1):
            p1, p2 = poly[k], poly[k + 1]
            bx, by, bz = p1[0] - ox, p1[1] - oy, p1[2] - oz
            cx, cy, cz = p2[0] - ox, p2[1] - oy, p2[2] - oz
            total += (ax * (by * cz - bz * cy)
                      - ay * (bx * cz - bz * cx)
                      + az * (bx * cy - by * cx))
    return total / 6.0


def waterline_extent(tris, h):
    """(Lwl, Bwl, y_lo) of the plane y = h, exactly, from the plane section.

    The grid in `Columns` samples cell centres, so its extent is short by up to
    a cell in each direction; principal dimensions are quoted to two decimals
    and deserve the exact cut. Every triangle straddling the plane contributes
    one segment; the extent of those endpoints is the waterline.
    """
    xs, zs, ys = [], [], []
    for t in tris:
        ys.extend(p[1] for p in t if p[1] <= h)
        if min(p[1] for p in t) >= h or max(p[1] for p in t) <= h:
            continue
        for i in range(3):
            a, b = t[i], t[(i + 1) % 3]
            if (a[1] - h) * (b[1] - h) >= 0.0:
                continue
            u = (h - a[1]) / (b[1] - a[1])
            xs.append(a[0] + u * (b[0] - a[0]))
            zs.append(a[2] + u * (b[2] - a[2]))
    if not xs:
        return 0.0, 0.0, h
    return max(xs) - min(xs), max(zs) - min(zs), min(ys)


def volume_below(tris, h):
    """Exact displaced volume below the plane y = h. Closed shells only."""
    return clipped_volume(tris, [(1, h)], (0.0, h, 0.0))


def stations(tris, h, x_lo, x_hi, n):
    """Mean immersed section per station: (edges, areas, cumulative volumes).

    `areas[i]` is station i's volume divided by its own width, so
    `sum(areas) * dx` is the displacement exactly — the areas are a
    decomposition of the volume, not samples of a curve through it.
    """
    edges = [x_lo + (x_hi - x_lo) * i / n for i in range(n + 1)]
    dx = (x_hi - x_lo) / n
    cum = [clipped_volume(tris, [(1, h), (0, x)], (x, h, 0.0)) for x in edges]
    return edges, [(cum[i + 1] - cum[i]) / dx for i in range(n)], cum


# ------------------------------------------------------- watertightness

def weld(d):
    """Vertex indices remapped so coincident positions share one index."""
    key = {}
    remap = []
    for p in d["verts"]:
        k = tuple(round(c / WELD) for c in p)
        remap.append(key.setdefault(k, len(key)))
    return remap


def edge_report(d):
    """(boundary, inconsistent, non-manifold) undirected edge counts, welded."""
    r = weld(d)
    half = Counter()
    for a, b, c in d["tris"]:
        a, b, c = r[a], r[b], r[c]
        for e in ((a, b), (b, c), (c, a)):
            half[e] += 1
    boundary = inconsistent = nonmanifold = 0
    for (a, b), n in half.items():
        if a > b:
            continue
        m = half.get((b, a), 0)
        if n == 1 and m == 1:
            continue
        if n + m == 1:
            boundary += 1
        elif n + m == 2:
            inconsistent += 1      # two half-edges, same direction
        else:
            nonmanifold += 1
    return boundary, inconsistent, nonmanifold


# --------------------------------------------------------- the union, by ray

class Columns:
    """One vertical stab per (x, z) cell centre, per mesh, intervals kept.

    Everything the union question needs comes out of one pass: per-mesh
    volume, union volume, station areas and waterplane area, at ANY height,
    because the intervals are stored rather than an integral of them.
    """

    def __init__(self, meshes, cell, y_top, bucket=0.5):
        self.cell, self.y_top = cell, y_top
        self.names = [n for n, _ in meshes]
        lo = [min(m["lo"][k] for _, m in meshes) for k in range(3)]
        hi = [max(m["hi"][k] for _, m in meshes) for k in range(3)]
        self.lo, self.hi = lo, hi

        index = {}
        for name, m in meshes:
            grid = defaultdict(list)
            for t in m["tris"]:
                if min(p[1] for p in t) > y_top:
                    continue            # cannot be crossed below y_top
                xs = [p[0] for p in t]
                zs = [p[2] for p in t]
                for i in range(int(math.floor(min(xs) / bucket)),
                               int(math.floor(max(xs) / bucket)) + 1):
                    for j in range(int(math.floor(min(zs) / bucket)),
                                   int(math.floor(max(zs) / bucket)) + 1):
                        grid[(i, j)].append(t)
            index[name] = grid

        nx = int(math.ceil((hi[0] - lo[0]) / cell))
        nz = int(math.ceil((hi[2] - lo[2]) / cell))
        self.cols = []          # (x, z, {name: [(y0, y1), ...]}, [union spans])
        self.odd = 0
        for i in range(nx):
            # The two offsets are near 0.5 but NOT equal, and neither is 0.5.
            # A sample landing exactly on an edge shared by two triangles is
            # counted twice, and parity stays even while the intervals go
            # wrong — silent. A hull built on a regular lattice puts a great
            # many edges on round numbers, and splitting a quad puts its
            # diagonal on x = z, which equal offsets hit in every column of
            # the diagonal. `demo()` covers exactly that case; section 3
            # calibrates the residue against the exact integral.
            px = lo[0] + (i + 0.5000001) * cell
            for j in range(nz):
                pz = lo[2] + (j + 0.5000003) * cell
                per, allspans = {}, []
                for name, m in meshes:
                    spans = self._stab(index[name].get(
                        (int(math.floor(px / bucket)), int(math.floor(pz / bucket))), ()),
                        px, pz)
                    if spans:
                        per[name] = spans
                        allspans.extend(spans)
                if allspans:
                    self.cols.append((px, pz, per, merge(allspans)))

    def _stab(self, tris, px, pz):
        ys = []
        for (ax, ay, az), (bx, by, bz), (cx, cy, cz) in tris:
            e1x, e1z = bx - ax, bz - az
            e2x, e2z = cx - ax, cz - az
            det = e1x * e2z - e1z * e2x
            if det == 0.0:
                continue            # vertical triangle: zero-measure in (x, z)
            ux, uz = px - ax, pz - az
            l1 = (ux * e2z - uz * e2x) / det
            l2 = (e1x * uz - e1z * ux) / det
            if l1 < 0.0 or l2 < 0.0 or l1 + l2 > 1.0:
                continue
            y = ay + l1 * (by - ay) + l2 * (cy - ay)
            if y <= self.y_top:
                ys.append(y)
        if not ys:
            return []
        ys.sort()
        if len(ys) % 2:
            # A shell that straddles y_top has an odd count below it. The solid
            # continues upward, so close it at the clip — correct, not a patch.
            ys.append(self.y_top)
            self.odd += 1
        return [(ys[k], ys[k + 1]) for k in range(0, len(ys), 2)]

    def volume(self, h, name=None):
        a = self.cell * self.cell
        tot = 0.0
        for _, _, per, uni in self.cols:
            spans = uni if name is None else per.get(name, ())
            for y0, y1 in spans:
                if y0 < h:
                    tot += min(y1, h) - y0
        return tot * a

    def waterplane(self, h):
        a = self.cell * self.cell
        return a * sum(1 for _, _, _, uni in self.cols
                       if any(y0 <= h < y1 for y0, y1 in uni))

def merge(spans):
    out = []
    for y0, y1 in sorted(spans):
        if out and y0 <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], y1))
        else:
            out.append((y0, y1))
    return out


def solve_draft(f, target, lo, hi, tol=1e-6):
    """Bisect the monotone displacement curve for the height holding `target`."""
    for _ in range(200):
        mid = 0.5 * (lo + hi)
        if f(mid) < target:
            lo = mid
        else:
            hi = mid
        if hi - lo < tol:
            break
    return 0.5 * (lo + hi)


# ------------------------------------------------------------------ report

def load(name):
    d = rigkit.read_obj(os.path.join(MESHDIR, name))
    lo, hi = rigkit.bounds(d)
    return {"name": name, "d": d, "tris": tri_coords(d), "lo": lo, "hi": hi,
            "shells": rigkit.shells(d), "edges": edge_report(d)}


def combine(meshes):
    """One triangle soup from several meshes. Tetrahedron sums are LINEAR in
    triangles, so the combined integral is exactly the sum of the parts — which
    is what makes §2.1's `union` row an addition rather than a boolean, and
    what makes the origin test below able to judge the whole set at once."""
    verts, tris = [], []
    for m in meshes:
        off = len(verts)
        verts.extend(m["d"]["verts"])
        tris.extend((a + off, b + off, c + off) for a, b, c in m["d"]["tris"])
    d = {"verts": verts, "tris": tris}
    lo, hi = rigkit.bounds(d)
    return {"name": "+".join(m["name"] for m in meshes), "d": d,
            "tris": tri_coords(d), "lo": lo, "hi": hi,
            "shells": None, "edges": edge_report(d)}


def origin_spread(tris, h=DESIGNED_Y, origins=ORIGINS):
    """The watertightness test that actually matters, and the strongest one.

    For a surface that CLOSES at the plane `y = h`, the clipped tetrahedron sum
    is exactly translation-invariant — the divergence theorem, and rigkit's own
    `signed_volume` docstring records it measured at 1000 m. So sweeping the
    apex around inside that plane and watching the answer is a direct test of
    the only property the integral needs. It is stronger than counting boundary
    edges, because it weights a hole by how much volume the hole loses rather
    than by how many edges bound it, and it cannot be fooled by a mesh whose
    holes are numerous and tiny.

    Returns (value at origins[0], spread). A spread of 0.00000 over apexes
    kilometres apart is proof; anything else means the figure is not a volume.
    """
    vs = [clipped_volume(tris, [(1, h)], o) for o in origins]
    return vs[0], max(vs) - min(vs)


def below_split(m, h=DESIGNED_Y):
    """(from closed shells, from open shells, list of the notable shells)."""
    closed = open_ = 0.0
    rows = []
    for tris in m["shells"]:
        sub = {"verts": m["d"]["verts"], "tris": tris}
        v = volume_below(tri_coords(sub), h)
        if abs(v) < 1e-9:
            continue
        er = edge_report(sub)
        lo, hi = rigkit.bounds(m["d"], tris)
        rows.append((v, er, len(tris), lo, hi))
        if er == (0, 0, 0):
            closed += v
        else:
            open_ += v
    rows.sort(key=lambda r: -abs(r[0]))
    return closed, open_, rows


def scene_numbers():
    """Re-read mass and density from the scene rather than quoting them."""
    mass = density = None
    for line in open(SCENE):
        s = line.strip()
        if s.startswith("mass") and mass is None:
            mass = float(s.split("=")[1])
        elif s.startswith("density") and density is None:
            density = float(s.split("=")[1])
    return mass, density


def read_pontoons():
    """(x, centre_y, radius) for every pontoon in the float scene, from the file.

    Parsed rather than quoted: SEA-REBUILD §2.1's own headline defect is a
    number carried forward from a configuration that had been replaced.
    """
    out, off = [], None
    for line in open(SCENE):
        s = line.strip()
        if s.startswith("offset"):
            off = [float(v) for v in s.split("[")[1].split("]")[0].split(",")]
        elif s.startswith("radius") and off is not None:
            out.append((off[0], off[1], float(s.split("=")[1])))
            off = None
    return out


def cap_volume(r, centre_y, surface_y):
    """The engine's own spherical cap — loom_water::buoyancy::submerged_volume."""
    d = min(max(surface_y - (centre_y - r), 0.0), 2.0 * r)
    return math.pi * d * d * (3.0 * r - d) / 3.0


def rule(t):
    print()
    print("== %s ==" % t)
    print()


def report():
    names = sorted(n for n in os.listdir(MESHDIR)
                   if n.startswith("jib_vi_") and n.endswith(".obj"))
    wide = load("jib_vi_wide.obj")
    mats = [load(n) for n in names if n != "jib_vi_wide.obj"]
    by = {m["name"]: m for m in mats}
    mass, density = scene_numbers()
    target = mass / density

    print("JIB VI — the hull, measured.  Volumes m^3, heights metres; y = 0 is the")
    print("designed waterline and the model's own origin. Read-only: this tool")
    print("changes nothing and is not on any gate.")
    print()
    print("mass = %.1f kg and WaterBody.density = %.1f kg/m^3, both re-read from"
          % (mass, density))
    print("%s, so the boat is asked to displace %.3f m^3."
          % (os.path.relpath(SCENE, ROOT), target))

    rule("1. Watertightness, per mesh")
    print("Edge counts on POSITION-WELDED indices (tolerance %g m), because an" % WELD)
    print("exporter may split a vertex for normals and leave the geometry closed.")
    print("`below y=0` is split by whether the shell carrying it is closed.")
    print()
    print("%-24s %6s %6s %7s %6s %6s %6s   %9s %9s"
          % ("mesh", "tris", "shells", "closed", "bound", "wound", "nonmf",
             "closed<0", "open<0"))
    wet = []
    for m in [wide] + mats:
        b, w, nm = m["edges"]
        nclosed = sum(1 for t in m["shells"]
                      if edge_report({"verts": m["d"]["verts"], "tris": t}) == (0, 0, 0))
        c, o, rows = below_split(m)
        m["below"] = (c, o, rows)
        if abs(c) + abs(o) > 1e-9:
            wet.append(m)
        print("%-24s %6d %6d %7d %6d %6d %6d   %9.4f %9.4f"
              % (m["name"], len(m["tris"]), len(m["shells"]), nclosed, b, w, nm,
                 c, o))
    print()
    print("**Not one mesh in the set is closed, `jib_vi_wide.obj` included.**")
    print("§2.1's premise that `jib_vi_wide.obj` is watertight on its own is not")
    print("true as written; nothing ever asserted it. `rigkit.verify_obj` checks")
    print("that each shell's signed volume is POSITIVE — that its normals point")
    print("out — and never that it is closed, so an open shell has always passed.")
    print()
    print("Almost all of the openness is above the waterline (railings, canopy,")
    print("window reveals: open sheets, correctly). What matters is the surface")
    print("BELOW y = 0, and section 2 tests exactly that.")

    rule("2. Which number is real")
    print("The test: for a surface that closes at the plane y = 0, the clipped")
    print("tetrahedron sum is exactly independent of the apex it is summed about.")
    print("Sweep the apex around inside the plane, out to kilometres, and watch.")
    print("Zero spread is proof the figure is a volume; a large spread is proof")
    print("it is not. Apexes used: %s."
          % ", ".join("(%g, 0, %g)" % (o[0], o[2]) for o in ORIGINS))
    print()
    print("%-34s %11s %12s" % ("set", "below y=0", "apex spread"))
    hull_shell = max(wide["shells"], key=len)
    cases = [
        ("jib_vi_wide.obj (whole file)", wide["tris"]),
        ("jib_vi_wide.obj hull shell only",
         tri_coords({"verts": wide["d"]["verts"], "tris": hull_shell})),
        ("jib_vi_antifoul.obj", by["jib_vi_antifoul.obj"]["tris"]),
        ("jib_vi_hull_white.obj", by["jib_vi_hull_white.obj"]["tris"]),
        ("jib_vi_grime.obj", by["jib_vi_grime.obj"]["tris"]),
        ("jib_vi_metal.obj", by["jib_vi_metal.obj"]["tris"]),
        ("antifoul + hull_white",
         combine([by["jib_vi_antifoul.obj"], by["jib_vi_hull_white.obj"]])["tris"]),
        ("antifoul + hull_white + metal",
         combine([by["jib_vi_antifoul.obj"], by["jib_vi_hull_white.obj"],
                  by["jib_vi_metal.obj"]])["tris"]),
        ("the whole material set", combine(mats)["tris"]),
    ]
    vals = {}
    for label, tris in cases:
        v, sp = origin_spread(tris)
        vals[label] = v
        print("%-34s %11.4f %12.5f" % (label, v, sp))
    print()
    print("**57.636 is real. 60.340 is not a measurement, and neither is any of")
    print("its four terms except `metal`.**")
    print()
    print("`jib_vi_wide.obj` is the ONLY candidate whose submerged integral is")
    print("exactly apex-independent — 0.00000 m^3 of spread over apexes 10 km")
    print("apart. Its surface below the waterline closes, whatever its railings")
    print("do above. `jib_vi_metal.obj` is the only other one: eight closed")
    print("appendage shells at y -1.65..-0.91, the skeg and running gear.")
    print()
    print("§2.1's four terms are individually meaningless. `antifoul` and")
    print("`hull_white` are two halves of ONE hull skin split at the paint line,")
    print("and the tell is that their spreads are equal and opposite — each is")
    print("open exactly where the other continues. Summed, the spread collapses")
    print("from ~3700 to 4.78; they nearly close each other and do not quite.")
    print()
    print("**The whole 2.70 m^3 is `grime`, and `grime` is two open strips.**")
    for v, er, n, lo, hi in by["jib_vi_grime.obj"]["below"][2]:
        print("    %8.4f m^3 from %d tris with %d boundary edges, y %.3f..%.3f"
              % (v, n, er[0], lo[1], hi[1]))
    print("Each is a streak decal lying ON the hull, spanning 9.3 cm below the")
    print("line. Its tetrahedron sum is the cone from the apex to an open strip,")
    print("which is not a volume and moves by hundreds of m^3 when the apex does.")
    print()
    print("And the two representations agree, which is the positive half of the")
    print("case rather than the negative one:")
    print()
    print("    material hull skin  (antifoul + hull_white)   %10.4f"
          % vals["antifoul + hull_white"])
    print("    + appendages        (metal, 8 closed shells)   %10.4f"
          % vals["jib_vi_metal.obj"])
    print("    = the shipped hull                             %10.4f"
          % vals["antifoul + hull_white + metal"])
    print("    jib_vi_wide.obj, an independent single mesh     %10.4f"
          % vals["jib_vi_wide.obj (whole file)"])
    print("    difference                                     %10.4f  (%.3f%%)"
          % (vals["jib_vi_wide.obj (whole file)"] - vals["antifoul + hull_white + metal"],
             100.0 * abs(vals["jib_vi_wide.obj (whole file)"]
                         - vals["antifoul + hull_white + metal"])
             / vals["jib_vi_wide.obj (whole file)"]))
    print()
    print("Two differently-split surfaces integrating to the same solid to within")
    print("0.02%. The material set does NOT double-count the hull; it adds one")
    print("thing that is not a hull.")

    rule("3. The same number by a method that shares no assumption")
    print("A vertical ray down every column of an (x, z) grid; crossings sorted;")
    print("parity from y = -inf gives the solid intervals. It knows nothing about")
    print("winding or the divergence theorem, so it fails differently.")
    print()
    print("%8s %12s %12s %10s" % ("cell (m)", "columns", "wide", "err vs exact"))
    exact = vals["jib_vi_wide.obj (whole file)"]
    cols = None
    for cell in (0.10, 0.05, 0.025):
        cols = Columns([("wide", wide)], cell, 0.5)
        v = cols.volume(DESIGNED_Y)
        print("%8.3f %12d %12.4f %10.4f" % (cell, len(cols.cols), v, v - exact))
    print()
    print("Column sampling error is quasi-random in the cell size rather than")
    print("monotone — a cell either straddles the hull's edge or does not — so")
    print("read the magnitudes, not a trend. All three land inside 0.11% of the")
    print("exact figure, from an algorithm sharing none of its assumptions.")

    rule("4. The three numbers")
    tris = wide["tris"]
    f = lambda h: volume_below(tris, h)
    boot = by["jib_vi_antifoul.obj"]["hi"][1]
    at_design, at_boot = f(DESIGNED_Y), f(boot)
    h_mass = solve_draft(f, target, wide["lo"][1], wide["hi"][1])
    print("Hull: jib_vi_wide.obj — the one mesh whose submerged integral passes")
    print("section 2, and the mesh jib_vi_float.loom actually renders.")
    print()
    print("%-46s %8s %12s %12s" % ("", "y", "m^3", "kg at rho"))
    print("%-46s %+8.3f %12.3f %12.1f"
          % ("displacement at the designed line", DESIGNED_Y, at_design,
             at_design * density))
    print("%-46s %+8.3f %12.3f %12.1f"
          % ("where mass = %.0f kg floats it today" % mass, h_mass, f(h_mass), mass))
    print("%-46s %+8.3f %12.3f %12.1f"
          % ("the painted line (antifoul tops out here)", boot, at_boot,
             at_boot * density))
    print()
    print("  mass is %.1f%% of the hull's displacement at the designed line;"
          % (100.0 * target / at_design))
    print("  the hull is %.1f%% light, %.0f kg short of its own lines."
          % (100.0 * (1.0 - target / at_design), at_design * density - mass))
    print("  the hull's own float sits %.3f m %s the paint it should meet."
          % (abs(h_mass - boot), "above" if h_mass > boot else "BELOW"))
    print("  the pontoon proxy meanwhile settles at y = +0.024 (SEA-REBUILD §2.1,")
    print("  measured by `loom sim`), so the proxy and the hull disagree by")
    print("  %.3f m at the same mass, in opposite directions about the paint."
          % (0.024 - h_mass))
    print("  waterplane area at the designed line: %.2f m^2 (grid 0.025 m)."
          % cols.waterplane(DESIGNED_Y))
    print("  to float at the painted line, mass must be %.0f kg." % (at_boot * density))
    print()
    hull_only = tri_coords({"verts": wide["d"]["verts"], "tris": hull_shell})
    lwl, bwl, y_lo = waterline_extent(hull_only, DESIGNED_Y)
    draft = DESIGNED_Y - y_lo
    print("  Sanity, because 57.6 t is the number everything downstream inherits.")
    print("  Principal dimensions of the hull shell at the designed line, cut")
    print("  exactly at the plane rather than sampled:")
    print("    Lwl %.2f m   Bwl %.2f m   moulded draft %.3f m   Aw %.2f m^2"
          % (lwl, bwl, draft, cols.waterplane(DESIGNED_Y)))
    print("    block coefficient Cb = V / (Lwl.Bwl.T) = %.3f"
          % (at_design / (lwl * bwl * draft)))
    print("  0.4-0.5 is an ordinary semi-displacement hull, and 57.6 t is an")
    print("  ordinary weight for a 19 m charter boat. The figure is credible as")
    print("  well as measured.")
    print()
    print("  Lwl is %.2f m against the 19.07 m the BoxCollider is sized on: the"
          % lwl)
    print("  stem rakes %.2f m forward from forefoot to stemhead, so a fifth of"
          % (wide["hi"][0] - (lwl + wide["lo"][0])))
    print("  the boat's length is overhang and carries no buoyancy at all.")

    rule("5. The sectional area curve")
    n, x_lo, x_hi = 20, wide["lo"][0], wide["hi"][0]
    dx = (x_hi - x_lo) / n
    heights = [-1.60, -1.20, -0.80, -0.40, boot, 0.0, 0.20]
    edges = stations(tris, DESIGNED_Y, x_lo, x_hi, n)[0]
    rows = [stations(tris, h, x_lo, x_hi, n)[1] for h in heights]
    print("Mean immersed section per station, m^2. Station i spans")
    print("x = edges[i]..edges[i+1] (dx = %.4f m); each area is that station's" % dx)
    print("own volume over its own width, so `sum(area) * dx` is the displacement")
    print("at that waterline EXACTLY rather than approximately. Bow is +X.")
    print()
    print("%3s %8s %8s %s" % ("st", "x_lo", "x_hi",
                              "".join("%8.2f" % h for h in heights)))
    for i in range(n):
        print("%3d %8.3f %8.3f %s"
              % (i, edges[i], edges[i + 1],
                 "".join("%8.3f" % (r[i] if abs(r[i]) > 5e-4 else 0.0)
                                 for r in rows)))
    print("%3s %8s %8s %s" % ("V", "m^3", "",
                              "".join("%8.3f" % (sum(r) * dx) for r in rows)))
    print("%3s %8s %8s %s" % ("Aw", "m^2", "",
                              "".join("%8.2f" % cols.waterplane(h) for h in heights)))
    print()
    print("The V row is sum(area) x dx: %.3f at y = 0 against section 2's %.3f."
          % (sum(rows[heights.index(0.0)]) * dx, at_design))
    print("Aw is the waterplane area from the ray grid, a third method again.")

    rule("6. The proxy against the hull, station by station")
    print("The twelve spheres of jib_vi_float.loom, binned into the same")
    print("stations and evaluated at y = 0 with the engine's own spherical cap")
    print("(loom_water::buoyancy::submerged_volume), against the hull's own")
    print("volume in that station. This is §6.1's second consumer, and it is")
    print("what a `stations` model would replace.")
    print()
    pontoons = read_pontoons()
    hull_v = [a * dx for a in rows[heights.index(DESIGNED_Y)]]
    prox = [0.0] * n
    for px, cy, r in pontoons:
        i = min(n - 1, max(0, int((px - x_lo) / dx)))
        prox[i] += cap_volume(r, cy, DESIGNED_Y)
    print("%3s %8s %8s %9s %9s %9s" % ("st", "x_lo", "x_hi", "hull", "proxy", "error"))
    for i in range(n):
        if hull_v[i] < 1e-6 and prox[i] < 1e-6:
            continue
        print("%3d %8.3f %8.3f %9.3f %9.3f %+9.3f"
              % (i, edges[i], edges[i + 1], hull_v[i], prox[i], prox[i] - hull_v[i]))
    print("%3s %8s %8s %9.3f %9.3f %+9.3f"
          % ("tot", "", "", sum(hull_v), sum(prox), sum(prox) - sum(hull_v)))


# ------------------------------------------------------------- self-check

def box(lo, hi):
    x0, y0, z0 = lo
    x1, y1, z1 = hi
    v = [(x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0),
         (x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)]
    f = [(4, 5, 6), (4, 6, 7), (1, 0, 3), (1, 3, 2), (5, 1, 2), (5, 2, 6),
         (0, 4, 7), (0, 7, 3), (3, 7, 6), (3, 6, 2), (0, 1, 5), (0, 5, 4)]
    d = {"verts": v, "tris": f, "uvs": [], "corners": [], "faces": [], "tags": []}
    return {"name": "box", "d": d, "tris": tri_coords(d), "lo": lo, "hi": hi,
            "shells": 1, "edges": edge_report(d)}


def demo():
    """One runnable check per non-trivial routine. Asserts, no framework."""
    b = box((0.0, 0.0, 0.0), (2.0, 2.0, 2.0))        # 8 m^3
    assert abs(rigkit.signed_volume(b["d"]) - 8.0) < 1e-9
    assert b["edges"] == (0, 0, 0), b["edges"]
    assert abs(volume_below(b["tris"], 2.0) - 8.0) < 1e-9
    assert abs(volume_below(b["tris"], 0.5) - 2.0) < 1e-9
    assert abs(volume_below(b["tris"], -1.0)) < 1e-12

    # a wedge, so the section curve is not constant and a station bug shows.
    # Triangular in (x, y), extruded 2 m in z: volume 8, section tapering
    # linearly 4 -> 0 with +x.
    w = {"verts": [(0.0, 0.0, 0.0), (4.0, 0.0, 0.0), (4.0, 0.0, 2.0),
                   (0.0, 0.0, 2.0), (0.0, 2.0, 0.0), (0.0, 2.0, 2.0)],
         "tris": [(0, 1, 2), (0, 2, 3), (0, 3, 5), (0, 5, 4),
                  (1, 5, 2), (1, 4, 5), (0, 4, 1), (3, 2, 5)]}
    wt = tri_coords(w)
    assert edge_report(w) == (0, 0, 0), edge_report(w)
    assert abs(rigkit.signed_volume(w) - 8.0) < 1e-9, rigkit.signed_volume(w)
    assert abs(volume_below(wt, 1.0) - 6.0) < 1e-9, volume_below(wt, 1.0)
    e, a, _ = stations(wt, 2.0, 0.0, 4.0, 4)
    assert abs(sum(a) * 1.0 - 8.0) < 1e-9, (a, sum(a))
    assert [round(x, 6) for x in a] == [3.5, 2.5, 1.5, 0.5], a

    # the union: two 2 m boxes overlapping in a 1 m slab. Sum 16, union 12.
    a1, a2 = box((0.0, 0.0, 0.0), (2.0, 2.0, 2.0)), box((1.0, 0.0, 0.0), (3.0, 2.0, 2.0))
    a2["name"] = "b2"
    c = Columns([("b1", a1), ("b2", a2)], 0.05, 3.0)
    assert abs(c.volume(2.0, "b1") - 8.0) < 1e-9, c.volume(2.0, "b1")
    assert abs(c.volume(2.0) - 12.0) < 1e-9, c.volume(2.0)
    assert abs(c.waterplane(1.0) - 6.0) < 1e-9, c.waterplane(1.0)
    _, sa, _ = stations(a1["tris"], 2.0, 0.0, 2.0, 4)
    assert all(abs(x - 4.0) < 1e-9 for x in sa), sa

    # bisection finds the half-full draft of the 2 m box
    h = solve_draft(lambda y: volume_below(a1["tris"], y), 4.0, 0.0, 2.0)
    assert abs(h - 1.0) < 1e-5, h

    # an OPEN shell: drop one face and the audit must say so
    o = {"verts": b["d"]["verts"], "tris": b["d"]["tris"][:-1]}
    assert edge_report(o)[0] > 0, edge_report(o)
    # the engine's cap: a sphere exactly half under displaces half of it
    assert abs(cap_volume(1.0, 0.0, 0.0) - 2.0 / 3.0 * math.pi) < 1e-12
    assert cap_volume(1.0, 0.0, -2.0) == 0.0
    assert abs(cap_volume(1.0, 0.0, 5.0) - 4.0 / 3.0 * math.pi) < 1e-12

    # the scene's twelve, read from the file, must reproduce its own header
    # arithmetic: 12 x 3.648 = 43.78 m^3 with the waterline at y = 0.
    p = read_pontoons()
    assert len(p) == 12, len(p)
    tot = sum(cap_volume(r, cy, 0.0) for _, cy, r in p)
    assert abs(tot - 43.78) < 0.05, tot

    print("self-check ok")


if __name__ == "__main__":
    demo()
    if "--self" not in sys.argv:
        print()
        report()
