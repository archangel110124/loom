# tools/mesh/rig/build_roof.py
"""The shed's roof — corrugated sheet with two holes rusted through it.

Replaces the DRAWN surface of `ShedRoof` in assets/games/deeper_demo.loom, a
`box` at `pos = [-4.0, 3.95, 4.60]`, `scale = [4.9, 0.15, 2.40]` — world
x -8.900..0.900, y 3.800..4.100, z 2.200..7.000. It overhangs the shed by 0.4 m
at the front and 0.4 m each side.

Corrugations run along Z, front to back, so water sheds off the overhang rather
than along the ridge. The profile therefore varies in X. Spec §1 Case 1:
axis-aligned, drawn, statically parented -- transcription is exact.

**The holes are geometry, not alpha.** An alpha-cut surface casts its whole
triangle's shadow, because ray queries never run a fragment shader
(loom_scene Material::alpha_cutoff). A hole that still shadows is not a hole.

Run: blender --background --factory-startup --python build_roof.py
"""
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bpy
import bmesh

import rigkit
import textures

REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                    "..", "..", ".."))
OBJ_PATH = os.path.join(REPO, "assets", "meshes", "rig_shed_roof.obj")
TEX_DIR = os.path.join(REPO, "assets", "textures")

ROOF_CENTRE = (-4.0, 3.95, 4.60)
HALF_X, HALF_Y, HALF_Z = 4.9, 0.15, 2.40
# **The budget is what sets the resolution here, and it binds hard.** Every
# cell is a closed box (6 quads, 12 tris) so each shell is watertight and
# `verify_obj`'s per-shell volume check can see it. At the obvious 0.15 m
# corrugation pitch and a z-resolution fine enough to cut small holes, that is
# 65 x 48 = 3,120 cells = 37,440 triangles against a 2,000 cap -- eighteen times
# over. Coarsening to a 0.30 m pitch and five z-bands gives 33 x 5 = 165 cells =
# 1,980 tris, which fits.
#
# The z-bands are 0.956 m, so a hole is a whole band deep. That is not a
# compromise: corrugated roofing does not develop pinholes, it loses a SHEET.
# A band-deep gap is what the failure actually looks like.
PITCH = 0.300            # one corrugation period, in X
AMP = 0.045              # ridge height, well inside HALF_Y
ZSTEPS = 5               # z-bands; see the budget note above
SEED = 19
ZONE_SIZE = 1024         # spec §3: this zone is 1024², not TEX_SIZE's 2048
# Holes are given in CELL INDICES, not metres, so they land on cell boundaries
# exactly and the count assertion below is exact rather than approximate.
HOLES = [(7, 1), (8, 1), (24, 3)]     # (period index, z-band index)
# The sheet falls from the back of the shed to the front, so water sheds over
# the north eave rather than sitting on the ridge.
#
# The whole 0.3 m y envelope, spent three ways -- and it IS the whole envelope,
# so none of these three can grow without another shrinking:
#     AMP 0.045 + FALL 0.190 + FASCIA_H 0.060 = 0.295, inside 0.300 by 5 mm
# The corrugation amplitude takes its cut FIRST. Measured down from the ceiling:
# back ridge +0.150, back centreline +0.105, front centreline -0.085, fascia
# bottom -0.145, envelope floor -0.150.
#
# 0.190 of fall across the sheet's 4.78 m of depth is 2.28 deg. Shallow, and
# deliberately so: a steeper drawn roof would have to rise above the collider,
# and the seen-<=-walked rule has held for three phases. The 3.6 deg a naive
# re-derivation gives is what a zero-thickness sheet with no fascia would get.
# The eave overhang -- which the roof already has, its local z being +/-2.400
# against the shed's +/-2.000 -- does more for the read than the last degree.
FALL = 0.190
FASCIA_H = 0.060         # the board closing the corrugation ends at the eave
FASCIA_T = 0.020
# The sheet stops FASCIA_T short of the front so the fascia has a plane of its
# own. If the sheet ran the full 2*HALF_Z the fascia's front face would be
# COPLANAR with the cells' front wall and buried inside the slab -- z-fighting,
# not an eave. The fascia supplies the +HALF_Z bound instead.
Z_SPAN = 2 * HALF_Z - FASCIA_T
FASCIA_TRIS = 12         # one closed box; the cell-count check subtracts it

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

mesh = bpy.data.meshes.new("rig_shed_roof")
obj = bpy.data.objects.new("rig_shed_roof", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")

periods = int(round((2 * HALF_X) / PITCH))
step = (2 * HALF_X) / periods
zstep = Z_SPAN / ZSTEPS


def prof(x):
    """The corrugation profile: y offset at a given x."""
    return AMP * math.sin(2.0 * math.pi * x / PITCH)


def slab_roof(x0, x1, y0, y1, z0, z1):
    """A closed box in LOCAL metres, UV'd to match the sheet.

    Arguments are local x/y/z, but the VERTEX is built (x, -z, y) -- Blender
    native Z-up, the same ordering the cell loop uses, because rigkit exports
    (x, y, z) -> (x, z, -y). A literal (x, y, z) tuple here builds the board
    lying down. Winding is left to `recalc_face_normals` like the cells.
    """
    v = [bm.verts.new((c[0], -c[2], c[1])) for c in (
        (x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0),
        (x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1))]
    for a, b, c, d_ in ((0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
                        (2, 3, 7, 6), (1, 2, 6, 5), (0, 4, 7, 3)):
        f = bm.faces.new((v[a], v[b], v[c], v[d_]))
        for loop in f.loops:
            x, y, _z = loop.vert.co
            loop[uv_layer].uv = (x / 2.0, -y / 2.0)


for i in range(periods):
    x0 = -HALF_X + i * step
    x1 = x0 + step
    for j in range(ZSTEPS):
        z0 = -HALF_Z + j * zstep
        z1 = z0 + zstep
        if (i, j) in HOLES:
            continue
        # Height of the sheet at this z: full at the back, FALL lower at the
        # front. `zt` is 0 at the back edge and 1 at the sheet's front edge, so
        # the drop at the eave is exactly FALL and `fy` below is exact.
        drop0 = FALL * (z0 + HALF_Z) / Z_SPAN
        drop1 = FALL * (z1 + HALF_Z) / Z_SPAN
        # Top surface, following the fold AND the fall -- four distinct heights.
        a = bm.verts.new((x0, -z0, HALF_Y - AMP + prof(x0) - drop0))
        b = bm.verts.new((x1, -z0, HALF_Y - AMP + prof(x1) - drop0))
        c = bm.verts.new((x1, -z1, HALF_Y - AMP + prof(x1) - drop1))
        d = bm.verts.new((x0, -z1, HALF_Y - AMP + prof(x0) - drop1))
        # Underside, flat.
        e = bm.verts.new((x0, -z0, -HALF_Y)); f = bm.verts.new((x1, -z0, -HALF_Y))
        g = bm.verts.new((x1, -z1, -HALF_Y)); h = bm.verts.new((x0, -z1, -HALF_Y))
        faces = [bm.faces.new((a, b, c, d)), bm.faces.new((e, h, g, f))]
        # Close the cell so every shell is watertight and its signed volume is
        # positive -- `verify_obj` checks that per shell, and an open cell would
        # read as inward-facing.
        faces.append(bm.faces.new((a, e, f, b)))
        faces.append(bm.faces.new((c, g, h, d)))
        faces.append(bm.faces.new((b, f, g, c)))
        faces.append(bm.faces.new((d, h, e, a)))
        for fc in faces:
            for loop in fc.loops:
                x, y, z = loop.vert.co
                loop[uv_layer].uv = (x / 2.0, -y / 2.0)

# The fascia: a board across the low edge, closing the corrugation ends.
# Without it the sheet reads as a floating plane -- you see straight into the
# flutes and there is nothing to cast a line along the eave. ONE box, not one
# per corrugation: the board has no profile, so 33 of them would be the same
# geometry at 396 tris against a 2,000 cap the sheet already spends 1,944 of.
fy = HALF_Y - AMP - FALL         # -0.085; fascia bottom -0.145 vs floor -0.150
slab_roof(-HALF_X, HALF_X, fy - FASCIA_H, fy + AMP, HALF_Z - FASCIA_T, HALF_Z)

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()

os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)
rigkit.export_obj(obj, OBJ_PATH, uv=True,
                  header="rig: shed roof. %d corrugations at %.3f m, amp %.3f, "
                         "%d holes rusted through. Falls %.3f m to a fascia "
                         "at the front eave."
                         % (periods, PITCH, AMP, len(HOLES), FALL))

albedo, height = textures.corrugated_metal(size=ZONE_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_roof_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_roof_normal.png"),
                   textures.normal_from_height(height))

# --- self-check ----------------------------------------------------------
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

for ax, name, half in ((0, "x", HALF_X), (2, "z", HALF_Z)):
    assert abs(lo[ax] + half) < 1e-3 and abs(hi[ax] - half) < 1e-3, \
        "roof %s spans %.4f..%.4f, want %.3f..%.3f" % (name, lo[ax], hi[ax], -half, half)
# The corrugation must stay INSIDE the primitive's y envelope, or the drawn roof
# stands above the collider the player would meet if he ever got up there.
# The message named AMP alone, and AMP alone can never trip it: the top is
# HALF_Y - AMP + prof(x) with |prof| <= AMP, so it caps at HALF_Y whatever AMP
# is, and the underside is flat at -HALF_Y. What spends the envelope now is the
# sum, and the sum is what the failure has to name -- measured: FASCIA_H=0.120
# fires this, AMP=0.120 does not (it trips verify_obj's per-shell volume first).
assert lo[1] >= -HALF_Y - 1e-4 and hi[1] <= HALF_Y + 1e-4, \
    "roof y spans %.4f..%.4f, outside the primitive's +/-%.3f — the envelope " \
    "is spent AMP=%.3f + FALL=%.3f + FASCIA_H=%.3f = %.3f of %.3f" \
    % (lo[1], hi[1], HALF_Y, AMP, FALL, FASCIA_H, AMP + FALL + FASCIA_H, 2 * HALF_Y)

# **First, that there ARE holes.** Both checks below are parameterised by
# `HOLES`: the count check derives both sides of its equality from it, and the
# geometric check loops over it. With `HOLES = []` the first reads
# "165 of 165 cells present, 0 dropped for holes", the second loops zero times
# and reports "0 holes confirmed geometrically absent", and the build passes
# clean -- a roof with no holes at all, waved through by two checks written to
# prove the holes are right. Nothing downstream of here can see the difference,
# so the non-emptiness has to be asserted before either of them runs.
assert HOLES, \
    "HOLES is empty -- a roof with no holes is not the roof the spec asked " \
    "for, and every check below it is parameterised by HOLES and therefore " \
    "vacuous: the count check compares %d cells against %d cells and the " \
    "geometric check loops zero times." % (periods * ZSTEPS, periods * ZSTEPS)

# The holes must actually be holes, and EXACTLY the ones asked for. Cells are
# indexed, so this is an equality rather than an inequality — an inequality
# would pass if a bug dropped the wrong cells, or twice as many.
full = periods * ZSTEPS
# Less the fascia, which is a closed box of the same 12 tris and would
# otherwise read as one extra cell and fail this by exactly one.
cells = (info["tris"] - FASCIA_TRIS) // 12   # 6 quads -> 12 tris per cell
assert cells == full - len(HOLES), \
    "expected %d cells (%d full minus %d holes), got %d — the holes cut are not " \
    "the holes asked for" % (full - len(HOLES), full, len(HOLES), cells)
print("roof: %d of %d cells present, %d dropped for holes" % (cells, full, full - cells))

# The count check above is cheap and it catches a different failure (too
# many/few cells dropped) but it proves a subtraction happened, not that a
# hole exists in the mesh -- `cells == full - len(HOLES)` derives both sides
# from `HOLES` itself, so it cannot tell a correctly-cut hole from a bug that
# cuts the WRONG cell while leaving the count untouched. This asks the
# geometry the question directly: read back the FILE that shipped (not the
# bmesh still in memory) and confirm no triangle centroid falls inside each
# claimed hole's footprint.
geom = rigkit.read_obj(OBJ_PATH)
gverts = geom["verts"]
# A margin bigger than the OBJ's ~1e-6 round-trip precision (export writes
# vertices to 6 decimals) and much smaller than a cell (0.297 m x 0.96 m). A
# full cell's SIDE WALLS sit exactly on a hole's boundary, and rounding can
# land a wall's centroid a hair inside an unmargined open interval, which
# misreads a neighbour's wall as a triangle inside the hole -- measured: with
# no margin, holes (7, 1) and (8, 1) came back with 2 stray centroids each
# and (24, 3) with 4, all boundary-riding neighbour walls, not real hits.
HOLE_MARGIN = 0.01
for hole_i, hole_j in HOLES:
    hx0 = -HALF_X + hole_i * step
    hx1 = hx0 + step
    hz0 = -HALF_Z + hole_j * zstep
    hz1 = hz0 + zstep
    inside = 0
    for ta, tb, tc in geom["tris"]:
        tcx = (gverts[ta][0] + gverts[tb][0] + gverts[tc][0]) / 3.0
        tcz = (gverts[ta][2] + gverts[tb][2] + gverts[tc][2]) / 3.0
        if (hx0 + HOLE_MARGIN < tcx < hx1 - HOLE_MARGIN
                and hz0 + HOLE_MARGIN < tcz < hz1 - HOLE_MARGIN):
            inside += 1
    assert inside == 0, \
        "hole (%d, %d) at x %.3f..%.3f z %.3f..%.3f is not empty -- %d " \
        "triangle centroids found inside it" % (hole_i, hole_j, hx0, hx1, hz0, hz1, inside)
print("roof: %d holes confirmed geometrically absent" % len(HOLES))

# **First, that there IS a fall**, for the same reason `HOLES` is asserted
# non-empty above: the check below is parameterised by FALL on both sides, so
# at FALL = 0.0 it compares 0.000 against an expected 0.000 and passes on a
# level sheet -- the flat slab this replaced, waved through by a check written
# to prove it is pitched. Nothing downstream can see the difference.
assert FALL > 0.0, \
    "FALL is 0.0 -- the sheet is level and reads as the flat slab this " \
    "replaced, and the fall check below is parameterised by FALL on both " \
    "sides and therefore vacuous: it compares a 0.000 m fall against a " \
    "0.000 m expectation."

# The sheet must fall the right way and by the right amount. Sampled on two
# whole z-planes of the SHIPPED file, by MAX rather than mean: the holes make
# the two planes carry different sets of x, and a mean over different x would
# drift by the corrugation. The front plane is one band in from the eave so no
# fascia vertex can reach it. The underside is flat at -HALF_Y, hence the
# filter that drops it.
zb, zf = -HALF_Z, HALF_Z - FASCIA_T - zstep
back = [v[1] for v in gverts if abs(v[2] - zb) < 1e-3 and v[1] > -HALF_Y + 1e-3]
front = [v[1] for v in gverts if abs(v[2] - zf) < 1e-3 and v[1] > -HALF_Y + 1e-3]
assert back and front, \
    "could not sample the sheet at z %.3f (%d verts) and z %.3f (%d verts)" \
    % (zb, len(back), zf, len(front))
fall = max(back) - max(front)
want = FALL * (zf - zb) / Z_SPAN
assert abs(fall - want) < 1e-3, \
    "the sheet falls %.4f m between z %.3f and z %.3f, want %.4f — it is " \
    "level, or falls the wrong way, and reads as the flat slab this replaced" \
    % (fall, zb, zf, want)

# The fascia must be there, and BELOW the sheet's low edge -- a board flush
# with the corrugation closes nothing and casts no line along the eave.
fy_check = HALF_Y - AMP - FALL
assert any(v[2] > HALF_Z - FASCIA_T + 1e-6 and v[1] < fy_check - 1e-6
           for v in gverts), \
    "no geometry below y %.3f at the front face — the fascia is missing or " \
    "flush with the sheet" % fy_check
print("roof: falls %.3f m back to front (%.2f deg), fascia %.3f m"
      % (FALL, math.degrees(math.atan2(FALL, Z_SPAN)), FASCIA_H))

assert info["tris"] <= 2000, "over budget: %d tris" % info["tris"]
print("roof: node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
      % (ROOF_CENTRE + (HALF_X, HALF_Y, HALF_Z)))
print("roof: %d tris, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (info["tris"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("roof: self-check passes")
