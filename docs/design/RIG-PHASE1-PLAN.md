# Rig Timber Rebuild — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the approved deck look, then build the rig's structure — piles,
bracing, the tide seam, and the bulwark — as textured meshes whose physics does
not move.

**Architecture:** Phase 0 proved the pipeline on one part. Phase 1 fixes the two
things Phase 0's review found in the deck's appearance, then repeats the proven
shape three more times: a numpy texture generator per material zone, a Blender
generator per part carrying its own assertions, an explicit `BoxCollider`
transcribing today's numbers, and a `GOLDEN` row that has been fault-injected.

**Tech Stack:** Blender 5.2.0 LTS headless (`bpy`, `bmesh`), Python 3.14 + numpy
2.5.2, Loom CLI (`validate`/`measure`/`sim`/`render`/`compare`), `cargo xtask`.

**Spec:** `docs/design/RIG-TIMBER-REBUILD.md` — read §1 (the three collider
cases) before touching any node, and §5 (frozen numbers) before moving one.

## Global Constraints

- **Generators live in the repo** at `tools/mesh/rig/`. Outputs to
  `assets/meshes/` and `assets/textures/`.
- **Export via `rigkit.export_obj`.** It already does `forward_axis='NEGATIVE_Z',
  up_axis='Y'`, the V flip, and tag stripping. Do not re-implement it.
- **`verify_obj` is the contract check** and now enforces per-shell outward
  normals. Inward normals render **pure black** — diffuse and
  `ambientVisibility` go to zero together.
- **Meshes are authored CENTRED on their local origin.** `play.rs:520` centres a
  static collider on the NODE's world position and `half_extents` is local, so a
  mesh baked at absolute height cannot be given a matching collider from any
  single node transform. Every part declares a `*_CENTRE` constant, subtracts it
  from emitted geometry, and the scene node carries it as `pos`.
- **One OBJ per material.** The engine never reads `.mtl`.
- **Every `[[asset]]` gets a real, distinct UUID `id`.** An empty id aliases every
  other empty id and the first adopted wins.
- **Palette is authored in LINEAR reflectance and written as sRGB bytes.**
  `materials.rs:194` loads `albedo_map` as `ColorSpace::Srgb`. Normal maps are
  data, not colour — never sRGB-encode them.
- **Build Rust with `-j 3`**, not `-j 6`. Memory pressure, not cores.
- **Never bless.** `cargo xtask image --bless` has no per-row filter and there is
  a 56-row re-bless already pending. Builders may not run `cargo xtask` at all —
  it holds a cross-worktree singleton lock. Use `tools/goldcheck.sh`.
- Never `git add -A`. Stage by explicit path.
- `crates/loom_cli/src/run.rs` and `scripts/green.sh` are modified in the working
  tree, belong to the user, and are never staged, moved, stashed or reverted.

## Frozen numbers this phase must not move

| number | value | why |
| --- | --- | --- |
| deck top | `y = 1.400` | every spawn and assert row |
| deck extent | `x ±12.000`, `z ±7.000` | the quay edge |
| water surface | `y = 0.000` | `WaterBody.surface_height`, `deeper_demo.loom:921` |
| piling extent | `y -1.400 … 1.000`, r 0.35 | six of them, see Task 3 |
| bulwark | rails `y 1.400…2.400`, caps `y 2.680…2.920` | the boarding gaps are cut from these |
| rail gap (boarding) | `x -6.900 … -3.100` | the demo's only taught gesture |
| rail gap (swim ladder) | `x -10.300 … -8.300` | the ladder head lands in it |
| hull clearance | nothing north of `z = -7.000` for `x ∈ [-6.6, +5.2]` | 0.15 m to a 43,776 kg dynamic body |

---

### Task 1: The deck's grain, and palette D

Phase 0's deck reads as pale driftwood with grain streaking hard along every
board. Two causes, one fix each.

**Cause A — four grain bands for sixty-six courses.** `build_deck.py` gives each
course `uv_v0 = (c % 4) * 0.25`, so courses 0, 4, 8, 12 … sample the same quarter
of the texture. Measured on a rendered deck: the repeat is plainly visible. Give
each course its own V, and each board its own U, so no two boards continue each
other's grain.

**Cause B — the palette.** The human compared four in-engine variants at standing
eye height and chose **D**: darker, wetter, rot spread wider.

Note what this task does **not** do: the UV texel aspect (2.0 m per U unit against
0.801 per V) was tested at 2.4963 and changed almost nothing visible. It is not
the lever it was thought to be. Leave `uv_scale = [1.0, 1.0]`.

**Files:**
- Modify: `tools/mesh/rig/textures.py` (the three palette arrays and three
  blend constants in `weathered_timber`)
- Modify: `tools/mesh/rig/build_deck.py` (per-course and per-board UV offsets)
- Modify: `tools/mesh/rig/test_textures.py` (the palette band moves with D)
- Regenerate: `assets/meshes/rig_deck_timber.obj`,
  `assets/textures/rig_deck_timber_albedo.png`, `..._normal.png`

**Interfaces:**
- Consumes: `rigkit.export_obj`, `rigkit.verify_obj`, `textures.weathered_timber`,
  `textures.normal_from_height`, `textures.write_png`.
- Produces: nothing new. Later tasks depend only on the palette constants being
  the ones the human approved.

- [ ] **Step 1: Land palette D in `weathered_timber`**

Replace the three palette arrays and the three blend constants. These are the
exact values from the approved variant:

```python
    g = grain[:, :, None]
    # **Palette D**, chosen by the human from four in-engine variants rendered at
    # standing eye height on 2026-08-26. Darker, wetter, rot spread wider — a
    # jetty over cold water rather than a maintained boardwalk. `silver` came
    # down hardest because it was what dominated the frame; the tide zone below
    # this deck needs somewhere darker to go, and the shipped palette left it
    # none. LINEAR reflectance; `_srgb_encode` handles the transfer on the way out.
    dry = np.array([0.070, 0.055, 0.040]) + g * np.array([0.090, 0.072, 0.052])
    silver = np.array([0.125, 0.120, 0.108]) + g * np.array([0.080, 0.077, 0.068])
    rotten = np.array([0.022, 0.026, 0.016]) + g * np.array([0.026, 0.029, 0.017])

    s = np.clip(silvering * 1.10 - 0.45, 0.0, 1.0)[:, :, None]
    r = rot[:, :, None]
```

and the rot threshold, in the same function:

```python
    rot = np.clip((rot_field - 0.46) / 0.22, 0.0, 1.0)
```

- [ ] **Step 2: Move the palette test's band to match, and run it**

`test_reads_as_dark_weathered_timber` asserts a linear mean band. Palette D is
darker than what the band was written for. Update the band and say why in the
comment — the assertion still has to be able to fail:

```python
    # Palette D, approved 2026-08-26. Calibrated to catch ONE constant going
    # wrong, which is the regression people actually make. Measured: palette D
    # 0.1009, reverting `silver` alone 0.1113, reverting all five Phase 0
    # constants 0.2316. The ceiling sits between the first two. ~5% headroom is
    # affordable because this generator is deterministic — there is no
    # run-to-run variance for a band to accommodate.
    assert (lin_mean < 0.106).all(), \
        "too pale for palette D: linear mean %s" % lin_mean.round(4)
    assert (lin_mean > 0.045).all(), \
        "too dark: linear mean %s" % lin_mean.round(4)
```

Run: `python3 tools/mesh/rig/test_textures.py`
Expected: all five checks pass, and the printed linear mean is near
`[0.098, 0.083, 0.062]`.

- [ ] **Step 3: Prove the palette assertion can still fail**

Temporarily set `silver` back to its Phase 0 value
(`[0.245, 0.240, 0.228] + g * [0.130, 0.128, 0.120]`) and re-run.
Expected: **`too pale for palette D`**. Restore palette D.

**The ceiling is 0.106 and it is calibrated against exactly this test.** An
earlier draft set it at 0.16 and prescribed the same injection — and the
injection passed, with 30% headroom, because D's `s` blend weight
(`1.10x − 0.45` against Phase 0's `1.30x − 0.30`) roughly halves how much of the
frame reaches `silver` at all. A second draft "tightened" it to 0.125, which is
still ABOVE the 0.1113 it was meant to catch and therefore caught nothing —
for `assert mean < X` to fail at 0.1113, X must be at or below it.

    palette D            0.1009   must pass
    silver-only revert   0.1113   must fail   <- the ceiling sits between these
    full Phase 0 revert  0.2316   must fail

0.106 leaves ~5% headroom, which is tight for a regression bound and affordable
because **this generator is deterministic** — same seed, same bytes, verified
across independent rebuilds. There is no run-to-run variance to accommodate, so
the only thing that can move the number is a deliberate palette edit, and a
deliberate edit tripping the band is the band working.

A band that only fails when every constant is wrong is a band that cannot catch
the regression people actually make, which is one constant at a time.

- [ ] **Step 4: Give every course and every board its own patch of texture**

In `build_deck.py`, `plank()` takes `uv_v0` and applies `x / 2.0` for U. Replace
both with per-board offsets. In `plank()`'s UV loop:

```python
    # UVs: 1 texture tile per 2 m along the board, 1 per board across. **Every
    # board gets its own patch**, not one of four. The Phase 0 code used
    # `(c % 4) * 0.25`, which gave 66 courses four distinct grain bands and made
    # the repeat plainly visible at standing height. `uv_u0` does the same job
    # along the board, so two boards meeting at a butt joint do not continue
    # each other's grain.
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            loop[uv_layer].uv = (uv_u0 + x / 2.0,
                                 uv_v0 + (-y - z0) / plank_w * 0.25)
```

with `plank`'s signature becoming
`def plank(z0, x0, x1, sink, uv_u0, uv_v0):`.

At the call site, draw both from the existing LCG so the build stays
deterministic:

```python
        sink = rand() * 0.004
        plank(z0, x0, x1, sink, rand(), rand())
        course_spans[c].append((x0, x1))
```

- [ ] **Step 5: Rebuild and check the numbers did not move**

Run:
```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/build_deck.py 2>&1 | grep -E '^deck:'
```
Expected: `deck: 7084 tris, 4048 verts, x -12.00..12.00 y -0.200..0.200 z -7.00..7.00`.
The triangle count and bounds **must not change** — this task moves UVs and
colour, not geometry. If tris moved, something else changed too; find out what.

- [ ] **Step 6: Confirm determinism and the coverage check**

```bash
cd ~/loom
md5sum assets/meshes/rig_deck_timber.obj
blender --background --factory-startup --python tools/mesh/rig/build_deck.py >/dev/null 2>&1
md5sum assets/meshes/rig_deck_timber.obj
```
Expected: identical. The coverage assertion and the `seen ≤ walked` height
assertion both run inside that build; a silent pass means both held.

- [ ] **Step 7: Render and look at it**

```bash
cd ~/loom
./target/release/loom render assets/test/rig_deck.loom \
  --out /tmp/deck_p1.png --size 620x400
```
**Open it.** The deck should be visibly darker than Phase 0's and the along-board
streaking visibly broken up. Report honestly whether the repeat is gone — if
adjacent boards still look like each other, say so; the fix did not work and
that is worth knowing before three more zones inherit the approach.

- [ ] **Step 8: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/textures.py tools/mesh/rig/build_deck.py \
        tools/mesh/rig/test_textures.py assets/meshes/rig_deck_timber.obj \
        assets/textures/rig_deck_timber_albedo.png \
        assets/textures/rig_deck_timber_normal.png
git commit -m "feat(rig): palette D, and a patch of grain per board"
```

---

### Task 2: The fifth collider case — the mesh alias picks the collider's SHAPE

Spec §1 documents three cases plus a trap, all about a collider's *position* and
*extent*. There is a fifth, about its **shape**, and Phase 1 walks straight into
it: the rig has **nine `cylinder` primitives**.

`crates/loom_cli/src/play.rs:588-591`:

```rust
let mesh = world.mesh_asset(*entity);
let ball = mesh == Some("sphere");
let round = matches!(mesh, Some("capsule" | "cylinder"));
let capped = mesh == Some("capsule");
```

The collider's shape is chosen by **string comparison on the mesh alias**. An
imported OBJ is not called `"cylinder"`, so replacing a cylinder primitive with a
mesh silently swaps a round collider for a box — a change of shape, not of size,
that no bounds check can see.

**Files:**
- Modify: `docs/design/RIG-TIMBER-REBUILD.md` §1

- [ ] **Step 1: Add the case to the spec**

After the fourth trap in §1, add:

```markdown
#### The fifth case: the mesh alias chooses the collider's SHAPE

`play.rs:588-591` picks the collider shape by **string comparison on the mesh
alias** — `"sphere"` gets a ball, `"capsule"` and `"cylinder"` get a round
collider, everything else gets a cuboid. An imported OBJ is never called
`"cylinder"`, so **replacing a cylinder primitive with a mesh silently turns a
round collider into a box.** No bounds check sees it: the extents are right and
the shape is wrong.

Nine rig nodes are `cylinder` primitives and every one of them is affected:

| node | `deeper_demo.loom` | radius / half-height |
| --- | --- | --- |
| `PilingNW` / `NC` / `NE` / `SW` / `SC` / `SE` | `:1063`–`:1125` | 0.35 / 1.20 |
| `BollardWest`, `BollardEast` | `:1363`, `:1376` | 0.22 / 0.30 |
| `Mast` | `:1505` | 0.18 / 2.20 |
| `BaitBarrel` | `:1580` | 0.45 / 0.45 |
| `LineSpool` | `:1592` | 0.40 / 0.30 |
| `Thermos` | `:1617` | 0.13 / 0.25 |

There is no `CylinderCollider` component to author — `BoxCollider` is the only
collider component that exists (`play.rs` carries a `ponytail:` note that a
`SphereCollider` is the upgrade path when something needs to differ from its
mesh). So for each of these the choice is: accept a circumscribing box and
**prove by measurement that nothing collides differently**, or leave the node a
primitive.

A box that circumscribes a cylinder is larger at the corners by
`r(√2 − 1)` = 41% of the radius — 0.145 m on a piling. That is only harmless if
nothing reaches it.
```

- [ ] **Step 2: Commit**

```bash
cd ~/loom
git add docs/design/RIG-TIMBER-REBUILD.md
git commit -m "docs(spec): the mesh alias chooses the collider's shape, not just its size"
```

---

### Task 3: `steel_rust`, and the substructure

Six piles and the bracing between them, in rusted steel. This is the first zone
that is not timber, so it needs its own texture function.

**Files:**
- Modify: `tools/mesh/rig/textures.py` (add `weathered_steel`)
- Modify: `tools/mesh/rig/test_textures.py` (add its checks)
- Create: `tools/mesh/rig/build_substructure.py`
- Create: `assets/meshes/rig_steel_frame.obj`,
  `assets/textures/rig_steel_frame_albedo.png`, `..._normal.png`

**Interfaces:**
- Consumes: `rigkit.export_obj`, `rigkit.verify_obj`, `textures._fbm`,
  `textures._srgb_encode`, `textures.normal_from_height`, `textures.write_png`.
- Produces: `textures.weathered_steel(size=2048, seed=11) -> (albedo_uint8, height_float)`;
  the OBJ above, authored centred on `PILE_CENTRE = -0.200` and spanning local
  `y -1.200 … +1.200`.

- [ ] **Step 1: Write the failing tests for `weathered_steel`**

Append to `test_textures.py`:

```python
def test_steel_is_darker_and_ruster_than_timber():
    """Rust is red-shifted and the plate under it is near-neutral. If R does not
    lead, this is grey paint, not rust."""
    alb, _ = textures.weathered_steel(size=256, seed=11)
    lin = np.where(alb / 255.0 <= 0.04045, (alb / 255.0) / 12.92,
                   (((alb / 255.0) + 0.055) / 1.055) ** 2.4)
    m = lin.reshape(-1, 3).mean(axis=0)
    assert m[0] > m[2] * 1.25, "not rust-shifted: linear mean %s" % m.round(4)
    assert (m < 0.14).all(), "too bright for wet steel: %s" % m.round(4)
    # Measured 0.0157 at the time of writing; the bound sits below it so this
    # is a regression check rather than a restatement of today's number.
    assert lin.reshape(-1, 3).std(axis=0).mean() > 0.012, \
        "too flat — the rust is not varying"
    print("  steel ok  linear mean=%s" % m.round(4))


def test_steel_tiles_and_is_deterministic():
    a1, _ = textures.weathered_steel(size=128, seed=4)
    a2, _ = textures.weathered_steel(size=128, seed=4)
    assert hashlib.sha256(a1.tobytes()).digest() == hashlib.sha256(a2.tobytes()).digest()
    chan = a1[:, :, 0].astype(int)
    ix = np.abs(np.diff(chan, axis=1)).mean(); iy = np.abs(np.diff(chan, axis=0)).mean()
    sx = np.abs(chan[:, 0] - chan[:, -1]).mean(); sy = np.abs(chan[0, :] - chan[-1, :]).mean()
    assert sx <= ix * 2.0 + 1.0, "vertical seam %.2f vs %.2f" % (sx, ix)
    assert sy <= iy * 2.0 + 1.0, "horizontal seam %.2f vs %.2f" % (sy, iy)
    print("  steel tiling ok  seam %.2f/%.2f  %.2f/%.2f" % (sx, ix, sy, iy))
```

and call both at the foot of the file.

- [ ] **Step 2: Run to verify they fail**

Run: `python3 tools/mesh/rig/test_textures.py`
Expected: `AttributeError: module 'textures' has no attribute 'weathered_steel'`.

- [ ] **Step 3: Write `weathered_steel`**

```python
def weathered_steel(size=2048, seed=11):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    Rusted steel plate. Three states, like the timber: sound paint, rust bloom,
    and the dark wet streak that runs DOWN from every bloom because water does.
    The streaking is vertical here where the timber's was horizontal — steel
    weathers along gravity, not along a grain — so `stretch` is applied to the
    other axis by transposing the field.

    LINEAR reflectance in, sRGB bytes out. See `_srgb_encode`.
    """
    rng = np.random.default_rng(seed)

    # Blooms are isotropic; the streaks below them are not. Build the streak
    # field stretched along Y by generating stretched-along-X and transposing,
    # which reuses `_fbm` rather than adding a second anisotropy path.
    bloom = _fbm(size, rng, octaves=5, base=5)
    bloom = (bloom - bloom.min()) / (bloom.max() - bloom.min())
    streak = _fbm(size, rng, octaves=5, base=24, stretch=6).T
    streak = (streak - streak.min()) / (streak.max() - streak.min())
    pit = _fbm(size, rng, octaves=6, base=40)

    rust = np.clip((bloom - 0.45) / 0.30, 0.0, 1.0)
    # A streak only exists below a bloom, so the bloom field is smeared
    # DOWNWARD and used to gate the streak field.
    #
    # **The smear must WRAP.** `np.maximum.accumulate(rust, axis=0)` is the
    # obvious way to write "anything above this has already rusted" and it does
    # not tile: it starts fresh at row 0, so the top of the image carries no
    # accumulated rust and the bottom carries all of it. Measured seam 8.72
    # against a 1.12 interior baseline — a visible band across every pile at the
    # same height. `np.roll` wraps, and a bounded smear is the better model
    # anyway: a streak fades with distance below the bloom that fed it.
    REACH = size // 6
    smear = rust.copy()
    for k in range(1, REACH):
        smear = np.maximum(smear, np.roll(rust, k, axis=0) * (1.0 - k / REACH))
    run = np.clip(smear * streak * 1.4 - 0.25, 0.0, 1.0)

    height = np.clip(0.35 + 0.45 * rust + 0.20 * pit, 0.0, 1.0).astype(np.float32)

    p = pit[:, :, None]
    plate = np.array([0.048, 0.050, 0.052]) + p * np.array([0.030, 0.031, 0.032])
    rusty = np.array([0.115, 0.052, 0.024]) + p * np.array([0.085, 0.040, 0.016])
    wet = np.array([0.030, 0.022, 0.017]) + p * np.array([0.022, 0.016, 0.012])

    r = rust[:, :, None]
    w = run[:, :, None]
    rgb = plate * (1.0 - r) + rusty * r
    rgb = rgb * (1.0 - w) + wet * w
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height
```

- [ ] **Step 4: Run the tests**

Run: `python3 tools/mesh/rig/test_textures.py`
Expected: all seven checks pass, including the two new `steel` lines.

- [ ] **Step 5: Look at it**

```bash
cd ~/loom && python3 - <<'PY'
import sys; sys.path.insert(0, "tools/mesh/rig")
import numpy as np, textures
alb, h = textures.weathered_steel(size=512, seed=11)
tile = np.concatenate([np.concatenate([alb, alb], 1)] * 2, 0)[::2, ::2]
textures.write_png("/tmp/steel_sheet.png",
                   np.concatenate([alb, textures.normal_from_height(h), tile], axis=1))
PY
```
**Open `/tmp/steel_sheet.png`.** Rust blooms with dark runs trailing downward
from them, on a near-neutral grey plate. If the runs go sideways the transpose is
wrong; if there are no runs the bloom gate is too tight; if there is a band right
across the image the wrapping smear was reverted to `accumulate`. Say which you see.

Measured when this plan was written, at size 256 seed 11 — your numbers should
land near these:

    linear mean [0.0786, 0.0616, 0.0542]   std 0.0146
    seam_x 2.14 / 1.50 baseline            seam_y 1.86 / 1.17 baseline
    R/B ratio 1.45  (the test wants > 1.25)

One honest observation to check rather than inherit: the steel's normal map came
out noticeably **softer** than the tide zone's — its height field is
`0.35 + 0.45·rust + 0.20·pit` and is mostly low-frequency. If the piles read as
smooth tubes in Task 6's render, raising the `pit` term is the knob.

- [ ] **Step 6: Write `build_substructure.py`**

```python
# tools/mesh/rig/build_substructure.py
"""The rig's substructure — six piles and the bracing between them.

Replaces the DRAWN surface of six `cylinder` primitives in
assets/games/deeper_demo.loom (`PilingNW` at :1063 through `PilingSE` at :1125),
each `pos = [x, -0.20, z]`, `scale = [0.35, 1.20, 0.35]` — a cylinder of radius
0.35 spanning world y -1.400 .. 1.000, at x in {-10.5, 0, 10.5} and z in {-5.5, 5.5}.

**These six are the fifth collider case** (spec §1): their collider shape today
is round, chosen by the string `"cylinder"` in `play.rs:588-591`. A mesh gets a
box. Step 8 of this task measures whether anything notices; if something does,
the piles stay primitives and only the bracing is built.

The bracing is new geometry with no primitive behind it and therefore no
collider to preserve — it is authored ABOVE the water and inboard of the piles
so nothing can reach it.

Run: blender --background --factory-startup --python build_substructure.py
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
OBJ_PATH = os.path.join(REPO, "assets", "meshes", "rig_steel_frame.obj")
TEX_DIR = os.path.join(REPO, "assets", "textures")

PILE_R = 0.35
PILE_TOP = 1.000          # frozen: world y, the deck's underside
PILE_BOT = -1.400         # frozen: world y
PILE_CENTRE = -0.200      # the node y this mesh hangs from = the primitive's pos.y
PILE_X = (-10.5, 0.0, 10.5)
PILE_Z = (-5.5, 5.5)
SIDES = 16                # a 0.35 m pile at 16 sides has a 6.7 mm chord error
BRACE_R = 0.075
BRACE_Y = 0.550           # world; above the 0.166 m significant wave height
SEED = 11
TEX_SIZE = 2048

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)

mesh = bpy.data.meshes.new("rig_steel_frame")
obj = bpy.data.objects.new("rig_steel_frame", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")


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

    faces = []
    for s in range(sides):
        a, b = rings[0][s], rings[0][(s + 1) % sides]
        c, d = rings[1][s], rings[1][(s + 1) % sides]
        faces.append(bm.faces.new((a, b, d, c)))
    faces.append(bm.faces.new(tuple(reversed(rings[0]))))
    faces.append(bm.faces.new(tuple(rings[1])))

    # U around the circumference (one tile per full turn), V along the length at
    # one tile per metre, so a 2.4 m pile shows 2.4 tiles and the rust's downward
    # runs stay the right way up and the right size on every part.
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            ang = math.atan2(y - (p0[1] + p1[1]) * 0.5, x - (p0[0] + p1[0]) * 0.5)
            along = ((x - p0[0]) * ax[0] + (y - p0[1]) * ax[1] + (z - p0[2]) * ax[2])
            loop[uv_layer].uv = (uv_u0 + ang / (2.0 * math.pi), along)
    return faces


# Blender is Z-up; PILE_* are LOOM y, which is Blender z. Loom z is Blender -y.
for i, px in enumerate(PILE_X):
    for j, pz in enumerate(PILE_Z):
        tube((px, -pz, PILE_BOT - PILE_CENTRE),
             (px, -pz, PILE_TOP - PILE_CENTRE),
             PILE_R, SIDES, (i * 2 + j) * 0.37)

# Cross-bracing: one horizontal run each way at BRACE_Y, pile to pile. It is
# ABOVE the water (Hs at the berth is 0.166 m) and inboard of the pile faces, so
# it adds no collider anything can reach.
bz = BRACE_Y - PILE_CENTRE
for pz in PILE_Z:
    for a, b in zip(PILE_X, PILE_X[1:]):
        tube((a, -pz, bz), (b, -pz, bz), BRACE_R, 8, 0.11)
for px in PILE_X:
    tube((px, -PILE_Z[0], bz), (px, -PILE_Z[1], bz), BRACE_R, 8, 0.29)

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()

os.makedirs(os.path.dirname(OBJ_PATH), exist_ok=True)
os.makedirs(TEX_DIR, exist_ok=True)
rigkit.export_obj(obj, OBJ_PATH, uv=True,
                  header="rig: steel frame. 6 piles r%.2f, %d sides; bracing r%.3f at world y %.3f."
                         % (PILE_R, SIDES, BRACE_R, BRACE_Y))

albedo, height = textures.weathered_steel(size=TEX_SIZE, seed=SEED)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_frame_albedo.png"), albedo)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_frame_normal.png"),
                   textures.normal_from_height(height))
print("substructure: textures %dx%d written" % (TEX_SIZE, TEX_SIZE))

# --- self-check ----------------------------------------------------------
info = rigkit.verify_obj(OBJ_PATH)
lo, hi = info["lo"], info["hi"]

LOCAL_TOP = PILE_TOP - PILE_CENTRE
LOCAL_BOT = PILE_BOT - PILE_CENTRE
assert abs(hi[1] - LOCAL_TOP) < 1e-3, \
    "top local y=%.5f (world %.5f), want %.3f (world %.3f) — the piles must " \
    "meet the deck's underside exactly" % (hi[1], hi[1] + PILE_CENTRE, LOCAL_TOP, PILE_TOP)
assert abs(lo[1] - LOCAL_BOT) < 1e-3, \
    "bottom local y=%.5f (world %.5f), want %.3f (world %.3f)" \
    % (lo[1], lo[1] + PILE_CENTRE, LOCAL_BOT, PILE_BOT)

# The piles must sit exactly where the primitives did, in x and z.
assert abs(lo[0] - (min(PILE_X) - PILE_R)) < 1e-3, "x lo %.4f" % lo[0]
assert abs(hi[0] - (max(PILE_X) + PILE_R)) < 1e-3, "x hi %.4f" % hi[0]
assert abs(lo[2] - (min(PILE_Z) - PILE_R)) < 1e-3, "z lo %.4f" % lo[2]
assert abs(hi[2] - (max(PILE_Z) + PILE_R)) < 1e-3, "z hi %.4f" % hi[2]

# Nothing may reach north of the quay edge in the hull's lane — the boat sits
# 0.15 m off and weighs 43,776 kg. The piles are at z -5.5 and the bracing runs
# between them, so this is a standing guard rather than a live risk.
assert hi[2] <= 7.0 and lo[2] >= -7.0, "substructure escapes the deck footprint"

assert info["tris"] <= 8000, "over budget: %d tris" % info["tris"]
print("substructure: %d tris, %d verts, x %.2f..%.2f y %.3f..%.3f z %.2f..%.2f"
      % (info["tris"], info["verts"], lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]))
print("substructure: self-check passes")
```

- [ ] **Step 7: Build it**

Run:
```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/build_substructure.py 2>&1 | grep -E '^substructure:|Error|assert'
```
Expected: a `substructure: N tris …` line with `y -1.200..1.200`, then
`substructure: self-check passes`.

- [ ] **Step 8: Measure whether ANYTHING can reach a pile**

Six piles in one mesh is one node is one collider, so the substructure ships
without one. The question this step answers is whether that costs anything, and
it must be measured rather than argued.

Author two throwaway scenes in `/tmp`, **with absolute asset paths** — a scene
copied out of `assets/` cannot resolve its relative ones, and then even an
unmodified control renders stand-in unit boxes. Both scenes have the deck, a
`WaterBody` at `surface_height = 0.0`, and a `CharacterController` named
`Probe/Drop` released in the water beside the north-west pile. One scene has the
six `cylinder` primitives exactly as `deeper_demo.loom` authors them
(`pos = [x, -0.20, z]`, `scale = [0.35, 1.20, 0.35]`); the other has the new
mesh and no collider at all.

Release the probe at `x = -10.5 + 0.35 + 0.10 = -10.05`, `z = -5.5`, `y = 0.40`
— close enough to touch the pile, clear enough to fall past it if nothing is
there. Then:

```bash
cd ~/loom
for V in piles nopiles; do
  printf "%-9s " "$V"
  ./target/release/loom sim /tmp/pile_$V.loom --ticks 400 \
    --assert "Probe/Drop.y == -999" 2>&1 | grep -o '"actual": [-0-9.]*'
done
```

Also compare the horizontal position — a pile that deflects rather than stops is
still a pile that does something:

```bash
cd ~/loom
for V in piles nopiles; do
  for AX in x z; do
    printf "%s.%s " "$V" "$AX"
    ./target/release/loom sim /tmp/pile_$V.loom --ticks 400 \
      --assert "Probe/Drop.$AX == -999" 2>&1 | grep -o '"actual": [-0-9.]*'
  done
done
```

**Record all six numbers in the commit message**, and take the fork in Task 6's
scene block: identical means the piles' colliders are dead physics and the
single-mesh version ships; different means stop and report, because the
substructure then needs seven OBJs and seven nodes, which is a change to this
file's output contract and its own task.

- [ ] **Step 9: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/textures.py tools/mesh/rig/test_textures.py \
        tools/mesh/rig/build_substructure.py \
        assets/meshes/rig_steel_frame.obj \
        assets/textures/rig_steel_frame_albedo.png \
        assets/textures/rig_steel_frame_normal.png
git commit -m "feat(rig): six piles and their bracing, in rusted steel"
```

---

### Task 4: `tidal_growth`, and the seam

The spec calls this **the horror seam** and gives it the most attention of any
zone. It is not separate structure: it is the band on the piles where they cross
`WaterBody.surface_height = 0.0`, and it is a second material on the same shapes,
so it is a second OBJ over the same geometry rather than new volume.

The band: significant wave height at the berth is **0.166 m** (`deeper_demo.loom
:936`), so the surface moves roughly ±0.09 m. Weed and barnacle survive in the
splash zone above that and drown below it. Band chosen as world
**y −0.65 … +0.45** — wetter below, dying above.

**Files:**
- Modify: `tools/mesh/rig/textures.py` (add `tidal_growth`)
- Modify: `tools/mesh/rig/test_textures.py`
- Modify: `tools/mesh/rig/build_substructure.py` (emit the band as a second object)
- Create: `assets/meshes/rig_steel_tidal.obj`,
  `assets/textures/rig_steel_tidal_albedo.png`, `..._normal.png`

**Interfaces:**
- Consumes: everything Task 3 produced.
- Produces: `textures.tidal_growth(size=2048, seed=13) -> (albedo_uint8, height_float)`,
  and `rig_steel_tidal.obj` centred on the same `PILE_CENTRE = -0.200`.

- [ ] **Step 1: Write the failing test**

```python
def test_tidal_growth_is_green_black_and_varied():
    """The seam is the one place in the rig that is not brown or grey. If G does
    not lead, this is more rust and the seam does not read."""
    alb, _ = textures.tidal_growth(size=256, seed=13)
    lin = np.where(alb / 255.0 <= 0.04045, (alb / 255.0) / 12.92,
                   (((alb / 255.0) + 0.055) / 1.055) ** 2.4)
    m = lin.reshape(-1, 3).mean(axis=0)
    assert m[1] > m[0] * 1.15, "not green-shifted: %s" % m.round(4)
    assert (m < 0.10).all(), "the tide zone must be the darkest thing on the rig: %s" % m.round(4)
    # Measured 0.0152; bound set below it, as a regression check.
    assert lin.reshape(-1, 3).std(axis=0).mean() > 0.012, "too flat"
    print("  tidal ok  linear mean=%s" % m.round(4))
```
Call it at the foot of the file.

- [ ] **Step 2: Run to verify it fails**

Run: `python3 tools/mesh/rig/test_textures.py`
Expected: `AttributeError: module 'textures' has no attribute 'tidal_growth'`.

- [ ] **Step 3: Write `tidal_growth`**

```python
def tidal_growth(size=2048, seed=13):
    """-> (albedo uint8 HxWx3, height float32 HxW in 0..1).

    The band where the piles cross the water. Three things live here: black
    weed that hangs in vertical fronds, barnacle crust that is pale and hard and
    clusters, and the wet steel showing through between them.

    It is the darkest and the only green thing on the rig, deliberately — the
    spec calls this the horror seam, and a seam that is the same colour as what
    it joins is not a seam. LINEAR in, sRGB bytes out.
    """
    rng = np.random.default_rng(seed)

    # Weed hangs, so its field is stretched along Y — generated stretched along
    # X and transposed, the same trick `weathered_steel` uses.
    weed_f = _fbm(size, rng, octaves=5, base=20, stretch=5).T
    weed_f = (weed_f - weed_f.min()) / (weed_f.max() - weed_f.min())
    # Barnacles cluster, so a low frequency gates a high one.
    cluster = _fbm(size, rng, octaves=3, base=4)
    cluster = (cluster - cluster.min()) / (cluster.max() - cluster.min())
    grit = _fbm(size, rng, octaves=6, base=64)

    weed = np.clip((weed_f - 0.42) / 0.30, 0.0, 1.0)
    barnacle = np.clip((grit - 0.60) / 0.14, 0.0, 1.0) * np.clip(cluster * 1.6 - 0.35, 0.0, 1.0)

    # Barnacles stand proud; weed lies flat and wet.
    height = np.clip(0.30 + 0.55 * barnacle - 0.10 * weed + 0.15 * grit, 0.0, 1.0).astype(np.float32)

    g = grit[:, :, None]
    wet_steel = np.array([0.022, 0.024, 0.023]) + g * np.array([0.016, 0.017, 0.016])
    weed_col = np.array([0.014, 0.030, 0.016]) + g * np.array([0.012, 0.026, 0.013])
    shell = np.array([0.090, 0.086, 0.076]) + g * np.array([0.055, 0.052, 0.045])

    w = weed[:, :, None]
    b = barnacle[:, :, None]
    rgb = wet_steel * (1.0 - w) + weed_col * w
    rgb = rgb * (1.0 - b) + shell * b
    return (_srgb_encode(rgb) * 255.0 + 0.5).astype(np.uint8), height
```

- [ ] **Step 4: Run the tests, then look**

Run: `python3 tools/mesh/rig/test_textures.py` — expected: eight checks pass.
Then render a sheet the same way as Step 5 of Task 3, with `tidal_growth`, and
**open it**. Vertical dark-green weed fronds over near-black wet steel, with pale
barnacle crust clustered on top and standing proud in the normal map. Report what
you actually see.

Measured when this plan was written, at size 256 seed 13:

    linear mean [0.0321, 0.0411, 0.0323]   std 0.0152
    seam_x 5.21 / 4.33 baseline            seam_y 4.00 / 4.15 baseline
    G/R ratio 1.28  (the test wants > 1.15), and every channel below 0.10

This is the darkest thing on the rig by a wide margin — the deck's palette D
sits near linear 0.098 and this is 0.032. That contrast IS the seam.

- [ ] **Step 5: Emit the band as a second object**

In `build_substructure.py`, add the band constants and a second mesh built from
the same `tube()` helper at a slightly larger radius so it sits on the pile
rather than inside it:

```python
TIDE_TOP = 0.450          # world y — above the splash, where growth dies
TIDE_BOT = -0.650         # world y — below it, drowned
TIDE_OVER = 0.004         # the band stands this far proud of the pile
```

and, after the frame object is exported, build and export the band:

```python
# --- the seam ------------------------------------------------------------
# A SECOND OBJ over the same shapes, not new volume: the engine takes one OBJ
# per material and this band is a different material on the piles it wraps. It
# stands TIDE_OVER proud so it does not z-fight the pile beneath it — 4 mm is
# far above the depth buffer's resolution at this range and far below anything
# the player can see as a step.
mesh2 = bpy.data.meshes.new("rig_steel_tidal")
obj2 = bpy.data.objects.new("rig_steel_tidal", mesh2)
bpy.context.collection.objects.link(obj2)
bm = bmesh.new()
uv_layer = bm.loops.layers.uv.new("UVMap")
for i, px in enumerate(PILE_X):
    for j, pz in enumerate(PILE_Z):
        tube((px, -pz, TIDE_BOT - PILE_CENTRE),
             (px, -pz, TIDE_TOP - PILE_CENTRE),
             PILE_R + TIDE_OVER, SIDES, (i * 2 + j) * 0.41)
bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh2)
bm.free()

TIDAL_PATH = os.path.join(REPO, "assets", "meshes", "rig_steel_tidal.obj")
rigkit.export_obj(obj2, TIDAL_PATH, uv=True,
                  header="rig: the tide seam. World y %.3f..%.3f, %.0f mm proud of the pile."
                         % (TIDE_BOT, TIDE_TOP, TIDE_OVER * 1000))
alb2, h2 = textures.tidal_growth(size=TEX_SIZE, seed=13)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_tidal_albedo.png"), alb2)
textures.write_png(os.path.join(TEX_DIR, "rig_steel_tidal_normal.png"),
                   textures.normal_from_height(h2))

t = rigkit.verify_obj(TIDAL_PATH)
assert abs(t["hi"][1] - (TIDE_TOP - PILE_CENTRE)) < 1e-3, \
    "seam top local %.5f (world %.5f), want world %.3f" \
    % (t["hi"][1], t["hi"][1] + PILE_CENTRE, TIDE_TOP)
assert abs(t["lo"][1] - (TIDE_BOT - PILE_CENTRE)) < 1e-3, "seam bottom %.5f" % t["lo"][1]
# The seam must STRADDLE the water. A band entirely above or below it is not a
# tide line, and this is the one assertion that says what the zone is FOR.
# TIDE_BOT and TIDE_TOP are WORLD y; WaterBody.surface_height is 0.0
# (deeper_demo.loom:921).
assert TIDE_BOT < 0.0 < TIDE_TOP, \
    "the seam spans world y %.3f..%.3f and does not cross the water surface " \
    "at y=0 — that is not a tide line" % (TIDE_BOT, TIDE_TOP)
assert t["tris"] <= 6000, "seam over budget: %d tris" % t["tris"]
print("seam: %d tris, world y %.3f..%.3f, straddles the waterline"
      % (t["tris"], t["lo"][1] + PILE_CENTRE, t["hi"][1] + PILE_CENTRE))
```

- [ ] **Step 6: Build, and prove the straddle assertion can fail**

Run the build; expected a `seam:` line reporting `world y -0.650..0.450`.
Then temporarily set `TIDE_BOT = 0.100` (a band entirely above the water) and
re-run. Expected: **`the seam does not cross the water surface at y=0`**.
Restore `TIDE_BOT = -0.650`.

- [ ] **Step 7: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/textures.py tools/mesh/rig/test_textures.py \
        tools/mesh/rig/build_substructure.py \
        assets/meshes/rig_steel_tidal.obj \
        assets/textures/rig_steel_tidal_albedo.png \
        assets/textures/rig_steel_tidal_normal.png
git commit -m "feat(rig): the tide seam, where the rust stops and the weed starts"
```

---

### Task 5: The bulwark and its cap rail

Fourteen axis-aligned boxes: seven rails at world `y 1.400 … 2.400` and seven
caps at `y 2.680 … 2.920`. **Case 1** of the collider rule — axis-aligned, drawn,
statically parented — so transcription is exact.

The two gaps in the north rail are gameplay, not decoration: `x -6.900 … -3.100`
is the boarding lane and `x -10.300 … -8.300` takes the swim ladder's head. The
geometry is generated FROM the surviving rail spans, so a gap cannot be lost by
a typo in a plank loop.

**Files:**
- Create: `tools/mesh/rig/build_bulwark.py`
- Create: `assets/meshes/rig_bulwark_timber.obj`, `assets/meshes/rig_cap_metal.obj`
- Reuses `rig_deck_timber_*.png`; the cap gets `hardware` colour with no map
  (it is `metallic = 0.4` today and 14 small boxes do not earn a 1024² texture).

**Interfaces:**
- Consumes: `rigkit`, `textures.weathered_timber`.
- Produces: two OBJs, both centred on `BULWARK_CENTRE = 1.900` and
  `CAP_CENTRE = 2.800` respectively — the primitives' own `pos.y` values.

- [ ] **Step 1: Write `build_bulwark.py`**

```python
# tools/mesh/rig/build_bulwark.py
"""The rig's bulwark and cap rail.

Replaces the drawn surface of fourteen axis-aligned `box` primitives in
assets/games/deeper_demo.loom. **Case 1 of spec §1** — axis-aligned, drawn,
statically parented — so an explicit BoxCollider transcribing each primitive's
numbers is exact, bit for bit.

**The two gaps are gameplay.** `x -6.900 .. -3.100` is the boarding lane, the
demo's only taught gesture, and `x -10.300 .. -8.300` takes the swim ladder's
head. They exist here as the ABSENCE of spans in RAILS below, so a gap cannot be
lost by an off-by-one inside a loop — there is no loop that could close one.

Run: blender --background --factory-startup --python build_bulwark.py
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
TIMBER_PATH = os.path.join(REPO, "assets", "meshes", "rig_bulwark_timber.obj")
CAP_PATH = os.path.join(REPO, "assets", "meshes", "rig_cap_metal.obj")

BULWARK_CENTRE = 1.900    # the rail primitives' pos.y
CAP_CENTRE = 2.800        # the cap primitives' pos.y
RAIL_LO, RAIL_HI = 1.400, 2.400
CAP_LO, CAP_HI = 2.680, 2.920
BOARD_H = 0.235           # four boards fill the 1.0 m rail with 3 x 12 mm gaps
BOARD_GAP = 0.012

# (name, x0, x1, z0, z1) in WORLD metres, straight off the nodes.
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

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o, do_unlink=True)


def slab(bm, uvl, x0, x1, y0, y1, z0, z1, centre, uv_u0):
    """One box in WORLD metres, emitted centred on `centre` in y."""
    a, b = y0 - centre, y1 - centre
    # Blender z is Loom y; Blender y is Loom -z.
    corners = [(x0, -z0, a), (x1, -z0, a), (x1, -z1, a), (x0, -z1, a),
               (x0, -z0, b), (x1, -z0, b), (x1, -z1, b), (x0, -z1, b)]
    v = [bm.verts.new(c) for c in corners]
    quads = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
             (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
    faces = [bm.faces.new(tuple(v[i] for i in q)) for q in quads]
    for f in faces:
        for loop in f.loops:
            x, y, z = loop.vert.co
            # U along the run, V up the face: one tile per 2 m and per 1 m, the
            # same texel density as the deck so the two read as one structure.
            loop[uvl].uv = (uv_u0 + x / 2.0, z + 0.5)
    return faces


# ---- the timber bulwark -------------------------------------------------
mesh = bpy.data.meshes.new("rig_bulwark_timber")
obj = bpy.data.objects.new("rig_bulwark_timber", mesh)
bpy.context.collection.objects.link(obj)
bm = bmesh.new()
uvl = bm.loops.layers.uv.new("UVMap")

rng_state = 23
def rand():
    global rng_state
    rng_state = (rng_state * 1103515245 + 12345) & 0x7FFFFFFF
    return rng_state / 0x7FFFFFFF

pitch = BOARD_H + BOARD_GAP
rows = int(round((RAIL_HI - RAIL_LO) / pitch))
# Same flush-both-ends rule as the deck: `rows` boards have `rows - 1` gaps.
board_h = ((RAIL_HI - RAIL_LO) - (rows - 1) * BOARD_GAP) / rows
for name, x0, x1, z0, z1 in RAILS:
    for r in range(rows):
        y0 = RAIL_LO + r * (board_h + BOARD_GAP)
        slab(bm, uvl, x0, x1, y0, y0 + board_h, z0, z1, BULWARK_CENTRE, rand())

bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh)
bm.free()
rigkit.export_obj(obj, TIMBER_PATH, uv=True,
                  header="rig: bulwark timber. %d runs x %d boards of %.4f m."
                         % (len(RAILS), rows, board_h))

# ---- the metal cap ------------------------------------------------------
mesh2 = bpy.data.meshes.new("rig_cap_metal")
obj2 = bpy.data.objects.new("rig_cap_metal", mesh2)
bpy.context.collection.objects.link(obj2)
bm = bmesh.new()
uvl = bm.loops.layers.uv.new("UVMap")
for name, x0, x1, z0, z1 in CAPS:
    slab(bm, uvl, x0, x1, CAP_LO, CAP_HI, z0, z1, CAP_CENTRE, 0.0)
bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
bm.to_mesh(mesh2)
bm.free()
rigkit.export_obj(obj2, CAP_PATH, uv=False,
                  header="rig: cap rail, %d runs. Flat colour; metallic 0.4." % len(CAPS))

# --- self-check ----------------------------------------------------------
ti = rigkit.verify_obj(TIMBER_PATH)
ci = rigkit.verify_obj(CAP_PATH)

assert abs(ti["lo"][1] - (RAIL_LO - BULWARK_CENTRE)) < 1e-3, "rail bottom %.5f" % ti["lo"][1]
assert abs(ti["hi"][1] - (RAIL_HI - BULWARK_CENTRE)) < 1e-3, "rail top %.5f" % ti["hi"][1]
assert abs(ci["lo"][1] - (CAP_LO - CAP_CENTRE)) < 1e-3, "cap bottom %.5f" % ci["lo"][1]
assert abs(ci["hi"][1] - (CAP_HI - CAP_CENTRE)) < 1e-3, "cap top %.5f" % ci["hi"][1]

# **THE GAPS MUST STILL BE GAPS.** Walk the north-side spans and assert that
# nothing was emitted inside either one. A closed boarding gap is a demo whose
# only taught gesture is impossible, and it would render as a perfectly ordinary
# railing — there is nothing to see.
GAPS = (("boarding", -6.900, -3.100), ("swim ladder", -10.300, -8.300))
north = [(x0, x1) for _, x0, x1, _, z1 in RAILS if z1 <= -6.800]
for label, g0, g1 in GAPS:
    for x0, x1 in north:
        assert x1 <= g0 + 1e-6 or x0 >= g1 - 1e-6, \
            "a rail span %.3f..%.3f intrudes into the %s gap %.3f..%.3f" \
            % (x0, x1, label, g0, g1)
print("bulwark: both north gaps intact (%s)"
      % ", ".join("%s %.3f..%.3f" % (n, a, b) for n, a, b in GAPS))

assert ti["tris"] + ci["tris"] <= 9000, \
    "over budget: %d + %d tris" % (ti["tris"], ci["tris"])
print("bulwark: timber %d tris, cap %d tris" % (ti["tris"], ci["tris"]))
print("bulwark: self-check passes")
```

- [ ] **Step 2: Build it**

Run:
```bash
cd ~/loom && blender --background --factory-startup \
  --python tools/mesh/rig/build_bulwark.py 2>&1 | grep -E '^bulwark:|Error|assert'
```
Expected: a `both north gaps intact` line, a triangle line, then
`bulwark: self-check passes`.

- [ ] **Step 3: Prove the gap assertion can fail**

Temporarily add `("NorthGap", -6.900, -3.100, -7.000, -6.800)` to `RAILS` —
a rail that fills the boarding lane — and re-run.
Expected: **`a rail span -6.900..-3.100 intrudes into the boarding gap`**.
Remove it. This is the single most expensive thing this task could get wrong and
it is the only assertion that would catch it.

- [ ] **Step 4: Commit**

```bash
cd ~/loom
git add tools/mesh/rig/build_bulwark.py \
        assets/meshes/rig_bulwark_timber.obj assets/meshes/rig_cap_metal.obj
git commit -m "feat(rig): the bulwark and its cap rail, gaps intact"
```

---

### Task 6: One scene, all four zones, and a gate row

**Files:**
- Create: `assets/test/rig_structure.loom`
- Modify: `xtask/src/main.rs` (`SCENES` 73 → 74, `GOLDEN` 57 → 58)
- Modify: `assets/SCENES.md` (regenerated)

- [ ] **Step 1: Generate seven UUIDs**

```bash
python3 -c "import uuid; [print(uuid.uuid4()) for _ in range(9)]"
```
One for the scene, one per `[[asset]]`. **Never leave an `id` off** — an empty id
aliases every other empty id and the first adopted wins.

- [ ] **Step 2: Author the scene**

```toml
# The rig's structure — deck, substructure, tide seam and bulwark, in the four
# materials Phase 1 builds. One node per material zone, because the engine reads
# no `.mtl` and takes one OBJ per material.
#
# **Every node sits at the transform its primitive already carries in
# `deeper_demo.loom`.** Each mesh is authored centred on its own local origin
# and hangs from that y — `play.rs:520` centres a static collider on the node,
# not on the vertices, so this is the only arrangement in which a mesh and a
# matching `BoxCollider` can coexist. Spec §1 Case 1: all four are axis-aligned,
# drawn and statically parented, so the transcription is exact, bit for bit.
#
# `Drop` is here so a human — or a future `green.sh` row — can measure the
# settle height. **The image row does not check it.** The deck's collider
# equality was proved by hand with `loom sim --assert`, not by any gate.

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

[[asset]]
key = "steel_frame"
id = "PASTE-UUID-5"
path = "../meshes/rig_steel_frame.obj"

[[asset]]
key = "steel_albedo"
id = "PASTE-UUID-6"
path = "../textures/rig_steel_frame_albedo.png"

[[asset]]
key = "steel_normal"
id = "PASTE-UUID-7"
path = "../textures/rig_steel_frame_normal.png"

[[asset]]
key = "steel_tidal"
id = "PASTE-UUID-8"
path = "../meshes/rig_steel_tidal.obj"

[[asset]]
key = "tidal_albedo"
id = "PASTE-UUID-9"
path = "../textures/rig_steel_tidal_albedo.png"

[[asset]]
key = "tidal_normal"
id = "PASTE-UUID-10"
path = "../textures/rig_steel_tidal_normal.png"

[[asset]]
key = "bulwark_timber"
id = "PASTE-UUID-11"
path = "../meshes/rig_bulwark_timber.obj"

[[asset]]
key = "cap_metal"
id = "PASTE-UUID-12"
path = "../meshes/rig_cap_metal.obj"

[[node]]
name = "Rig"

  # The demo's own environment, so this row's picture is comparable to the scene
  # it stands in for. `deeper_demo.loom:668-676`.
  [node.components.Environment]
  sun_direction = [0.42, 0.46, -0.78]
  sun_strength = 1.25
  sun_color = [1.0, 0.93, 0.80]
  ambient = 0.45
  sky_zenith = [0.16, 0.30, 0.52]
  sky_horizon = [0.74, 0.72, 0.66]
  fog_density = 0.0028
  fog_falloff = 0.05

  # The seam is a tide line, so there has to be a tide to line. Surface at y=0,
  # the same plane `deeper_demo.loom:921` puts it on.
  [node.components.WaterBody]
  kind = "ocean"
  simulation = "deterministic"
  surface_height = 0.0
  fetch = 15000.0

  [node.components.Wind]
  direction_degrees = 20.0
  speed = 3.0
  gustiness = 0.25
  turbulence = 0.15

[[node]]
name = "Deck"
parent = "Rig"

  [node.transform]
  pos = [0.0, 1.20, 0.0]

  [node.components.MeshRenderer]
  mesh = { asset = "deck_timber" }

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
name = "Substructure"
parent = "Rig"

  # Six piles and their bracing, in ONE mesh and therefore under ONE node — so
  # it can carry at most ONE collider, and a single box at this node's origin
  # would be a 0.7 m post in the middle of the rig, not six posts under it.
  # This node deliberately has no collider; Task 3 Step 8 measures whether that
  # costs anything, and the fork below says what to do if it does.
  [node.transform]
  pos = [0.0, -0.20, 0.0]

  [node.components.MeshRenderer]
  mesh = { asset = "steel_frame" }

  [node.components.Material]
  albedo = [1.0, 1.0, 1.0]
  albedo_map = { asset = "steel_albedo" }
  normal_map = { asset = "steel_normal" }
  roughness = 0.85
  metallic = 0.25
  uv_scale = [1.0, 1.0]

[[node]]
name = "Seam"
parent = "Rig"

  # The tide band, 4 mm proud of the piles it wraps. No collider: it is a
  # surface on shapes that already have one, and giving it a second would put a
  # 24 m box around six piles.
  [node.transform]
  pos = [0.0, -0.20, 0.0]

  [node.components.MeshRenderer]
  mesh = { asset = "steel_tidal" }

  [node.components.Material]
  albedo = [1.0, 1.0, 1.0]
  albedo_map = { asset = "tidal_albedo" }
  normal_map = { asset = "tidal_normal" }
  roughness = 0.70
  metallic = 0.0
  uv_scale = [1.0, 1.0]

[[node]]
name = "Bulwark"
parent = "Rig"

  [node.transform]
  pos = [0.0, 1.90, 0.0]

  [node.components.MeshRenderer]
  mesh = { asset = "bulwark_timber" }

  [node.components.Material]
  albedo = [1.0, 1.0, 1.0]
  albedo_map = { asset = "deck_albedo" }
  normal_map = { asset = "deck_normal" }
  roughness = 0.85
  metallic = 0.0
  uv_scale = [1.0, 1.0]

[[node]]
name = "CapRail"
parent = "Rig"

  # Flat colour and no maps: it is `metallic = 0.4` today, fourteen small boxes
  # do not earn a 1024² texture, and the values here are `CapSouth`'s own.
  [node.transform]
  pos = [0.0, 2.80, 0.0]

  [node.components.MeshRenderer]
  mesh = { asset = "cap_metal" }

  [node.components.Material]
  albedo = [0.34, 0.26, 0.21]
  roughness = 0.7
  metallic = 0.4

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

  # Low and off the north-west corner, so one frame carries all four zones: the
  # deck's surface, the bulwark above it, the piles below, and the seam where
  # they cross the water. A deck photographed from above is a texture swatch.
  [node.transform]
  pos = [-17.5, 2.20, -13.0]
  rot_euler = [-9.0, -34.0, 0.0]

  [node.components.Camera]
  fov_y_degrees = 52.0
```

**The `Substructure` node has no collider, and Task 3 Step 8 decides whether
that is free.** One mesh is one node is one collider, so six piles cannot be
given six boxes from a single node — the fork is structural, not cosmetic:

- **If nothing can reach a pile** (Step 8's swimmer never touches one either
  way): ship as written. One mesh, one node, no collider. Record in the scene's
  header that the demo's six pilings currently carry colliders nothing uses, so
  Phase 4 is *removing dead physics*, not losing live physics — and that this
  was measured, on this date, with the numbers.
- **If something can reach a pile**: the substructure must become **six
  per-pile OBJs plus one for the bracing**, seven nodes, each pile carrying
  `BoxCollider { half_extents = [0.35, 1.20, 0.35] }` at its own
  `pos = [x, -0.20, z]`. Splitting one material across several OBJs is legal —
  the one-OBJ-per-material rule forbids two materials in a file, not one
  material in two files — and all seven share the same texture pair. It costs
  seven draw calls instead of one. **Do not attempt this as a patch on the
  single-mesh version**; it changes `build_substructure.py`'s output contract,
  so stop and report, and it becomes its own task.

Either way the measurement goes in the commit message. "Nothing reaches the
piles" is a claim about the whole scene's geometry and it should not be
re-derived by the next person from scratch.

- [ ] **Step 3: Validate and measure**

```bash
cd ~/loom
./target/release/loom validate assets/test/rig_structure.loom
./target/release/loom measure assets/test/rig_structure.loom
```
The `assets` array **must be empty**. A non-empty one means a path is wrong and
the renderer is drawing stand-in unit boxes, which looks exactly like success.

- [ ] **Step 4: Render at gate size and at eye height, and look**

```bash
cd ~/loom
./target/release/loom render assets/test/rig_structure.loom --out /tmp/struct.png --size 320x200
```
**Open it.** Then take one closer shot at standing eye height. The seam should be
the darkest thing in frame and the only green. Report honestly whether the four
zones read as one structure or as four materials that happen to touch.

- [ ] **Step 5: Diff-sweep the tick, then add the rows**

The structure is static, so the expected answer is `--sim 0` — prove it:

```bash
cd ~/loom
for T in 0 60 300; do
  ./target/release/loom render assets/test/rig_structure.loom \
    --out /tmp/sw_$T.png --size 320x200 --sim $T
done
./target/release/loom compare /tmp/sw_0.png /tmp/sw_60.png
./target/release/loom compare /tmp/sw_0.png /tmp/sw_300.png
```
Expected: zero differing pixels. Then bump `const SCENES: [&str; 73]` to `74` and
`const GOLDEN: [(&str, &str, &[&str]); 57]` to `58` — **the length is part of the
type and the edit will not compile otherwise** — and add both rows with comments
in house style saying what each is the only picture of.

- [ ] **Step 6: Fault-inject the row**

Take a no-op control copy first, with asset paths rewritten to absolute — a
scene copied to `/tmp` cannot resolve its relative ones, and then the control
fails too and every fault number measures the copy:

```bash
cd ~/loom
abs() { sed 's|\.\./meshes/|'"$PWD"'/assets/meshes/|; s|\.\./textures/|'"$PWD"'/assets/textures/|' "$1"; }
abs assets/test/rig_structure.loom > /tmp/c0.loom
./target/release/loom render /tmp/c0.loom --out /tmp/c0.png --size 320x200
./target/release/loom compare /tmp/c0.png /tmp/struct.png     # expect 0 differing
```
Then two faults, each measured and recorded in the commit message: delete the
seam node (the horror seam should move a large fraction), and delete the
substructure's `normal_map` (should move a smaller one). **If either moves
nothing, the row does not guard what its comment claims** — change the comment.

- [ ] **Step 7: Regenerate the scene index and run the gates a builder may run**

```bash
cd ~/loom
python3 tools/scene_index.py
cargo clippy --workspace --all-targets -j 3 -- -D warnings
cargo test --workspace -j 3
bash tools/goldcheck.sh rig_structure assets/test/rig_structure.loom
```
`goldcheck.sh` will report **no reference image**, which is correct and expected.
**Do not bless.**

- [ ] **Step 8: Commit**

```bash
cd ~/loom
git add assets/test/rig_structure.loom xtask/src/main.rs assets/SCENES.md
git commit -m "test(gate): rig_structure joins SCENES and GOLDEN"
```

---

## Phase 1 exit criteria

1. The deck is palette D and its grain repeat is visibly gone — **looked at, not
   inferred**.
2. `weathered_steel` and `tidal_growth` both pass tiling, determinism and colour
   checks, and both have been opened and judged.
3. Every generator's self-check has been **seen to fail** under an injected
   fault: the deck's palette band, the seam's straddle assertion, the bulwark's
   gap assertion.
4. The fifth collider case is in the spec, and Task 3 Step 8's measurement is
   recorded either way.
5. `rig_structure` is in both `SCENES` and `GOLDEN`, with two recorded
   fault-injection fractions.
6. `cargo clippy` and `cargo test --workspace` pass.
7. **Nothing is blessed.**
8. `assets/games/deeper_demo.loom` is untouched. Integration is Phase 4.

## What Phase 1 deliberately does not do

- No change to `deeper_demo.loom`. The demo keeps its primitives and stays
  runnable throughout; swapping them is Phase 4's whole job.
- No re-pin of the exact-equality asserts in `green.sh`. Those move when the six
  rotated ramp nodes change, which is Phase 3.
- No fix for the `StepLow`/`StepMid`/`StepHigh` squared colliders. Still its own
  change, its own gate row.
- No Cycles bake. Still the documented upgrade path if flat lighting reads poorly.
