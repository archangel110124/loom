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


# Outward-wound unit cube, corners 0..1, 12 tris. Translated and scaled to
# build the counterexample below.
_CUBE_V = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0),
           (0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)]
_CUBE_F = [(1, 4, 3), (1, 3, 2), (5, 6, 7), (5, 7, 8), (1, 2, 6), (1, 6, 5),
           (4, 8, 7), (4, 7, 3), (1, 5, 8), (1, 8, 4), (2, 3, 7), (2, 7, 6)]


def _write_cubes(path, cubes):
    """cubes: list of (origin, size, outward). Writes one OBJ, no tags."""
    lines, base = [], 0
    for (ox, oy, oz), size, outward in cubes:
        for vx, vy, vz in _CUBE_V:
            lines.append("v %.6f %.6f %.6f\n"
                         % (ox + vx * size, oy + vy * size, oz + vz * size))
        for a, b, c in _CUBE_F:
            # Reversing the winding is exactly what a missed per-shell
            # recalc_face_normals leaves behind.
            tri = (a, b, c) if outward else (a, c, b)
            lines.append("f %d %d %d\n" % (base + tri[0], base + tri[1], base + tri[2]))
        base += len(_CUBE_V)
    open(path, "w").writelines(lines)


def test_inverted_shell_is_refused():
    """The counterexample rule 3 is written about, and that the whole-mesh
    signed volume cannot see.

    An outward 2 m cube (volume +8) plus an INVERTED 1 m cube (volume -1) sums
    to +7 and passes any global check — while that second shell draws PURE
    BLACK in Loom, diffuse and ambientVisibility to zero together. Only a
    per-shell volume catches it, and it must be taken about the shell's OWN
    centroid: a shell far enough off the origin has an origin-referenced
    volume dominated by its position, not its winding.
    """
    _write_cubes(OUT, [((-1.0, -1.0, -1.0), 2.0, True),
                       (( 3.0,  3.0,  3.0), 1.0, False)])
    whole = rigkit.signed_volume(rigkit.read_obj(OUT))
    assert abs(whole - 7.0) < 1e-6, "counterexample is not the one described: %.6f" % whole
    try:
        rigkit.verify_obj(OUT)
    except SystemExit as e:
        assert "shell" in str(e).lower(), "refused, but not for the shell: %s" % e
        print("  inverted shell refused ok  whole-mesh volume=%.3f, but: %s" % (whole, e))
        return
    raise AssertionError(
        "verify_obj accepted an OBJ containing an INVERTED shell — whole-mesh "
        "volume %.3f > 0 hid it. That shell draws pure black in Loom." % whole)


def test_open_parts_still_allowed():
    """min_volume=False means parts need not be closed — and the per-shell
    check must respect that flag too, or every split part fails to export."""
    open(OUT, "w").write("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")
    info = rigkit.verify_obj(OUT, min_volume=False)
    assert info["tris"] == 1, "tris %d" % info["tris"]
    print("  open part ok  min_volume=False accepts an unclosed shell")


test_axes_and_tags()
test_v_is_flipped()
test_tag_stripping_is_asserted()
test_inverted_shell_is_refused()
test_open_parts_still_allowed()
print("rigkit: all contract checks pass")
