# tools/mesh/rig/test_split_parts.py
"""Run: python3 tools/mesh/rig/test_split_parts.py     (no Blender needed)"""
import os, sys, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rigkit, split_parts

SRC = """# a two-material file
mtllib thing.mtl
o Thing
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 5.0 0.0 0.0
v 6.0 0.0 0.0
v 5.0 1.0 0.0
vt 0.0 0.0
vt 1.0 0.0
vt 0.0 1.0
vn 0.0 0.0 1.0
usemtl deck_timber
f 1/1/1 2/2/1 3/3/1
usemtl steel_frame
f 4/1/1 5/2/1 6/3/1
"""


def test_splits_by_material_and_keeps_coordinates():
    d = tempfile.mkdtemp()
    src = os.path.join(d, "src.obj")
    open(src, "w").write(SRC)

    out = split_parts.split(src, d, "rig")

    assert set(out) == {"deck_timber", "steel_frame"}, "materials: %s" % sorted(out)

    timber = rigkit.read_obj(out["deck_timber"])
    steel = rigkit.read_obj(out["steel_frame"])

    assert timber["tags"] == [], "timber kept a tag: %s" % timber["tags"]
    assert steel["tags"] == [], "steel kept a tag: %s" % steel["tags"]
    assert len(timber["tris"]) == 1 and len(steel["tris"]) == 1

    # THE POINT OF THE WHOLE FILE: authored coordinates survive. The steel
    # triangle sits at x 5..6 in the source and must still sit at x 5..6 here.
    # A `#Group` import would have recentred it on its own bbox to x -0.5..0.5.
    lo, hi = rigkit.bounds(steel)
    assert abs(lo[0] - 5.0) < 1e-6 and abs(hi[0] - 6.0) < 1e-6, \
        "steel recentred: x %.3f..%.3f, want 5.000..6.000" % (lo[0], hi[0])

    lo, hi = rigkit.bounds(timber)
    assert abs(lo[0] - 0.0) < 1e-6 and abs(hi[0] - 1.0) < 1e-6, \
        "timber moved: x %.3f..%.3f" % (lo[0], hi[0])

    print("  split ok  %s" % {k: os.path.basename(v) for k, v in out.items()})


def test_refuses_a_file_with_no_materials():
    d = tempfile.mkdtemp()
    src = os.path.join(d, "bare.obj")
    open(src, "w").write("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")
    try:
        split_parts.split(src, d, "rig")
    except SystemExit as e:
        print("  refusal ok  %s" % e)
        return
    raise AssertionError("split accepted a file with no usemtl")


NEG_SRC = """usemtl mat1
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f -3 -2 -1
v 9.0 9.0 9.0
v 8.0 8.0 8.0
"""


def test_negative_indices_resolve_against_counts_at_face_time():
    # OBJ negative indices are relative to how many elements of that type had
    # been declared AT THE FACE LINE, not to the file's final count. Two more
    # `v` lines after the face must not change what the face resolved to.
    d = tempfile.mkdtemp()
    src = os.path.join(d, "neg.obj")
    open(src, "w").write(NEG_SRC)

    out = split_parts.split(src, d, "rig")
    mat1 = rigkit.read_obj(out["mat1"])

    assert len(mat1["tris"]) == 1, "tris: %s" % (mat1["tris"],)
    tri = mat1["tris"][0]
    got = [mat1["verts"][i] for i in tri]
    want = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)]
    assert got == want, "negative index resolved wrong: got %s want %s" % (got, want)

    print("  negative index ok  %s" % (got,))


test_splits_by_material_and_keeps_coordinates()
test_refuses_a_file_with_no_materials()
test_negative_indices_resolve_against_counts_at_face_time()
print("split_parts: all checks pass")
