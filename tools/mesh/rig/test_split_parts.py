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


test_splits_by_material_and_keeps_coordinates()
test_refuses_a_file_with_no_materials()
print("split_parts: all checks pass")
