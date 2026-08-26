# Rig Timber Rebuild — Phase 0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the whole Blender→Loom textured-asset pipeline end to end on one
part — the deck field — and rebuild the per-material splitter that was lost in
the machine migration.

**Architecture:** A shared contract module (`rigkit.py`) owns the export rules so
no generator restates them. A numpy texture generator writes tiling albedo and
normal PNGs from parameters. A parametric deck builder emits plank geometry whose
highest drawn point is exactly the invisible collider top. A `.loom` test scene
proves the collider transcription is bit-identical to the primitive it replaces,
and a `GOLDEN` row makes it a gate.

**Tech Stack:** Blender 5.2.0 LTS headless (`bpy`, `bmesh`), Python 3.14 + numpy
2.5.2, Loom CLI (`validate`/`measure`/`sim`/`render`/`compare`), `cargo xtask`.

**Spec:** `docs/design/RIG-TIMBER-REBUILD.md`

## Global Constraints

- **Generators live in the repo**, at `tools/mesh/rig/`. `build_boat.py` lived
  outside git on `/mnt/data` and was nearly lost in the migration; the migration
  README calls `sources/` *"the one thing here that git does not have and cannot
  regenerate."* Do not repeat that.
- **Export contract, exact:** `bpy.ops.wm.obj_export(forward_axis='NEGATIVE_Z',
  up_axis='Y', export_triangulated_mesh=True, export_normals=True,
  export_uv=True, export_materials=False, export_selected_objects=True)`.
  Permutation build `(x, y, z) -> obj (x, z, -y)`, determinant +1.
- **Flip V on export.** `vt u v` becomes `vt u (1.0 - v)`. Measured 2026-08-25;
  the importer flips nothing.
- **Strip `o`, `g`, `s`, `usemtl`, `mtllib`** and assert on read-back that none
  survived.
- **`recalc_face_normals` per shell** (`normals_make_consistent(inside=False)`
  per connected region). Inward normals render **pure black**.
- **One OBJ per material.** The engine never reads `.mtl`.
- **Every `[[asset]]` gets a real UUID `id`.** An empty id aliases every other
  empty id and the first adopted wins.
- **Build with `-j 3`**, not `-j 6`. Memory pressure, not cores.
- **Deck collider top is `y = 1.400` and does not move.** No drawn vertex may
  exceed it.
- Never `git add -A`. Stage by explicit path.

---

### Task 1: `rigkit.py` — the export contract, with a self-check

Everything downstream imports this. It exists so the contract is written once
and asserted, rather than restated in ten generators and wrong in one.

**Files:**
- Create: `tools/mesh/rig/rigkit.py`
- Create: `tools/mesh/rig/test_rigkit.py`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `export_obj(obj, path, uv=True, header="") -> str` — selects `obj`, exports
    through the contract, strips tags, flips V, writes `path`, returns `path`.
  - `read_obj(path) -> dict` with keys `verts` (list of 3-tuples), `uvs` (list of
    2-tuples), `tris` (list of 3-tuples of 0-based vertex indices), `corners`
    (list of `(v_idx, vt_idx_or_None)` 0-based pairs, in file order — this is
    the only way to pair a vertex with *its* UV, since OBJ keeps the two lists
    independent), `tags` (list of any forbidden tag lines found — must be empty).
  - `signed_volume(obj_data) -> float`
  - `bounds(obj_data) -> (lo, hi)` each a 3-tuple.
  - `verify_obj(path, *, min_volume=True) -> dict` — raises `SystemExit` on any
    contract violation; returns `{"tris": int, "verts": int, "lo": ..., "hi": ...,
    "volume": float}`.

- [ ] **Step 1: Write the failing test**

```python
# tools/mesh/rig/test_rigkit.py
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/test_rigkit.py 2>&1 | tail -20
```

Expected: `ModuleNotFoundError: No module named 'rigkit'`.

- [ ] **Step 3: Write `rigkit.py`**

```python
# tools/mesh/rig/rigkit.py
"""The Blender -> Loom export contract, written once.

Every rig generator imports this rather than restating the rules. `build_boat.py`
restated them, its axis comment said the wrong thing for eight rounds, and the
boat shipped with its sidelights reversed. One copy, asserted.

    Model in Blender native Z-up.
    Export with forward_axis='NEGATIVE_Z', up_axis='Y'.
    Permutation build (x, y, z) -> obj (x, z, -y), determinant +1.
    Delivered: metres, Y up, -Z forward, +X right.

Four rules, each a scar, all enforced by `verify_obj`:

1. V is FLIPPED on export. The importer takes `vt` exactly as written
   (loom_asset/src/mesh.rs:200-208) and flips nothing. Measured 2026-08-25 with
   a red-top/blue-bottom texture on two otherwise identical quads. The failure
   is not a legible upside-down picture — it is UV islands landing on the atlas
   gutter, which reads as black patches.
2. Tags `o`/`g`/`s`/`usemtl`/`mtllib` are STRIPPED, and their absence asserted.
   A surviving tag makes `file.obj#Group` usable, and that path recentres the
   selection on its own bounding box (mesh.rs:268-276), which takes a model
   apart when the library IS one model.
3. Normals are recalculated PER SHELL by the caller before export. A whole-mesh
   signed-volume check passes while most of a surface points inward, and Loom
   draws that pure black — diffuse and ambientVisibility go to zero together.
   `signed_volume` here is the coarse net underneath, not the guarantee.
4. One OBJ per material. The engine never reads `.mtl`.
"""
import os

TAGS = ("o", "g", "s", "usemtl", "mtllib")


def export_obj(obj, path, uv=True, header=""):
    """Export one object through the contract. Returns `path`."""
    import bpy

    for other in bpy.data.objects:
        other.select_set(False)
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj

    raw = path + ".raw"
    bpy.ops.wm.obj_export(
        filepath=raw,
        export_selected_objects=True,
        export_triangulated_mesh=True,
        export_normals=True,
        export_uv=uv,
        export_materials=False,
        forward_axis="NEGATIVE_Z",
        up_axis="Y",
    )

    lines = []
    if header:
        for line in header.rstrip("\n").split("\n"):
            lines.append("# " + line + "\n")
    lines.append("# Loom axes -- metres, Y up, forward -Z. V flipped on export.\n")
    lines.append("# Generated by tools/mesh/rig; never edit this file.\n")

    for line in open(raw):
        parts = line.split()
        if not parts:
            continue
        if parts[0] in TAGS:
            continue
        if parts[0] == "vt" and len(parts) >= 3:
            lines.append("vt %s %.6f\n" % (parts[1], 1.0 - float(parts[2])))
            continue
        lines.append(line)

    open(path, "w").writelines(lines)
    os.remove(raw)
    return path


def read_obj(path):
    """Parse an OBJ into plain Python. Only what Loom's own parser reads.

    `corners` pairs each face corner's vertex index with its own UV index.
    OBJ keeps `v` and `vt` as independent lists of different lengths, so
    `uvs[vertex_index]` is a bug that happens to work on a single quad and
    silently lies on anything with shared vertices.
    """
    verts, uvs, tris, corners, tags = [], [], [], [], []
    for line in open(path):
        parts = line.split()
        if not parts:
            continue
        head = parts[0]
        if head in TAGS:
            tags.append(line.rstrip("\n"))
        elif head == "v":
            verts.append(tuple(float(c) for c in parts[1:4]))
        elif head == "vt":
            uvs.append(tuple(float(c) for c in parts[1:3]))
        elif head == "f":
            idx = []
            for spec in parts[1:]:
                bits = spec.split("/")
                i = int(bits[0])
                vi = i - 1 if i > 0 else len(verts) + i
                ti = None
                if len(bits) > 1 and bits[1]:
                    j = int(bits[1])
                    ti = j - 1 if j > 0 else len(uvs) + j
                idx.append(vi)
                corners.append((vi, ti))
            for k in range(1, len(idx) - 1):
                tris.append((idx[0], idx[k], idx[k + 1]))
    return {"verts": verts, "uvs": uvs, "tris": tris,
            "corners": corners, "tags": tags}


def signed_volume(d):
    """Sum of tetrahedron volumes to the origin. Positive means outward."""
    total = 0.0
    v = d["verts"]
    for a, b, c in d["tris"]:
        ax, ay, az = v[a]
        bx, by, bz = v[b]
        cx, cy, cz = v[c]
        total += (ax * (by * cz - bz * cy)
                  - ay * (bx * cz - bz * cx)
                  + az * (bx * cy - by * cx)) / 6.0
    return total


def bounds(d):
    v = d["verts"]
    lo = tuple(min(p[i] for p in v) for i in range(3))
    hi = tuple(max(p[i] for p in v) for i in range(3))
    return lo, hi


def verify_obj(path, *, min_volume=True):
    """Refuse anything that breaks the contract. Raises SystemExit."""
    d = read_obj(path)
    if d["tags"]:
        raise SystemExit("%s: forbidden tag survived: %r" % (path, d["tags"][:3]))
    if not d["tris"]:
        raise SystemExit("%s: no triangles" % path)
    vol = signed_volume(d)
    if min_volume and vol <= 0.0:
        raise SystemExit(
            "%s: signed volume %.6f <= 0 — normals point inward and Loom will "
            "draw this pure black. Run recalc_face_normals per shell." % (path, vol))
    lo, hi = bounds(d)
    return {"tris": len(d["tris"]), "verts": len(d["verts"]),
            "lo": lo, "hi": hi, "volume": vol}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/test_rigkit.py 2>&1 | grep -E '  |rigkit:'
```

Expected, all three lines:
```
  axes+tags ok  size=(2.0, 6.0, 4.0) volume=48.000 tris=12
  V flip ok  top V={0.0} bottom V={1.0}
  refusal ok  /tmp/rigkit_test.obj: forbidden tag survived: ['o thing']
rigkit: all contract checks pass
```

- [ ] **Step 5: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/rigkit.py tools/mesh/rig/test_rigkit.py
git commit -m "feat(rig): the export contract, written once and asserted"
```

---

### Task 2: `split_parts.py` — rebuild the splitter that was lost

Cited at `build_boat.py:77` as the thing that turns one multi-material Blender
export into 21 per-material OBJs. It exists nowhere on disk or in the migration
bundle. This rebuilds it.

**Files:**
- Create: `tools/mesh/rig/split_parts.py`
- Create: `tools/mesh/rig/test_split_parts.py`

**Interfaces:**
- Consumes: `rigkit.read_obj`, `rigkit.verify_obj`, `rigkit.TAGS`.
- Produces: `split(src_obj_path, out_dir, prefix) -> dict` mapping material name
  to written OBJ path. Each output is a **whole-file** import at the source's
  authored coordinates — never a `#Group` selector, which would recentre it.

- [ ] **Step 1: Write the failing test**

```python
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/loom && python3 tools/mesh/rig/test_split_parts.py
```

Expected: `ModuleNotFoundError: No module named 'split_parts'`.

- [ ] **Step 3: Write `split_parts.py`**

```python
# tools/mesh/rig/split_parts.py
"""Split one multi-material OBJ into one whole-file OBJ per material.

`build_boat.py:77` names this file as the thing that produced the boat's 21
per-material OBJs. It did not survive the migration to Omarchy; the outputs did
and the script did not. This is a rebuild from the contract its outputs imply.

**Whole files, never `#Group`.** Loom can address one object inside a multi-object
OBJ with `path.obj#Name`, and that path recentres the selection on its own
bounding box — X and Z to the centre, Y to the minimum (loom_asset/src/mesh.rs
:268-276). That is right for a tree library and catastrophic for one model split
into parts: every part snaps to its own centroid and the model comes apart. So
each output here keeps the source's authored coordinates and carries no tag at
all, which makes the fragment path unusable and the mistake loud.

The vertex list is rewritten per part rather than shared, because a part that
carries the whole model's `v` list is a part that costs the whole model's memory
twenty times over.

Run: python3 split_parts.py <src.obj> <out_dir> <prefix>
"""
import os
import sys

import rigkit


def split(src, out_dir, prefix):
    """-> {material: path}. Raises SystemExit if the source names no material."""
    verts, uvs, norms = [], [], []
    parts = {}          # material -> list of face specs, each a list of "v/vt/vn"
    current = None

    for line in open(src):
        f = line.split()
        if not f:
            continue
        head = f[0]
        if head == "v":
            verts.append(tuple(float(c) for c in f[1:4]))
        elif head == "vt":
            uvs.append(tuple(float(c) for c in f[1:3]))
        elif head == "vn":
            norms.append(tuple(float(c) for c in f[1:4]))
        elif head == "usemtl":
            current = f[1]
            parts.setdefault(current, [])
        elif head == "f":
            if current is None:
                raise SystemExit(
                    "%s: a face appears before any `usemtl`. Every face must "
                    "belong to a named material, because a material is the unit "
                    "of separation here." % src)
            # A negative index is relative to how many elements of that type
            # had been declared AT THIS LINE (OBJ spec), not to the file's
            # final count -- record the counts now so a `v` further down the
            # file can never change what an earlier face resolved to. Resolving
            # against the final length instead put two of three vertices of a
            # `f -3 -2 -1` face onto vertices declared AFTER it, silently, in a
            # file that still passed every tag, triangle and volume check.
            parts[current].append((f[1:], (len(verts), len(uvs), len(norms))))

    if not parts:
        raise SystemExit(
            "%s: no `usemtl` in the file, so there is nothing to split by. The "
            "engine reads no .mtl and takes one OBJ per material; a single-"
            "material part should be exported directly instead." % src)

    written = {}
    for mat, faces in parts.items():
        # Renumber: collect only the elements this part actually references.
        vmap, tmap, nmap = {}, {}, {}
        out = ["# %s: %s. Loom axes -- metres, Y up, forward -Z.\n" % (prefix, mat),
               "# One OBJ per material: the engine reads no .mtl, and a `#Group`\n",
               "# selector would recentre this part on its own bounding box.\n",
               "# Generated by tools/mesh/rig/split_parts.py; never edit.\n"]

        def keep(store, table, source, idx, count):
            """1-based OBJ index -> 1-based index in this part's own list.

            `count` is how many elements of this type existed when the FACE was
            read, which is the only correct frame for a negative index.
            """
            key = idx - 1 if idx > 0 else count + idx
            if key not in table:
                table[key] = len(table) + 1
                store.append(source[key])
            return table[key]

        pv, pt, pn = [], [], []
        specs = []
        for face, (nv, nt, nn) in faces:
            spec = []
            for corner in face:
                bits = (corner.split("/") + ["", ""])[:3]
                vi = keep(pv, vmap, verts, int(bits[0]), nv)
                ti = keep(pt, tmap, uvs, int(bits[1]), nt) if bits[1] else None
                ni = keep(pn, nmap, norms, int(bits[2]), nn) if bits[2] else None
                if ti and ni:
                    spec.append("%d/%d/%d" % (vi, ti, ni))
                elif ni:
                    spec.append("%d//%d" % (vi, ni))
                elif ti:
                    spec.append("%d/%d" % (vi, ti))
                else:
                    spec.append("%d" % vi)
            specs.append(spec)

        for x, y, z in pv:
            out.append("v %.6f %.6f %.6f\n" % (x, y, z))
        for u, v in pt:
            out.append("vt %.6f %.6f\n" % (u, v))
        for x, y, z in pn:
            out.append("vn %.4f %.4f %.4f\n" % (x, y, z))
        for spec in specs:
            out.append("f " + " ".join(spec) + "\n")

        path = os.path.join(out_dir, "%s_%s.obj" % (prefix, mat))
        open(path, "w").writelines(out)
        rigkit.verify_obj(path, min_volume=False)   # a part need not be closed
        written[mat] = path

    return written


if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit("usage: split_parts.py <src.obj> <out_dir> <prefix>")
    for mat, path in sorted(split(sys.argv[1], sys.argv[2], sys.argv[3]).items()):
        info = rigkit.verify_obj(path, min_volume=False)
        print("%-16s %6d tris  %s" % (mat, info["tris"], path))
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd ~/loom && python3 tools/mesh/rig/test_split_parts.py
```

Expected:
```
  split ok  {'deck_timber': 'rig_deck_timber.obj', 'steel_frame': 'rig_steel_frame.obj'}
  refusal ok  .../bare.obj: a face appears before any `usemtl`, ...
split_parts: all checks pass
```

Note which refusal `bare.obj` actually trips. It carries an `f` line and no
`usemtl` at all, so the *face-before-material* guard fires first and the
*no-materials* guard below it is unreachable for this fixture. Both are correct
refusals and the test asserts on `SystemExit` rather than on message text, so
either satisfies it — but the message above is the one you will really see.

- [ ] **Step 5: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/split_parts.py tools/mesh/rig/test_split_parts.py
git commit -m "feat(rig): rebuild split_parts.py, lost in the migration"
```

---

### Task 3: `textures.py` — tiling albedo and normal from parameters

**Files:**
- Create: `tools/mesh/rig/textures.py`
- Create: `tools/mesh/rig/test_textures.py`

**Interfaces:**
- Consumes: nothing (numpy only).
- Produces:
  - `weathered_timber(size=2048, seed=7) -> (albedo_uint8_HxWx3, height_float_HxW)`
  - `normal_from_height(height, strength=2.0) -> uint8_HxWx3`
  - `write_png(path, rgb_uint8) -> None`

- [ ] **Step 1: Write the failing test**

```python
# tools/mesh/rig/test_textures.py
"""Run: python3 tools/mesh/rig/test_textures.py"""
import os, sys, hashlib, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
import textures


def test_tiles_seamlessly():
    """A texture that does not tile shows a seam on every plank butt joint.

    **Each axis is judged against its own interior baseline.** The grain is 8:1
    anisotropic, so neighbouring ROWS differ several times as much as
    neighbouring COLUMNS; comparing a horizontal seam against the column-wise
    baseline fails a texture that tiles perfectly well. That bug was in this
    test first, and it is the reason the two baselines are computed separately.
    """
    alb, h = textures.weathered_timber(size=256, seed=3)
    chan = alb[:, :, 0].astype(int)
    interior_x = np.abs(np.diff(chan, axis=1)).mean()   # column to column
    interior_y = np.abs(np.diff(chan, axis=0)).mean()   # row to row
    seam_x = np.abs(chan[:, 0] - chan[:, -1]).mean()
    seam_y = np.abs(chan[0, :] - chan[-1, :]).mean()
    assert seam_x <= interior_x * 2.0 + 1.0, \
        "vertical seam: %.2f vs column baseline %.2f" % (seam_x, interior_x)
    assert seam_y <= interior_y * 2.0 + 1.0, \
        "horizontal seam: %.2f vs row baseline %.2f" % (seam_y, interior_y)
    print("  tiling ok  seam_x=%.2f/%.2f  seam_y=%.2f/%.2f"
          % (seam_x, interior_x, seam_y, interior_y))


def test_is_deterministic():
    """The whole reason for generating rather than painting."""
    a1, _ = textures.weathered_timber(size=128, seed=11)
    a2, _ = textures.weathered_timber(size=128, seed=11)
    a3, _ = textures.weathered_timber(size=128, seed=12)
    assert hashlib.sha256(a1.tobytes()).digest() == hashlib.sha256(a2.tobytes()).digest(), \
        "same seed gave different bytes"
    assert hashlib.sha256(a1.tobytes()).digest() != hashlib.sha256(a3.tobytes()).digest(), \
        "different seeds gave identical bytes — the seed is not wired up"
    print("  determinism ok")


def test_normal_map_is_a_unit_field():
    _, h = textures.weathered_timber(size=128, seed=5)
    n = textures.normal_from_height(h)
    v = (n.astype(np.float32) / 255.0) * 2.0 - 1.0
    length = np.sqrt((v ** 2).sum(axis=2))
    assert abs(length.mean() - 1.0) < 0.02, "mean normal length %.4f" % length.mean()
    # A flat-blue normal map means the height field never reached it.
    assert v[:, :, 0].std() > 0.01, "normal X is flat — height is not being read"
    print("  normal ok  |n|=%.4f  Xstd=%.4f" % (length.mean(), v[:, :, 0].std()))


def test_reads_as_dark_weathered_timber():
    """Taste is not gateable, but *brightness* is, and the first version of this
    generator failed on brightness: mean RGB [113, 107, 97] and std 8, a milky
    beige. The deck it replaces is authored `albedo = [0.19, 0.17, 0.15]`, i.e.
    [48, 43, 38]. A texture is allowed to sit above a flat colour it replaces —
    it has variation the flat one did not — but not by double.
    """
    alb, _ = textures.weathered_timber(size=256, seed=7)
    mean = alb.reshape(-1, 3).mean(axis=0)
    std = alb.reshape(-1, 3).std(axis=0)
    assert (mean < 80).all(), \
        "too pale: mean %s, target is near [48 43 38]" % mean.round(1)
    assert (mean > 30).all(), "too dark: mean %s" % mean.round(1)
    assert (std > 10).all(), \
        "too flat: std %s — the grain is not reaching the colour" % std.round(1)
    assert mean[0] > mean[2], "timber is warm; R should exceed B, got %s" % mean.round(1)
    print("  palette ok  mean=%s std=%s" % (mean.round(1), std.round(1)))


def test_png_round_trip():
    alb, _ = textures.weathered_timber(size=64, seed=1)
    p = os.path.join(tempfile.mkdtemp(), "t.png")
    textures.write_png(p, alb)
    assert os.path.getsize(p) > 100
    assert open(p, "rb").read(8) == b"\x89PNG\r\n\x1a\n", "not a PNG"
    print("  png ok  %d bytes" % os.path.getsize(p))


test_tiles_seamlessly()
test_is_deterministic()
test_normal_map_is_a_unit_field()
test_reads_as_dark_weathered_timber()
test_png_round_trip()
print("textures: all checks pass")
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/loom && python3 tools/mesh/rig/test_textures.py
```

Expected: `ModuleNotFoundError: No module named 'textures'`.

- [ ] **Step 3: Write `textures.py`**

```python
# tools/mesh/rig/textures.py
"""Tiling PBR maps for the rig, generated from parameters.

**Not baked in Cycles, deliberately.** Cycles is available here (OPTIX and CUDA
on the 4090) and would give richer nodes, but it is not bit-reproducible across
machines or driver versions, and this project's entire verification story is
that a result reproduces. A numpy field is the same bytes on any box, costs no
GPU while somebody is at the machine, and keeps the asset a pure function of its
parameters — the property that let `build_boat.py` survive a migration intact.

ponytail: value-noise fBm, not a Perlin/Worley library. If the timber needs
true grain anisotropy that this cannot reach, the upgrade path is a Cycles AO +
curvature bake composited on top of these, NOT a new dependency.

Everything wraps: `np.roll`-based interpolation on a periodic lattice, so every
octave is seamless and therefore so is the sum. A seam on a tiling deck texture
appears at every plank butt joint at once, which is the most visible way for
this to be wrong.
"""
import struct
import zlib

import numpy as np


def _axis(size, freq):
    """Wrapped bilinear sample positions for one axis, at `freq` cells."""
    t = (np.arange(size, dtype=np.float32) + 0.5) * freq / size - 0.5
    i0 = np.floor(t).astype(np.int32)
    frac = (t - i0).astype(np.float32)
    smooth = frac * frac * (3.0 - 2.0 * frac)      # smoothstep, C1 at the cells
    return i0 % freq, (i0 + 1) % freq, smooth


def _value_noise(size, freq_y, freq_x, rng):
    """One octave of periodic value noise, ANISOTROPIC.

    Separate frequencies per axis is what makes timber read as timber: grain is
    a low frequency across the board and a high one along it. Averaging shifted
    isotropic copies — the obvious alternative — introduces its own periodicity
    at the shift interval and reads as corduroy.

    Index arithmetic is modulo the frequency, which is what makes each octave
    periodic and therefore the whole sum tileable. A seam on a decking texture
    shows at every butt joint at once.
    """
    lattice = rng.random((freq_y, freq_x)).astype(np.float32)
    y0, y1, sy = _axis(size, freq_y)
    x0, x1, sx = _axis(size, freq_x)

    a = lattice[np.ix_(y0, x0)]
    b = lattice[np.ix_(y0, x1)]
    c = lattice[np.ix_(y1, x0)]
    d = lattice[np.ix_(y1, x1)]
    top = a + (b - a) * sx[None, :]
    bot = c + (d - c) * sx[None, :]
    return top + (bot - top) * sy[:, None]


def _fbm(size, rng, octaves=5, base=4, gain=0.5, stretch=1):
    """`stretch` divides the ALONG-grain frequency: 8 gives 8:1 timber grain."""
    out = np.zeros((size, size), dtype=np.float32)
    amp, freq, norm = 1.0, base, 0.0
    for _ in range(octaves):
        out += amp * _value_noise(size, freq, max(1, freq // stretch), rng)
        norm += amp
        amp *= gain
        freq *= 2
    return out / norm


def weathered_timber(size=2048, seed=7):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    The plank runs along +U. Grain is fBm stretched 8:1 along the grain, which
    is what makes it read as timber rather than as noise. On top of that: a
    low-frequency silvering that greys the exposed surface, and sparse dark rot
    blooms that eat both the colour and the height.
    """
    rng = np.random.default_rng(seed)

    # Grain runs ALONG the board, which is +U. 8:1 anisotropy: fine detail
    # across the grain, long smooth runs along it.
    grain = (_fbm(size, rng, octaves=6, base=32, stretch=8) * 0.70
             + _fbm(size, rng, octaves=4, base=4, stretch=4) * 0.30)
    # **Normalise to the full range.** Without this the contrast depends on how
    # many octaves happened to land near the mean, and the first version of this
    # function came out milky — mean RGB [113, 107, 97] against a target of
    # [48, 43, 38], with a standard deviation of 8. Measured, then fixed.
    grain = (grain - grain.min()) / (grain.max() - grain.min())

    silvering = _fbm(size, rng, octaves=3, base=2, stretch=3)
    silvering = (silvering - silvering.min()) / (silvering.max() - silvering.min())
    rot_field = _fbm(size, rng, octaves=5, base=6, stretch=5)
    rot = np.clip((rot_field - 0.58) / 0.22, 0.0, 1.0)

    height = np.clip(grain * (1.0 - 0.55 * rot), 0.0, 1.0).astype(np.float32)

    # Colour. Three states blended by two masks: wet-dark timber, lifted toward
    # grey where the weather has silvered it, dropped toward black-green where
    # it has gone. The grain drives colour as hard as it drives height, which is
    # what the milky first version was missing.
    g = grain[:, :, None]
    dry = np.array([0.150, 0.124, 0.099]) + g * np.array([0.135, 0.115, 0.092])
    silver = np.array([0.245, 0.240, 0.228]) + g * np.array([0.130, 0.128, 0.120])
    rotten = np.array([0.045, 0.052, 0.038]) + g * np.array([0.040, 0.045, 0.030])

    s = np.clip(silvering * 1.30 - 0.30, 0.0, 1.0)[:, :, None]
    r = rot[:, :, None]
    rgb = dry * (1.0 - s) + silver * s
    rgb = rgb * (1.0 - r) + rotten * r
    return (np.clip(rgb, 0.0, 1.0) * 255.0 + 0.5).astype(np.uint8), height


def normal_from_height(height, strength=2.0):
    """Tangent-space normal map, +Y green (OpenGL). Wraps, like its source."""
    dx = (np.roll(height, -1, axis=1) - np.roll(height, 1, axis=1)) * strength
    dy = (np.roll(height, -1, axis=0) - np.roll(height, 1, axis=0)) * strength
    nx, ny, nz = -dx, -dy, np.ones_like(height)
    length = np.sqrt(nx * nx + ny * ny + nz * nz)
    v = np.stack([nx / length, ny / length, nz / length], axis=2)
    return ((v * 0.5 + 0.5) * 255.0 + 0.5).astype(np.uint8)


def write_png(path, rgb):
    """Minimal RGB8 PNG. No pillow — this is thirty lines and one less dep."""
    h, w = rgb.shape[:2]
    raw = np.concatenate(
        [np.zeros((h, 1), dtype=np.uint8), rgb.reshape(h, w * 3)], axis=1)

    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff))

    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(raw.tobytes(), 6))
           + chunk(b"IEND", b""))
    open(path, "wb").write(png)
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd ~/loom && python3 tools/mesh/rig/test_textures.py
```

Expected, five lines then the summary — these are the numbers this code
actually produced when the plan was written, so a material deviation means
something changed:

```
  tiling ok  seam_x=0.29/0.26  seam_y=2.13/1.71
  determinism ok
  normal ok  |n|=1.0002  Xstd=0.0601
  palette ok  mean=[59.9 54.1 46.7] std=[14.  13.4 13.9]
  png ok  6354 bytes
textures: all checks pass
```

- [ ] **Step 4b: Fault-inject the seam test**

A tiling check that has never seen a non-tiling texture proves nothing. Crop a
non-periodic window out of a larger field and confirm both axes fail:

```bash
cd ~/loom && python3 - <<'PY'
import sys; sys.path.insert(0, "tools/mesh/rig")
import numpy as np, textures
big, _ = textures.weathered_timber(size=512, seed=3)
chan = big[100:356, 100:356, 0].astype(int)
ix = np.abs(np.diff(chan, axis=1)).mean(); iy = np.abs(np.diff(chan, axis=0)).mean()
sx = np.abs(chan[:, 0] - chan[:, -1]).mean(); sy = np.abs(chan[0, :] - chan[-1, :]).mean()
print("seam_x %.2f/%.2f pass=%s   seam_y %.2f/%.2f pass=%s"
      % (sx, ix, sx <= ix*2+1, sy, iy, sy <= iy*2+1))
PY
```

Expected: **both `pass=False`** (measured: 6.21/0.13 and 10.17/0.90).

- [ ] **Step 4c: Look at the texture, do not just measure it**

```bash
cd ~/loom && python3 - <<'PY'
import sys; sys.path.insert(0, "tools/mesh/rig")
import numpy as np, textures
alb, h = textures.weathered_timber(size=512, seed=7)
nrm = textures.normal_from_height(h)
tile = np.concatenate([np.concatenate([alb, alb], 1)] * 2, 0)[::2, ::2]
textures.write_png("/tmp/tex_sheet.png", np.concatenate([alb, nrm, tile], axis=1))
PY
```

**Open `/tmp/tex_sheet.png`.** Three panels: albedo, normal, and a 2×2 tiling.
It should read as dark weathered timber with silvered streaks running along the
grain and rot in the low bands — not as beige mould, which is what the first
version of this generator produced before the palette was tuned.

- [ ] **Step 5: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/textures.py tools/mesh/rig/test_textures.py
git commit -m "feat(rig): tiling timber maps, generated rather than painted"
```

---

### Task 4: `build_deck.py` — the plank field

**Files:**
- Create: `tools/mesh/rig/build_deck.py`
- Create: `assets/meshes/rig_deck_timber.obj` (generated)
- Create: `assets/textures/rig_deck_timber_albedo.png` (generated)
- Create: `assets/textures/rig_deck_timber_normal.png` (generated)

**Interfaces:**
- Consumes: `rigkit.export_obj`, `rigkit.verify_obj`, `textures.*`.
- Produces: the three files above. Deck occupies exactly
  `x -12.000 … 12.000`, `z -7.000 … 7.000`, `y 1.000 … 1.400`.

**The constraint that governs this task.** `deeper_demo.loom:1041-1046` states
the rig's design principle: *"a renderable node with no dynamic ancestor becomes
a static collider from its own drawn bounds, so what is walked on is exactly what
is seen. That equivalence is the whole reason nothing here is an invisible
ramp."* Transcribing the collider explicitly breaks that equivalence. The
mitigation is geometric: **no drawn vertex may exceed y = 1.400.** Weathered
planks cup upward, so each plank is sunk until its raised edges land exactly on
1.400 and its centre dips ~8 mm below. Seen ≤ walked, always, and asserted.

- [ ] **Step 1: Read the self-check this task must satisfy**

The builder carries its own assertions rather than a separate test file — the
pattern `build_deckhand.py:432-461` uses, because a generator whose checks live
elsewhere is a generator somebody runs without them. This block is the tail of
`build_deck.py` and is written as part of Step 3; it is shown first so the
acceptance is clear before any geometry exists.

```python
# --- self-check, runs on every build -------------------------------------
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

assert abs(lo[0] + 12.0) < 1e-3 and abs(hi[0] - 12.0) < 1e-3, \
    "x %.4f..%.4f, want -12.000..12.000" % (lo[0], hi[0])
assert abs(lo[2] + 7.0) < 1e-3 and abs(hi[2] - 7.0) < 1e-3, \
    "z %.4f..%.4f, want -7.000..7.000" % (lo[2], hi[2])

# THE ONE THAT MATTERS. Nothing drawn may stand above the invisible floor.
# The mesh is LOCAL, centred on DECK_CENTRE, so the bound is the local top; a
# vertex above it still puts drawn deck above the collider once the node places it.
LOCAL_TOP = DECK_TOP - DECK_CENTRE
assert hi[1] <= LOCAL_TOP + 1e-4, \
    "a vertex reaches local y=%.5f (world %.5f once the node sits at " \
    "DECK_CENTRE=%.3f), above the local top %.3f (world deck top %.3f) — the " \
    "player would see deck above the surface he stands on" \
    % (hi[1], hi[1] + DECK_CENTRE, DECK_CENTRE, LOCAL_TOP, DECK_TOP)
assert hi[1] > LOCAL_TOP - 1e-3, \
    "highest vertex is local y=%.5f, %.1f mm BELOW the local top — the player " \
    "would float" % (hi[1], (LOCAL_TOP - hi[1]) * 1000.0)

assert info["tris"] <= 12000, "over budget: %d tris" % info["tris"]
assert len(rigkit.read_obj(OBJ_PATH)["uvs"]) > 0, "no UVs — the texture cannot land"

print("deck: %d tris, %d verts, x %.2f..%.2f y %.3f..%.3f z %.2f..%.2f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("deck: self-check passes")
```

- [ ] **Step 2: Run to verify it fails**

```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/build_deck.py 2>&1 | tail -5
```

Expected: file-not-found for `build_deck.py`.

- [ ] **Step 3: Write `build_deck.py`**

```python
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
DECK_TOP = 1.400                  # frozen: every spawn and assert row reads it
DECK_BOTTOM = 1.000               # the primitive's underside
# **This mesh is authored CENTRED ON ITS LOCAL ORIGIN, not at world height.**
# `play.rs:520` builds a static collider at the NODE's world position and
# `half_extents` is LOCAL (:552-556), so a mesh baked at absolute height cannot
# be given a matching box collider from ANY single node transform -- the
# collider lands at the origin while the deck is drawn 1.2 m above it. The scene
# node carries pos.y = DECK_CENTRE, which puts local +/-0.200 back at world
# 1.000..1.400 AND centres the collider on the same range. It is also exactly
# the transform deeper_demo's existing `Deck` primitive already uses, so this
# mesh is a drop-in replacement at an unchanged node transform.
DECK_CENTRE = 1.200               # the node y this mesh is authored to hang from
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
# x edges below. `courses` boards have `courses - 1` gaps BETWEEN them, not
# `courses`: a pitch that reserves a trailing GAP after every course including
# the last leaves the far edge short by one gap -- 12 mm of open sea at the
# exact line the collider ends. Distribute the rounding into plank width, which
# widens each board by about 0.3 mm and puts both edges on the boundary.
plank_w = (2 * HALF_Z - (courses - 1) * GAP) / courses
pitch = plank_w + GAP


def plank(z0, x0, x1, sink, uv_v0):
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

    # UVs: 1 texture tile per 2 m along the board, 1 per board across. Every
    # board gets its own V band so no two neighbours share grain.
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            loop[uv_layer].uv = (x / 2.0, uv_v0 + (-y - z0) / plank_w * 0.25)
    return faces


rng_state = SEED
def rand():
    """A tiny LCG. Deterministic, and independent of Python's hash seed."""
    global rng_state
    rng_state = (rng_state * 1103515245 + 12345) & 0x7FFFFFFF
    return rng_state / 0x7FFFFFFF


# The x-spans actually emitted, per course, for the coverage check at the foot
# of this file. Collected as boards are made rather than re-parsed out of the
# OBJ, because the question is what the generator decided, not what survived.
course_spans = [[] for _ in range(courses)]

for c in range(courses):
    z0 = -HALF_Z + c * pitch
    # Butt joints: 3 to 5 boards per course, staggered course to course so the
    # joints never line up. A deck whose joints line up reads as a texture.
    n = 3 + int(rand() * 3)
    cuts = sorted(-HALF_X + (2 * HALF_X) * rand() for _ in range(n - 1))
    # **Reject the CUT, do not drop the BOARD.** Dropping a sliver board leaves
    # an actual hole in the deck — measured at 11 of them across the field, some
    # reaching the x edge — and `verify_obj` cannot see it, because its bounds
    # check is global min/max and other courses still reach the edges. Keeping
    # only cuts that clear MIN_BOARD on both sides makes every resulting span
    # legal by construction, so nothing is ever dropped. Board count is
    # unchanged either way: dropping a board removes one, and rejecting a cut
    # merges two spans into one, which also removes one.
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
        plank(z0, x0, x1, sink, (c % 4) * 0.25)
        course_spans[c].append((x0, x1))

# --- coverage self-check: a dropped sliver board is a hole in the deck ----
# `verify_obj`'s bounds check is global min/max — other courses still reach the
# edges, so a per-course gap is invisible to it and real in the asset. This
# walks the boards as emitted (not a re-parse of the OBJ) and asserts each
# course is one continuous run from -HALF_X to +HALF_X, joints only.
BUTT = 0.006 + 1e-4  # the internal board-start offset, plus float slack
for c, spans in enumerate(course_spans):
    z0 = -HALF_Z + c * pitch
    spans = sorted(spans)
    assert spans, "course z=%.4f emitted no boards at all" % z0
    assert abs(spans[0][0] + HALF_X) < 1e-6, \
        "course z=%.4f: hole x=%.4f..%.4f at the x=-12 edge" % (z0, -HALF_X, spans[0][0])
    assert abs(spans[-1][1] - HALF_X) < 1e-6, \
        "course z=%.4f: hole x=%.4f..%.4f at the x=+12 edge" % (z0, spans[-1][1], HALF_X)
    for (a0, a1), (b0, b1) in zip(spans, spans[1:]):
        gap = b0 - a1
        assert gap <= BUTT, \
            "course z=%.4f: hole x=%.4f..%.4f (%.1f cm) between boards" \
            % (z0, a1, b0, gap * 100)

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
# The mesh is LOCAL, centred on DECK_CENTRE, so the bound is the local top; a
# vertex above it still puts drawn deck above the collider once the node places it.
LOCAL_TOP = DECK_TOP - DECK_CENTRE
assert hi[1] <= LOCAL_TOP + 1e-4, \
    "a vertex reaches local y=%.5f (world %.5f once the node sits at " \
    "DECK_CENTRE=%.3f), above the local top %.3f (world deck top %.3f) — the " \
    "player would see deck above the surface he stands on" \
    % (hi[1], hi[1] + DECK_CENTRE, DECK_CENTRE, LOCAL_TOP, DECK_TOP)
assert hi[1] > LOCAL_TOP - 1e-3, \
    "highest vertex is local y=%.5f, %.1f mm BELOW the local top — the player " \
    "would float" % (hi[1], (LOCAL_TOP - hi[1]) * 1000.0)

assert info["tris"] <= 12000, "over budget: %d tris" % info["tris"]
assert len(rigkit.read_obj(OBJ_PATH)["uvs"]) > 0, "no UVs — the texture cannot land"

print("deck: %d tris, %d verts, x %.2f..%.2f y %.3f..%.3f z %.2f..%.2f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("deck: self-check passes")
```

- [ ] **Step 4: Run the build and read its self-check**

```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/build_deck.py 2>&1 | grep -E '^deck:|Error|assert'
```

Expected: a `deck: N tris …` line with `y 1.000..1.400`, then
`deck: self-check passes`. If `hi[1]` exceeds 1.400 the build refuses — that is
the check doing its job, not a bug to work around.

- [ ] **Step 5: Fault-inject the height assertion**

Temporarily set `CUP = -0.008` (cup downward, so edges rise above `DECK_TOP`)
and re-run. Expected: the build **fails** with *"a vertex reaches y=…, above the
collider top"*. Restore `CUP = 0.008`. A check that has never failed has never
been tested.

- [ ] **Step 6: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/build_deck.py assets/meshes/rig_deck_timber.obj \
        assets/textures/rig_deck_timber_albedo.png \
        assets/textures/rig_deck_timber_normal.png
git commit -m "feat(rig): the deck field, cupped so that seen never exceeds walked"
```

---

### Task 5: The test scene, and the collider transcription proved

**Files:**
- Create: `assets/test/rig_deck.loom`

**Interfaces:**
- Consumes: `assets/meshes/rig_deck_timber.obj`, both PNGs.
- Produces: a scene named `rig_deck` for `SCENES` and `GOLDEN` in Task 6.

- [ ] **Step 1: Generate three UUIDs**

```bash
python3 -c "import uuid; [print(uuid.uuid4()) for _ in range(4)]"
```

Use them for the scene id and the three `[[asset]]` ids. **Never leave an `id`
off** — an empty id aliases every other empty id and the first adopted wins.

- [ ] **Step 2: Write `assets/test/rig_deck.loom`**

```toml
# The rig's deck field, and the proof that replacing a primitive with a mesh
# moves no physics.
#
# `Rig/Deck` in the demo is one `box` at scale [12.0, 0.20, 7.0], and a
# renderable with no dynamic ancestor takes its collider from its drawn bounds.
# This scene draws timber instead and restates that collider explicitly, which
# `play.rs:556-573` multiplies by world scale — so at scale 1 these ARE the
# primitive's numbers. Measured: a character settles at 2.1180999279022217 on
# either, bit for bit.
#
# `Drop` is the assertion's subject. It spawns 1.6 m up and falls; the gate row
# asserts where it lands.

[scene]
format = 1
id = "PASTE-UUID-1"

[[asset]]
key = "deck_timber"
id = "PASTE-UUID-2"
path = "../meshes/rig_deck_timber.obj"

[[asset]]
key = "deck_albedo"
id = "PASTE-UUID-3"
path = "../textures/rig_deck_timber_albedo.png"

[[asset]]
key = "deck_normal"
id = "PASTE-UUID-4"
path = "../textures/rig_deck_timber_normal.png"

[[node]]
name = "Rig"

  # The demo's own environment, so this row's picture is comparable to the
  # scene it is a stand-in for. `deeper_demo.loom:668-676`.
  [node.components.Environment]
  sun_direction = [0.42, 0.46, -0.78]
  sun_strength = 1.25
  sun_color = [1.0, 0.93, 0.80]
  ambient = 0.45
  sky_zenith = [0.16, 0.30, 0.52]
  sky_horizon = [0.74, 0.72, 0.66]
  fog_density = 0.0028
  fog_falloff = 0.05

[[node]]
name = "Deck"
parent = "Rig"

  # **The mesh is authored centred on its local origin, and this is the node
  # that puts it back.** 1.20 is not a fudge: it is exactly the transform
  # `deeper_demo`'s existing `Deck` primitive already carries, so the timber is
  # a drop-in replacement for the box at an unchanged node transform. A mesh
  # baked at absolute height could not be given a matching collider at all --
  # `play.rs:520` centres a static collider on the NODE, not on the vertices.
  [node.transform]
  pos = [0.0, 1.20, 0.0]

  [node.components.MeshRenderer]
  mesh = { asset = "deck_timber" }

  # The transcription. LOCAL half-extents, multiplied by world scale — and the
  # scale here is 1, so these are literally the primitive's numbers. The mesh
  # already sits at its authored height, so the node needs no offset.
  [node.components.BoxCollider]
  half_extents = [12.0, 0.20, 7.0]

  [node.components.Material]
  albedo = [1.0, 1.0, 1.0]
  albedo_map = { asset = "deck_albedo" }
  normal_map = { asset = "deck_normal" }
  roughness = 0.92
  metallic = 0.0
  uv_scale = [1.0, 1.0]

[[node]]
name = "Drop"
parent = "Rig"

  [node.transform]
  pos = [-4.30, 3.00, 0.90]

  [node.components.CharacterController]
  height = 1.8
  radius = 0.35
  step_height = 0.25

[[node]]
name = "Camera"
parent = "Rig"

  # Low and raking, because a deck photographed from above is a texture swatch
  # and a deck photographed from eye height is a floor. 320x200 is GOLDEN_SIZE.
  [node.transform]
  pos = [-9.0, 2.60, 9.5]
  rot_euler = [-12.0, -28.0, 0.0]

  [node.components.Camera]
  fov_y_degrees = 48.0
```

- [ ] **Step 3: Validate and measure**

```bash
cd ~/loom
./target/release/loom validate assets/test/rig_deck.loom
./target/release/loom measure assets/test/rig_deck.loom --node Rig/Deck
```

Expected: `"ok": true` with an empty `assets` array (an `asset_file_missing`
warning means a path is wrong and the renderer is drawing a stand-in unit box,
which *looks exactly like a scene that loaded*). The measured `Rig/Deck` size
must be `[24.0, 0.4, 14.0]`.

- [ ] **Step 4: Prove the collider is bit-identical to the primitive's**

```bash
cd ~/loom
# The mesh scene:
./target/release/loom sim assets/test/rig_deck.loom --ticks 300 \
  --assert "Rig/Drop.y == -999" 2>&1 | grep actual
```

Record the number. Then build the primitive twin and compare.

**Hand-author the twin; do not sed it.** The twin differs from the mesh scene in
four places at once and a multi-expression `sed` over a TOML file is a way to get
a silently different scene rather than a controlled one. Copy
`assets/test/rig_deck.loom` to `/tmp/rig_deck_prim.loom` and make exactly these
four edits by hand:

1. `mesh = { asset = "deck_timber" }` becomes `mesh = { asset = "box" }`
2. add `scale = [12.0, 0.20, 7.0]` to the `Deck` transform — **`pos` stays
   `[0.0, 1.20, 0.0]`**, because that is already the primitive's own transform
3. delete the whole `[node.components.BoxCollider]` block and its `half_extents`
   line — the primitive takes its collider from its drawn bounds, which is the
   behaviour under comparison
4. delete `albedo_map`, `normal_map` and `uv_scale` from the `Material` — a
   primitive has no UVs to hang them on

Then:

```bash
cd ~/loom
./target/release/loom sim /tmp/rig_deck_prim.loom --ticks 300 \
  --assert "Rig/Drop.y == -999" 2>&1 | grep actual
```

Expected: **the two `actual` values are identical to every digit.** If they are
not, stop — the transcription is wrong and no amount of geometry will fix it.

- [ ] **Step 5: Render and open the PNG**

```bash
cd ~/loom
./target/release/loom render assets/test/rig_deck.loom \
  --out /tmp/rig_deck.png --size 320x200
```

**Open it.** Check three things: the planks read as planks and not as noise; the
texture is not upside down (rot blooms should sit where the normal map dents,
not opposite them); and there are no black patches, which would mean UV islands
on the atlas gutter.

- [ ] **Step 6: Commit**

```bash
cd ~/loom
git add assets/test/rig_deck.loom
git commit -m "test(rig): the deck field, and the collider transcription proved"
```

---

### Task 6: Make it a gate

**Files:**
- Modify: `xtask/src/main.rs:41` (`SCENES`, bump `72` → `73`)
- Modify: `xtask/src/main.rs:409` (`GOLDEN`, bump `56` → `57`)
- Modify: `assets/SCENES.md` (regenerated)

- [ ] **Step 1: Choose `--sim` by diff sweep, not by taste**

The deck is static, so the expected answer is `--sim 0`. Prove it rather than
assume it:

```bash
cd ~/loom
for T in 0 60 300; do
  ./target/release/loom render assets/test/rig_deck.loom \
    --out /tmp/sweep_$T.png --size 320x200 --sim $T
done
./target/release/loom compare /tmp/sweep_0.png /tmp/sweep_60.png
./target/release/loom compare /tmp/sweep_0.png /tmp/sweep_300.png
```

Expected: zero differing pixels at every tick. If not, something in the scene is
animated and the row needs a `--sim` that catches it settled.

- [ ] **Step 2: Add the `SCENES` row**

At `xtask/src/main.rs:41`, change `const SCENES: [&str; 72]` to `[&str; 73]` —
**the length is part of the type and the edit will not compile otherwise** —
and add, beside the other rig entries:

```rust
    // The deck field: the first textured mesh in the project, and the first
    // node whose collider is transcribed rather than taken from drawn bounds.
    // If this row ever renders a flat grey plate, the OBJ did not load and the
    // renderer substituted a unit box — which looks exactly like success.
    "assets/test/rig_deck.loom",
```

- [ ] **Step 3: Add the `GOLDEN` row**

At `xtask/src/main.rs:409`, change `[(&str, &str, &[&str]); 56]` to `; 57]` and
add:

```rust
    // The only picture of a UV-mapped, normal-mapped imported mesh anywhere in
    // the library. Delete the albedo_map and this row still draws timber-shaped
    // geometry at the right height — so what it actually guards is the TEXTURE
    // reaching the surface, which nothing else in GOLDEN can see. Static scene,
    // measured flat across ticks 0/60/300, so --sim adds nothing.
    ("rig_deck", "assets/test/rig_deck.loom", &[]),
```

- [ ] **Step 4: Fault-inject the row**

A row that cannot fail is not a gate. Prove all three ways it should break:

```bash
cd ~/loom
# 1. texture removed -> the surface loses its grain
sed '/albedo_map/d' assets/test/rig_deck.loom > /tmp/f1.loom
./target/release/loom render /tmp/f1.loom --out /tmp/f1.png --size 320x200
./target/release/loom compare /tmp/f1.png /tmp/rig_deck.png

# 2. V un-flipped -> the normal map fights the albedo
python3 - <<'PY'
src = "assets/meshes/rig_deck_timber.obj"
out = open("/tmp/unflipped.obj", "w")
for line in open(src):
    p = line.split()
    if p and p[0] == "vt":
        out.write("vt %s %.6f\n" % (p[1], 1.0 - float(p[2])))
    else:
        out.write(line)
PY
sed 's|../meshes/rig_deck_timber.obj|/tmp/unflipped.obj|' \
    assets/test/rig_deck.loom > /tmp/f2.loom
./target/release/loom render /tmp/f2.loom --out /tmp/f2.png --size 320x200
./target/release/loom compare /tmp/f2.png /tmp/rig_deck.png
```

**Record the differing-pixel fraction each fault produces in the commit
message.** If any fault moves nothing, the row does not guard what its comment
claims and the comment must change.

- [ ] **Step 5: Regenerate the scene index**

```bash
cd ~/loom && python3 tools/scene_index.py
```

`assets/SCENES.md` is currently stale at "49 scenes" against an actual 72; this
step is part of adding a scene and has been skipped before.

- [ ] **Step 6: Run the gates that are the builder's to run**

Builders may not run `cargo xtask` — the gates hold a cross-worktree singleton
lock. Use the hand equivalents:

```bash
cd ~/loom
cargo clippy --workspace --all-targets -j 3 -- -D warnings
cargo test --workspace -j 3
tools/goldcheck.sh rig_deck assets/test/rig_deck.loom
```

`goldcheck.sh` will report **no reference image**. That is correct and expected:
blessing is the verifier's act, not the builder's, and `--bless` has no per-row
filter — one invocation overwrites all 56 existing references. **Do not bless.**
Hand off with the render and the fault-injection numbers.

- [ ] **Step 7: Commit**

```bash
cd ~/loom
git add xtask/src/main.rs assets/SCENES.md
git commit -m "test(gate): rig_deck joins SCENES and GOLDEN

The only picture of a UV-mapped, normal-mapped imported mesh in the library.
Fault-injected two ways: <fill in the measured fractions>."
```

---

## Phase 0 exit criteria

Phase 1 does not start until all of these are true:

1. `rigkit`, `split_parts` and `textures` self-checks all pass.
2. `build_deck.py` runs clean and its height assertion has been **seen to fail**
   under injected fault.
3. `loom sim` gives the **same digits** on the mesh scene and its primitive twin.
4. The render has been **opened and looked at** — planks read as planks, texture
   is not inverted, no gutter-black.
5. `rig_deck` is in both `SCENES` and `GOLDEN`, and both fault injections moved
   a recorded number of pixels.
6. `cargo clippy` and `cargo test --workspace` pass.
7. The reference is **not blessed** — that conversation happens with the human.

## What Phase 0 deliberately does not do

- No Cycles AO or curvature bake. Recorded as the upgrade path if flat lighting
  reads poorly on the deck.
- No fix for the `StepLow`/`StepMid`/`StepHigh` squared colliders. Separate
  change, separate gate row.
- No change to `assets/games/deeper_demo.loom`. The demo keeps its primitive
  deck until Phase 4, so it stays runnable throughout.
- No `.mtl` files. The engine reads none.
