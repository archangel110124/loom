# Rig Shed Rebuild — Phase 2b Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the rig's shed from a photograph of a real one, so it stops
reading as a striped box and starts reading as a building.

**Architecture:** Phase 2 built the shed as vertical board-and-batten on a flat
slab. A reference photograph says that is the wrong building. This replaces the
wall construction with **horizontal lapped weatherboard** — wedge-section boards
whose proud bottom edge throws the shadow line that defines the idiom — adds
corner boards, stands the shed on a plinth, pitches the roof and gives it an
eave and a fascia, and repaints it. Every collider stays exactly where it is.

**Tech Stack:** Blender 5.2.0 LTS headless (`bpy`, `bmesh`), Python 3.14 + numpy
2.5.2, Loom CLI.

**Spec:** `docs/design/RIG-TIMBER-REBUILD.md` — §1 Cases 1 and 6 bind every node
here. §5's frozen numbers bind the shed and the roof.

**Reference:** `/mnt/data/loom-migration/sources/refs/shed/` — kept off the drive
git lives on, because the photographs are CC BY-SA 2.0 and this repo is
MIT/Apache. `ATTRIBUTION.md` there names every photographer. The primary is
`st_abbs_fishermans_shed.jpg` (Mark Hope). **Look at it before you build.**

## What the photograph says, and what we built instead

| the reference | Phase 2's shed |
| --- | --- |
| horizontal **lapped** boards, each lapping the one below | vertical board-and-batten |
| the lap stands proud; its bottom edge throws a hard shadow line | flush wall, no shadow line |
| board ends step outward — a sawtooth silhouette at any grazing angle | flat mitred edge |
| a **corner board** at each vertical, boards dying into it | bare corner |
| **pitched** roof, eave overhang, fascia along it | flat slab, no overhang |
| stands on a **plinth** of bearers, daylight underneath | flush on the deck |
| **painted**, weathered, paint worn back to timber in places | bare timber |

The first three are silhouette. No palette change reaches them, which is why
Phase 2's texture work could not fix this.

## Global Constraints

- **Generators live in the repo** at `tools/mesh/rig/`. Outputs to
  `assets/meshes/` and `assets/textures/`.
- **The shed's collider does not move.** World `x -8.500…0.500`,
  `y 1.400…3.800`, `z 2.600…6.600`. `green.sh:1903` strafes a character into it
  and asserts `state.stuck == 1`.
- **NOTHING MAY PROJECT NORTH OF WORLD `z = 2.600`.** That face is the mirror's
  mount: `MirrorFrame` occupies `z 2.570…2.600`, `Mirror` sits in front at
  `2.550…2.570`. A proud lap solves the same way the battens did — the proud
  edge lands *on* the plane and everything else recedes behind it.
- **The roof stays inside its own y envelope**, `y 3.800…4.100` — chosen
  deliberately over a steeper drawn roof, to keep the seen-≤-walked rule this
  project has held for three phases. **The pitch that actually fits is 2.27°,
  not the 3.6° first quoted.** 3.6° is what a sheet of ZERO thickness with no
  fascia would get by spending the whole 0.3 m envelope on fall; a solid sheet
  and a fascia hanging below its low edge take their share first. The budget:
  `AMP 0.045 + FALL 0.190 + FASCIA_H 0.060 = 0.295`, inside 0.300 by 5 mm. Anyone
  re-deriving the pitch from the envelope alone will get 3.6° again and be
  wrong.
- **Meshes are authored CENTRED on their own local origin, in all three axes.**
  `play.rs:520` centres a static collider on the NODE.
- **Export via `rigkit.export_obj`**; `verify_obj` enforces per-shell outward
  normals. Inward normals render **pure black**.
- **Palette in LINEAR reflectance, written as sRGB bytes** via `_srgb_encode`.
  Normal maps are data — never sRGB-encode them.
- **This zone ships at 1024²**, not `TEX_SIZE`'s 2048 (spec §3).
- **Reference photographs are reference only** — never sampled into a texture,
  never committed. Every texture stays a pure function of its parameters.
- No new dependencies: numpy, struct, zlib only. Byte-deterministic builds.
- Never `git add -A`. `crates/loom_cli/src/run.rs` and `scripts/green.sh` are the
  user's uncommitted work — never staged, moved, stashed or reverted.

## A standing warning, earned across two phases

Seven assertions have shipped in this project that could not fail, and every one
was found by somebody running it rather than reading it. When you fault-inject,
**check which assertion actually fired** — twice a fault tripped a more generic
check first, and twice a fault was self-consistent with the check it was meant
to break. Reporting "it did not fire" is a result, not a nuisance.

Also: **`blender --background --python` exits 0 on an unhandled exception.**
Every assertion here is stdout-only. Read the output; do not trust `$?`.

---

### Task 1: `weathered_paint`

Painted timber that has been in salt air: the paint holds in the sheltered
grain and has worn back to bare wood on every exposed edge, with white salt
bloom over both.

**Files:**
- Modify: `tools/mesh/rig/textures.py` (add `weathered_paint`)
- Modify: `tools/mesh/rig/test_textures.py` (append two checks, call both)

**Interfaces:**
- Consumes: `_fbm`, `_srgb_encode`, `normal_from_height`, `TEX_SIZE`.
- Produces: `weathered_paint(size=1024, seed=29, paint=(0.022, 0.055, 0.125)) -> (albedo_uint8, height_float)`.
  The `paint` argument is the knob — Task 4 may retune it without touching the
  generator.

**On the colour.** Sampled from the reference's sunlit wall, the paint reads
linear `[0.036, 0.108, 0.311]` — blue at 8.6× red. **That sample has the sun in
it**, so it is a hue ratio, not an albedo; taking it literally would bake
daylight into the surface. The default below keeps the ratio and drops the level
to something a painted board actually reflects.

- [ ] **Step 1: Write the failing tests**

Append to `test_textures.py`, and call both at the foot of the file:

```python
def test_paint_is_blue_and_worn_through():
    """The reference shed is painted blue and the paint has worn back to bare
    timber on the exposed edges. Both states must be present: all-paint reads as
    a plastic box, all-bare reads as the shed we are replacing."""
    alb, _ = textures.weathered_paint(size=256, seed=29)
    lin = np.where(alb / 255.0 <= 0.04045, (alb / 255.0) / 12.92,
                   (((alb / 255.0) + 0.055) / 1.055) ** 2.4)
    flat = lin.reshape(-1, 3)
    m = flat.mean(axis=0)
    assert m[2] > m[0] * 2.0, \
        "not blue enough: linear mean %s, B/R %.2f" % (m.round(4), m[2] / m[0])
    # Bare timber is warm; paint is cold. If the warm pixels have vanished the
    # paint never wore through, and if the cold ones have, it never was painted.
    warm = (flat[:, 0] > flat[:, 2]).mean()
    assert 0.04 < warm < 0.40, \
        "worn fraction %.3f — want some bare timber showing, not none and not most" % warm
    print("  paint ok  linear mean=%s  B/R %.2f  worn %.3f"
          % (m.round(4), m[2] / m[0], warm))


def test_paint_tiles_and_is_deterministic():
    a1, _ = textures.weathered_paint(size=128, seed=29)
    a2, _ = textures.weathered_paint(size=128, seed=29)
    assert hashlib.sha256(a1.tobytes()).digest() == hashlib.sha256(a2.tobytes()).digest(), \
        "same seed gave different bytes"
    chan = a1[:, :, 2].astype(int)          # blue channel: the paint's own
    ix = np.abs(np.diff(chan, axis=1)).mean(); iy = np.abs(np.diff(chan, axis=0)).mean()
    sx = np.abs(chan[:, 0] - chan[:, -1]).mean(); sy = np.abs(chan[0, :] - chan[-1, :]).mean()
    assert sx <= ix * 2.0 + 1.0, "vertical seam %.2f vs %.2f" % (sx, ix)
    assert sy <= iy * 2.0 + 1.0, "horizontal seam %.2f vs %.2f" % (sy, iy)
    print("  paint tiling ok  seam %.2f/%.2f %.2f/%.2f" % (sx, ix, sy, iy))
```

- [ ] **Step 2: Run to verify they fail**

Run: `python3 tools/mesh/rig/test_textures.py`
Expected: `AttributeError: module 'textures' has no attribute 'weathered_paint'`.

- [ ] **Step 3: Write `weathered_paint`**

```python
def weathered_paint(size=1024, seed=29, paint=(0.022, 0.055, 0.125)):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    Painted timber, salt-worn. Three states: paint that has held, bare grey-warm
    timber where it has worn through, and white salt bloom drifted over both.

    **`paint` is a knob, not a constant.** Sampled off the reference's sunlit
    wall the colour reads linear [0.036, 0.108, 0.311] — blue at 8.6x red — but
    that sample has the SUN in it. What transfers is the hue ratio; the level
    does not. The default keeps the ratio and drops the level to what a painted
    board actually reflects. Retune it from the scene, not from the photograph.

    Grain runs horizontally here because the boards are laid horizontally — the
    opposite of `weathered_boards`, which was written for vertical siding.

    LINEAR reflectance in, sRGB bytes out. See `_srgb_encode`.
    """
    rng = np.random.default_rng(seed)

    # Grain along the board, which is +U for horizontal siding.
    grain = (_fbm(size, rng, octaves=6, base=32, stretch=8) * 0.70
             + _fbm(size, rng, octaves=4, base=4, stretch=4) * 0.30)
    grain = (grain - grain.min()) / (grain.max() - grain.min())

    # Where the paint has gone. Wear follows the weather, so it is broad and
    # isotropic rather than following the grain.
    wear_f = _fbm(size, rng, octaves=5, base=5)
    wear_f = (wear_f - wear_f.min()) / (wear_f.max() - wear_f.min())
    # Grain sits proud of the softer wood between it, so the grain wears first.
    worn = np.clip((wear_f * 0.75 + grain * 0.25 - 0.60) / 0.16, 0.0, 1.0)

    # Salt dries white in the sheltered parts, over paint and bare wood alike.
    salt = _fbm(size, rng, octaves=4, base=9)
    salt = np.clip((salt - salt.min()) / (salt.max() - salt.min()) * 1.2 - 0.62, 0.0, 1.0)

    height = np.clip(0.35 + 0.35 * grain - 0.20 * worn, 0.0, 1.0).astype(np.float32)

    g = grain[:, :, None]
    p = np.array(paint)
    painted = p + g * (p * 0.55)
    # Bare timber under it: warm, mid-grey, the same family as the deck.
    bare = np.array([0.085, 0.068, 0.050]) + g * np.array([0.070, 0.056, 0.040])
    bloom = np.array([0.210, 0.212, 0.208]) + g * np.array([0.060, 0.060, 0.058])

    w = worn[:, :, None]
    s = salt[:, :, None]
    rgb = painted * (1.0 - w) + bare * w
    rgb = rgb * (1.0 - s) + bloom * s
    return (_srgb_encode(np.clip(rgb, 0.0, 1.0)) * 255.0 + 0.5).astype(np.uint8), height
```

- [ ] **Step 4: Run the tests, then look**

Run: `python3 tools/mesh/rig/test_textures.py` — all pass.

Then render a sheet and **open it**:

```bash
cd ~/loom && python3 - <<'PY'
import sys; sys.path.insert(0, "tools/mesh/rig")
import numpy as np, textures
alb, h = textures.weathered_paint(size=512, seed=29)
tile = np.concatenate([np.concatenate([alb, alb], 1)] * 2, 0)[::2, ::2]
textures.write_png("/tmp/paint_sheet.png",
                   np.concatenate([alb, textures.normal_from_height(h), tile], axis=1))
PY
```

Blue paint with bare warm timber showing through where it has worn, and a
drift of white bloom over both. **Open
`/mnt/data/loom-migration/sources/refs/shed/st_abbs_fishermans_shed.jpg` beside
it** and say honestly whether they belong to the same building.

- [ ] **Step 5: Prove the worn-fraction band can fail, both ways**

The band is `0.04 < warn < 0.40` and it has two failure modes, so inject twice:
- Set the wear threshold to `- 0.95` (paint never wears through). Expect the
  **lower** bound to fire.
- Set it to `- 0.20` (paint almost entirely gone). Expect the **upper** bound.

Restore. Paste both, and **name which bound fired each time** — a band that only
ever fails on one side is half a band.

- [ ] **Step 6: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/textures.py tools/mesh/rig/test_textures.py
git commit -m "feat(rig): paint that has worn back to the timber under it"
```

---

### Task 2: The shed, rebuilt as lapped weatherboard

**Files:**
- Rewrite: `tools/mesh/rig/build_shed.py` (the `wall()` construction; keep the
  file's header, its constants block and its self-check shape)
- Regenerate: `assets/meshes/rig_shed.obj`,
  `assets/textures/rig_shed_albedo.png`, `..._normal.png`

**Interfaces:**
- Consumes: `rigkit.export_obj`, `rigkit.verify_obj`, `rigkit.read_obj`,
  `textures.weathered_paint`, `textures.normal_from_height`, `textures.write_png`.
- Produces: `assets/meshes/rig_shed.obj` centred on
  `SHED_CENTRE = (-4.0, 2.60, 4.60)`, spanning local `x ±4.500`, `y ±1.200`,
  `z ±2.000`. Prints the node position and `BoxCollider` half-extents unchanged.

**The section that defines the idiom.** A weatherboard is nailed with its top
edge against the sheathing and its bottom edge standing proud, because it laps
over the board below. In section the outer surface is a **sawtooth**: it ramps
outward from the top of each board to its bottom, then steps back in at the next
board's top. That step is the shadow line, and it is the whole look.

**Which way the sawtooth points is the thing that must not be got wrong.** The
proud edge is the BOTTOM one, and on the north wall proud means toward the
mirror. So the proud bottom edge lands exactly on the wall plane and the top
edge recedes `BOARD_T` behind it. Everything is behind the plane; nothing is in
front. The self-check asserts it and Step 5 proves the assertion fires.

- [ ] **Step 1: Replace the constants block**

```python
# --- the wall, as the reference builds it ---------------------------------
# Horizontal lapped weatherboard. Each board's TOP edge sits against the
# sheathing and its BOTTOM edge stands BOARD_T proud, lapping the board below.
# In section the outer surface is a sawtooth, and the step at each board's
# bottom is the shadow line the whole idiom is made of.
#
# The proud edge is the bottom one, and on the north wall proud points at the
# mirror. So the proud edge lands ON the wall plane and the rest recedes behind
# it -- the same solve the battens used, for the same reason.
BOARD_H = 0.180          # board height, top edge to bottom edge
LAP = 0.045              # how far each board laps over the one below
BOARD_T = 0.022          # board thickness = how far the bottom edge stands proud
CORNER_W = 0.090         # corner board covering each vertical joint
PLINTH_H = 0.150         # bearers the shed stands on
PLINTH_W = 0.180         # bearer width
PLINTH_N = 5             # bearers along the length, daylight between them
DOOR_W, DOOR_H = 0.90, 1.95
SEED = 29
ZONE_SIZE = 1024         # spec §3: this zone is 1024², not TEX_SIZE's 2048
```

Keep `SHED_CENTRE`, `HALF_X`, `HALF_Y`, `HALF_Z` and `NORTH_FACE_WORLD` exactly
as they are. Delete `BOARD_W`, `BATTEN_W` and `RECESS` — the vertical
construction is gone.

- [ ] **Step 2: Write the board emitter**

Replace `wall()` with this. `slab()` stays as it is — a board is not a slab
because its outer face is sloped, so it gets its own emitter:

```python
def board(axis, plane, outward, a, b, y0, y1, uv_u0):
    """One lapped weatherboard, in LOCAL metres.

    `axis` is the wall's fixed axis ("x" or "z"), `plane` its coordinate, and
    `outward` the sign pointing out of the shed. The board's BOTTOM edge sits at
    `plane` and its top recedes BOARD_T inward, which is what makes the section
    a sawtooth and throws the shadow line.
    """
    out = plane                       # the proud bottom edge -- ON the plane
    inn = plane - outward * BOARD_T   # the recessed top edge
    back = plane - outward * (BOARD_T + 0.030)   # the sheathing behind it

    def P(u, depth, y):
        """A vertex on this wall: `u` along the run, `depth` across it, `y` up.

        For a z-wall the run is x and the depth is z; for an x-wall they swap.
        Getting this pair backwards builds the wall lying down, and it still
        passes a bounds check, so do not paraphrase it.
        """
        if axis == "z":
            return bm.verts.new((u, y, depth))
        return bm.verts.new((depth, y, u))

    # Outer face: proud at the bottom, recessed at the top.
    o0b, o1b = P(a, out, y0), P(b, out, y0)
    o0t, o1t = P(a, inn, y1), P(b, inn, y1)
    # Inner face, flat against the sheathing.
    i0b, i1b = P(a, back, y0), P(b, back, y0)
    i0t, i1t = P(a, back, y1), P(b, back, y1)

    faces = [
        bm.faces.new((o0b, o1b, o1t, o0t)),   # the visible sloped face
        bm.faces.new((i0t, i1t, i1b, i0b)),   # back
        bm.faces.new((o0b, o0t, i0t, i0b)),   # end a
        bm.faces.new((o1t, o1b, i1b, i1t)),   # end b
        bm.faces.new((o0t, o1t, i1t, i0t)),   # top
        bm.faces.new((o1b, o0b, i0b, i1b)),   # bottom -- this is the shadow line
    ]
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            u = x if axis == "z" else z          # the run axis, matching P()
            # One tile per 2 m along the run and per 1 m up. The grain runs
            # along the board, so U is the along-grain axis and gets the longer
            # span -- matching `weathered_paint`, which is built for horizontal
            # siding. Getting this the other way round makes the boards look
            # like they are standing on end.
            loop[uv_layer].uv = (uv_u0 + u / 2.0, z / 1.0)
    return faces
```

- [ ] **Step 3: Write the wall loop, the corner boards and the plinth**

```python
# Walls, as (fixed axis, plane, from, to, outward sign). Local coordinates.
WALLS = [
    ("z", -HALF_Z, -HALF_X, HALF_X, -1),   # north -- THE MIRROR FACE
    ("z",  HALF_Z, -HALF_X, HALF_X, +1),   # south
    ("x", -HALF_X, -HALF_Z, HALF_Z, -1),   # west
    ("x",  HALF_X, -HALF_Z, HALF_Z, +1),   # east -- carries the door
]

rng_state = SEED
def rand():
    global rng_state
    rng_state = (rng_state * 1103515245 + 12345) & 0x7FFFFFFF
    return rng_state / 0x7FFFFFFF


# The shed stands on bearers, so the boarding starts above them.
SILL = -HALF_Y + PLINTH_H
step = BOARD_H - LAP                       # exposed height of each course
rows = int((2 * HALF_Y - PLINTH_H) / step)

for axis, plane, a, b, outward in WALLS:
    door = (axis == "x" and outward > 0)
    # Corner boards stop the lapped ends short, exactly as the reference does.
    a2, b2 = a + CORNER_W, b - CORNER_W
    for r in range(rows):
        y0 = SILL + r * step
        y1 = min(y0 + BOARD_H, HALF_Y)
        if y0 >= HALF_Y:
            break
        if door and y0 < SILL + DOOR_H:
            # The door is a gap in the boarding, centred on the run.
            board(axis, plane, outward, a2, -DOOR_W * 0.5, y0, y1, rand())
            board(axis, plane, outward, DOOR_W * 0.5, b2, y0, y1, rand())
        else:
            board(axis, plane, outward, a2, b2, y0, y1, rand())

# Corner boards: one plain vertical plank at each of the four verticals, sitting
# at the wall plane so the lapped ends die into it.
for cx in (-HALF_X, HALF_X):
    for cz in (-HALF_Z, HALF_Z):
        sx = 1 if cx > 0 else -1
        sz = 1 if cz > 0 else -1
        slab(cx - sx * CORNER_W, cx, SILL, HALF_Y,
             cz - sz * CORNER_W, cz, rand(), "z")

# The plinth: bearers running across the shed, with daylight between them.
bp = (2 * HALF_X - PLINTH_W) / (PLINTH_N - 1)
for i in range(PLINTH_N):
    px = -HALF_X + i * bp
    slab(px, px + PLINTH_W, -HALF_Y, SILL, -HALF_Z, HALF_Z, rand(), "z")
```

Delete the old flat roof-deck slab: the roof is a separate mesh and Task 3 gives
it an overhang, so the shed no longer needs a lid of its own. **Keep one** — a
thin cap at `y HALF_Y - 0.04 … HALF_Y` spanning the full plan — or the shed is
open from above and `verify_obj`'s per-shell check will refuse the open shells.

- [ ] **Step 4: Rewrite the self-check**

```python
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

# **THE ONE THAT MATTERS, and it runs FIRST.** Local -HALF_Z is world 2.600, the
# mirror's mount. It is checked before the generic centring loop because a
# symmetric fault trips centring on x first and this would never run -- that
# happened in Phase 2 and the assertion went unexercised.
assert lo[2] >= -HALF_Z - 1e-4, \
    "a vertex reaches local z=%.5f (world %.5f), north of the shed's face at " \
    "world %.3f — MirrorFrame occupies 2.570..2.600 and this punches through it" \
    % (lo[2], lo[2] + SHED_CENTRE[2], NORTH_FACE_WORLD)

for ax, name, half in ((0, "x", HALF_X), (1, "y", HALF_Y), (2, "z", HALF_Z)):
    assert abs(lo[ax] + half) < 1e-3 and abs(hi[ax] - half) < 1e-3, \
        "shed %s spans %.4f..%.4f, want %.3f..%.3f — not centred on its own " \
        "origin, so its collider cannot be made to match" \
        % (name, lo[ax], hi[ax], -half, half)

# **The sawtooth points the right way.** Every board's proud edge is its BOTTOM
# one. Read the shipped mesh and confirm: on the north wall, the vertices AT the
# plane must be the low ones of their board, and the recessed vertices above
# them. If the section were inverted the wall would still be inside the plane
# and would still be centred -- both checks above would pass -- and the shadow
# would fall the wrong way, which is the one thing a photograph would show and
# no number here would.
d = rigkit.read_obj(OBJ_PATH)
# Sample the BOARDS only. The corner boards and the plinth also sit on the
# north plane -- corners spanning the full wall height, bearers below the sill --
# and either one dragged into this mean would swamp the boards' own signal and
# make the test read whatever those happen to average. Exclude both by extent.
def _board_vert(v):
    return (-HALF_X + CORNER_W + 1e-4 < v[0] < HALF_X - CORNER_W - 1e-4
            and v[1] > SILL + 1e-4)

at_plane = [v for v in d["verts"] if abs(v[2] + HALF_Z) < 1e-6 and _board_vert(v)]
assert at_plane, "no board vertex sits on the north plane at all"
recessed = [v for v in d["verts"]
            if -HALF_Z + BOARD_T * 0.5 < v[2] < -HALF_Z + BOARD_T * 1.5 and _board_vert(v)]
assert recessed, "nothing sits one board-thickness behind the plane"
mean_proud = sum(v[1] for v in at_plane) / len(at_plane)
mean_back = sum(v[1] for v in recessed) / len(recessed)
assert mean_proud < mean_back, \
    "the sawtooth is inverted: the proud edge averages y=%.4f and the recessed " \
    "edge y=%.4f, so the boards lap upward and every shadow falls the wrong way" \
    % (mean_proud, mean_back)

assert info["tris"] <= 6000, "over budget: %d tris" % info["tris"]
print("shed: node pos=(%.2f, %.2f, %.2f) half_extents=(%.3f, %.3f, %.3f)"
      % (SHED_CENTRE + (HALF_X, HALF_Y, HALF_Z)))
print("shed: %d courses of %.3f m, lap %.3f, proud %.3f; %d bearers"
      % (rows, BOARD_H, LAP, BOARD_T, PLINTH_N))
print("shed: %d tris, %d verts, local x %.3f..%.3f y %.3f..%.3f z %.3f..%.3f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("shed: self-check passes")
```

Point the texture call at the new generator:

```python
albedo, height = textures.weathered_paint(size=ZONE_SIZE, seed=SEED)
```

- [ ] **Step 5: Build, and prove BOTH new assertions fire — separately**

```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/build_shed.py 2>&1 | grep -E '^shed:|Error|assert'
```
Expected: three `shed:` lines then `shed: self-check passes`, with bounds
`x -4.500..4.500 y -1.200..1.200 z -2.000..2.000`.

Then two injections, each restored:
- **North face**: in `WALLS`, flip only the NORTH row's `outward` sign. That
  makes that wall alone proud toward the mirror. Expected: the north-face
  message, and **not** a centring message. If a centring message fires instead,
  the fault was not isolated — say so.
- **Inverted sawtooth**: in `board()`, swap `out` and `inn`. The wall stays
  inside the plane and stays centred, so the first two checks pass. Expected:
  **`the sawtooth is inverted`**.

The second is the important one. It is the only check here that would catch a
mistake a photograph shows and no bounding box does.

- [ ] **Step 6: Look at it**

```bash
cd ~/loom
sed 's|\.\./meshes/|'"$PWD"'/assets/meshes/|; s|\.\./textures/|'"$PWD"'/assets/textures/|' \
  assets/test/rig_shed.loom > /tmp/shed2b.loom
./target/release/loom render /tmp/shed2b.loom --out /tmp/shed2b.png --size 1100x690
```
**Open it, and open the reference beside it.** The questions are specific: does
the wall band horizontally with a hard shadow under each board? Do the corners
read as boards rather than mitres? Is there daylight under the shed? Report what
you actually see, and if the shadow lines are not there say so — that is the
whole point of the task and a build that passes its assertions can still miss it.

- [ ] **Step 7: Determinism, then commit**

```bash
cd ~/loom
md5sum assets/meshes/rig_shed.obj
blender --background --factory-startup --python tools/mesh/rig/build_shed.py >/dev/null 2>&1
md5sum assets/meshes/rig_shed.obj
git add tools/mesh/rig/build_shed.py assets/meshes/rig_shed.obj \
        assets/textures/rig_shed_albedo.png assets/textures/rig_shed_normal.png
git commit -m "feat(rig): the shed is lapped weatherboard on a plinth, from the photograph"
```

---

### Task 3: The roof gets a pitch, an eave and a fascia

**Files:**
- Modify: `tools/mesh/rig/build_roof.py`
- Regenerate: `assets/meshes/rig_shed_roof.obj`

**Interfaces:**
- Consumes: everything `build_roof.py` already consumes.
- Produces: the same OBJ, centred on `ROOF_CENTRE = (-4.0, 3.95, 4.60)`, local
  `x ±4.900`, `y ±0.150`, `z ±2.400`, still under 2000 tris.

The roof is already corrugated with three holes. This tilts it, and adds the two
details that make an eave read: the sheet oversails the wall, and a fascia board
closes the end of the corrugations.

**The pitch is 2.27° and that is deliberate.** The roof's y envelope is 0.3 m
and the fall gets 0.190 of it, the rest going to the sheet's thickness and the
fascia. A steeper drawn roof would have to rise above the collider, and the
seen-≤-walked rule has held for three phases; it is not being broken for a few
degrees. **Do not "correct" this to 3.6° by re-deriving it from the envelope** —
that figure ignores the sheet's own thickness and lands the fascia below the
envelope's floor. The eave overhang and the fascia line do most of the work that
pitch would have done.

- [ ] **Step 1: Add the constants**

```python
# The sheet falls from the back of the shed to the front, so water sheds over
# the north eave rather than sitting on the ridge. 0.290 of fall across 4.8 m of
# depth is 3.46 deg -- shallow, and every millimetre of it is inside the
# primitive's own 0.3 m envelope. See the plan for why it is not steeper.
# The whole 0.3 m envelope, spent three ways -- and it IS the whole envelope,
# so none of these three can grow without another shrinking:
#     AMP 0.045 + FALL 0.190 + FASCIA_H 0.060 = 0.295, inside 0.300 by 5 mm
# The corrugation amplitude takes its cut FIRST -- forgetting it is what put
# the fascia 5 mm through the envelope floor on the first pass.
# 0.190 of fall across 4.8 m of depth is 2.27 deg. Shallow, and honestly so:
# the 3.6 deg this plan first quoted is what a zero-thickness sheet with no
# fascia would get. The eave overhang -- which the roof already has, its local
# z being +/-2.400 against the shed's +/-2.000 -- does more for the read than
# the last degree of pitch would.
FALL = 0.190
FASCIA_H = 0.060         # the board closing the corrugation ends at the eave
FASCIA_T = 0.020
```

- [ ] **Step 2: Tilt the sheet**

In the cell loop, the top surface's y currently comes from `prof(x)` alone. Add
the fall, which depends on z:

```python
        # Height of the sheet at this z: full at the back, FALL lower at the
        # front. `zt` is 0 at the back edge and 1 at the front.
        zt0 = (z0 + HALF_Z) / (2 * HALF_Z)
        zt1 = (z1 + HALF_Z) / (2 * HALF_Z)
        drop0, drop1 = FALL * (1.0 - zt0), FALL * (1.0 - zt1)
        yt00 = HALF_Y - AMP + prof(x0) - drop0
        yt10 = HALF_Y - AMP + prof(x1) - drop0
        yt01 = HALF_Y - AMP + prof(x0) - drop1
        yt11 = HALF_Y - AMP + prof(x1) - drop1
```

and build the top quad from the four distinct heights (`a` at `(x0,z0,yt00)`,
`b` at `(x1,z0,yt10)`, `c` at `(x1,z1,yt11)`, `d` at `(x0,z1,yt01)`) instead of
the two it uses today. The underside stays flat at `-HALF_Y`.

- [ ] **Step 3: Add the fascia**

After the cell loop, a plain board across the front eave:

```python
# The fascia: a board across the low edge, closing the corrugation ends. Without
# it the sheet reads as a floating plane, because you see straight into the
# flutes from below and there is nothing to cast a line along the eave.
fy = HALF_Y - AMP - FALL         # = -0.085; fascia bottom -0.145 vs floor -0.150
for i in range(periods):
    x0 = -HALF_X + i * step
    slab_roof(x0, x0 + step, fy - FASCIA_H, fy + AMP,
              HALF_Z - FASCIA_T, HALF_Z)
```

`slab_roof` is a plain closed box, defined alongside `prof()`:

```python
def slab_roof(x0, x1, y0, y1, z0, z1):
    """A closed box in LOCAL metres, UV'd to match the sheet."""
    v = [bm.verts.new(c) for c in (
        (x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0),
        (x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1))]
    for a, b, c, d_ in ((0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
                        (2, 3, 7, 6), (1, 2, 6, 5), (0, 4, 7, 3)):
        f = bm.faces.new((v[a], v[b], v[c], v[d_]))
        for loop in f.loops:
            x, y, _z = loop.vert.co
            loop[uv_layer].uv = (x / 2.0, -y / 2.0)
```

- [ ] **Step 4: Extend the self-check**

Keep every existing assertion. Add two:

```python
# The sheet must actually FALL. A roof that is level is the slab we replaced,
# and the bounds check cannot tell the difference -- a level sheet and a pitched
# one occupy the same envelope.
d = rigkit.read_obj(OBJ_PATH)
back = [v[1] for v in d["verts"] if v[2] < -HALF_Z + 0.05 and v[1] > 0.0]
front = [v[1] for v in d["verts"] if v[2] > HALF_Z - 0.05 - FASCIA_T and v[1] > -HALF_Y + 0.01]
assert back and front, "could not find the sheet's back and front edges"
fall = sum(back) / len(back) - sum(front) / len(front)
assert fall > FALL * 0.5, \
    "the sheet falls %.4f m from back to front, want about %.3f — it is level " \
    "and reads as the flat slab this replaced" % (fall, FALL)

# The fascia must be there. Without it the eave has no line.
assert any(v[2] > HALF_Z - FASCIA_T - 1e-6 and v[1] < fy_check for v in d["verts"]), \
    "no geometry below the sheet at the front edge — the fascia is missing"
print("roof: falls %.3f m back to front (%.2f deg), fascia %.3f m"
      % (fall, math.degrees(math.atan2(FALL, 2 * HALF_Z)), FASCIA_H))
```

Define `fy_check = HALF_Y - AMP - FALL` immediately before that last
assertion — it is the sheet's height at the low edge, so anything below it at
the front face is the fascia and nothing else.

- [ ] **Step 5: Build, and prove the fall assertion fires**

Run the build; expect a `roof: falls …` line and `roof: self-check passes`,
with the tri count still under 2000 and the y bounds still inside `±0.150`.

Then set `FALL = 0.0` and re-run. Expected: **`it is level and reads as the flat
slab this replaced`**. Restore `FALL = 0.290`. Note whether the y-envelope
assertion fires instead — if it does, the fault was not isolated.

- [ ] **Step 6: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/build_roof.py assets/meshes/rig_shed_roof.obj
git commit -m "feat(rig): the roof falls to the front and its eave has a line"
```

---

### Task 4: Repaint the scene, look, and judge

**Files:**
- Modify: `assets/test/rig_structure.loom`, `assets/test/rig_shed.loom`
  (the `Shed` node's `Material` only)

**Interfaces:**
- Consumes: everything Tasks 1-3 produced. Adds no assets — the shed's two
  texture files are regenerated in place and keep their existing `[[asset]]`
  entries and UUIDs.

- [ ] **Step 1: Retune the shed's material for paint**

The `Shed` node currently carries `roughness = 0.88, metallic = 0.0` from the
bare-timber build. Paint is smoother than weathered board. In BOTH scenes:

```toml
  [node.components.Material]
  albedo = [1.0, 1.0, 1.0]
  albedo_map = { asset = "shed_albedo" }
  normal_map = { asset = "shed_normal" }
  roughness = 0.72
  metallic = 0.0
  uv_scale = [1.0, 1.0]
```

Leave the asset keys, the UUIDs, the transform and the `BoxCollider` untouched.
Update each scene's header where it describes the shed as bare timber.

- [ ] **Step 2: Validate**

```bash
cd ~/loom
./target/release/loom validate assets/test/rig_structure.loom
./target/release/loom validate assets/test/rig_shed.loom
```
Both must report an EMPTY `assets` array. A non-empty one means a path broke and
the renderer is drawing stand-in unit boxes, which looks exactly like success.

- [ ] **Step 3: Render both, and judge against the photograph**

```bash
cd ~/loom
for S in rig_shed rig_structure; do
  sed 's|\.\./meshes/|'"$PWD"'/assets/meshes/|; s|\.\./textures/|'"$PWD"'/assets/textures/|' \
    assets/test/$S.loom > /tmp/$S.2b.loom
  ./target/release/loom render /tmp/$S.2b.loom --out /tmp/$S.2b.png --size 1100x690
done
```

**Open both, and open `st_abbs_fishermans_shed.jpg` beside them.** Answer these,
honestly, one line each:
- Does the wall band horizontally, with a hard shadow under each board?
- Do the corners read as boards, or as mitres?
- Is there daylight under the shed?
- Does the roof read as pitched, and does the eave have a line under it?
- Is the paint colour right, or does it need retuning? It is a knob —
  `weathered_paint`'s `paint=` argument — and this render is what decides it.

The last one is the human's call, not yours. Say what you would change and why,
and leave it.

- [ ] **Step 4: Confirm the gate rows still measure what they claim**

The `rig_shed` GOLDEN row has no blessed reference, so nothing breaks — but the
fault injections recorded for it must still hold. Take a no-op control with
absolute paths FIRST and confirm 0 differing pixels, then re-run the `Shed`
deletion fault and record the new fraction. It will differ from Phase 2's
0.0959 because the shed is now a different shape; that is expected, and the
number belongs in the commit message.

- [ ] **Step 5: Run the gates a builder may run**

```bash
cd ~/loom
cargo clippy --workspace --all-targets -j 3 -- -D warnings
cargo test --workspace -j 3
```
Build with **-j 3**, not -j 6 — memory pressure, not cores.

**Do not run `cargo xtask`** (cross-worktree singleton lock) and **never
`--bless`**. Five GOLDEN rows already lack references; blessing now would freeze
whatever this render happens to be.

- [ ] **Step 6: Commit**

```bash
cd ~/loom
git add assets/test/rig_structure.loom assets/test/rig_shed.loom
git commit -m "feat(rig): the shed is painted now, and the scenes say so"
```

---

## Phase 2b exit criteria

1. The wall bands horizontally with a visible shadow under each board —
   **looked at against the photograph**, not inferred from an assertion.
2. The sawtooth assertion has been **seen to fire** under an inverted section,
   and the north-face assertion under a north-only fault.
3. The roof falls, its fall assertion fires when set level, and the eave has a
   fascia.
4. Every collider is exactly where Phase 2 left it — the shed's world bounds
   still `x -8.500…0.500, y 1.400…3.800, z 2.600…6.600`.
5. Nothing projects north of world `z = 2.600`.
6. `cargo clippy` and `cargo test --workspace` pass. Nothing is blessed.
7. `assets/games/deeper_demo.loom` is untouched.

## What Phase 2b deliberately does not do

- **No windows, no plank door with a brace, no ridge capping.** The human chose
  silhouette first; those are the next slice if the silhouette lands.
- **No steeper roof.** 3.6° inside the collider, by choice.
- **No change to `deeper_demo.loom`.** Integration is still Phase 4.
- **No props.** The creels, crates and floats that make the reference read as a
  working shed are Phase 3.
- **No sampling from the photographs.** They are reference; every texture stays
  a pure function of its parameters.
