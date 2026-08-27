# Rig Timber Rebuild — Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the rig's shed, its roof, the mast and the two bollards as
textured meshes, without moving a collider or disturbing the mirror.

**Architecture:** The same shape Phase 1 settled into four times over: a numpy
texture generator per material zone, a Blender generator per part carrying its
own assertions, a mesh centred on its own local origin, one node per primitive
replaced, and an explicit `BoxCollider` transcribing today's numbers.

**Tech Stack:** Blender 5.2.0 LTS headless (`bpy`, `bmesh`), Python 3.14 + numpy
2.5.2, Loom CLI, `cargo xtask`.

**Spec:** `docs/design/RIG-TIMBER-REBUILD.md` — read §1 in full before touching
any node. Case 6 (one node per primitive) and Case 5 (the mesh alias picks the
collider's *shape*) both bite in this phase.

## Global Constraints

- **Generators live in the repo** at `tools/mesh/rig/`. Outputs to
  `assets/meshes/` and `assets/textures/`.
- **Meshes are authored CENTRED on their own local origin, in all three axes.**
  `play.rs:520` centres a static collider on the NODE, and `half_extents` is
  local, so a mesh baked at a world position can never be given a matching
  collider. Declare a `*_CENTRE` triple, subtract it, and print the node
  position and half-extents the scene will need.
- **One node per primitive replaced** (spec §1 Case 6). One mesh is one node is
  one collider. Two primitives merged into one mesh get one collider spanning
  both — that is how a bulwark nearly closed the boarding gap in Phase 1.
- **A cylinder primitive's collider is ROUND; a mesh's is a BOX** (spec §1
  Case 5). The mast and both bollards are cylinders. A box circumscribing a
  cylinder exceeds it by `r(√2 − 1)`. Measure what that costs; do not reason
  about it.
- **Export via `rigkit.export_obj`** — it does the axis conversion, the V flip
  and the tag stripping. `verify_obj` enforces per-shell outward normals;
  inward normals render **pure black**.
- **Palette authored in LINEAR reflectance, written as sRGB bytes** via
  `_srgb_encode`. `materials.rs:194` loads `albedo_map` as `ColorSpace::Srgb`.
  Normal maps are data — never sRGB-encode them.
- **Relief is size-invariant, and that is why these zones may be 1024².**
  `normal_from_height` takes a slope across the tile, so the X-std is the same
  at 512, 1024 and 2048 — measured 0.0167 / 0.0170 / 0.0167. The test asserts at
  `textures.TEX_SIZE` (2048); these three zones ship at **1024², per spec §3's
  zone table**, and the same statistic covers both. Six 2048² maps would cost
  75 MB against 19 MB at 1024, and spec §8 already flags texture memory as
  unbudgeted.
- **No new dependencies**: numpy, struct, zlib only.
- **Byte-deterministic builds.** Build twice, md5sum every output.
- **Never bless.** `--bless` has no per-row filter and four rows already lack
  references. Builders may not run `cargo xtask` — it holds a singleton lock.
  Use `bash tools/goldcheck.sh`.
- Never `git add -A`. Stage by explicit path.
- `crates/loom_cli/src/run.rs` and `scripts/green.sh` are modified in the
  working tree, belong to the user, and are never staged, moved, stashed or
  reverted.

## Frozen numbers this phase must not move

| node | world extent | why it is frozen |
| --- | --- | --- |
| `Shed` | `x -8.500…0.500`, `y 1.400…3.800`, `z 2.600…6.600` | `green.sh:1903` strafes a character into it and asserts `state.stuck == 1` |
| **Shed north face** | **`z = 2.600`** | `MirrorFrame` occupies `z 2.570…2.600` and `Mirror` `z 2.550…2.570`. **Nothing may project north of 2.600.** |
| `ShedRoof` | `x -8.900…0.900`, `y 3.800…4.100`, `z 2.200…7.000` | overhangs the shed 0.4 m at the front |
| `Mast` | `x 7.820…8.180`, `y 1.400…5.800`, `z 2.820…3.180` | carries `MastLamp` at `(8.0, 5.60, 3.0)`, the hub's only point light |
| `BollardWest` | centred `x -6.60`, `y 1.400…2.000`, `z -6.620…-6.180` | the pair *is* the berth marker — the only thing saying it is a berth |
| `BollardEast` | centred `x -3.40`, same y and z | both sit **inside** the boarding gap `x -6.900…-3.100` |

**`Mirror` and `MirrorFrame` are NOT rebuilt in this phase.** The spec's zone
table marks `glass_mirror` *untouched*, and the frame is 3 cm of geometry
guarding a stack the demo records as having cost an hour to get right. They stay
`box` primitives. Do not "finish the job" by converting them.

---

### Task 1: `weathered_boards`, and the shed

The shed is one `box` today. It becomes board-and-batten: vertical boards with
battens over the joints, and a door on the east face.

**The constraint that governs this task.** The shed's north face at `z = 2.600`
is the mirror's mount. `MirrorFrame` sits at `z 2.570…2.600` — its back flush to
that face — and `Mirror` in front of it at `z 2.550…2.570`. **A board or batten
standing proud of `z = 2.600` punches through the frame.** So the battens sit
*at* the face plane and the boards are recessed behind them, which is also how
board-and-batten is actually built: the batten covers the joint and stands proud
of the boards, not of the wall line.

**Files:**
- Modify: `tools/mesh/rig/textures.py` (add `weathered_boards`)
- Modify: `tools/mesh/rig/test_textures.py`
- Create: `tools/mesh/rig/build_shed.py`
- Create: `assets/meshes/rig_shed.obj`,
  `assets/textures/rig_shed_albedo.png`, `..._normal.png`

**Interfaces:**
- Consumes: `rigkit.export_obj`, `rigkit.verify_obj`, `textures._fbm`,
  `textures._srgb_encode`, `textures.normal_from_height`, `textures.write_png`,
  `textures.TEX_SIZE`.
- Produces: `textures.weathered_boards(size=1024, seed=17) -> (albedo_uint8, height_float)`;
  `assets/meshes/rig_shed.obj` centred on `SHED_CENTRE = (-4.0, 2.60, 4.60)`,
  spanning local `x ±4.500`, `y ±1.200`, `z ±2.000`. The generator prints the
  node position and `BoxCollider` half-extents Task 4 needs.

- [ ] **Step 1: Write the failing tests for `weathered_boards`**

Append to `test_textures.py` and call both at the foot of the file:

```python
def test_boards_grain_runs_the_short_way():
    """The deck's grain runs ALONG its planks, which are laid flat. A wall board
    stands UP, so its grain runs vertically — the other axis. If this reads the
    same as the deck the shed will look like a floor stood on its edge."""
    alb, _ = textures.weathered_boards(size=256, seed=17)
    chan = alb[:, :, 0].astype(float)
    along_y = np.abs(np.diff(chan, axis=0)).mean()   # down the board
    across_x = np.abs(np.diff(chan, axis=1)).mean()  # across it
    assert across_x > along_y * 1.8, \
        "grain is not vertical: across %.3f vs along %.3f" % (across_x, along_y)
    print("  boards grain ok  across=%.3f along=%.3f" % (across_x, along_y))


def test_boards_are_timber_and_tile():
    alb, h = textures.weathered_boards(size=256, seed=17)
    lin = np.where(alb / 255.0 <= 0.04045, (alb / 255.0) / 12.92,
                   (((alb / 255.0) + 0.055) / 1.055) ** 2.4)
    m = lin.reshape(-1, 3).mean(axis=0)
    # The shed is authored `albedo = [0.46, 0.30, 0.22]` — warmer and lighter
    # than the deck's palette D, because it is a painted-then-faded wall rather
    # than a walked-on floor. R must lead by a clear margin.
    assert m[0] > m[2] * 1.5, "not warm enough for painted timber: %s" % m.round(4)
    # Calibrated the way palette D's was, and for the same reason: a ceiling
    # above the regression it is meant to catch catches nothing. Measured —
    # shipped 0.1487, `painted` brightened to fresh paint 0.2119. The ceiling
    # sits between them. Floor is a regression bound, not a restatement.
    assert (m < 0.175).all(), "too pale for weathered boards: %s" % m.round(4)
    assert (m > 0.05).all(), "too dark: %s" % m.round(4)
    chan = alb[:, :, 0].astype(int)
    ix = np.abs(np.diff(chan, axis=1)).mean(); iy = np.abs(np.diff(chan, axis=0)).mean()
    sx = np.abs(chan[:, 0] - chan[:, -1]).mean(); sy = np.abs(chan[0, :] - chan[-1, :]).mean()
    assert sx <= ix * 2.0 + 1.0, "vertical seam %.2f vs %.2f" % (sx, ix)
    assert sy <= iy * 2.0 + 1.0, "horizontal seam %.2f vs %.2f" % (sy, iy)
    print("  boards ok  linear mean=%s  seam %.2f/%.2f %.2f/%.2f"
          % (m.round(4), sx, ix, sy, iy))
```

- [ ] **Step 2: Run to verify they fail**

Run: `python3 tools/mesh/rig/test_textures.py`
Expected: `AttributeError: module 'textures' has no attribute 'weathered_boards'`.

- [ ] **Step 3: Write `weathered_boards`**

```python
def weathered_boards(size=1024, seed=17):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    A painted timber wall that has been in salt air for years: the paint is
    mostly gone, what is left is chalky, and the grain shows through.

    **The grain runs VERTICALLY here** — down the image — because a wall board
    stands on end. The deck's grain runs along +U because its planks lie flat.
    Same generator, transposed anisotropy; getting it wrong makes the shed read
    as a floor stood on its edge, which is the one thing a viewer notices
    immediately and cannot name.

    LINEAR reflectance in, sRGB bytes out. See `_srgb_encode`.
    """
    rng = np.random.default_rng(seed)

    # Stretched along X and transposed, so the long runs end up vertical.
    grain = (_fbm(size, rng, octaves=6, base=32, stretch=8).T * 0.70
             + _fbm(size, rng, octaves=4, base=4, stretch=4).T * 0.30)
    grain = (grain - grain.min()) / (grain.max() - grain.min())

    # Paint survives in patches, not evenly. Low frequency, isotropic.
    paint = _fbm(size, rng, octaves=4, base=3)
    paint = (paint - paint.min()) / (paint.max() - paint.min())
    kept = np.clip(paint * 1.45 - 0.35, 0.0, 1.0)

    # Rot creeps up from the bottom of a board, so it is gated by a vertical
    # ramp as well as by its own field. **The ramp has to WRAP.** A
    # `np.linspace` down the image is the obvious ramp and it does not tile:
    # measured seam_y 12.10 against a 0.24 interior baseline, a hard band across
    # the wall wherever V passes 1. The shed is 2.4 m tall and the UV maps 2 m
    # per tile, so this texture IS tiled vertically — an earlier draft of this
    # comment claimed otherwise and was wrong. A raised cosine is periodic and
    # puts the rot at both ends of the tile, which on a wall reads as rot at the
    # sill and under the eaves. Seam_y 0.32 against 0.29.
    rot_f = _fbm(size, rng, octaves=5, base=6, stretch=4).T
    rot_f = (rot_f - rot_f.min()) / (rot_f.max() - rot_f.min())
    t = np.arange(size, dtype=np.float32) / size
    rise = (0.5 - 0.5 * np.cos(2.0 * np.pi * t))[:, None]
    rot = np.clip((rot_f * 0.6 + rise * 0.6) - 0.62, 0.0, 1.0)

    height = np.clip(0.30 + 0.45 * grain - 0.25 * rot, 0.0, 1.0).astype(np.float32)

    g = grain[:, :, None]
    bare = np.array([0.088, 0.066, 0.047]) + g * np.array([0.080, 0.060, 0.042])
    painted = np.array([0.160, 0.098, 0.064]) + g * np.array([0.068, 0.042, 0.027])
    rotten = np.array([0.030, 0.027, 0.020]) + g * np.array([0.024, 0.022, 0.016])

    k = kept[:, :, None]
    r = rot[:, :, None]
    rgb = bare * (1.0 - k) + painted * k
    rgb = rgb * (1.0 - r) + rotten * r
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height
```

- [ ] **Step 4: Run the tests, then look**

Run: `python3 tools/mesh/rig/test_textures.py` — expected: all checks pass,
including the two new `boards` lines.

Then render a three-panel sheet and **open it**:

```bash
cd ~/loom && python3 - <<'PY'
import sys; sys.path.insert(0, "tools/mesh/rig")
import numpy as np, textures
alb, h = textures.weathered_boards(size=512, seed=17)
tile = np.concatenate([np.concatenate([alb, alb], 1)] * 2, 0)[::2, ::2]
textures.write_png("/tmp/boards_sheet.png",
                   np.concatenate([alb, textures.normal_from_height(h), tile], axis=1))
PY
```

Grain running **down** the image, paint surviving in patches over bare timber,
rot heavier at both ends of the tile. If the grain runs sideways the transpose
is missing. Measured when this plan was written, size 256 seed 17:

    linear mean [0.1487, 0.1002, 0.0680]   std 0.0166   R/B 2.19
    seam_x 0.32 / 0.29 baseline            seam_y 0.32 / 0.29

**One thing to judge rather than inherit:** the raised-cosine rot ramp puts a
dark band at each end of the tile, and the wall is 2.4 m against a 2 m tile — so
you see 1.2 bands up a wall. In the swatch that reads as rot at sill and eaves.
On the built shed it might read as a stripe instead. Task 4's render is where
that gets decided; if it stripes, the ramp's 0.6 weight is the knob.

- [ ] **Step 5: Write `build_shed.py`**

```python
# tools/mesh/rig/build_shed.py
"""The rig's shed — board-and-batten walls with a door, and nothing proud of
the north face.

Replaces the DRAWN surface of `Shed` in assets/games/deeper_demo.loom, a single
`box` at `pos = [-4.0, 2.60, 4.60]`, `scale = [4.5, 1.20, 2.00]` — world
x -8.500..0.500, y 1.400..3.800, z 2.600..6.600.

**THE NORTH FACE IS THE MIRROR'S MOUNT AND MUST STAY EXACTLY AT z = 2.600.**
`MirrorFrame` occupies z 2.570..2.600 with its back flush to that face, and
`Mirror` sits in front of it at z 2.550..2.570. A board or batten standing proud
of the face punches through the frame. The demo's own header records getting
this stack backwards as the mistake that cost an hour.

So the construction is inverted from the obvious one: the BATTENS sit at the
face plane and the BOARDS are recessed behind them. That is also how
board-and-batten is really built -- a batten covers the joint between two
boards and stands proud of THEM, not of the wall line.

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
TEX_DIR = os.path.join(REPO, "assets", "textures")

# World extents, straight off the node. HALF_* are the primitive's `scale`.
SHED_CENTRE = (-4.0, 2.60, 4.60)
HALF_X, HALF_Y, HALF_Z = 4.5, 1.20, 2.00
NORTH_FACE_WORLD = SHED_CENTRE[2] - HALF_Z          # 2.600 — frozen
BOARD_W = 0.200          # board width
BATTEN_W = 0.070         # batten covering each joint
RECESS = 0.018           # how far the boards sit BEHIND the batten face
DOOR_W, DOOR_H = 0.90, 1.95
SEED = 17
ZONE_SIZE = 1024         # spec §3: this zone is 1024², not TEX_SIZE's 2048

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

mesh = bpy.data.meshes.new("rig_shed")
obj = bpy.data.objects.new("rig_shed", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")


def slab(x0, x1, y0, y1, z0, z1, uv_u0):
    """One box in LOCAL metres (already centred). U across the run, V up it."""
    c = [(x0, -z0, y0), (x1, -z0, y0), (x1, -z1, y0), (x0, -z1, y0),
         (x0, -z0, y1), (x1, -z0, y1), (x1, -z1, y1), (x0, -z1, y1)]
    v = [bm.verts.new(p) for p in c]
    quads = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
             (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
    faces = [bm.faces.new(tuple(v[i] for i in q)) for q in quads]
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            # One tile per 1 m across and per 2 m up. The grain runs vertically
            # in this texture, so V is the along-grain axis and gets the longer
            # span -- the reverse of the deck's mapping, deliberately. The wall
            # is 2.4 m, so V passes 1 and the texture DOES tile vertically:
            # `weathered_boards`'s rot ramp is a raised cosine for exactly that
            # reason. Do not "simplify" it to a linspace.
            loop[uv_layer].uv = (uv_u0 + x / 1.0, z / 2.0)
    return faces


# Each wall is a run of vertical boards, recessed, with a batten over every
# joint standing at the wall plane. `outward` is the local axis the wall faces.
WALLS = [
    # (fixed axis, plane, from, to, outward sign)
    ("z", -HALF_Z, -HALF_X, HALF_X, -1),   # north — THE MIRROR FACE
    ("z",  HALF_Z, -HALF_X, HALF_X, +1),   # south
    ("x", -HALF_X, -HALF_Z, HALF_Z, -1),   # west
    ("x",  HALF_X, -HALF_Z, HALF_Z, +1),   # east — carries the door
]

rng_state = SEED
def rand():
    global rng_state
    rng_state = (rng_state * 1103515245 + 12345) & 0x7FFFFFFF
    return rng_state / 0x7FFFFFFF


def wall(axis, plane, a, b, outward, door=False):
    """Boards recessed, battens at the plane. Nothing crosses `plane`."""
    span = b - a
    n = max(1, int(round(span / BOARD_W)))
    w = span / n
    # `inner` is the recessed board face; `plane` is where the batten sits.
    inner = plane - outward * RECESS
    for i in range(n):
        p0, p1 = a + i * w, a + (i + 1) * w
        # The door is a gap in the boards of the east wall, centred on the run.
        mid = (p0 + p1) * 0.5
        door_here = door and abs(mid) < DOOR_W * 0.5
        y0 = -HALF_Y + (DOOR_H if door_here else 0.0)
        if y0 >= HALF_Y:
            continue
        if axis == "z":
            slab(p0, p1, y0, HALF_Y, min(plane, inner), max(plane, inner), rand())
        else:
            slab(min(plane, inner), max(plane, inner), y0, HALF_Y, p0, p1, rand())
        # A batten over the joint at the far edge of every board but the last.
        if i < n - 1:
            j0, j1 = p1 - BATTEN_W * 0.5, p1 + BATTEN_W * 0.5
            if axis == "z":
                slab(j0, j1, y0, HALF_Y, min(plane, inner), max(plane, inner), rand())
            else:
                slab(min(plane, inner), max(plane, inner), y0, HALF_Y, j0, j1, rand())


for axis, plane, a, b, outward in WALLS:
    wall(axis, plane, a, b, outward, door=(axis == "x" and outward > 0))

# A flat roof deck so the box is closed and the shed is not hollow from above.
slab(-HALF_X, HALF_X, HALF_Y - 0.04, HALF_Y, -HALF_Z, HALF_Z, 0.0)

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()

os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)
rigkit.export_obj(obj, OBJ_PATH, uv=True,
                  header="rig: shed, board-and-batten. Boards recessed %.0f mm; "
                         "battens at the wall plane. Nothing proud of the north face."
                         % (RECESS * 1000))

albedo, height = textures.weathered_boards(size=ZONE_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_shed_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_shed_normal.png"),
                   textures.normal_from_height(height))

# --- self-check ----------------------------------------------------------
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

for ax, name, half in ((0, "x", HALF_X), (1, "y", HALF_Y), (2, "z", HALF_Z)):
    assert abs(lo[ax] + half) < 1e-3 and abs(hi[ax] - half) < 1e-3, \
        "shed %s spans %.4f..%.4f, want %.3f..%.3f — the mesh is not centred on " \
        "its own origin and its collider cannot be made to match" \
        % (name, lo[ax], hi[ax], -half, half)

# **THE ONE THAT MATTERS.** Local -HALF_Z is world NORTH_FACE_WORLD = 2.600, and
# MirrorFrame starts there. A vertex below it in local z is a board through the
# frame. `lo[2]` is already asserted equal to -HALF_Z above; this states WHY, in
# world terms, so the next person does not relax it as a duplicate.
assert lo[2] >= -HALF_Z - 1e-4, \
    "a vertex reaches local z=%.5f (world %.5f), north of the shed's face at " \
    "world %.3f — MirrorFrame occupies %.3f..%.3f and this punches through it" \
    % (lo[2], lo[2] + SHED_CENTRE[2], NORTH_FACE_WORLD, 2.570, 2.600)

assert info["tris"] <= 6000, "over budget: %d tris" % info["tris"]
print("shed: node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
      % (SHED_CENTRE + (HALF_X, HALF_Y, HALF_Z)))
print("shed: %d tris, %d verts, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("shed: self-check passes")
```

- [ ] **Step 6: Build it**

```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/build_shed.py 2>&1 | grep -E '^shed:|Error|assert'
```
Expected: a `shed:` node line, a `shed:` bounds line reading
`x -4.500..4.500 y -1.200..1.200 z -2.000..2.000`, then `shed: self-check passes`.

- [ ] **Step 7: Prove the north-face assertion can fail**

Temporarily set `RECESS = -0.018` — battens recessed and boards proud, the
inverted construction. Re-run. Expected: **`north of the shed's face at world
2.600 — MirrorFrame occupies 2.570..2.600 and this punches through it`**.
Restore `RECESS = 0.018`. This is the assertion the whole task is shaped around
and it must be seen to fire.

- [ ] **Step 8: Confirm determinism, then commit**

```bash
cd ~/loom
md5sum assets/meshes/rig_shed.obj
blender --background --factory-startup --python tools/mesh/rig/build_shed.py >/dev/null 2>&1
md5sum assets/meshes/rig_shed.obj
git add tools/mesh/rig/textures.py tools/mesh/rig/test_textures.py \
        tools/mesh/rig/build_shed.py assets/meshes/rig_shed.obj \
        assets/textures/rig_shed_albedo.png assets/textures/rig_shed_normal.png
git commit -m "feat(rig): the shed, board-and-batten, nothing proud of the mirror"
```

---

### Task 2: `corrugated_metal`, and the roof

**Files:**
- Modify: `tools/mesh/rig/textures.py` (add `corrugated_metal`)
- Modify: `tools/mesh/rig/test_textures.py`
- Create: `tools/mesh/rig/build_roof.py`
- Create: `assets/meshes/rig_shed_roof.obj`,
  `assets/textures/rig_roof_albedo.png`, `..._normal.png`

**Interfaces:**
- Consumes: the same `rigkit` and `textures` helpers as Task 1.
- Produces: `textures.corrugated_metal(size=1024, seed=19) -> (albedo_uint8, height_float)`;
  `assets/meshes/rig_shed_roof.obj` centred on `ROOF_CENTRE = (-4.0, 3.95, 4.60)`,
  local `x ±4.900`, `y ±0.150`, `z ±2.400`.

- [ ] **Step 1: Write the failing test**

```python
def test_corrugated_metal_is_cold_and_streaked():
    """Galvanised sheet gone to rust. Unlike the piles' steel this is a COLD
    base with warm rust on top, so the mean should sit near neutral rather than
    rust-shifted — if R leads strongly this is just more pile."""
    alb, _ = textures.corrugated_metal(size=256, seed=19)
    lin = np.where(alb / 255.0 <= 0.04045, (alb / 255.0) / 12.92,
                   (((alb / 255.0) + 0.055) / 1.055) ** 2.4)
    m = lin.reshape(-1, 3).mean(axis=0)
    # Measured 1.24 shipped; 1.05 with the rust threshold at its first-draft
    # 0.62, which is visually bare galvanise. The band excludes both that and a
    # roof rusted as hard as the piles.
    assert 1.10 < m[0] / m[2] < 1.45, \
        "should be galvanise with rust on top, got R/B %.2f (%s)" % (m[0] / m[2], m.round(4))
    assert (m < 0.16).all(), "too bright for weathered galvanise: %s" % m.round(4)
    assert lin.reshape(-1, 3).std(axis=0).mean() > 0.012, "too flat"
    print("  roof ok  linear mean=%s  R/B %.2f" % (m.round(4), m[0] / m[2]))
```
Call it at the foot of the file.

- [ ] **Step 2: Run to verify it fails**

Run: `python3 tools/mesh/rig/test_textures.py`
Expected: `AttributeError: module 'textures' has no attribute 'corrugated_metal'`.

- [ ] **Step 3: Write `corrugated_metal`**

```python
def corrugated_metal(size=1024, seed=19):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    Galvanised sheet, weathered. The spangle has gone chalky, rust has taken the
    laps and the fixings, and dirt has run down from every one.

    **The corrugation itself is GEOMETRY, not this texture.** `build_roof.py`
    folds the sheet; this supplies the surface on top of it. Baking ridges into
    the height map as well would double them and read as a moire.

    LINEAR reflectance in, sRGB bytes out.
    """
    rng = np.random.default_rng(seed)

    chalk = _fbm(size, rng, octaves=5, base=6)
    chalk = (chalk - chalk.min()) / (chalk.max() - chalk.min())
    # Rust starts at fixings and laps -- sparse points, not a field.
    spot = _fbm(size, rng, octaves=6, base=14)
    spot = (spot - spot.min()) / (spot.max() - spot.min())
    # 0.62 leaves so little rust that the mean comes back R/B 1.05 — visually
    # bare galvanise, and sitting on its own assertion's boundary. 0.52 gives
    # 1.24, which reads as a rusting roof and has headroom both ways.
    rust = np.clip((spot - 0.52) / 0.22, 0.0, 1.0)
    # And runs DOWN from each one. Same wrapping smear as `weathered_steel`:
    # `np.maximum.accumulate` does not tile and puts a band across the sheet.
    REACH = size // 8
    smear = rust.copy()
    for k in range(1, REACH):
        smear = np.maximum(smear, np.roll(rust, k, axis=0) * (1.0 - k / REACH))
    dirt = np.clip(smear * 0.55 - 0.15, 0.0, 1.0)

    height = np.clip(0.40 + 0.35 * rust + 0.15 * chalk, 0.0, 1.0).astype(np.float32)

    c = chalk[:, :, None]
    zinc = np.array([0.088, 0.092, 0.096]) + c * np.array([0.046, 0.048, 0.050])
    rusty = np.array([0.150, 0.072, 0.034]) + c * np.array([0.080, 0.038, 0.016])
    grime = np.array([0.042, 0.041, 0.038]) + c * np.array([0.028, 0.027, 0.024])

    r = rust[:, :, None]
    d = dirt[:, :, None]
    rgb = zinc * (1.0 - r) + rusty * r
    rgb = rgb * (1.0 - d) + grime * d
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height
```

- [ ] **Step 4: Run the tests and look at it**

Run: `python3 tools/mesh/rig/test_textures.py` — all pass. Then render a sheet
the same way as Task 1 Step 4 with `corrugated_metal` and **open it**. A cold
grey base with warm rust patches and dark runs trailing **downward** from them.
No band across the image — if there is one the wrapping smear was replaced with
`accumulate`. Measured, size 256 seed 19:

    linear mean [0.1201, 0.1037, 0.0968]   std 0.0200   R/B 1.24

- [ ] **Step 5: Write `build_roof.py`**

```python
# tools/mesh/rig/build_roof.py
"""The shed's roof — corrugated sheet with two holes rusted through it.

Replaces the DRAWN surface of `ShedRoof` in assets/games/deeper_demo.loom, a
`box` at `pos = [-4.0, 3.95, 4.60]`, `scale = [4.9, 0.15, 2.40]` — world
x -8.900..0.900, y 3.800..4.100, z 2.200..7.000. It overhangs the shed by 0.4 m
at the front and 0.4 m each side.

Corrugations run along Z, front to back, so water sheds off the overhang rather
than along the ridge. The profile therefore varies in X. Spec §1 Case 1:
axis-aligned, drawn, statically parented -- transcription is exact.

**The holes are geometry, not alpha.** An alpha-cut surface casts its whole
triangle's shadow, because ray queries never run a fragment shader
(loom_scene Material::alpha_cutoff). A hole that still shadows is not a hole.

Run: blender --background --factory-startup --python build_roof.py
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
OBJ_PATH = os.path.join(REPO, "assets", "meshes", "rig_shed_roof.obj")
TEX_DIR = os.path.join(REPO, "assets", "textures")

ROOF_CENTRE = (-4.0, 3.95, 4.60)
HALF_X, HALF_Y, HALF_Z = 4.9, 0.15, 2.40
# **The budget is what sets the resolution here, and it binds hard.** Every
# cell is a closed box (6 quads, 12 tris) so each shell is watertight and
# `verify_obj`'s per-shell volume check can see it. At the obvious 0.15 m
# corrugation pitch and a z-resolution fine enough to cut small holes, that is
# 65 x 48 = 3,120 cells = 37,440 triangles against a 2,000 cap -- eighteen times
# over. Coarsening to a 0.30 m pitch and five z-bands gives 33 x 5 = 165 cells =
# 1,980 tris, which fits.
#
# The z-bands are 0.96 m, so a hole is a whole band deep. That is not a
# compromise: corrugated roofing does not develop pinholes, it loses a SHEET.
# A band-deep gap is what the failure actually looks like.
PITCH = 0.300            # one corrugation period, in X
AMP = 0.045              # ridge height, well inside HALF_Y
ZSTEPS = 5               # z-bands; see the budget note above
SEED = 19
ZONE_SIZE = 1024         # spec §3: this zone is 1024², not TEX_SIZE's 2048
# Holes are given in CELL INDICES, not metres, so they land on cell boundaries
# exactly and the count assertion below is exact rather than approximate.
HOLES = [(7, 1), (8, 1), (24, 3)]     # (period index, z-band index)

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

mesh = bpy.data.meshes.new("rig_shed_roof")
obj = bpy.data.objects.new("rig_shed_roof", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")

periods = int(round((2 * HALF_X) / PITCH))
step = (2 * HALF_X) / periods
zstep = (2 * HALF_Z) / ZSTEPS


def prof(x):
    """The corrugation profile: y offset at a given x."""
    return AMP * math.sin(2.0 * math.pi * x / PITCH)


for i in range(periods):
    x0 = -HALF_X + i * step
    x1 = x0 + step
    for j in range(ZSTEPS):
        z0 = -HALF_Z + j * zstep
        z1 = z0 + zstep
        if (i, j) in HOLES:
            continue
        yt0, yt1 = HALF_Y - AMP + prof(x0), HALF_Y - AMP + prof(x1)
        # Top surface, following the fold.
        a = bm.verts.new((x0, -z0, yt0)); b = bm.verts.new((x1, -z0, yt1))
        c = bm.verts.new((x1, -z1, yt1)); d = bm.verts.new((x0, -z1, yt0))
        # Underside, flat.
        e = bm.verts.new((x0, -z0, -HALF_Y)); f = bm.verts.new((x1, -z0, -HALF_Y))
        g = bm.verts.new((x1, -z1, -HALF_Y)); h = bm.verts.new((x0, -z1, -HALF_Y))
        faces = [bm.faces.new((a, b, c, d)), bm.faces.new((e, h, g, f))]
        # Close the cell so every shell is watertight and its signed volume is
        # positive -- `verify_obj` checks that per shell, and an open cell would
        # read as inward-facing.
        faces.append(bm.faces.new((a, e, f, b)))
        faces.append(bm.faces.new((c, g, h, d)))
        faces.append(bm.faces.new((b, f, g, c)))
        faces.append(bm.faces.new((d, h, e, a)))
        for fc in faces:
            for loop in fc.loops:
                x, y, z = loop.vert.co
                loop[uv_layer].uv = (x / 2.0, -y / 2.0)

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()

os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)
rigkit.export_obj(obj, OBJ_PATH, uv=True,
                  header="rig: shed roof. %d corrugations at %.3f m, amp %.3f, "
                         "%d holes rusted through." % (periods, PITCH, AMP, len(HOLES)))

albedo, height = textures.corrugated_metal(size=ZONE_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_roof_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_roof_normal.png"),
                   textures.normal_from_height(height))

# --- self-check ----------------------------------------------------------
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

for ax, name, half in ((0, "x", HALF_X), (2, "z", HALF_Z)):
    assert abs(lo[ax] + half) < 1e-3 and abs(hi[ax] - half) < 1e-3, \
        "roof %s spans %.4f..%.4f, want %.3f..%.3f" % (name, lo[ax], hi[ax], -half, half)
# The corrugation must stay INSIDE the primitive's y envelope, or the drawn roof
# stands above the collider the player would meet if he ever got up there.
assert lo[1] >= -HALF_Y - 1e-4 and hi[1] <= HALF_Y + 1e-4, \
    "roof y spans %.4f..%.4f, outside the primitive's +/-%.3f — the fold amplitude " \
    "AMP=%.3f is too large for HALF_Y" % (lo[1], hi[1], HALF_Y, AMP)

# The holes must actually be holes, and EXACTLY the ones asked for. Cells are
# indexed, so this is an equality rather than an inequality — an inequality
# would pass if a bug dropped the wrong cells, or twice as many.
full = periods * ZSTEPS
cells = info["tris"] // 12          # 6 quads -> 12 tris per closed cell
assert cells == full - len(HOLES), \
    "expected %d cells (%d full minus %d holes), got %d — the holes cut are not " \
    "the holes asked for" % (full - len(HOLES), full, len(HOLES), cells)
print("roof: %d of %d cells present, %d dropped for holes" % (cells, full, full - cells))

assert info["tris"] <= 2000, "over budget: %d tris" % info["tris"]
print("roof: node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
      % (ROOF_CENTRE + (HALF_X, HALF_Y, HALF_Z)))
print("roof: %d tris, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (info["tris"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("roof: self-check passes")
```

- [ ] **Step 6: Build, and prove both assertions can fail**

Run the build; expect a `roof:` cells line, a bounds line and
`roof: self-check passes`.

Then two injections, each restored after:
- Set `AMP = 0.400` (a fold taller than the roof is thick). Expected:
  **`outside the primitive's +/-0.150`**.
- Set `HOLES = []`. Expected: **`expected 162 cells (165 full minus 3 holes), got 165`**.

- [ ] **Step 7: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/textures.py tools/mesh/rig/test_textures.py \
        tools/mesh/rig/build_roof.py assets/meshes/rig_shed_roof.obj \
        assets/textures/rig_roof_albedo.png assets/textures/rig_roof_normal.png
git commit -m "feat(rig): a corrugated roof with two holes rusted through it"
```

---

### Task 3: `painted_iron`, the mast and the bollards

Three cylinder primitives. **Spec §1 Case 5 applies**: a `cylinder` alias gets a
round collider from `play.rs:588-591`; a mesh gets a box. **Case 6 applies too**:
the two bollards are identical (`scale = [0.22, 0.30, 0.22]` on both), so they
are ONE mesh placed by TWO nodes — never one mesh containing both.

**The bollards sit inside the boarding gap.** `x -6.820…-6.380` and
`-3.620…-3.180`, within `x -6.900…-3.100`. Spec §5 calls that lane the demo's
only taught gesture. A box collider there is 0.145 × 0.22 = 32 mm wider at the
corners than the cylinder it replaces. **Step 6 measures whether the player can
still walk between them.**

**Files:**
- Modify: `tools/mesh/rig/textures.py` (add `painted_iron`)
- Modify: `tools/mesh/rig/test_textures.py`
- Create: `tools/mesh/rig/build_hardware.py`
- Create: `assets/meshes/rig_mast.obj`, `assets/meshes/rig_bollard.obj`,
  `assets/textures/rig_hardware_albedo.png`, `..._normal.png`

**Interfaces:**
- Consumes: `rigkit`, `textures`, and a `tube()` helper — **copy the corrected
  one from `build_substructure.py`**, which assigns U from the ring index rather
  than from `atan2`. The `atan2` form has no seam split and makes one facet draw
  94% of the texture mirrored; that was fixed in Phase 1 and must not come back.
- Produces: `textures.painted_iron(size=1024, seed=23) -> (albedo_uint8, height_float)`;
  `rig_mast.obj` centred on `(8.0, 3.60, 3.0)` local `±0.18, ±2.20, ±0.18`;
  `rig_bollard.obj` centred on its own origin, local `±0.22, ±0.30, ±0.22`.

- [ ] **Step 1: Write the failing test**

```python
def test_painted_iron_is_dark_and_worn():
    """Bollards and a mast: iron under paint that has mostly gone. Darker than
    the roof, less rust-shifted than the piles — it is painted, not bare."""
    alb, _ = textures.painted_iron(size=256, seed=23)
    lin = np.where(alb / 255.0 <= 0.04045, (alb / 255.0) / 12.92,
                   (((alb / 255.0) + 0.055) / 1.055) ** 2.4)
    m = lin.reshape(-1, 3).mean(axis=0)
    assert (m < 0.11).all(), "too bright for worn ironwork: %s" % m.round(4)
    assert (m > 0.03).all(), "too dark: %s" % m.round(4)
    assert lin.reshape(-1, 3).std(axis=0).mean() > 0.010, "too flat"
    print("  iron ok  linear mean=%s" % m.round(4))
```
Call it at the foot of the file.

- [ ] **Step 2: Run to verify it fails**

Expected: `AttributeError: module 'textures' has no attribute 'painted_iron'`.

- [ ] **Step 3: Write `painted_iron`**

```python
def painted_iron(size=1024, seed=23):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    Bollards and a mast: cast iron that was painted once. The paint survives in
    the hollows and has been rubbed off every edge and every place a rope has
    passed, which is most of a bollard.

    Isotropic on purpose -- unlike the timber and unlike the piles' vertical
    streaking, wear on a bollard has no grain and no gravity direction. It
    follows the rope, and the rope goes everywhere.

    LINEAR reflectance in, sRGB bytes out.
    """
    rng = np.random.default_rng(seed)

    wear = _fbm(size, rng, octaves=5, base=7)
    wear = (wear - wear.min()) / (wear.max() - wear.min())
    pit = _fbm(size, rng, octaves=6, base=30)
    pit = (pit - pit.min()) / (pit.max() - pit.min())
    bare = np.clip((wear - 0.45) / 0.28, 0.0, 1.0)
    rust = np.clip((pit - 0.68) / 0.18, 0.0, 1.0) * bare

    height = np.clip(0.45 + 0.25 * pit - 0.20 * bare, 0.0, 1.0).astype(np.float32)

    p = pit[:, :, None]
    paint = np.array([0.052, 0.048, 0.044]) + p * np.array([0.030, 0.028, 0.026])
    iron = np.array([0.070, 0.068, 0.070]) + p * np.array([0.040, 0.039, 0.040])
    rusty = np.array([0.098, 0.050, 0.028]) + p * np.array([0.050, 0.026, 0.014])

    b = bare[:, :, None]
    r = rust[:, :, None]
    rgb = paint * (1.0 - b) + iron * b
    rgb = rgb * (1.0 - r) + rusty * r
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height
```

- [ ] **Step 4: Run the tests and look at it**

All checks pass. Render a sheet as before and **open it**: near-black paint with
bare iron showing through in worn patches and a little rust in the pits. It
should look nothing like the piles — no vertical streaking, no direction at all.
Measured, size 256 seed 23:

    linear mean [0.0755, 0.0703, 0.0673]   std 0.0112   R/B 1.12

- [ ] **Step 5: Write `build_hardware.py`**

Copy `tube()` verbatim from `build_substructure.py` — including its ring-index
UV assignment — and emit two meshes:

```python
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
is round, chosen by the string "cylinder" in play.rs:588-591. A mesh gets a box,
which exceeds the cylinder by r(sqrt2 - 1) at the corners -- 32 mm on a bollard.
Step 6 measures whether the boarding walk notices.

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
```

- [ ] **Step 6: Build, then MEASURE the box-vs-round change on the boarding walk**

This is the Case 5 measurement and it must be measured, not argued. Build two
throwaway scenes in `/tmp` with **absolute asset paths** — a scene copied out of
`assets/` cannot resolve relative ones, and then even an unmodified control
renders stand-in boxes. Both have the deck and both bollards; one uses the
`cylinder` primitives exactly as the demo authors them, the other the mesh with
`BoxCollider { half_extents = [0.22, 0.30, 0.22] }` at each position.

Two traps already paid for in Phase 1, both now in spec §1 — read them:
- **`--hold` does not move a bare `CharacterController`.** The movement model
  lives in the node's `Script`; attach `assets/scripts/deeper_player.rhai`.
- **An unaimed probe reads identical across every configuration**, including
  no-collider. Always run a no-collider control and confirm your probe was
  obstructed at all.

Walk a probe north through the gap between the bollards, from `x = -5.0`
(mid-gap), and separately walk one directly INTO a bollard from the east:

```bash
cd ~/loom
for V in prim mesh none; do
  printf "%-5s through: " "$V"
  ./target/release/loom sim /tmp/boll_$V.loom --ticks 600 --hold "0:move_z=-1" \
    --assert "Rig/Probe.z == -999" 2>&1 | grep -o '"actual": [-0-9.]*'
  printf "%-5s into:    " "$V"
  ./target/release/loom sim /tmp/boll_${V}_side.loom --ticks 600 --hold "0:move_x=-1" \
    --assert "Rig/Probe.x == -999" 2>&1 | grep -o '"actual": [-0-9.]*'
done
```

**Record all six numbers.** What must be true: the *through* walk is unobstructed
in both `prim` and `mesh` (the gap is 2.76 m and a capsule is 0.70), and the
*into* walk stops in both, within roughly 0.05 m of each other. If the through
walk is blocked in `mesh` but not `prim`, stop and report BLOCKED — a box in the
boarding lane that a cylinder was not is exactly what spec §5 forbids.

- [ ] **Step 7: Prove the clearance assertion can fail**

Temporarily set `BOLLARD_R = 1.20` and re-run the build. Expected:
**`only 0.800 m between the bollards' box faces`**. Restore `0.22`.

**Not `1.10`** — this step said so and the value did not fire. The bollards
are 3.20 m apart, so the gap is `3.20 - 2R`: `1.10` leaves 1.000 m against a
bound of `2 * 0.35 + 0.20 = 0.90`, and the build prints
`hardware: 1.000 m clear between the bollards' box faces` and passes.
`1.20` leaves 0.800 and refuses. Both measured 2026-08-27.

- [ ] **Step 8: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/textures.py tools/mesh/rig/test_textures.py \
        tools/mesh/rig/build_hardware.py \
        assets/meshes/rig_mast.obj assets/meshes/rig_bollard.obj \
        assets/textures/rig_hardware_albedo.png assets/textures/rig_hardware_normal.png
git commit -m "feat(rig): the mast and the bollards, one mesh placed twice"
```

---

### Task 4: The scene, and a gate row

**Files:**
- Modify: `assets/test/rig_structure.loom` (add the five new nodes)
- Modify: `xtask/src/main.rs` (`GOLDEN` gains one row; `SCENES` unchanged —
  the scene is already registered)
- Modify: `assets/SCENES.md` (regenerated)

**Interfaces:**
- Consumes: everything Tasks 1-3 produced, and the node positions and
  half-extents those generators PRINT.

- [ ] **Step 1: Read what the generators print**

```bash
cd ~/loom
for G in shed roof hardware; do
  blender --background --factory-startup --python tools/mesh/rig/build_$G.py 2>&1 \
    | grep -E "^(shed|roof|hardware|rig_mast|rig_bollard):"
done
```
Those lines are the source of truth for the five new nodes. Do not hand-transcribe
them from this plan.

- [ ] **Step 2: Add the five nodes to `assets/test/rig_structure.loom`**

The scene already holds 30 nodes and about 23 assets. Add:

| node | mesh | transform | collider |
| --- | --- | --- | --- |
| `Shed` | `rig_shed.obj` | `[-4.0, 2.60, 4.60]` | `[4.5, 1.20, 2.00]` |
| `ShedRoof` | `rig_shed_roof.obj` | `[-4.0, 3.95, 4.60]` | `[4.9, 0.15, 2.40]` |
| `Mast` | `rig_mast.obj` | `[8.0, 3.60, 3.0]` | `[0.18, 2.20, 0.18]` |
| `BollardWest` | `rig_bollard.obj` | `[-6.60, 1.70, -6.40]` | `[0.22, 0.30, 0.22]` |
| `BollardEast` | `rig_bollard.obj` | `[-3.40, 1.70, -6.40]` | `[0.22, 0.30, 0.22]` |

**Both bollards reference the SAME `[[asset]]`.** One mesh, two nodes — that is
the point of Task 3 and it must not become two assets.

Materials, taken from the primitives they replace:

    Shed         albedo [1,1,1] + rig_shed_*      roughness 0.88  metallic 0.0
    ShedRoof     albedo [1,1,1] + rig_roof_*      roughness 0.75  metallic 0.4
    Mast         albedo [1,1,1] + rig_hardware_*  roughness 0.80  metallic 0.5
    Bollard×2    albedo [1,1,1] + rig_hardware_*  roughness 0.70  metallic 0.6

**Do NOT add `Mirror` or `MirrorFrame`.** They stay primitives and are not part
of this scene — but note in the scene header that the shed's north face at
world `z = 2.600` is their mount and why nothing may project past it, so the
next person does not add a proud batten in a later phase.

New `[[asset]]` entries need real, DISTINCT UUIDs — three meshes and six
textures. Generate them; verify no duplicates and no empty ids before
committing. An empty id aliases every other empty id and the first adopted wins.

- [ ] **Step 3: Validate and measure**

```bash
cd ~/loom
./target/release/loom validate assets/test/rig_structure.loom
./target/release/loom measure assets/test/rig_structure.loom --node Rig/Shed
```
The `assets` array **must be empty**. `Rig/Shed` must measure `[9.0, 2.4, 4.0]`.

- [ ] **Step 4: Render and look**

```bash
cd ~/loom
./target/release/loom render assets/test/rig_structure.loom --out /tmp/p2.png --size 320x200
```
**Open it**, and take one closer shot. The shed should read as boards, not as a
painted box; the roof should show corrugation; the two bollards should read as
the berth markers they are. Report honestly whether the shed's boards survive at
gate size or whether only the silhouette changed.

- [ ] **Step 5: Add the GOLDEN row and fault-inject it**

The scene is already in `SCENES`. Bump `const GOLDEN: [(&str, &str, &[&str]); 58]`
to `59` — **the length is part of the type** — and add a second row viewing the
shed end, since the existing `rig_structure` camera looks at the north-west
corner and barely sees it. Name it `rig_shed`.

Take a no-op control copy first, with asset paths rewritten to absolute:

```bash
cd ~/loom
abs() { sed 's|\.\./meshes/|'"$PWD"'/assets/meshes/|; s|\.\./textures/|'"$PWD"'/assets/textures/|' "$1"; }
abs assets/test/rig_structure.loom > /tmp/c0.loom
./target/release/loom render /tmp/c0.loom --out /tmp/c0.png --size 320x200
./target/release/loom compare /tmp/c0.png /tmp/p2.png     # expect 0 differing
```
Then two faults, each measured and recorded in the commit message: delete the
`Shed` node, and delete the roof's `albedo_map`. **If either moves nothing, the
row does not guard what its comment claims** — change the comment.

- [ ] **Step 6: Regenerate the index and run the gates a builder may run**

```bash
cd ~/loom
python3 tools/scene_index.py
cargo clippy --workspace --all-targets -j 3 -- -D warnings
cargo test --workspace -j 3
bash tools/goldcheck.sh rig_shed assets/test/rig_structure.loom
```
`goldcheck.sh` will report **no reference image**. Correct and expected.
**Do not bless.**

- [ ] **Step 7: Commit**

```bash
cd ~/loom
git add assets/test/rig_structure.loom xtask/src/main.rs assets/SCENES.md
git commit -m "test(gate): the shed end joins GOLDEN"
```

---

## Phase 2 exit criteria

1. The shed reads as boards and **nothing projects north of world `z = 2.600`** —
   the assertion has been *seen to fire* under the inverted-recess injection.
2. The roof shows corrugation and its holes are real geometry; both its
   assertions have been seen to fire.
3. The mast and bollards are one-mesh-per-shape, placed by one node each, and
   the box-vs-round change on the boarding walk is **measured and recorded**.
4. `Mirror` and `MirrorFrame` are untouched.
5. All colliders transcribe their primitives exactly.
6. `cargo clippy` and `cargo test --workspace` pass; nothing is blessed.
7. `assets/games/deeper_demo.loom` is untouched. Integration is Phase 4.

## What Phase 2 deliberately does not do

- No change to `deeper_demo.loom`. The demo keeps its primitives and stays
  runnable; swapping them is Phase 4's whole job.
- No rebuild of `Mirror` or `MirrorFrame`.
- No props — crates, barrel, bench, lantern, spool, thermos, fish crate are
  Phase 3, along with cordage and the two rotated ramps.
- No re-pin of `green.sh`'s exact-equality asserts. Those move when the rotated
  ramp nodes change, which is Phase 3.
