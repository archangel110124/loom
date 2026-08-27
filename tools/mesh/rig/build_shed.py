# tools/mesh/rig/build_shed.py
"""The rig's shed -- horizontal lapped weatherboard on a plinth, with a door,
and nothing proud of the north face.

**Two meshes.** `rig_shed.obj` is the painted building; `rig_shed_bearers.obj`
is the timbers it stands on. They are split because the engine is one OBJ per
material and never reads `.mtl`, so a shared mesh means a shared albedo -- and
the bearers shipped painted the same blue as the wall above them, which the
photograph is emphatic they are not. The bearer node is renderable and carries
NO collider: the Shed node's BoxCollider already spans world y 1.400..3.800 and
contains that volume whole, and those numbers are frozen by `green.sh:1903`.

Replaces the DRAWN surface of `Shed` in assets/games/deeper_demo.loom, a single
`box` at `pos = [-4.0, 2.60, 4.60]`, `scale = [4.5, 1.20, 2.00]` -- world
x -8.500..0.500, y 1.400..3.800, z 2.600..6.600.

**THE NORTH FACE IS THE MIRROR'S MOUNT AND MUST STAY EXACTLY AT z = 2.600.**
`MirrorFrame` occupies z 2.570..2.600 with its back flush to that face, and
`Mirror` sits in front of it at z 2.550..2.570. A board standing proud of the
face punches through the frame. The demo's own header records getting this
stack backwards as the mistake that cost an hour.

The wall is the reference's: horizontal lapped weatherboard, each board's TOP
edge against the sheathing and its BOTTOM edge standing proud, lapping the
board below. So the construction is inverted from the obvious one -- the PROUD
BOTTOM EDGE sits at the face plane and everything else recedes behind it. That
is the same solve the battens used before it, for the same reason, and it is
also how a weatherboard is really nailed.

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
BEARER_PATH = os.path.join(REPO, "assets", "meshes", "rig_shed_bearers.obj")
TEX_DIR = os.path.join(REPO, "assets", "textures")

# World extents, straight off the node. HALF_* are the primitive's `scale`.
SHED_CENTRE = (-4.0, 2.60, 4.60)
HALF_X, HALF_Y, HALF_Z = 4.5, 1.20, 2.00
NORTH_FACE_WORLD = SHED_CENTRE[2] - HALF_Z          # 2.600 -- frozen

# --- the wall, as the reference builds it ---------------------------------
# Horizontal lapped weatherboard. Each board's TOP edge sits against the
# sheathing and its BOTTOM edge stands BOARD_T proud, lapping the board below.
# In section the outer surface is a sawtooth, and the step at each board's
# bottom is the shadow line the whole idiom is made of.
#
# The proud edge is the bottom one, and on the north wall proud points at the
# mirror. So the proud edge lands ON the wall plane and the rest recedes behind
# it -- the same solve the battens used, for the same reason.
BOARD_H = 0.180          # board height, top edge to bottom edge
LAP = 0.045              # how far each board laps over the one below
BOARD_T = 0.022          # board thickness = how far the bottom edge stands proud
CORNER_W = 0.090         # corner board covering each vertical joint
PLINTH_H = 0.150         # bearers the shed stands on
PLINTH_W = 0.180         # bearer width
PLINTH_N = 5             # bearers along the length, daylight between them
DOOR_W, DOOR_H = 0.90, 1.95
SEED = 29
ZONE_SIZE = 1024         # spec §3: this zone is 1024², not TEX_SIZE's 2048

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

# **Two meshes, not one, and the reason is the material.** The engine is one
# OBJ per material and never reads `.mtl` (`rigkit`'s rule 4), so anything
# sharing this mesh shares the shed's painted-blue albedo. The bearers the
# building stands on are bare timber in the photograph -- and they came out
# painted the same blue as the wall above them, which reads as a building
# that was tipped up and dipped. Splitting them into their own mesh and node
# is the only way to give them their own map, and it is the same solve
# `build_bulwark.py` documents for its fourteen runs.
bm = None
uv_layer = None


def begin(name):
    """Start a new mesh, and point the module-level `bm`/`uv_layer` at it.

    `slab` and `board` write to those globals, which is what the file already
    did with one mesh; rebinding them is a smaller change than threading a
    bmesh through both signatures and every call.
    """
    global bm, uv_layer
    mesh = bpy.data.meshes.new(name)
    o = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(o)
    bm = bmesh.new()
    uv_layer = bm.loops.layers.uv.new("UVMap")
    return o, mesh


def finish(o, mesh, path, header):
    """Recalculate normals per the export contract, write the OBJ, verify."""
    global bm
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    bm.to_mesh(mesh)
    bm.free()
    bm = None
    rigkit.export_obj(o, path, uv=True, header=header)
    return rigkit.verify_obj(path)


os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)

obj, mesh = begin("rig_shed")


def slab(x0, x1, y0, y1, z0, z1, uv_u0, run):
    """One box in LOCAL metres (already centred). U across the run, V up it.

    `run` is the LOCAL axis this part is laid out along -- "x" for a bearer or
    a cap spanning the length, "z" for a corner board. U has to come from that
    axis or it comes from a direction the part has no width in: a corner board
    spans CORNER_W in both x and z, so either serves, but a gable board of the
    old construction spanned 0.200 m in local z and only 0.018 m in x, and a U
    taken from x came out constant across the whole visible face. Measured
    before this argument existed: 1028 of 3084 faces had a zero U span.
    """
    c = [(x0, -z0, y0), (x1, -z0, y0), (x1, -z1, y0), (x0, -z1, y0),
         (x0, -z0, y1), (x1, -z0, y1), (x1, -z1, y1), (x0, -z1, y1)]
    v = [bm.verts.new(p) for p in c]
    quads = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
             (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
    faces = [bm.faces.new(tuple(v[i] for i in q)) for q in quads]
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            # Blender y is NEGATIVE local z (see `c` above), so local z is -y.
            u = x if run == "x" else -y
            # One tile per 1 m across and per 2 m up.
            loop[uv_layer].uv = (uv_u0 + u / 1.0, z / 2.0)
    return faces


def board(axis, plane, outward, a, b, y0, y1, uv_u0):
    """One lapped weatherboard, in LOCAL metres.

    `axis` is the wall's fixed axis ("x" or "z"), `plane` its coordinate, and
    `outward` the sign pointing out of the shed. The board's BOTTOM edge sits at
    `plane` and its top recedes BOARD_T inward, which is what makes the section
    a sawtooth and throws the shadow line.
    """
    out = plane                       # the proud bottom edge -- ON the plane
    inn = plane - outward * BOARD_T   # the recessed top edge
    back = plane - outward * (BOARD_T + 0.030)   # the sheathing behind it

    def P(u, depth, y):
        """A vertex on this wall: `u` along the run, `depth` across it, `y` up.

        For a z-wall the run is x and the depth is z; for an x-wall they swap.
        Getting this pair backwards builds the wall lying down -- measured, not
        feared: written as `(u, y, depth)` this emitted a mesh spanning y
        -2.000..2.000, the DEPTH axis standing in for up, and the y centring
        assertion caught it. So do not paraphrase it.

        Blender here is Z-UP and `slab` above is the reference for the mapping:
        local (x, y, z) -> Blender (x, -z, y). `rigkit`'s export permutation
        then puts it back, (bx, by, bz) -> obj (bx, bz, -by). The up
        coordinate MUST go to Blender z, and local z to Blender -y.
        """
        if axis == "z":
            return bm.verts.new((u, -depth, y))
        return bm.verts.new((depth, -u, y))

    # Outer face: proud at the bottom, recessed at the top.
    o0b, o1b = P(a, out, y0), P(b, out, y0)
    o0t, o1t = P(a, inn, y1), P(b, inn, y1)
    # Inner face, flat against the sheathing.
    i0b, i1b = P(a, back, y0), P(b, back, y0)
    i0t, i1t = P(a, back, y1), P(b, back, y1)

    faces = [
        bm.faces.new((o0b, o1b, o1t, o0t)),   # the visible sloped face
        bm.faces.new((i0t, i1t, i1b, i0b)),   # back
        bm.faces.new((o0b, o0t, i0t, i0b)),   # end a
        bm.faces.new((o1t, o1b, i1b, i1t)),   # end b
        bm.faces.new((o0t, o1t, i1t, i0t)),   # top
        bm.faces.new((o1b, o0b, i0b, i1b)),   # bottom -- this is the shadow line
    ]
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            # The run axis, matching P(): a z-wall runs along local x, which is
            # Blender x; an x-wall runs along local z, which is Blender -y.
            # Blender z is the up axis for both, so it is what V comes from.
            u = x if axis == "z" else -y
            # One tile per 2 m along the run and per 1 m up. The grain runs
            # along the board, so U is the along-grain axis and gets the longer
            # span -- matching `weathered_paint`, which is built for horizontal
            # siding. Getting this the other way round makes the boards look
            # like they are standing on end.
            loop[uv_layer].uv = (uv_u0 + u / 2.0, z / 1.0)
    return faces


# Walls, as (fixed axis, plane, from, to, outward sign). Local coordinates.
WALLS = [
    ("z", -HALF_Z, -HALF_X, HALF_X, -1),   # north -- THE MIRROR FACE
    ("z",  HALF_Z, -HALF_X, HALF_X, +1),   # south
    ("x", -HALF_X, -HALF_Z, HALF_Z, -1),   # west
    ("x",  HALF_X, -HALF_Z, HALF_Z, +1),   # east -- carries the door
]

rng_state = SEED
def rand():
    global rng_state
    rng_state = (rng_state * 1103515245 + 12345) & 0x7FFFFFFF
    return rng_state / 0x7FFFFFFF


# The shed stands on bearers, so the boarding starts above them.
SILL = -HALF_Y + PLINTH_H
step = BOARD_H - LAP                       # exposed height of each course
rows = int((2 * HALF_Y - PLINTH_H) / step)

for axis, plane, a, b, outward in WALLS:
    door = (axis == "x" and outward > 0)
    # Corner boards stop the lapped ends short, exactly as the reference does.
    a2, b2 = a + CORNER_W, b - CORNER_W
    for r in range(rows):
        y0 = SILL + r * step
        y1 = min(y0 + BOARD_H, HALF_Y)
        if y0 >= HALF_Y:
            break
        if door and y0 < SILL + DOOR_H:
            # The door is a gap in the boarding, centred on the run.
            board(axis, plane, outward, a2, -DOOR_W * 0.5, y0, y1, rand())
            board(axis, plane, outward, DOOR_W * 0.5, b2, y0, y1, rand())
        else:
            board(axis, plane, outward, a2, b2, y0, y1, rand())

# Corner boards: one plain vertical plank at each of the four verticals, sitting
# at the wall plane so the lapped ends die into it.
#
# An earlier version of this comment claimed these were what reach +/-HALF_X and
# +/-HALF_Z, and that the x and z centring assertions fail without them. Both
# false, and checked by deleting them: everything still passes at 1020 tris. The
# wall PLANES pin x and z on their own -- the east and west walls sit at
# +/-HALF_X and the north and south at +/-HALF_Z, by construction. What these
# actually pin is y, jointly with the cap. They are here because the reference
# has them and the corners read as mitres without them, which is reason enough.
for cx in (-HALF_X, HALF_X):
    for cz in (-HALF_Z, HALF_Z):
        sx = 1 if cx > 0 else -1
        sz = 1 if cz > 0 else -1
        slab(cx - sx * CORNER_W, cx, SILL, HALF_Y,
             cz - sz * CORNER_W, cz, rand(), "z")

# A thin cap so the box is closed from above and `verify_obj`'s per-shell check
# has no open shell to refuse. Task 3's roof is a separate mesh with an
# overhang, so this is a lid and not a roof.
#
# It reaches y = HALF_Y, which the top course of boards does not -- that ends at
# y 1.155. But an earlier comment here claimed the y centring assertion fails
# without the cap, and that is false: the corner boards also reach HALF_Y, so
# either one alone holds it. Removing BOTH is what fails. Verified by deleting
# each in turn.
slab(-HALF_X, HALF_X, HALF_Y - 0.04, HALF_Y, -HALF_Z, HALF_Z, 0.0, "x")

info = finish(obj, mesh, OBJ_PATH,
              "rig: shed, horizontal lapped weatherboard. Each board's proud "
              "BOTTOM edge sits on the wall plane and its top recedes %.0f mm. "
              "Nothing proud of the north face. The bearers under it are "
              "rig_shed_bearers.obj -- a separate mesh because they are bare "
              "timber, not painted." % (BOARD_T * 1000))

# --- the plinth: its own mesh, its own node, bare timber -------------------
# Bearers running across the shed with daylight between them. Emitted CENTRED
# ON THEIR OWN ORIGIN in all three axes, like every other rig mesh, so one
# node transform places them: the group spans local y -HALF_Y..SILL, whose
# centre is BEARER_CY below the shed's own.
#
# **It gets NO collider.** The shed's BoxCollider already spans world
# y 1.400..3.800, which contains this volume whole; adding a second one would
# change what `green.sh:1903` strafes into, and that node's numbers are frozen.
BEARER_CY = (-HALF_Y + SILL) / 2.0
BEARER_HY = PLINTH_H / 2.0                                    # y half-extent
bobj, bmesh_data = begin("rig_shed_bearers")
bp = (2 * HALF_X - PLINTH_W) / (PLINTH_N - 1)
for i in range(PLINTH_N):
    px = -HALF_X + i * bp
    slab(px, px + PLINTH_W, -HALF_Y - BEARER_CY, SILL - BEARER_CY,
         -HALF_Z, HALF_Z, rand(), "z")
binfo = finish(bobj, bmesh_data, BEARER_PATH,
               "rig: shed bearers -- %d timbers %.0f mm wide under the shed, "
               "bare timber, NOT painted. Centred on its own origin. Node "
               "pos=(%.3f, %.3f, %.3f) relative to the shed's own centre "
               "(0, %.3f, 0). No collider: the Shed node's already covers this."
               % (PLINTH_N, PLINTH_W * 1000,
                  SHED_CENTRE[0], SHED_CENTRE[1] + BEARER_CY, SHED_CENTRE[2],
                  BEARER_CY))

albedo, height = textures.weathered_paint(size=ZONE_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_shed_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_shed_normal.png"),
                   textures.normal_from_height(height))

# --- self-check ----------------------------------------------------------
lo, hi = info["lo"], info["hi"]
blo, bhi = binfo["lo"], binfo["hi"]

# **THE ONE THAT MATTERS, and it runs FIRST.** Local -HALF_Z is world 2.600, the
# mirror's mount. It is checked before the generic centring loop because a
# symmetric fault trips centring on x first and this would never run -- that
# happened in Phase 2 and the assertion went unexercised.
assert lo[2] >= -HALF_Z - 1e-4, \
    "a vertex reaches local z=%.5f (world %.5f), north of the shed's face at " \
    "world %.3f — MirrorFrame occupies 2.570..2.600 and this punches through it" \
    % (lo[2], lo[2] + SHED_CENTRE[2], NORTH_FACE_WORLD)

for ax, name, half in ((0, "x", HALF_X), (2, "z", HALF_Z)):
    assert abs(lo[ax] + half) < 1e-3 and abs(hi[ax] - half) < 1e-3, \
        "shed %s spans %.4f..%.4f, want %.3f..%.3f — not centred on its own " \
        "origin, so its collider cannot be made to match" \
        % (name, lo[ax], hi[ax], -half, half)

# **y is no longer symmetric, and that is the bearer split.** The bearers were
# the only geometry below the sill; they are now their own mesh, so this one
# spans SILL..HALF_Y and stops PLINTH_H short of the collider's floor. The
# COLLIDER does not move -- `green.sh:1903` strafes a character into world
# y 1.400..3.800 and those numbers are frozen -- so what is checked here is
# that the boarding still starts exactly at the sill and the cap still closes
# it at the top. Moving either (a course emitted below SILL, a cap that stops
# short of HALF_Y) fires this.
assert abs(lo[1] - SILL) < 1e-3 and abs(hi[1] - HALF_Y) < 1e-3, \
    "shed y spans %.4f..%.4f, want %.3f..%.3f — the boarding starts at the " \
    "sill and the cap closes it at HALF_Y; anything else means a course or " \
    "the cap has moved" % (lo[1], hi[1], SILL, HALF_Y)

# --- the bearers: their own mesh, centred on their own origin -------------
# Same north-face rule, checked on this mesh too: it spans the full z depth,
# so an edit that widened it past -HALF_Z would punch the mirror frame exactly
# as a wall board would, and the shed's own check above would never see it.
assert blo[2] >= -HALF_Z - 1e-4, \
    "a bearer vertex reaches local z=%.5f (world %.5f), north of the shed's " \
    "face at world %.3f — MirrorFrame occupies 2.570..2.600" \
    % (blo[2], blo[2] + SHED_CENTRE[2], NORTH_FACE_WORLD)

for ax, name, half in ((0, "x", HALF_X), (1, "y", BEARER_HY), (2, "z", HALF_Z)):
    assert abs(blo[ax] + half) < 1e-3 and abs(bhi[ax] - half) < 1e-3, \
        "bearers %s span %.4f..%.4f, want %.3f..%.3f — not centred on their " \
        "own origin, so one node transform cannot place them" \
        % (name, blo[ax], bhi[ax], -half, half)

# **Why the bearers need no collider of their own.** They sit inside the
# volume the Shed node's BoxCollider already covers, world y 1.400..3.800.
# Only the TOP of that containment is worth asserting: the bearers' world
# floor is `SHED_CENTRE[1] - HALF_Y + PLINTH_H/2 - PLINTH_H/2`, identically
# the collider's floor whatever PLINTH_H is, so a lower-bound assertion here
# could not fail for any input and is not written. The upper bound can:
# PLINTH_H > 2*HALF_Y (2.400 m) pushes the bearers out through the collider's
# roof, and that is the input that fires this.
BEARER_TOP = SHED_CENTRE[1] + BEARER_CY + BEARER_HY
assert BEARER_TOP <= SHED_CENTRE[1] + HALF_Y + 1e-4, \
    "the bearers reach world y %.4f, above the Shed collider's ceiling at " \
    "%.3f — PLINTH_H %.3f exceeds the shed's full height %.3f, so they would " \
    "need a collider of their own" \
    % (BEARER_TOP, SHED_CENTRE[1] + HALF_Y, PLINTH_H, 2 * HALF_Y)

# **The sawtooth points the right way.** Every board's proud edge is its BOTTOM
# one. Read the shipped mesh and confirm: on the north wall, the vertices AT the
# plane must be the low ones of their board, and the recessed vertices above
# them. If the section were inverted the wall would still be inside the plane
# and would still be centred -- both checks above would pass -- and the shadow
# would fall the wrong way, which is the one thing a photograph would show and
# no number here would.
d = rigkit.read_obj(OBJ_PATH)
# Sample the BOARDS only. The corner boards also sit on the north plane,
# spanning the full wall height, and dragging them into this mean would swamp
# the boards' own signal and make the test read whatever those happen to
# average. Exclude them by extent. (The bearers used to be excluded here too;
# since the split they are not in this mesh at all, and the y window below
# still excludes anything at or below the sill either way.)
#
# The x window is INCLUSIVE of the corner-board line and the exclusion is done
# in y instead, which is not a paraphrase but a correction. Measured on the
# shipped file: EVERY board vertex on the north plane sits at x = ±4.410
# exactly -- the boards run a2..b2 = ±(HALF_X - CORNER_W) and the north wall
# has no door, so it has no other x at all. A window that excludes ±4.410
# excludes the entire sample and `at_plane` comes out empty. The corner boards
# share that same x, so x cannot separate them; y can, because a corner board
# spans SILL..HALF_Y and therefore has vertices at exactly those two values and
# nowhere between, while every board's proud edge is strictly inside them.
# Cross-checked at the plane: x=±4.410 carries 16 board bottoms plus the corner
# board's y=-1.050 and y=1.200, and this window keeps 15 board bottoms per end
# and neither corner vertex.
# **All four walls, not just the north one.** The first version of this check
# filtered on the north plane alone, and an inversion applied to the south, east
# or west wall passed it silently -- three quarters of the shed shipping with no
# check at all. The generalisation is cheap; the coverage gap was not.
def _board_vert(v, axis):
    """Is this a weatherboard vertex on `axis`'s wall, rather than a corner
    board or the cap?

    The run window is the axis the boards RUN along -- x for a z-wall, z for an
    x-wall. It is INCLUSIVE of the corner-board line, because every board vertex
    sits exactly on it: the boards run a2..b2 = +/-(half - CORNER_W) and have no
    other coordinate there. A window that excludes that line excludes the whole
    sample. So y does the separating instead: a corner board spans SILL..HALF_Y
    and has vertices at exactly those two values and nowhere between. The cap is
    caught by the run window (its verts sit at +/-HALF_X and +/-HALF_Z, outside
    every wall's run).
    """
    run = v[0] if axis == "z" else v[2]
    half = HALF_X if axis == "z" else HALF_Z
    return (-half + CORNER_W - 1e-4 < run < half - CORNER_W + 1e-4
            and SILL + 1e-4 < v[1] < HALF_Y - 1e-4)

for axis, plane, _a, _b, outward in WALLS:
    k = 2 if axis == "z" else 0
    r0 = plane - outward * BOARD_T * 1.5
    r1 = plane - outward * BOARD_T * 0.5
    r_lo, r_hi = min(r0, r1), max(r0, r1)
    at_plane = [v for v in d["verts"]
                if abs(v[k] - plane) < 1e-6 and _board_vert(v, axis)]
    assert at_plane, "no board vertex sits on the %s=%.3f plane at all" % (axis, plane)
    recessed = [v for v in d["verts"]
                if r_lo < v[k] < r_hi and _board_vert(v, axis)]
    assert recessed, \
        "nothing sits one board-thickness behind the %s=%.3f plane" % (axis, plane)
    mean_proud = sum(v[1] for v in at_plane) / len(at_plane)
    mean_back = sum(v[1] for v in recessed) / len(recessed)
    assert mean_proud < mean_back, \
        "the sawtooth is inverted on the %s=%.3f wall: the proud edge averages " \
        "y=%.4f and the recessed edge y=%.4f, so the boards lap upward and " \
        "every shadow falls the wrong way" % (axis, plane, mean_proud, mean_back)
print("shed: sawtooth verified proud-edge-down on all %d walls" % len(WALLS))

# Spec §3 gives the `shed_timber` zone 6000 tris. The bearers left that zone
# with their mesh -- they draw with the deck's timber maps now -- so they are
# counted and printed separately rather than folded into this number.
assert info["tris"] <= 6000, "over budget: %d tris" % info["tris"]
print("shed: node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
      % (SHED_CENTRE + (HALF_X, HALF_Y, HALF_Z)))
print("shed: %d courses of %.3f m, lap %.3f, proud %.3f; %d bearers"
      % (rows, BOARD_H, LAP, BOARD_T, PLINTH_N))
print("shed: %d tris, %d verts, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("bearers: node pos=(%.3f, %.3f, %.3f) half_extents=(%.3f, %.3f, %.3f), "
      "NO collider" % (SHED_CENTRE[0], SHED_CENTRE[1] + BEARER_CY,
                       SHED_CENTRE[2], HALF_X, BEARER_HY, HALF_Z))
print("bearers: %d tris, %d verts, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (binfo["tris"], binfo["verts"], blo[0], bhi[0], blo[1], bhi[1],
         blo[2], bhi[2]))
print("shed: self-check passes")
