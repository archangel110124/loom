# tools/mesh/rig/build_shed.py
"""The rig's shed -- board-and-batten walls with a door, and nothing proud of
the north face.

Replaces the DRAWN surface of `Shed` in assets/games/deeper_demo.loom, a single
`box` at `pos = [-4.0, 2.60, 4.60]`, `scale = [4.5, 1.20, 2.00]` -- world
x -8.500..0.500, y 1.400..3.800, z 2.600..6.600.

**THE NORTH FACE IS THE MIRROR'S MOUNT AND MUST STAY EXACTLY AT z = 2.600.**
`MirrorFrame` occupies z 2.570..2.600 with its back flush to that face, and
`Mirror` sits in front of it at z 2.550..2.570. A board or batten standing proud
of the face punches through the frame. The demo's own header records getting
this stack backwards as the mistake that cost an hour.

So the construction is inverted from the obvious one: the BATTENS sit at the
face plane and the BOARDS are recessed behind them. That is also how
board-and-batten is really built -- a batten covers the joint between two
boards and stands proud of THEM, not of the wall line.

`green.sh:1903` strafes a character into this shed and asserts `state.stuck == 1`,
so its collider is load-bearing. Spec §1 Case 1: axis-aligned, drawn, statically
parented, so an explicit BoxCollider transcribing the primitive's numbers is exact.

Run: blender --background --factory-startup --python build_shed.py
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bpy
import bmesh

import rigkit
import textures

REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                    "..", "..", ".."))
OBJ_PATH = os.path.join(REPO, "assets", "meshes", "rig_shed.obj")
TEX_DIR = os.path.join(REPO, "assets", "textures")

# World extents, straight off the node. HALF_* are the primitive's `scale`.
SHED_CENTRE = (-4.0, 2.60, 4.60)
HALF_X, HALF_Y, HALF_Z = 4.5, 1.20, 2.00
NORTH_FACE_WORLD = SHED_CENTRE[2] - HALF_Z          # 2.600 -- frozen
BOARD_W = 0.200          # board width
BATTEN_W = 0.070         # batten covering each joint
RECESS = 0.018           # how far the boards sit BEHIND the batten face
DOOR_W, DOOR_H = 0.90, 1.95
SEED = 17
ZONE_SIZE = 1024         # spec §3: this zone is 1024², not TEX_SIZE's 2048

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

mesh = bpy.data.meshes.new("rig_shed")
obj = bpy.data.objects.new("rig_shed", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")


def slab(x0, x1, y0, y1, z0, z1, uv_u0):
    """One box in LOCAL metres (already centred). U across the run, V up it."""
    c = [(x0, -z0, y0), (x1, -z0, y0), (x1, -z1, y0), (x0, -z1, y0),
         (x0, -z0, y1), (x1, -z0, y1), (x1, -z1, y1), (x0, -z1, y1)]
    v = [bm.verts.new(p) for p in c]
    quads = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
             (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
    faces = [bm.faces.new(tuple(v[i] for i in q)) for q in quads]
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            # One tile per 1 m across and per 2 m up. The grain runs vertically
            # in this texture, so V is the along-grain axis and gets the longer
            # span -- the reverse of the deck's mapping, deliberately. The wall
            # is 2.4 m, so V passes 1 and the texture DOES tile vertically:
            # `weathered_boards`'s rot ramp is a raised cosine for exactly that
            # reason. Do not "simplify" it to a linspace.
            loop[uv_layer].uv = (uv_u0 + x / 1.0, z / 2.0)
    return faces


# Each wall is a run of vertical boards, recessed, with a batten over every
# joint standing at the wall plane. `outward` is the local axis the wall faces.
# Recess is carried PER WALL (not read from the module constant inside
# `wall()`) so a fault injection can isolate one wall -- e.g. flip only the
# north row's sign -- instead of inverting every wall's construction at once,
# which is what module-level RECESS did and why the north-face check was
# never actually seen to fire the first time this was tried.
WALLS = [
    # (fixed axis, plane, from, to, outward sign, recess)
    ("z", -HALF_Z, -HALF_X, HALF_X, -1, RECESS),   # north -- THE MIRROR FACE
    ("z",  HALF_Z, -HALF_X, HALF_X, +1, RECESS),   # south
    ("x", -HALF_X, -HALF_Z, HALF_Z, -1, RECESS),   # west
    ("x",  HALF_X, -HALF_Z, HALF_Z, +1, RECESS),   # east -- carries the door
]

rng_state = SEED
def rand():
    global rng_state
    rng_state = (rng_state * 1103515245 + 12345) & 0x7FFFFFFF
    return rng_state / 0x7FFFFFFF


def wall(axis, plane, a, b, outward, recess, door=False):
    """Boards recessed, battens at the plane. Nothing crosses `plane`."""
    span = b - a
    n = max(1, int(round(span / BOARD_W)))
    w = span / n
    # `inner` is the recessed board face; `plane` is where the batten sits.
    # `recess` comes from the WALLS row, not the module constant, so one row
    # can be inverted in isolation.
    inner = plane - outward * recess
    for i in range(n):
        p0, p1 = a + i * w, a + (i + 1) * w
        # The door is a gap in the boards of the east wall, centred on the run.
        mid = (p0 + p1) * 0.5
        door_here = door and abs(mid) < DOOR_W * 0.5
        y0 = -HALF_Y + (DOOR_H if door_here else 0.0)
        if y0 >= HALF_Y:
            continue
        if axis == "z":
            slab(p0, p1, y0, HALF_Y, min(plane, inner), max(plane, inner), rand())
        else:
            slab(min(plane, inner), max(plane, inner), y0, HALF_Y, p0, p1, rand())
        # A batten over the joint at the far edge of every board but the last.
        if i < n - 1:
            j0, j1 = p1 - BATTEN_W * 0.5, p1 + BATTEN_W * 0.5
            if axis == "z":
                slab(j0, j1, y0, HALF_Y, min(plane, inner), max(plane, inner), rand())
            else:
                slab(min(plane, inner), max(plane, inner), y0, HALF_Y, j0, j1, rand())


for axis, plane, a, b, outward, recess in WALLS:
    wall(axis, plane, a, b, outward, recess, door=(axis == "x" and outward > 0))

# A flat roof deck so the box is closed and the shed is not hollow from above.
slab(-HALF_X, HALF_X, HALF_Y - 0.04, HALF_Y, -HALF_Z, HALF_Z, 0.0)

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()

os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)
rigkit.export_obj(obj, OBJ_PATH, uv=True,
                  header="rig: shed, board-and-batten. Boards recessed %.0f mm; "
                         "battens at the wall plane. Nothing proud of the north face."
                         % (RECESS * 1000))

albedo, height = textures.weathered_boards(size=ZONE_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_shed_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_shed_normal.png"),
                   textures.normal_from_height(height))

# --- self-check ----------------------------------------------------------
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

# **THE ONE THAT MATTERS, CHECKED FIRST.** Local -HALF_Z is world
# NORTH_FACE_WORLD = 2.600, and MirrorFrame starts there. A vertex below it in
# local z is a board through the frame. This runs BEFORE the generic per-axis
# centring loop below on purpose: a fault that is symmetric across all four
# walls (e.g. a sign flip applied to every wall's recess at once) trips the
# generic x-centring check first and this one is never reached, which is
# exactly what happened the first time this assertion was fault-injected --
# the build refused, but for the wrong reason, and this specific guard was
# never actually seen to fire. Running it first makes it the one that fires
# whenever it is the one that is true, instead of dead code behind a more
# generic check.
assert lo[2] >= -HALF_Z - 1e-4, \
    "a vertex reaches local z=%.5f (world %.5f), north of the shed's face at " \
    "world %.3f -- MirrorFrame occupies %.3f..%.3f and this punches through it" \
    % (lo[2], lo[2] + SHED_CENTRE[2], NORTH_FACE_WORLD, 2.570, 2.600)

for ax, name, half in ((0, "x", HALF_X), (1, "y", HALF_Y), (2, "z", HALF_Z)):
    assert abs(lo[ax] + half) < 1e-3 and abs(hi[ax] - half) < 1e-3, \
        "shed %s spans %.4f..%.4f, want %.3f..%.3f -- the mesh is not centred on " \
        "its own origin and its collider cannot be made to match" \
        % (name, lo[ax], hi[ax], -half, half)

assert info["tris"] <= 6000, "over budget: %d tris" % info["tris"]
print("shed: node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
      % (SHED_CENTRE + (HALF_X, HALF_Y, HALF_Z)))
print("shed: %d tris, %d verts, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("shed: self-check passes")
