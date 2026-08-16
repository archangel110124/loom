"""Export the meshes from a `.blend` to a Wavefront OBJ Loom can import.

Run headless:

    blender -b <file.blend> --python tools/mesh/blend_to_obj.py -- <out.obj> [max_faces]

**Why this exists.** Several vendors ship a model whose geometry is *only*
inside a `.blend` — the archive is otherwise all textures. `loom_asset` reads
OBJ and glTF, so without a conversion pass those assets cannot enter the engine
at all.

**OBJ rather than glTF, deliberately.** `import_gltf` merges every primitive
into one mesh and so does `import_obj`, so neither preserves a hierarchy; OBJ
keeps `o` groups, which `import_obj_object` can select with the `file.obj#Name`
fragment syntax. That is the difference between importing a lantern and
importing a lantern permanently fused to its glass.

**Triangulated and decimated on the way out.** Measured rather than assumed:
these `.blend` files ship an LOD1 mesh at almost exactly 50,000 faces each, not
the million-triangle film original the archive size suggests. 50k is still more
than this whole project's mesh library for one piece of set dressing a few
metres across, so `max_faces` applies a planar-collapse decimate above its
budget — 30,000 gives a 2.6 MiB OBJ per rock. Pass 0 to keep the original.

**+Y up, which is Blender's -Z forward.** Loom is Y-up like glTF; Blender is
Z-up. Getting this wrong lays every rock on its side, which is obvious, and
mirrors the normals, which is not.
"""
import sys

import bpy

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
if not argv:
    raise SystemExit("usage: ... --python blend_to_obj.py -- <out.obj> [max_faces]")
out = argv[0]
max_faces = int(argv[1]) if len(argv) > 1 else 40000

bpy.ops.object.select_all(action="DESELECT")

meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
if not meshes:
    raise SystemExit("no mesh objects in this .blend")

total_before = 0
for o in meshes:
    o.select_set(True)
    bpy.context.view_layer.objects.active = o

    # Modifiers first: a subdivision or displace modifier is where most of these
    # models keep their detail, and an export without it is a smooth blob.
    for m in list(o.modifiers):
        try:
            bpy.ops.object.modifier_apply(modifier=m.name)
        except RuntimeError:
            o.modifiers.remove(m)

    faces = len(o.data.polygons)
    total_before += faces
    if max_faces and faces > max_faces:
        d = o.modifiers.new(name="loom_decimate", type="DECIMATE")
        d.ratio = max_faces / faces
        bpy.ops.object.modifier_apply(modifier=d.name)

    t = o.modifiers.new(name="loom_tri", type="TRIANGULATE")
    bpy.ops.object.modifier_apply(modifier=t.name)

total_after = sum(len(o.data.polygons) for o in meshes)

bpy.ops.wm.obj_export(
    filepath=out,
    export_selected_objects=True,
    export_uv=True,
    export_normals=True,
    export_materials=False,
    export_triangulated_mesh=True,
    # Blender is Z-up; Loom is Y-up like glTF.
    forward_axis="NEGATIVE_Z",
    up_axis="Y",
)

names = ", ".join(o.name for o in meshes[:8])
print(f"LOOM-EXPORT {out} objects={len(meshes)} "
      f"faces={total_before}->{total_after} names={names}")
