"""Contract test for rigkit. Run:
     blender --background --factory-startup --python tools/mesh/rig/test_rigkit.py
No pytest — this project's generators self-check with asserts
(build_deckhand.py:432-446) and adding a framework for four checks is not worth
the dependency."""
import os, sys, math
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bpy, bmesh
import rigkit

OUT = "/tmp/rigkit_test.obj"


def clear():
    for o in list(bpy.data.objects):
        bpy.data.objects.remove(o, do_unlink=True)


def test_axes_and_tags():
    """A 2 x 4 x 6 m box in BLENDER axes must arrive as 2 x 6 x 4 in LOOM axes,
    because the export sends Blender +Z to Loom +Y."""
    clear()
    mesh = bpy.data.meshes.new("t")
    obj = bpy.data.objects.new("t", mesh)
    bpy.context.collection.objects.link(obj)
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    bmesh.ops.scale(bm, vec=(2.0, 4.0, 6.0), verts=bm.verts)   # x, y, z
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    bm.to_mesh(mesh)
    bm.free()

    rigkit.export_obj(obj, OUT, uv=False)
    info = rigkit.verify_obj(OUT)

    size = tuple(round(info["hi"][i] - info["lo"][i], 4) for i in range(3))
    assert size == (2.0, 6.0, 4.0), "axes wrong: got %s, want (2.0, 6.0, 4.0)" % (size,)
    assert info["volume"] > 0, "signed volume %.4f — normals point inward" % info["volume"]
    assert info["tris"] == 12, "tris %d" % info["tris"]
    print("  axes+tags ok  size=%s volume=%.3f tris=%d" % (size, info["volume"], info["tris"]))


def test_v_is_flipped():
    """A quad whose top edge carries V=1 in Blender must arrive carrying V=0,
    because the engine reads `vt` as written and the top of a PNG is row 0."""
    clear()
    mesh = bpy.data.meshes.new("q")
    obj = bpy.data.objects.new("q", mesh)
    bpy.context.collection.objects.link(obj)
    bm = bmesh.new()
    v0 = bm.verts.new((-1.0, 0.0, 0.0))
    v1 = bm.verts.new(( 1.0, 0.0, 0.0))
    v2 = bm.verts.new(( 1.0, 0.0, 2.0))   # top edge, z = 2
    v3 = bm.verts.new((-1.0, 0.0, 2.0))   # top edge
    f = bm.faces.new((v0, v1, v2, v3))
    uvl = bm.loops.layers.uv.new("UVMap")
    for loop, uv in zip(f.loops, [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]):
        loop[uvl].uv = uv
    bm.normal_update()
    bm.to_mesh(mesh)
    bm.free()

    rigkit.export_obj(obj, OUT, uv=True)
    d = rigkit.read_obj(OUT)
    # Pair each vertex with ITS OWN uv via the face corners. OBJ keeps `v` and
    # `vt` as independent lists, so indexing uvs by a vertex index is a bug that
    # happens to work on a quad and silently lies on anything else.
    tops = {round(d["uvs"][t][1], 3)
            for (v, t) in d["corners"]
            if t is not None and abs(d["verts"][v][1] - 2.0) < 1e-4}
    bots = {round(d["uvs"][t][1], 3)
            for (v, t) in d["corners"]
            if t is not None and abs(d["verts"][v][1] - 0.0) < 1e-4}
    assert tops == {0.0}, "top edge should carry V=0 after the flip, got %s" % tops
    assert bots == {1.0}, "bottom edge should carry V=1 after the flip, got %s" % bots
    print("  V flip ok  top V=%s bottom V=%s" % (tops, bots))


def test_tag_stripping_is_asserted():
    """verify_obj must REFUSE a file with a surviving tag, not warn about it."""
    open(OUT, "w").write("o thing\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")
    try:
        rigkit.verify_obj(OUT, min_volume=False)
    except SystemExit as e:
        print("  refusal ok  %s" % e)
        return
    raise AssertionError("verify_obj accepted a file with an `o` tag")


test_axes_and_tags()
test_v_is_flipped()
test_tag_stripping_is_asserted()
print("rigkit: all contract checks pass")
