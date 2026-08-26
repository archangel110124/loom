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
DECK_TOP = 1.400                  # frozen: every spawn and assert row reads it -- a WORLD height
DECK_BOTTOM = 1.000               # the primitive's underside -- also WORLD
# play.rs:520 anchors a static collider's BoxCollider at the node's world
# position, and half_extents is LOCAL (:552-556) -- there is no node transform
# that can place a box collider under a mesh baked at absolute height. So the
# mesh is baked centred on its own local origin instead, and the scene node
# carries pos.y = DECK_CENTRE to put it back at DECK_TOP/DECK_BOTTOM in world.
DECK_CENTRE = 1.200                # the node y this mesh is authored to hang from
PLANK_W = 0.200                   # across the deck, in Z
GAP = 0.012                       # a gap you can see the sea through
CUP = 0.008                       # centre dips this far below the edges
MIN_BOARD = 0.30                  # narrowest legal board width
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


def plank(z0, x0, x1, sink, uv_u0, uv_v0):
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
            row.append(bm.verts.new((x, -z, top - dip - DECK_CENTRE)))
        verts_top.append(row)

    bot = [[bm.verts.new((x, -(z0 + (s / SPAN) * plank_w), DECK_BOTTOM - DECK_CENTRE))
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

    # UVs: 1 texture tile per 2 m along the board, 1 per board across. **Every
    # board gets its own patch**, not one of four. The Phase 0 code used
    # `(c % 4) * 0.25`, which gave 66 courses four distinct grain bands and made
    # the repeat plainly visible at standing height. `uv_u0` does the same job
    # along the board, so two boards meeting at a butt joint do not continue
    # each other's grain.
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            loop[uv_layer].uv = (uv_u0 + x / 2.0,
                                 uv_v0 + (-y - z0) / plank_w * 0.25)
    return faces


rng_state = SEED
def rand():
    """A tiny LCG. Deterministic, and independent of Python's hash seed."""
    global rng_state
    rng_state = (rng_state * 1103515245 + 12345) & 0x7FFFFFFF
    return rng_state / 0x7FFFFFFF


course_spans = [[] for _ in range(courses)]
# Board geometry is decided in this loop and only this loop -- (z0, x0, x1,
# sink) per board, in emission order. UVs are drawn in a SEPARATE pass below,
# after every course's board count and cut positions are already fixed. Draw
# them from the same rand() calls here instead and the deck's board layout
# stops being deterministic-and-fixed: an extra rand() call per board shifts
# every later course's `n` and `cuts` draws too, so boards 0..N are unchanged
# but course counts downstream drift -- 66 tris * 28 became 7420 tris instead
# of the frozen 7084 the first time this was tried. Two passes over the same
# LCG stream keep geometry bit-identical to Phase 0 while still giving every
# board its own patch of texture.
boards = []

for c in range(courses):
    z0 = -HALF_Z + c * pitch
    # Butt joints: 3 to 5 boards per course, staggered course to course so the
    # joints never line up. A deck whose joints line up reads as a texture.
    n = 3 + int(rand() * 3)
    cuts = sorted(-HALF_X + (2 * HALF_X) * rand() for _ in range(n - 1))
    # Reject the CUT, don't drop the BOARD. A cut too close to the last kept
    # edge or too close to +HALF_X would leave a sliver on one side of it --
    # dropping that sliver board (the old behaviour) leaves an actual hole in
    # the deck, invisible to verify_obj's global min/max bounds check. Walking
    # the cuts and keeping only ones that clear MIN_BOARD on both sides makes
    # every resulting span legal by construction, so nothing is ever dropped.
    kept = []
    last = -HALF_X
    for x in cuts:
        if x - last >= MIN_BOARD and HALF_X - x >= MIN_BOARD:
            kept.append(x)
            last = x
    edges = [-HALF_X] + kept + [HALF_X]
    for k in range(len(edges) - 1):
        x0, x1 = edges[k] + (0.006 if k else 0.0), edges[k + 1]
        # Each board sits a little differently: a nail lifting, a board proud
        # of its neighbour is FORBIDDEN (it would exceed DECK_TOP), so the
        # variation is one-sided — boards sink, never rise.
        sink = rand() * 0.004
        boards.append((z0, x0, x1, sink))
        course_spans[c].append((x0, x1))

# Second pass: one board's worth of geometry is already fixed above, so these
# rand() calls can no longer perturb board counts or cut positions -- they
# only pick each board's patch of texture, still deterministic off the same
# LCG.
for z0, x0, x1, sink in boards:
    plank(z0, x0, x1, sink, rand(), rand())

# --- coverage self-check: a dropped sliver board is a hole in the deck ----
# verify_obj's bounds check is global min/max -- other courses still reach
# the edges, so a per-course gap is invisible to it and real in the asset.
# This walks the boards as emitted (not a re-parse of the OBJ) and asserts
# each course is one continuous run from -HALF_X to +HALF_X, joints only.
BUTT = 0.006 + 1e-4  # the internal board-start offset, plus float slack
for c, spans in enumerate(course_spans):
    z0 = -HALF_Z + c * pitch
    spans = sorted(spans)
    assert spans, "course z=%.4f emitted no boards at all" % z0
    assert abs(spans[0][0] + HALF_X) < 1e-6, \
        "course z=%.4f: hole x=%.4f..%.4f at the x=-12 edge (starts at %.4f, want %.3f)" \
        % (z0, -HALF_X, spans[0][0], spans[0][0], -HALF_X)
    assert abs(spans[-1][1] - HALF_X) < 1e-6, \
        "course z=%.4f: hole x=%.4f..%.4f at the x=+12 edge (ends at %.4f, want %.3f)" \
        % (z0, spans[-1][1], HALF_X, spans[-1][1], HALF_X)
    for (a0, a1), (b0, b1) in zip(spans, spans[1:]):
        gap = b0 - a1
        assert gap <= BUTT, \
            "course z=%.4f: hole x=%.4f..%.4f (%.1f cm) between boards" \
            % (z0, a1, b0, gap * 100)

# ONE call on the whole bmesh, not one per board — and that is not a violation
# of rigkit's rule 3. `recalc_face_normals` works per connected region already,
# so handing it every face at once IS the per-shell recalc; each board is its
# own region and gets its own outward orientation. `verify_obj` now proves it
# per shell, so if this were ever wrong the build would refuse, not ship black.
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
# This mesh is authored LOCAL, centred on DECK_CENTRE -- the scene node carries
# pos.y = DECK_CENTRE, so a local vertex at (DECK_TOP - DECK_CENTRE) sits at
# world DECK_TOP once placed. A vertex above that local top would still put
# drawn deck above the collider the player stands on once the node is placed.
LOCAL_TOP = DECK_TOP - DECK_CENTRE
assert hi[1] <= LOCAL_TOP + 1e-4, \
    "a vertex reaches local y=%.5f (world %.5f once the node sits at " \
    "DECK_CENTRE=%.3f), above the local top %.3f (world deck top %.3f) — the " \
    "player would see deck above the surface he stands on" \
    % (hi[1], hi[1] + DECK_CENTRE, DECK_CENTRE, LOCAL_TOP, DECK_TOP)
assert hi[1] > LOCAL_TOP - 1e-3, \
    "highest vertex is local y=%.5f, %.1f mm BELOW the local top %.3f (world " \
    "deck top %.3f) — the player would float" \
    % (hi[1], (LOCAL_TOP - hi[1]) * 1000.0, LOCAL_TOP, DECK_TOP)

assert info["tris"] <= 12000, "over budget: %d tris" % info["tris"]
assert len(rigkit.read_obj(OBJ_PATH)["uvs"]) > 0, "no UVs — the texture cannot land"

print("deck: %d tris, %d verts, x %.2f..%.2f y %.3f..%.3f z %.2f..%.2f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("deck: self-check passes")
