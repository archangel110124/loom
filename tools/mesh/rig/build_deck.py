# tools/mesh/rig/build_deck.py
"""The rig's deck field — a 24 x 14 m plank surface, rotted.

Replaces the drawn surface of `Rig/Deck` in assets/games/deeper_demo.loom, which
is one `box` primitive at scale [12.0, 0.20, 7.0]. The collider does not change:
the scene node keeps an explicit BoxCollider carrying those same numbers.

**No vertex may exceed DECK_TOP.** The demo's own comment says the rig has no
invisible ramps because drawn bounds and collider are the same thing. This build
breaks that equivalence by construction, so it re-establishes it as an
inequality instead: seen <= walked. Planks cup UPWARD (weathered timber does —
the outer rings shrink more), so each is sunk until its raised edges touch
DECK_TOP exactly and its centre dips CUP metres below.

Run: blender --background --factory-startup --python build_deck.py
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
OBJ_PATH = os.path.join(REPO, "assets", "meshes", "rig_deck_timber.obj")
TEX_DIR = os.path.join(REPO, "assets", "textures")

# ---- the numbers -----------------------------------------------------------
HALF_X, HALF_Z = 12.0, 7.0        # frozen: the quay edge is z = -7.000
DECK_TOP = 1.400                  # frozen: every spawn and assert row reads it
DECK_BOTTOM = 1.000               # the primitive's underside
PLANK_W = 0.200                   # across the deck, in Z
GAP = 0.012                       # a gap you can see the sea through
CUP = 0.008                       # centre dips this far below the edges
SPAN = 3                          # segments across a plank's width -> the cup
SEED = 7
TEX_SIZE = 2048

# Blender is Z-up while modelling; DECK_TOP is a LOOM y, which is Blender z.
# The export permutation is (x, y, z) -> (x, z, -y), so Blender z IS Loom y and
# Blender y becomes Loom -z. Planks run along X in both frames.

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

mesh = bpy.data.meshes.new("rig_deck_timber")
obj = bpy.data.objects.new("rig_deck_timber", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")

pitch = PLANK_W + GAP
courses = int(round((2 * HALF_Z) / pitch))
# Courses must be FLUSH at both z edges, the same way boards are flush at both
# x edges below. A course pitch that reserves a trailing GAP after every
# course, including the last, leaves the far edge short by one GAP -- a strip
# of sea visible past the last board, at the exact line the collider ends.
# Distribute the rounding error into plank width instead so boards butt the
# collider edge exactly, with courses still meeting at internal joints.
plank_w = (2 * HALF_Z - (courses - 1) * GAP) / courses
pitch = plank_w + GAP


def plank(z0, x0, x1, sink, uv_v0):
    """One board, cupped, its top edges at DECK_TOP - sink."""
    top = DECK_TOP - sink
    verts_top = []
    for s in range(SPAN + 1):
        f = s / SPAN
        z = z0 + f * plank_w
        # Cup: zero at the edges, CUP at the centre. cos gives C1 continuity at
        # the edges, so neighbouring boards do not shear against each other.
        dip = CUP * 0.5 * (1.0 - math.cos(2.0 * math.pi * f))
        row = []
        for x in (x0, x1):
            row.append(bm.verts.new((x, -z, top - dip)))
        verts_top.append(row)

    bot = [[bm.verts.new((x, -(z0 + (s / SPAN) * plank_w), DECK_BOTTOM))
            for x in (x0, x1)] for s in range(SPAN + 1)]

    faces = []
    for s in range(SPAN):
        a, b = verts_top[s], verts_top[s + 1]
        faces.append(bm.faces.new((a[0], a[1], b[1], b[0])))          # top
        c, d = bot[s], bot[s + 1]
        faces.append(bm.faces.new((c[0], d[0], d[1], c[1])))          # bottom
    # Four sides, closing the board so signed volume is positive.
    faces.append(bm.faces.new((verts_top[0][0], bot[0][0], bot[0][1], verts_top[0][1])))
    faces.append(bm.faces.new((verts_top[SPAN][1], bot[SPAN][1], bot[SPAN][0],
                               verts_top[SPAN][0])))
    for s in range(SPAN):
        faces.append(bm.faces.new((verts_top[s][0], verts_top[s + 1][0],
                                   bot[s + 1][0], bot[s][0])))
        faces.append(bm.faces.new((verts_top[s + 1][1], verts_top[s][1],
                                   bot[s][1], bot[s + 1][1])))

    # UVs: 1 texture tile per 2 m along the board, 1 per board across. Every
    # board gets its own V band so no two neighbours share grain.
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            loop[uv_layer].uv = (x / 2.0, uv_v0 + (-y - z0) / plank_w * 0.25)
    return faces


rng_state = SEED
def rand():
    """A tiny LCG. Deterministic, and independent of Python's hash seed."""
    global rng_state
    rng_state = (rng_state * 1103515245 + 12345) & 0x7FFFFFFF
    return rng_state / 0x7FFFFFFF


for c in range(courses):
    z0 = -HALF_Z + c * pitch
    # Butt joints: 3 to 5 boards per course, staggered course to course so the
    # joints never line up. A deck whose joints line up reads as a texture.
    n = 3 + int(rand() * 3)
    cuts = sorted(-HALF_X + (2 * HALF_X) * rand() for _ in range(n - 1))
    edges = [-HALF_X] + cuts + [HALF_X]
    for k in range(len(edges) - 1):
        x0, x1 = edges[k] + (0.006 if k else 0.0), edges[k + 1]
        if x1 - x0 < 0.30:
            continue
        # Each board sits a little differently: a nail lifting, a board proud
        # of its neighbour is FORBIDDEN (it would exceed DECK_TOP), so the
        # variation is one-sided — boards sink, never rise.
        sink = rand() * 0.004
        plank(z0, x0, x1, sink, (c % 4) * 0.25)

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()

os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)

rigkit.export_obj(obj, OBJ_PATH, uv=True,
                  header="rig: deck timber. %d courses at %.4f m pitch, cup %.0f mm."
                         % (courses, pitch, CUP * 1000))

albedo, height = textures.weathered_timber(size=TEX_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_deck_timber_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_deck_timber_normal.png"),
                   textures.normal_from_height(height))
print("deck: textures %dx%d written" % (TEX_SIZE, TEX_SIZE))

# --- self-check, runs on every build -------------------------------------
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

assert abs(lo[0] + 12.0) < 1e-3 and abs(hi[0] - 12.0) < 1e-3, \
    "x %.4f..%.4f, want -12.000..12.000" % (lo[0], hi[0])
assert abs(lo[2] + 7.0) < 1e-3 and abs(hi[2] - 7.0) < 1e-3, \
    "z %.4f..%.4f, want -7.000..7.000" % (lo[2], hi[2])

# THE ONE THAT MATTERS. Nothing drawn may stand above the invisible floor.
assert hi[1] <= DECK_TOP + 1e-4, \
    "a vertex reaches y=%.5f, above the collider top %.3f — the player would " \
    "see deck above the surface he stands on" % (hi[1], DECK_TOP)
assert hi[1] > DECK_TOP - 1e-3, \
    "highest vertex is y=%.5f, %.1f mm BELOW the collider top — the player " \
    "would float" % (hi[1], (DECK_TOP - hi[1]) * 1000.0)

assert info["tris"] <= 12000, "over budget: %d tris" % info["tris"]
assert len(rigkit.read_obj(OBJ_PATH)["uvs"]) > 0, "no UVs — the texture cannot land"

print("deck: %d tris, %d verts, x %.2f..%.2f y %.3f..%.3f z %.2f..%.2f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("deck: self-check passes")
