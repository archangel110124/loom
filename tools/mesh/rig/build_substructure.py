# tools/mesh/rig/build_substructure.py
"""The rig's substructure — six piles and the bracing between them.

Replaces the DRAWN surface of six `cylinder` primitives in
assets/games/deeper_demo.loom (`PilingNW` at :1063 through `PilingSE` at :1125),
each `pos = [x, -0.20, z]`, `scale = [0.35, 1.20, 0.35]` — a cylinder of radius
0.35 spanning world y -1.400 .. 1.000, at x in {-10.5, 0, 10.5} and z in {-5.5, 5.5}.

**These six are the fifth collider case** (spec §1): their collider shape today
is round, chosen by the string `"cylinder"` in `play.rs:588-591`. A mesh gets a
box. Step 8 of this task measures whether anything notices; if something does,
the piles stay primitives and only the bracing is built.

The bracing is new geometry with no primitive behind it and therefore no
collider to preserve — it is authored ABOVE the water and inboard of the piles
so nothing can reach it.

Run: blender --background --factory-startup --python build_substructure.py
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
OBJ_PATH = os.path.join(REPO, "assets", "meshes", "rig_steel_frame.obj")
TEX_DIR = os.path.join(REPO, "assets", "textures")

PILE_R = 0.35
PILE_TOP = 1.000          # frozen: world y, the deck's underside
PILE_BOT = -1.400         # frozen: world y
PILE_CENTRE = -0.200      # the node y this mesh hangs from = the primitive's pos.y
PILE_X = (-10.5, 0.0, 10.5)
PILE_Z = (-5.5, 5.5)
SIDES = 16                # a 0.35 m pile at 16 sides has a 6.7 mm chord error
BRACE_R = 0.075
BRACE_Y = 0.550           # world; above the 0.166 m significant wave height
SEED = 11
TEX_SIZE = 2048

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

mesh = bpy.data.meshes.new("rig_steel_frame")
obj = bpy.data.objects.new("rig_steel_frame", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")


def tube(p0, p1, radius, sides, uv_u0):
    """A closed capped tube from p0 to p1, in Blender coords, UV-mapped so U runs
    around the circumference and V along the length."""
    ax = [p1[i] - p0[i] for i in range(3)]
    length = math.sqrt(sum(c * c for c in ax))
    ax = [c / length for c in ax]
    # Any vector not parallel to the axis gives a usable frame.
    up = (0.0, 0.0, 1.0) if abs(ax[2]) < 0.9 else (1.0, 0.0, 0.0)
    u = [up[1] * ax[2] - up[2] * ax[1], up[2] * ax[0] - up[0] * ax[2],
         up[0] * ax[1] - up[1] * ax[0]]
    ul = math.sqrt(sum(c * c for c in u))
    u = [c / ul for c in u]
    v = [ax[1] * u[2] - ax[2] * u[1], ax[2] * u[0] - ax[0] * u[2],
         ax[0] * u[1] - ax[1] * u[0]]

    rings = []
    for end, p in ((0, p0), (1, p1)):
        ring = []
        for s in range(sides):
            t = 2.0 * math.pi * s / sides
            c, sn = math.cos(t), math.sin(t)
            ring.append(bm.verts.new((
                p[0] + radius * (c * u[0] + sn * v[0]),
                p[1] + radius * (c * u[1] + sn * v[1]),
                p[2] + radius * (c * u[2] + sn * v[2]))))
        rings.append(ring)

    faces = []
    for s in range(sides):
        a, b = rings[0][s], rings[0][(s + 1) % sides]
        c, d = rings[1][s], rings[1][(s + 1) % sides]
        faces.append(bm.faces.new((a, b, d, c)))
    faces.append(bm.faces.new(tuple(reversed(rings[0]))))
    faces.append(bm.faces.new(tuple(rings[1])))

    # U around the circumference (one tile per full turn), V along the length at
    # one tile per metre, so a 2.4 m pile shows 2.4 tiles and the rust's downward
    # runs stay the right way up and the right size on every part.
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            ang = math.atan2(y - (p0[1] + p1[1]) * 0.5, x - (p0[0] + p1[0]) * 0.5)
            along = ((x - p0[0]) * ax[0] + (y - p0[1]) * ax[1] + (z - p0[2]) * ax[2])
            loop[uv_layer].uv = (uv_u0 + ang / (2.0 * math.pi), along)
    return faces


# Blender is Z-up; PILE_* are LOOM y, which is Blender z. Loom z is Blender -y.
for i, px in enumerate(PILE_X):
    for j, pz in enumerate(PILE_Z):
        tube((px, -pz, PILE_BOT - PILE_CENTRE),
             (px, -pz, PILE_TOP - PILE_CENTRE),
             PILE_R, SIDES, (i * 2 + j) * 0.37)

# Cross-bracing: one horizontal run each way at BRACE_Y, pile to pile. It is
# ABOVE the water (Hs at the berth is 0.166 m) and inboard of the pile faces, so
# it adds no collider anything can reach.
bz = BRACE_Y - PILE_CENTRE
for pz in PILE_Z:
    for a, b in zip(PILE_X, PILE_X[1:]):
        tube((a, -pz, bz), (b, -pz, bz), BRACE_R, 8, 0.11)
for px in PILE_X:
    tube((px, -PILE_Z[0], bz), (px, -PILE_Z[1], bz), BRACE_R, 8, 0.29)

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()

os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)
rigkit.export_obj(obj, OBJ_PATH, uv=True,
                  header="rig: steel frame. 6 piles r%.2f, %d sides; bracing r%.3f at world y %.3f."
                         % (PILE_R, SIDES, BRACE_R, BRACE_Y))

albedo, height = textures.weathered_steel(size=TEX_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_frame_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_frame_normal.png"),
                   textures.normal_from_height(height))
print("substructure: textures %dx%d written" % (TEX_SIZE, TEX_SIZE))

# --- self-check ----------------------------------------------------------
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

LOCAL_TOP = PILE_TOP - PILE_CENTRE
LOCAL_BOT = PILE_BOT - PILE_CENTRE
assert abs(hi[1] - LOCAL_TOP) < 1e-3, \
    "top local y=%.5f (world %.5f), want %.3f (world %.3f) — the piles must " \
    "meet the deck's underside exactly" % (hi[1], hi[1] + PILE_CENTRE, LOCAL_TOP, PILE_TOP)
assert abs(lo[1] - LOCAL_BOT) < 1e-3, \
    "bottom local y=%.5f (world %.5f), want %.3f (world %.3f)" \
    % (lo[1], lo[1] + PILE_CENTRE, LOCAL_BOT, PILE_BOT)

# The piles must sit exactly where the primitives did, in x and z.
assert abs(lo[0] - (min(PILE_X) - PILE_R)) < 1e-3, "x lo %.4f" % lo[0]
assert abs(hi[0] - (max(PILE_X) + PILE_R)) < 1e-3, "x hi %.4f" % hi[0]
assert abs(lo[2] - (min(PILE_Z) - PILE_R)) < 1e-3, "z lo %.4f" % lo[2]
assert abs(hi[2] - (max(PILE_Z) + PILE_R)) < 1e-3, "z hi %.4f" % hi[2]

# Nothing may reach north of the quay edge in the hull's lane — the boat sits
# 0.15 m off and weighs 43,776 kg. The piles are at z -5.5 and the bracing runs
# between them, so this is a standing guard rather than a live risk.
assert hi[2] <= 7.0 and lo[2] >= -7.0, "substructure escapes the deck footprint"

assert info["tris"] <= 8000, "over budget: %d tris" % info["tris"]
print("substructure: %d tris, %d verts, x %.2f..%.2f y %.3f..%.3f z %.2f..%.2f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("substructure: self-check passes")
