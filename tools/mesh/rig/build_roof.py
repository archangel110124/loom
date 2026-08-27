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
# The z-bands are 0.96 m, so a hole is a whole band deep. That is not a
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

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

mesh = bpy.data.meshes.new("rig_shed_roof")
obj = bpy.data.objects.new("rig_shed_roof", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")

periods = int(round((2 * HALF_X) / PITCH))
step = (2 * HALF_X) / periods
zstep = (2 * HALF_Z) / ZSTEPS


def prof(x):
    """The corrugation profile: y offset at a given x."""
    return AMP * math.sin(2.0 * math.pi * x / PITCH)


for i in range(periods):
    x0 = -HALF_X + i * step
    x1 = x0 + step
    for j in range(ZSTEPS):
        z0 = -HALF_Z + j * zstep
        z1 = z0 + zstep
        if (i, j) in HOLES:
            continue
        yt0, yt1 = HALF_Y - AMP + prof(x0), HALF_Y - AMP + prof(x1)
        # Top surface, following the fold.
        a = bm.verts.new((x0, -z0, yt0)); b = bm.verts.new((x1, -z0, yt1))
        c = bm.verts.new((x1, -z1, yt1)); d = bm.verts.new((x0, -z1, yt0))
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

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()

os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)
rigkit.export_obj(obj, OBJ_PATH, uv=True,
                  header="rig: shed roof. %d corrugations at %.3f m, amp %.3f, "
                         "%d holes rusted through." % (periods, PITCH, AMP, len(HOLES)))

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
assert lo[1] >= -HALF_Y - 1e-4 and hi[1] <= HALF_Y + 1e-4, \
    "roof y spans %.4f..%.4f, outside the primitive's +/-%.3f — the fold amplitude " \
    "AMP=%.3f is too large for HALF_Y" % (lo[1], hi[1], HALF_Y, AMP)

# The holes must actually be holes, and EXACTLY the ones asked for. Cells are
# indexed, so this is an equality rather than an inequality — an inequality
# would pass if a bug dropped the wrong cells, or twice as many.
full = periods * ZSTEPS
cells = info["tris"] // 12          # 6 quads -> 12 tris per closed cell
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

assert info["tris"] <= 2000, "over budget: %d tris" % info["tris"]
print("roof: node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
      % (ROOF_CENTRE + (HALF_X, HALF_Y, HALF_Z)))
print("roof: %d tris, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (info["tris"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("roof: self-check passes")
