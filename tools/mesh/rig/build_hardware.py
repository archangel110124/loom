# tools/mesh/rig/build_hardware.py
"""The rig's ironwork -- the mast, and one bollard placed twice.

Replaces the DRAWN surface of three `cylinder` primitives in
assets/games/deeper_demo.loom:

    Mast         pos [ 8.00, 3.60,  3.00]  scale [0.18, 2.20, 0.18]
    BollardWest  pos [-6.60, 1.70, -6.40]  scale [0.22, 0.30, 0.22]
    BollardEast  pos [-3.40, 1.70, -6.40]  scale [0.22, 0.30, 0.22]

**Both bollards are the same shape**, so this emits ONE bollard mesh and the
scene places it twice (spec §1 Case 6). They are NOT merged into one mesh: that
would give both one collider spanning 3.64 m of the boarding gap, which is the
worked warning Case 6 carries.

**All three are cylinders, so spec §1 Case 5 applies**: the collider shape today
is round, chosen by the string "cylinder" in play.rs:588-591. A mesh gets a
box, which exceeds the cylinder by r(sqrt2 - 1) at the corners -- 32 mm on a
bollard. Step 6 measures whether the boarding walk notices.

Run: blender --background --factory-startup --python build_hardware.py
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
MAST_PATH = os.path.join(REPO, "assets", "meshes", "rig_mast.obj")
BOLLARD_PATH = os.path.join(REPO, "assets", "meshes", "rig_bollard.obj")

MAST_CENTRE = (8.0, 3.60, 3.0)
MAST_R, MAST_HY = 0.18, 2.20
BOLLARD_CENTRE_Y = 1.70
BOLLARD_R, BOLLARD_HY = 0.22, 0.30
BOLLARD_X = (-6.60, -3.40)        # the scene places one mesh at each
SIDES = 16
SEED = 23
ZONE_SIZE = 1024         # spec §3: this zone is 1024², not TEX_SIZE's 2048

# `tube()` is reproduced here rather than imported, because `build_substructure`
# executes at import time. **Its U comes from the ring index, never from
# `atan2`.** The atan2 form has no seam split: at the +/-pi crossing U jumps
# instead of wrapping, and one facet of every tube ends up drawing 94% of the
# texture, reversed and compressed. Measured in Phase 1 at a max face U span of
# 0.9375 where 0.0625 is correct for 16 sides. Do not "simplify" it back.
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

    def along_of(co):
        return sum((co[i] - p0[i]) * ax[i] for i in range(3))

    # U around the circumference (one tile per full turn), V along the length at
    # one tile per metre, so a 2.4 m pile shows 2.4 tiles and the rust's downward
    # runs stay the right way up and the right size on every part.
    #
    # **U comes from the RING INDEX `s`, not from `atan2` of the vertex.** Two
    # separate defects came out of the atan2 version, both measured on the
    # shipped meshes 2026-08-26 with a per-face max/median U-span sweep:
    #
    #   1. *No seam split.* `atan2` returns a value per VERTEX, so the two
    #      vertices either side of the -pi/+pi crossing get U 0.9688 and
    #      0.0312 and the quad between them spans 0.9375 backwards instead of
    #      0.0625 forwards. One of sixteen side quads on `rig_pile.obj` and on
    #      `rig_steel_tidal.obj` drew 94% of the texture, reversed and
    #      compressed 15:1 — on all six piles and all six seams, 12 of this
    #      scene's 27 renderables. The ring index has no such crossing: the
    #      last quad runs (sides-1)/sides -> sides/sides = 1.0, and the texture
    #      tiles, so U=1.0 samples what U=0.0 samples.
    #   2. *Wrong axis entirely for a tube that is not z-aligned.*
    #      `atan2(y - mid_y, x - mid_x)` is the angle about Blender's z, which
    #      is only "around the circumference" when the tube's own axis IS z.
    #      The bracing runs along x and along z, so U was being taken from the
    #      ALONG axis: measured median face U span 0.4968 and max 0.9984 on an
    #      8-sided tube where 0.125 is correct, U running down the tube and the
    #      circumference carrying no variation at all. `s` is the tube's own
    #      frame by construction and is orientation-free.
    #
    # V needed no change and is correct for any orientation: `along` is a
    # projection onto the tube's own axis `ax`, not onto a world axis. Measured
    # after the fix: the bracing's V spans equal its tube lengths (10.5 and
    # 11.0 m) as intended, unchanged from before.
    faces = []
    for s in range(sides):
        a, b = rings[0][s], rings[0][(s + 1) % sides]
        c, d = rings[1][s], rings[1][(s + 1) % sides]
        f = bm.faces.new((a, b, d, c))
        for loop in f.loops:
            k = s + 1 if loop.vert in (b, d) else s
            loop[uv_layer].uv = (uv_u0 + k / sides, along_of(loop.vert.co))
        faces.append(f)

    # The caps are discs, not a strip around anything, so a circumferential U
    # is meaningless on them — feeding them the ring index would reintroduce
    # exactly the 0.9375 span this fix removes, on a face that has no seam to
    # split. They get a planar patch in the tube's OWN (u, v) frame instead,
    # at the same one-tile-per-metre scale as V, so a cap is 2*radius of
    # texture and nothing is stretched.
    for p, ring in ((p0, tuple(reversed(rings[0]))), (p1, tuple(rings[1]))):
        f = bm.faces.new(ring)
        for loop in f.loops:
            o = [loop.vert.co[i] - p[i] for i in range(3)]
            loop[uv_layer].uv = (uv_u0 + sum(o[i] * u[i] for i in range(3)),
                                 along_of(p) + sum(o[i] * v[i] for i in range(3)))
        faces.append(f)
    return faces


for name, path, r, hy, centre in (
        ("rig_mast", MAST_PATH, MAST_R, MAST_HY, MAST_CENTRE),
        ("rig_bollard", BOLLARD_PATH, BOLLARD_R, BOLLARD_HY, None)):
    for o in list(bpy.data.objects):
        bpy.data.objects.remove(o, do_unlink=True)
    mesh = bpy.data.meshes.new(name)
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(obj)
    bm = bmesh.new()
    uv_layer = bm.loops.layers.uv.new("UVMap")
    tube((0.0, 0.0, -hy), (0.0, 0.0, hy), r, SIDES, 0.0)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    bm.to_mesh(mesh)
    bm.free()
    rigkit.export_obj(obj, path, uv=True,
                      header="rig: %s, r%.2f half-height %.2f, %d sides, centred "
                             "on its own origin." % (name, r, hy, SIDES))
    i = rigkit.verify_obj(path)
    for ax, axn, half in ((0, "x", r), (1, "y", hy), (2, "z", r)):
        assert abs(i["lo"][ax] + half) < 1e-3 and abs(i["hi"][ax] - half) < 1e-3, \
            "%s %s spans %.4f..%.4f, want %.3f..%.3f — not centred on its own " \
            "origin, so no node transform can match its collider" \
            % (name, axn, i["lo"][ax], i["hi"][ax], -half, half)
    print("%s: %d tris, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
          % (name, i["tris"], i["lo"][0], i["hi"][0], i["lo"][1], i["hi"][1],
             i["lo"][2], i["hi"][2]))

albedo, height = textures.painted_iron(size=ZONE_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_hardware_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_hardware_normal.png"),
                   textures.normal_from_height(height))

print("hardware: mast node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
      % (MAST_CENTRE + (MAST_R, MAST_HY, MAST_R)))
for x in BOLLARD_X:
    print("hardware: bollard node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
          % (x, BOLLARD_CENTRE_Y, -6.40, BOLLARD_R, BOLLARD_HY, BOLLARD_R))

# The pair must still leave a gap a 0.35 m capsule can pass. Boxes, not
# cylinders, once these are meshes -- so measure the box faces, not the radii.
inner = (BOLLARD_X[1] - BOLLARD_R) - (BOLLARD_X[0] + BOLLARD_R)
assert inner > 2 * 0.35 + 0.20, \
    "only %.3f m between the bollards' box faces; a 0.35 m capsule needs 0.70 " \
    "plus clearance" % inner
print("hardware: %.3f m clear between the bollards' box faces" % inner)
print("hardware: self-check passes")
