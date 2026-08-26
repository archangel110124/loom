# tools/mesh/rig/build_bulwark.py
"""The rig's bulwark and its cap rail — fourteen meshes, fourteen nodes.

Replaces the drawn surface of fourteen axis-aligned `box` primitives in
assets/games/deeper_demo.loom: seven rails (`RailSouth` :1163 .. `RailNorthEast`
:1256, world `y 1.400 .. 2.400`) and seven caps (`CapSouth` :1268 ..
`CapEastSouth` :1346, world `y 2.680 .. 2.920`). **Case 1 of spec §1** —
axis-aligned, drawn, statically parented — so an explicit `BoxCollider`
transcribing each primitive's numbers is exact, bit for bit.

**Fourteen meshes, not one.** The task-5 brief asked for a single combined
`rig_bulwark_timber.obj` / `rig_cap_metal.obj` — all seven rail runs (and all
seven caps) baked into one mesh apiece. One mesh is one node is one
`BoxCollider` (`play.rs:520` centres a static collider on the NODE), and the
combined mesh's bounding box is `x -12..12, z -7..7` — a solid slab at rail
height across the WHOLE rig. That collider would close the boarding gap
(`x -6.900..-3.100`, the demo's only taught gesture) and the swim-ladder gap
(`x -10.300..-8.300`), even though the DRAWN geometry correctly leaves both
open. The brief's own gap assertion walked the drawn spans and would have
passed while this happened, because it is the collider — not the geometry —
that closes a gap. All seven rail runs are different lengths (24.0, 14.0, 9.0,
2.2, 1.7, 1.4, 15.1 m) so they cannot share a mesh and a matching collider
regardless; the fix is the same one task 3b already made for the six piles:
one mesh per distinct shape, one node per placement, each mesh centred on its
OWN origin in x, y AND z (not just y — each node sits at its own world x/z
too) so a node transform can carry both mesh and `BoxCollider` to the same
place.

**The gap assertion must see the COLLIDER, not just the geometry.** A per-run
mesh happens to make the collider footprint equal the drawn span by
construction, which is exactly the property the single-mesh design broke.
`check_collider_gaps` below is fault-injection tested on every build: it
proves a fake collider filling the boarding lane gets caught, so the assertion
that matters here cannot silently go blind.

Run: blender --background --factory-startup --python build_bulwark.py
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bpy
import bmesh

import rigkit

REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                    "..", "..", ".."))
MESH_DIR = os.path.join(REPO, "assets", "meshes")

RAIL_LO, RAIL_HI = 1.400, 2.400   # frozen: world y, every rail run
CAP_LO, CAP_HI = 2.680, 2.920     # frozen: world y, every cap run

# (name, x0, x1, z0, z1) in WORLD metres, straight off the nodes in
# deeper_demo.loom (pos +- scale, since a `box` primitive's scale IS its
# half-extent). The two gaps exist here as the ABSENCE of a NorthWest..
# NorthMid..NorthEast span, so a gap cannot be lost inside a loop -- there is
# no loop that could close one.
RAILS = [
    ("South",     -12.000, 12.000,  6.800,  7.000),
    ("West",      -12.000, -11.800, -7.000,  7.000),
    ("EastNorth",  11.800, 12.000, -7.000,  2.000),
    ("EastSouth",  11.800, 12.000,  4.800,  7.000),
    ("NorthWest", -12.000, -10.300, -7.000, -6.800),
    ("NorthMid",   -8.300, -6.900, -7.000, -6.800),
    ("NorthEast",  -3.100, 12.000, -7.000, -6.800),
]
CAPS = [
    ("South",     -12.000, 12.000,  6.840,  6.960),
    ("NorthWest", -12.000, -10.300, -6.960, -6.840),
    ("NorthMid",   -8.300, -6.900, -6.960, -6.840),
    ("NorthEast",  -3.100, 12.000, -6.960, -6.840),
    ("West",      -11.960, -11.840, -7.000,  7.000),
    ("EastNorth",  11.840, 11.960, -7.000,  2.000),
    ("EastSouth",  11.840, 11.960,  4.800,  7.000),
]

GAPS = (("boarding", -6.900, -3.100), ("swim ladder", -10.300, -8.300))
NORTH_Z = -6.800   # a run's z1 <= this puts it on the north rail line

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)


def box_local(bm, uvl, x0, x1, y0, y1, z0, z1, cx, cy, cz):
    """One box, in WORLD metres, emitted centred on (cx, cy, cz).

    Blender z is Loom y; Blender y is Loom -z (rigkit's export permutation).
    A vertex at local Loom (x-cx, y-cy, z-cz) is therefore Blender
    (x-cx, cz-z, y-cy).
    """
    a, b = y0 - cy, y1 - cy
    corners = [(x0 - cx, cz - z0, a), (x1 - cx, cz - z0, a),
               (x1 - cx, cz - z1, a), (x0 - cx, cz - z1, a),
               (x0 - cx, cz - z0, b), (x1 - cx, cz - z0, b),
               (x1 - cx, cz - z1, b), (x0 - cx, cz - z1, b)]
    v = [bm.verts.new(c) for c in corners]
    quads = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
             (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
    faces = [bm.faces.new(tuple(v[i] for i in q)) for q in quads]
    if uvl is not None:
        # U along whichever local axis is the run's own long axis, V up the
        # local height -- one tile per 2 m along the run and per 1 m up, the
        # same texel density the deck uses so the two read as one structure.
        along_x = (x1 - x0) >= (z1 - z0)
        for f in faces:
            for loop in f.loops:
                bx, by, bz = loop.vert.co
                along = bx if along_x else by
                loop[uvl].uv = (along / 2.0, bz + 0.5)
    return faces


def build_run(kind, name, x0, x1, z0, z1, lo, hi, uv):
    """Export one run's box, centred on its own origin. Returns its record."""
    cx, cy, cz = (x0 + x1) / 2.0, (lo + hi) / 2.0, (z0 + z1) / 2.0
    hx, hy, hz = (x1 - x0) / 2.0, (hi - lo) / 2.0, (z1 - z0) / 2.0
    mesh_name = "rig_%s_%s" % (kind, name.lower())
    mesh = bpy.data.meshes.new(mesh_name)
    obj = bpy.data.objects.new(mesh_name, mesh)
    bpy.context.collection.objects.link(obj)
    bm = bmesh.new()
    uvl = bm.loops.layers.uv.new("UVMap") if uv else None
    box_local(bm, uvl, x0, x1, lo, hi, z0, z1, cx, cy, cz)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    bm.to_mesh(mesh)
    bm.free()
    path = os.path.join(MESH_DIR, mesh_name + ".obj")
    rigkit.export_obj(obj, path, uv=uv,
                      header="rig: %s %s. World x %.3f..%.3f y %.3f..%.3f "
                             "z %.3f..%.3f, centred on its own origin. Node "
                             "pos=(%.3f, %.3f, %.3f) half_extents=(%.3f, %.3f, "
                             "%.3f)."
                             % (kind, name, x0, x1, lo, hi, z0, z1,
                                cx, cy, cz, hx, hy, hz))
    info = rigkit.verify_obj(path)
    return {
        "kind": kind, "name": name, "path": path, "tris": info["tris"],
        "lo": info["lo"], "hi": info["hi"],
        "pos": (cx, cy, cz), "half": (hx, hy, hz),
        "is_north": z1 <= NORTH_Z,
    }


RUNS = []
for name, x0, x1, z0, z1 in RAILS:
    RUNS.append(build_run("rail", name, x0, x1, z0, z1, RAIL_LO, RAIL_HI, uv=True))
for name, x0, x1, z0, z1 in CAPS:
    RUNS.append(build_run("cap", name, x0, x1, z0, z1, CAP_LO, CAP_HI, uv=False))

# --- self-check: every mesh centred on its own origin ---------------------
# The whole reason this task exists: a node transform can only place BOTH a
# mesh and a matching BoxCollider at the same spot if the mesh is centred on
# the node it hangs from, in all three axes.
for r in RUNS:
    for ax, label in ((0, "x"), (1, "y"), (2, "z")):
        half = r["half"][ax]
        assert abs(r["lo"][ax] + half) < 1e-3 and abs(r["hi"][ax] - half) < 1e-3, \
            "%s %s %s spans %.4f..%.4f, want %.3f..%.3f -- not centred on " \
            "its own origin" \
            % (r["kind"], r["name"], label, r["lo"][ax], r["hi"][ax], -half, half)

print("bulwark: %d meshes, node positions and BoxCollider half-extents "
      "(what task 6 needs):" % len(RUNS))
for r in RUNS:
    print("  %-4s %-9s tris=%3d node_pos=(%7.3f, %5.3f, %7.3f) "
          "half_extents=(%6.3f, %5.3f, %6.3f)"
          % (r["kind"], r["name"], r["tris"], r["pos"][0], r["pos"][1],
             r["pos"][2], r["half"][0], r["half"][1], r["half"][2]))

total_tris = sum(r["tris"] for r in RUNS)
assert total_tris <= 9000, "over budget: %d tris across %d meshes" % (total_tris, len(RUNS))
print("bulwark: %d tris total across %d meshes" % (total_tris, len(RUNS)))

# --- gap check 1: the DRAWN geometry (the brief's own check, kept) --------
north_spans = [(x0, x1) for _, x0, x1, _, z1 in RAILS if z1 <= NORTH_Z]
for label, g0, g1 in GAPS:
    for x0, x1 in north_spans:
        assert x1 <= g0 + 1e-6 or x0 >= g1 - 1e-6, \
            "a rail span %.3f..%.3f intrudes into the %s gap %.3f..%.3f" \
            % (x0, x1, label, g0, g1)
print("bulwark: both north gaps intact in DRAWN geometry (%s)"
      % ", ".join("%s %.3f..%.3f" % (n, a, b) for n, a, b in GAPS))


# --- gap check 2: the COLLIDER footprint -----------------------------------
# **This is the check the brief was missing.** It walks the printed node
# positions and half-extents -- what Task 6 will actually give the physics
# engine -- not the emitted geometry. A single combined collider spanning the
# whole rig would sail through check 1 (the drawn spans still leave the gaps
# open) and fail here (its footprint covers them). Covers rails AND caps: the
# cap rail is split at the same x-ranges and gets its own colliders too.
def check_collider_gaps(entries, gaps):
    """entries: iterable of (label, x0, x1) collider footprints."""
    for gap_label, g0, g1 in gaps:
        for name, x0, x1 in entries:
            assert x1 <= g0 + 1e-6 or x0 >= g1 - 1e-6, \
                "collider %r footprint %.3f..%.3f intrudes into the %s gap " \
                "%.3f..%.3f" % (name, x0, x1, gap_label, g0, g1)


north_colliders = [("%s/%s" % (r["kind"], r["name"]),
                    r["pos"][0] - r["half"][0], r["pos"][0] + r["half"][0])
                   for r in RUNS if r["is_north"]]
check_collider_gaps(north_colliders, GAPS)
print("bulwark: both north gaps intact in COLLIDER footprint (%d colliders "
      "checked)" % len(north_colliders))

# --- fault injection: prove the collider check has teeth -------------------
# A fake collider that exactly fills the boarding lane -- the shape the
# single-combined-mesh design would have produced there. Run on every build,
# not just once by hand, so a future edit to check_collider_gaps cannot go
# blind without this catching it.
fake = north_colliders + [("FakeNorthGapCollider", -6.900, -3.100)]
try:
    check_collider_gaps(fake, GAPS)
except AssertionError as e:
    assert "boarding" in str(e), "fired, but not for the boarding gap: %s" % e
    print("bulwark: fault injection ok -- fake collider filling the boarding "
          "lane was caught: %s" % e)
else:
    raise AssertionError(
        "check_collider_gaps did NOT fire for a fake collider filling the "
        "boarding lane -- this is the exact check that would have let an "
        "unwalkable demo ship")

print("bulwark: self-check passes")
