# tools/mesh/rig/build_substructure.py
"""The rig's substructure — one pile mesh (placed by six nodes) and the
bracing between them.

Replaces the DRAWN surface of six `cylinder` primitives in
assets/games/deeper_demo.loom (`PilingNW` at :1063 through `PilingSE` at :1125),
each `pos = [x, -0.20, z]`, `scale = [0.35, 1.20, 0.35]` — a cylinder of radius
0.35 spanning world y -1.400 .. 1.000, at x in {-10.5, 0, 10.5} and z in {-5.5, 5.5}.

**These six are the fifth collider case** (spec §1): their collider shape today
is round, chosen by the string `"cylinder"` in `play.rs:588-591`. Task 3 Step 8
measured what a mesh's box collider costs a single combined mesh — nothing, if
there is no collider at all: a probe driven horizontally into the pile line
stopped at x=-9.7809 with the six `cylinder` primitives present and ran to
x=-46.0 without them. The single-mesh, no-collider substructure could not
ship. Task 3b is the fix: **two meshes, seven nodes, not seven meshes.** All
six pilings share one scale, so they are one shape — one `rig_pile.obj`
centred on its own origin (in all three axes, not just y, because
`play.rs:520` centres a static collider on the NODE and each node sits at its
own world x/z), placed by six nodes each carrying its own transform and its
own `BoxCollider`. The bracing is new geometry with no primitive behind it and
therefore no collider to preserve — it is authored ABOVE the water and inboard
of the piles so nothing can reach it, and stays one object at the rig origin.

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

# --- the tide seam ---------------------------------------------------------
# The band on the piles where they cross the water. **Superseding the plan
# brief's per-pile loop**: task 3b replaced the single `rig_steel_frame.obj`
# (all six piles at their world positions) with one `rig_pile.obj` centred on
# its own origin, placed by six nodes. The seam follows the same shape: one
# mesh, centred on its own origin, one `tube()` call at (0.0, 0.0, ...), six
# nodes to place it (Task 6).
#
# `play.rs:768` gives ANY renderable static node with no `BoxCollider` an
# extent of `|world_scale|` -- 2x2x2 m at scale 1 -- so "no collider" is not
# achievable by omitting the component; the seam gets an explicit one, sized
# and centred to sit INSIDE the pile's own [PILE_R, PILE_TOP-PILE_CENTRE,
# PILE_R] box, so it presents nothing a swimmer could meet that the pile did
# not already.
TIDE_TOP = 0.450          # world y — above the splash, where growth dies
TIDE_BOT = -0.650         # world y — below it, drowned
TIDE_OVER = 0.004         # the band stands this far proud of the pile, drawn only
# Half-height and node y are DERIVED from TIDE_TOP/TIDE_BOT, not authored
# twice: a mesh centred on its own local origin needs a half-height equal to
# half the band's total span, and a node y equal to the span's midpoint --
# get either wrong and the mesh is not centred, which the self-check below
# catches the same way it catches the pile.
SEAM_HALF_Y = (TIDE_TOP - TIDE_BOT) / 2.0    # 0.550
SEAM_NODE_Y = (TIDE_TOP + TIDE_BOT) / 2.0    # -0.100 -- world y the node sits at
# The seam's COLLIDER uses PILE_R, not PILE_R + TIDE_OVER. The mesh is drawn
# TIDE_OVER proud so it does not z-fight the pile beneath it -- a cosmetic,
# sub-tolerance render offset with no gameplay meaning. Carrying that 4 mm
# into the collider would put the seam's x/z half-extent (0.354) fractionally
# OUTSIDE the pile's own (0.350), breaking the one property that makes a
# redundant collider harmless: that it is a subset of a hazard the pile
# already presents. Named here, not re-derived at the assertion.
SEAM_COLLIDER_HALF = (PILE_R, SEAM_HALF_Y, PILE_R)

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)


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


# ---- one pile, at its own origin ----------------------------------------
# **All six pilings are the same shape** (`scale = [0.35, 1.20, 0.35]` on every
# one of them), so this is ONE mesh placed by six nodes rather than six meshes.
# It is centred in x and z as well as y, because each node sits at its own
# [x, -0.20, z] and `play.rs:520` centres that node's collider on the node.
mesh = bpy.data.meshes.new("rig_pile")
obj = bpy.data.objects.new("rig_pile", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")
tube((0.0, 0.0, PILE_BOT - PILE_CENTRE),
     (0.0, 0.0, PILE_TOP - PILE_CENTRE), PILE_R, SIDES, 0.0)
bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()
PILE_PATH = os.path.join(REPO, "assets", "meshes", "rig_pile.obj")
rigkit.export_obj(obj, PILE_PATH, uv=True,
                  header="rig: one pile, r%.2f, %d sides, centred on its own "
                         "origin. Six nodes place it." % (PILE_R, SIDES))

# ---- the bracing, at the rig origin --------------------------------------
# New geometry with no primitive behind it, so there is no collider to preserve
# and it gets none. It sits at world y %.3f, above the berth's 0.166 m
# significant wave height, and inboard of the pile faces.
mesh2 = bpy.data.meshes.new("rig_bracing")
obj2 = bpy.data.objects.new("rig_bracing", mesh2)
bpy.context.collection.objects.link(obj2)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")
bz = BRACE_Y - PILE_CENTRE
for pz in PILE_Z:
    for a, b in zip(PILE_X, PILE_X[1:]):
        tube((a, -pz, bz), (b, -pz, bz), BRACE_R, 8, 0.11)
for px in PILE_X:
    tube((px, -PILE_Z[0], bz), (px, -PILE_Z[1], bz), BRACE_R, 8, 0.29)
bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh2)
bm.free()
BRACE_PATH = os.path.join(REPO, "assets", "meshes", "rig_bracing.obj")
rigkit.export_obj(obj2, BRACE_PATH, uv=True,
                  header="rig: cross-bracing, r%.3f at world y %.3f. No collider: "
                         "new geometry, no primitive behind it." % (BRACE_R, BRACE_Y))

# ---- the tide seam, at its own origin ------------------------------------
# A SECOND OBJ over the SAME shape the pile already is, not new volume: the
# engine takes one OBJ per material and this band is a different material on
# the piles it wraps. It stands TIDE_OVER proud so it does not z-fight the
# pile beneath it -- 4 mm is far above the depth buffer's resolution at this
# range and far below anything the player can see as a step.
#
# Centred on its own local origin in x/y/z, exactly like `rig_pile.obj` --
# ONE `tube()` call at (0.0, 0.0, ...), not one per pile. Six nodes place it
# in Task 6, at the same x/z as the piles and at world y = SEAM_NODE_Y.
mesh3 = bpy.data.meshes.new("rig_steel_tidal")
obj3 = bpy.data.objects.new("rig_steel_tidal", mesh3)
bpy.context.collection.objects.link(obj3)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")
tube((0.0, 0.0, TIDE_BOT - SEAM_NODE_Y), (0.0, 0.0, TIDE_TOP - SEAM_NODE_Y),
     PILE_R + TIDE_OVER, SIDES, 0.0)
bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh3)
bm.free()
TIDAL_PATH = os.path.join(REPO, "assets", "meshes", "rig_steel_tidal.obj")
rigkit.export_obj(obj3, TIDAL_PATH, uv=True,
                  header="rig: the tide seam, centred on its own origin. World y "
                         "%.3f..%.3f, %.0f mm proud of the pile. Six nodes place "
                         "it at world y %.3f (SEAM_NODE_Y)."
                         % (TIDE_BOT, TIDE_TOP, TIDE_OVER * 1000, SEAM_NODE_Y))

os.makedirs(TEX_DIR, exist_ok=True)
albedo, height = textures.weathered_steel(size=TEX_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_frame_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_frame_normal.png"),
                   textures.normal_from_height(height))
alb2, h2 = textures.tidal_growth(size=TEX_SIZE, seed=13)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_tidal_albedo.png"), alb2)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_tidal_normal.png"),
                   textures.normal_from_height(h2))
print("substructure: textures %dx%d written" % (TEX_SIZE, TEX_SIZE))

# --- self-check ----------------------------------------------------------
pi = rigkit.verify_obj(PILE_PATH)
bi = rigkit.verify_obj(BRACE_PATH)

# The pile must be centred on its own origin in ALL THREE axes. If it is not,
# no node transform can put both the drawn pile and its BoxCollider in the same
# place -- that is the whole reason this task exists.
for ax, name, half in ((0, "x", PILE_R), (1, "y", PILE_TOP - PILE_CENTRE), (2, "z", PILE_R)):
    assert abs(pi["lo"][ax] + half) < 1e-3 and abs(pi["hi"][ax] - half) < 1e-3, \
        "pile %s spans %.4f..%.4f, want %.3f..%.3f — it is not centred on its " \
        "own origin and its collider cannot be made to match" \
        % (name, pi["lo"][ax], pi["hi"][ax], -half, half)

# The bracing must sit ABOVE the water at the berth. Below it, it is a submerged
# obstacle nothing was told about; at it, it is in the splash.
assert bi["lo"][1] + PILE_CENTRE > 0.166, \
    "bracing reaches world y %.3f, at or below the berth's 0.166 m significant " \
    "wave height" % (bi["lo"][1] + PILE_CENTRE)
# And inboard of the pile faces, so it can never be the thing a swimmer meets.
# **Tolerance is PILE_R, not BRACE_R, and not the 1e-6 this started as.**
# A pile face sits PILE_R=0.35 out from the pile's own centre, so "inboard of
# the pile faces" means within PILE_X +- PILE_R — the check has to name that
# quantity, not the bracing's.
#
# The 1e-6 version failed on the very first correct build: the north/south
# braces (`for px in PILE_X: tube(...)`) run along z at a fixed x=px, and
# `tube()`'s circular cross-section, perpendicular to its own axis, bulges
# +-BRACE_R in x around px — the same way any round tube's footprint exceeds
# its centreline off-axis. Measured x -10.575..10.575 against PILE_X's
# -10.5..10.5: exactly BRACE_R=0.075 over.
#
# **The first fix, widening the tolerance to BRACE_R, was a tautology and
# wrong.** A z-running tube of radius BRACE_R bulges to exactly px +- BRACE_R
# — that IS the bulge, so a bound of px +- BRACE_R can never fail no matter
# what BRACE_R is. A tolerance that scales with the exact quantity it is
# supposed to be bounding is not a tolerance, it is the check disabling
# itself, and it is worth naming as a trap for that reason: it reads as a
# reasonable-looking fix and it still passed the assertion right below it.
# Today's geometry (BRACE_R=0.075 << PILE_R=0.35) happens to satisfy the
# correct bound too, so nothing shipped was ever actually wrong — but a
# heavier rig thickening the bracing toward PILE_R would put it past the pile
# faces into swimmer-reachable space with a BRACE_R-tolerant guard still
# green. Fault-injected below: BRACE_R=0.5 fires this assertion; BRACE_R=0.075
# does not.
assert bi["lo"][0] >= min(PILE_X) - PILE_R - 1e-6 and bi["hi"][0] <= max(PILE_X) + PILE_R + 1e-6, \
    "bracing x %.4f..%.4f escapes the pile line" % (bi["lo"][0], bi["hi"][0])

assert pi["tris"] + bi["tris"] <= 8000, \
    "over budget: %d + %d tris" % (pi["tris"], bi["tris"])
print("pile: %d tris, x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (pi["tris"], pi["lo"][0], pi["hi"][0], pi["lo"][1], pi["hi"][1], pi["lo"][2], pi["hi"][2]))
print("bracing: %d tris, world y %.3f..%.3f"
      % (bi["tris"], bi["lo"][1] + PILE_CENTRE, bi["hi"][1] + PILE_CENTRE))

# --- the seam's own self-check --------------------------------------------
t = rigkit.verify_obj(TIDAL_PATH)

# Centred on its own origin in ALL THREE axes, exactly like the pile check
# above -- same reason: no node transform can put a mesh and a matching
# BoxCollider in the same place if the mesh is not centred on the node it
# hangs from.
for ax, name, half in ((0, "x", PILE_R + TIDE_OVER), (1, "y", SEAM_HALF_Y), (2, "z", PILE_R + TIDE_OVER)):
    assert abs(t["lo"][ax] + half) < 1e-3 and abs(t["hi"][ax] - half) < 1e-3, \
        "seam %s spans %.4f..%.4f, want %.3f..%.3f — it is not centred on its " \
        "own origin and its collider cannot be made to match" \
        % (name, t["lo"][ax], t["hi"][ax], -half, half)

# The seam must STRADDLE the water. A band entirely above or below it is not a
# tide line, and this is the one assertion that says what the zone is FOR.
# TIDE_BOT and TIDE_TOP are WORLD y; WaterBody.surface_height is 0.0
# (deeper_demo.loom:921).
assert TIDE_BOT < 0.0 < TIDE_TOP, \
    "the seam spans world y %.3f..%.3f and does not cross the water surface " \
    "at y=0 — that is not a tide line" % (TIDE_BOT, TIDE_TOP)

# The property that makes a redundant collider harmless: it must not exceed
# the pile's own [PILE_R, PILE_TOP-PILE_CENTRE, PILE_R] box in any axis, so it
# can never present something a swimmer could meet that the pile did not
# already. This is why SEAM_COLLIDER_HALF drops TIDE_OVER (see its definition
# above) rather than matching the drawn mesh's radius exactly.
PILE_COLLIDER_HALF = (PILE_R, PILE_TOP - PILE_CENTRE, PILE_R)
for ax, name in ((0, "x"), (1, "y"), (2, "z")):
    assert SEAM_COLLIDER_HALF[ax] <= PILE_COLLIDER_HALF[ax] + 1e-9, \
        "seam collider half-extent %s=%.3f exceeds the pile's %.3f — it would " \
        "present something a swimmer could meet that the pile does not " \
        "already" % (name, SEAM_COLLIDER_HALF[ax], PILE_COLLIDER_HALF[ax])

assert t["tris"] <= 6000, "seam over budget: %d tris" % t["tris"]
print("seam: %d tris, world y %.3f..%.3f, node_y=%.3f, "
      "collider half_extents=[%.3f, %.3f, %.3f], straddles the waterline"
      % (t["tris"], TIDE_BOT, TIDE_TOP, SEAM_NODE_Y,
         SEAM_COLLIDER_HALF[0], SEAM_COLLIDER_HALF[1], SEAM_COLLIDER_HALF[2]))
print("substructure: self-check passes")
