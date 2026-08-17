# Foliage painting — grass you paint, vegetation you place in bulk

*Round 2. Extends `PLAN.md`, which supersedes `01`–`07`; where this document and `03-painting-splat-and-vertex.md` disagree, this one wins for **placement** and doc 03 still wins for **shading**. Design phase — no `cargo` command was run. Every `file:line` was read in this worktree at `62f9ebe`; §13 lists what could not be checked.*

`PLAN.md` mentions foliage once, as "the `Ground.rock` grass hook" inside Stage 6 item 4. That hook is three lines and it is not this feature. This document is Unity's Detail brush and Unreal's Foliage mode, built on a placement engine that already exists and has never been reachable from a mouse.

---

## 1. The shape of it, in one page

**Loom already has both halves of foliage painting and neither has a brush.** `loom_grass` places blades as a pure function of coordinates; `loom_scatter` places meshes by order-independent elimination, with `reach_of` sitting in the crate specifically to size a dirty-region rebuild — **and `reach_of` has no caller anywhere in the workspace** (verified: `rg reach_of` matches only its own definition). The incremental machinery was built, tested, documented, and never wired to anything a person can touch.

So this feature is mostly wiring, plus one genuinely new idea:

> **A painted mask multiplies the placement rule. It never overrides it.** `coverage` and `viability` keep every term they have; painting scales the result. Untouched ground multiplies by exactly `1.0` and is bit-identical to today. Erased ground multiplies by exactly `0.0`. And because both rules are *products* that already contain a slope term and a rock term, **no stroke can put grass on a cliff or a tree in a hole** — the guarantee `slope_cutoff` makes, and that `grass_thins_on_a_slope_and_stops_on_rock` asserts, survives painting by construction rather than by care.

Three storage tiers, and the file only ever holds the first two at scale:

| | What it is | What it costs in the file |
| --- | --- | --- |
| **The rule** | `Grass` / `Scatter` — density, spacing, slope, seed | 8 lines, exists today |
| **The mask** | `FoliagePaint.strokes` — a stroke list, ADR 0027's mechanism | ~50 bytes per stroke point |
| **The exceptions** | a hand-placed tree is a **node**; a deleted one is a **point** | 5 lines per placed tree, 2 floats per deletion |

**A species is a node**, not an entry in a species array. Unity's detail-type list becomes the hierarchy, and every species inherits selection, naming, hide/isolate, per-node `Material`, prefab overrides and per-node undo for free.

**No shader is touched.** Not `grassVertexMain`, not `vertexMain`, not `fragmentMain`. The foliage mask is CPU-only — placement happens on the CPU, so the mask never becomes a texture, never takes a bindless slot, never needs a `paint_upload` pass, a barrier or an `ObjectData` field. This stage is therefore far cheaper than Stage 6 and its "no golden reference moves" claim is nearly free.

---

## 2. The composition rule — how a mask feeds a pure function without breaking purity

### 2.1 Why ADR 0028's `lerp` is wrong here, and must not simply be copied

ADR 0028 fixed the splat form as `w = lerp(groundLayerWeight(…), value, authority)` — the mask *takes over* where it has authority. That is right for a **blend weight**: a blend weight has no invariants, and a painter who paints rock wants rock.

It is wrong for a **placement probability**. `coverage` returns a fraction that is multiplied against a rejection roll, and `slope_cutoff` is documented as *"grass stops entirely"* — a promise an agent can check and a test does check. A `lerp` form lets a stroke restore grass past the cutoff, which fails `grass_thins_on_a_slope_and_stops_on_rock` (`loom_grass/src/lib.rs:500`) and re-opens the floating-blades hole that `rock = 1.0` closes for terrain with no surface.

**So foliage composes multiplicatively:**

```rust
// loom_grass::coverage, one added factor at the end
(steepness * soil * lush * ground.paint / LUSH).clamp(0.0, 1.0)
```

where `ground.paint` is `lerp(1.0, value, authority)` sampled from the mask, and defaults to `1.0`.

Four properties, each the reason a simpler encoding was rejected:

**1. An unpainted scene is bit-identical.** No `FoliagePaint` → the closure returns `paint: 1.0` → `x * 1.0` is exactly `x` in IEEE. A node carrying a mask painted and then fully undone has authority 0 everywhere → `lerp(1, v, 0)` is exactly `1.0` → still bit-identical. `meadow` and `grass_slope` do not move and nothing needs re-blessing. This is the same exactness argument ADR 0028 makes and it holds for the same reason.

**2. Erase is absolute.** `value = 0`, `authority = 1` → gain `0.0` → the product is exactly zero whatever the rule says. A painter who erases grass gets no grass. That is the single most important thing a brush must be able to promise, and a `lerp` cannot promise it in the other direction.

**3. Painting cannot defeat a hard rule.** `steepness` is zero past the cutoff and `soil` is zero on rock or on no-ground; zero times anything is zero. Grass never grows on a cliff, never floats over a hole, and a crater still clears its tile. **The crater test keeps passing without being modified.**

**4. Painting cannot exceed the authored density.** `value` is clamped to `[0, LUSH]` — `LUSH` is 1.6, the headroom the candidate grid already carries for gullies (`loom_grass/src/lib.rs:255`). So the densest a painted patch reaches is exactly the densest a gully reaches, the `density` field in the inspector remains the truth about the field's maximum, and `the_blade_count_follows_the_requested_density` is untouched because it paints nothing. **Painted "grow" borrows the flow headroom rather than inventing a second one.**

The same factor lands in `loom_scatter::viability`, and it lands *inside* `viability` rather than at either call site, which is not a stylistic choice. `viability` is read twice, by `habitable` (before elimination) and `kept` (after), and the crate's comments explain at length why the split matters: a candidate the ground refuses outright must not compete, or a cliff sterilises the ground beside it. **An erased region must behave like a cliff, not like poor ground** — otherwise the fringe of everything you erase comes out thinned, which is the shaved-ring artifact arriving from the third direction. Putting the factor in `viability` gets both behaviours from one line.

### 2.2 Purity is preserved because the mask enters through the closure that already exists

`loom_grass::tile` takes ground as `&dyn Fn(f32, f32) -> Ground` (`lib.rs:315`) and `loom_scatter::region_on` takes the identical shape (`lib.rs:357`). **Neither crate gains a dependency.** The CLI's `GroundGrid` already answers that closure by marching the voxel SDF; it gains a mask reference and one more field to fill. This is exactly the seam ADR 0028 uses for `rock`, used a second time — and the second use is what turns a one-off into a pattern.

The purity chain, stated once so the tests can be written against it:

> strokes → *(deterministic rasterisation)* → mask → *(pure closure)* → blades

`tile()` remains a pure function of `(tile, rules, closure)`, so tile-level dirty-region regeneration is still byte-identical and every existing `loom_grass` test stands unmodified. The new obligation is that **the mask is a pure function of the stroke list**, which is precisely what ADR 0027's `incremental_painting_equals_a_full_rasterisation` test already guarantees. One new test per crate closes the loop:

- `loom_grass`: `painting_a_patch_leaves_blades_outside_it_untouched` — the crater test with a mask instead of a crater.
- `loom_scatter`: `an_erased_region_does_not_thin_its_own_fringe` — the `habitable`-vs-`kept` ordering, asserted rather than assumed.

### 2.3 The painted boundary must wander, and the cheapest place is the rasteriser

This project has learned three times that **a clean curve in a density field is the synthetic tell** — `coverage`'s slope wander, `viability`'s slope fade, `claim`'s outward biome blend. A brush stroke's edge is a clean curve by construction: it is a disc.

Perturbing the *sampled* gain cannot work. Scaling authority by noise breaks erase-exactness (`lerp(1, 0, 1-ε)` is `ε`, not zero); scaling the gain breaks the unpainted-is-identical property unless it is itself gated on authority, which puts you back at the first problem.

**So the break-up goes into the dab, at bake time.** The foliage baker modulates each dab's radius by `loom_field::noise::value` on world position — low frequency, amplitude ~12% of the radius, subtractive only, the same asymmetry rule `coverage`'s wander follows and for the same reason. Consequences: the interior of a stroke is exactly `value` at authority 1, untouched texels are exactly authority 0, and only the transition band is ragged. It costs nothing per blade, it is deterministic because the noise is frozen ABI, it re-rasterises identically so the incremental-equals-full test still holds, and **the painter sees the ragged edge in the live preview**, which is the only way they can judge it.

`FOLIAGE_EDGE_BREAKUP` is a constant in the baker with the measurement written beside it. No knob until someone paints something it gets wrong.

---

## 3. Where the mask lives, and why a species is a node

**`FoliagePaint` goes on the same node as the `Grass` or `Scatter` it modulates**, projected top-down over that node's own `half_extent` in world XZ — the identical projection rule doc 03 §3 fixed for `SplatPaint`, for the identical reason (the subject is ground; vertical faces are where the slope rule is already unambiguous).

```toml
[[node]]
name = "Meadow grass"

  [node.components.Grass]
  half_extent = [64.0, 64.0]
  density = 140.0

  [node.components.FoliagePaint]
  texels_per_meter = 2.0

    [[node.components.FoliagePaint.strokes]]
    value = 0.0                       # erase
    points = [[12.5, -3.0], [13.1, -3.4], [14.0, -3.9]]
      [node.components.FoliagePaint.strokes.brush]
      radius_m = 3.0
      hardness = 0.6
      strength = 1.0
      flow = 0.8
      spacing = 0.25
```

**`texels_per_meter` defaults to 2.0, not doc 03's 4.0, and the reason is the clump.** Blades within a `CLUMP` (0.5 m) agree about facing, height and colour by design, so a mask finer than half a metre controls a quantity nobody can see. Two texels per metre puts one texel per clump. Clamped to `256..=2048` as doc 03 has it, so a 256 m field is 512² — **1 MB of mask governing 9 million blades**, which is the compression ratio that makes this feature possible at all.

### Why a species is a node rather than a species array

Unity gives a terrain a list of detail types and Unreal gives a foliage type asset per mesh. Both are workarounds for engines whose scene graph cannot hold a million things. Loom's already can't and already doesn't — `Grass` and `Scatter` are the rules, and the hierarchy shows one node per field. Adding a *second* list inside that node would be a hierarchy nobody can select in, and it would need its own naming, its own reorder op and its own override semantics.

**A species is a node.** Painting a second grass type is `SpawnNode` + `Grass` + `Material` + `FoliagePaint`, which is one transaction over ops that all exist. The editor's Foliage palette is a filtered view of the scene's `Grass`/`Scatter` nodes — no new model, and the hierarchy stays the one place things are named.

The cost is honest and worth stating: **N species means N `GroundGrid` bakes and N mask rasters.** `grass_blades` already loops nodes and bakes a grid per field (`main.rs:1928`), so six species over one terrain is six marches of the same SDF. §7.4 fixes that by hoisting the grid, which is a change worth making anyway.

### The first stroke on a scene with no field creates the field

A stranger selects the foliage tool on a bare terrain and drags. If nothing happens because there is no `Grass` node, the feature is undiscoverable. **The first stroke spawns the field and paints, in one transaction, one Ctrl+Z**, labelled `"Add grass field and paint 1 stroke"` — `SpawnNode`, three `SetField`s, one `SpliceArray`. The field's `half_extent` is sized to the terrain node's bounds, clamped to 128 m so the first accident is not a nine-million-blade bake (§7).

---

## 4. The brush

One brush model, ADR 0027's `BrushParams { radius_m, hardness, strength, flow, spacing }`, radius always in world metres, one falloff, one dab walker. Foliage adds exactly one authored value and rejects the rest.

**Density is the only thing the mask carries.** The brush paints toward a target multiplier shown as `×0.0 … ×1.6` with three presets — **Clear** (0.0), **Thin** (0.5), **Grow** (1.6) — and `flow` decides how fast authority accumulates under repeated dabs, which is what makes a light touch feather. Erase is `value = 0`, not a mode, per ADR 0027.

**Species is which node is active in the Foliage palette, not a brush parameter.** The brush cursor is tinted with that field's `Material.albedo`, and while the foliage tool is active the active field's `half_extent` is drawn as a rectangle on the ground — because the single most likely first confusion is painting outside the field and seeing nothing. Painting at the edge raises a one-line banner with a **Grow field to fit** button issuing one `SetField` on `half_extent`.

**Size jitter is a rule field, not a brush field, and this is a deliberate refusal.** `Scatter.scale` is `[min, max]` and already does it. A second mask channel for scale would double the raster, add an encoding question, and answer a need nobody has stated. *Trigger to add it: someone paints two size-populations of one species by duplicating the field twice.*

**Align-to-normal is a rule field, and the engine is missing it.** Verified: `scatter_objects` builds `translation * rotation_y(yaw) * scale` (`main.rs:1858`) with no reference to the surface normal at all, so every scattered rock on a 20° slope currently stands plumb. Add `Scatter.align: f32`, default `0.0` — upright, so every existing scene is byte-identical — slerping the instance's up-axis toward `GroundGrid::at().normal`, which is already computed and thrown away. Six lines in `scatter_objects`, no shader change, and it is what the research doc's `align = "surface_normal"` asked for (`loom-pcg-and-editor.md:105`).

**Grass stays upright and does not get an align control.** Real grass is gravitropic; blades on a hillside stand up, they do not lie normal to the slope. `blade.tilt` already leans them away from their clump centre, which is the variation that matters. Adding a slope-align term would need the normal in the 48-byte blade payload for an effect that is wrong.

### The refusal message, which is the whole onboarding story

Because §2.1 makes it impossible to paint grass onto a cliff, a painter *will* drag across a steep bank and see nothing appear. Silence there is the failure mode this project keeps writing tests against — a rule that placed nothing looks exactly like a rule that matched nothing (`main.rs:1834`).

**So the tool counts its own dabs.** When a committed stroke lands more than a quarter of its dabs where `coverage` is already zero, the console and a viewport banner say it in the words a stranger needs:

> *No grass placed on 62% of that stroke — the ground there is steeper than this field's `slope_cutoff` (0.70, about 45°). Raise it on **Meadow grass**, or paint soil under it first.* **[Raise to 0.55]**

The button is one `SetField`. This is the same "explain, then offer the one-click fix" shape as PLAN §Stage 5's **Add Player** banner, it teaches the rule-first model on the one occasion the user is guaranteed to be curious, and it costs about thirty lines.

---

## 5. Vertex-shader grass versus mesh scatter — when each is right

The boundary is not aesthetic and it is not "small versus large". It is **the object buffer and the TLAS**, and it is checkable.

**Use `Grass` when** the thing is sub-metre, thin, needs no silhouette in a shadow or a reflection, and you want tens of thousands. Cost is measured: **0.054 ms for 45,460 blades** at 1080p/4× MSAA. Blades are not in the acceleration structure, so they cast no ray-traced shadow, occlude nothing in RTAO and appear in no reflection — which ADR 0019 notes is a coincidence of the implementation that happens to be what saved AO from the depth-hairball objection. Blades are not ECS entities, not in physics, and outside the sim hash.

**Use `Scatter` when** the thing has a silhouette that must be shadowed, reflected or collided: bushes, rocks, saplings, logs. Each instance becomes an `Object` (`main.rs:1867`) — a row in the object buffer (`MAX_OBJECTS = 4096` initially, grown by `reserve_objects`) and an instance in the **per-frame TLAS rebuild**. Instances sharing a mesh are one instanced draw, because `render` sorts by mesh and `batch_by_mesh` runs them together (`renderer.rs:2969`).

**The crossover is around four thousand instances**, and §7.2 shows that at plausible spacings a 256 m field lands well under it, so **mesh foliage needs no new culling and grass does**. That is the opposite of the intuition and it is the single most useful number in this document.

**A third "detail mesh" path — batched, not in the TLAS, cheap, Unity's detail-mesh mode — is rejected for now.** It would be a fourth way to put geometry on screen, and the measured numbers say the two we have cover the range. *Trigger: a scene wants more than ~4,000 small meshes and the frame telemetry shows the TLAS rebuild dominating.* Octahedral impostors are the same answer one level further out and are deferred with the same trigger.

---

## 6. Storage: a rule, a mask, and exactly two kinds of exception

The brief's guess is right — a rule plus a seed, not a list — and the interesting half is what happens when a human wants one specific tree somewhere the rule did not put one, or gone from where it did.

### 6.1 Adding one is a node

**A tree the user placed by hand is a scene node**, spawned by `SpawnNode { prefab }` (ADR 0026) or `SpawnNode { mesh }`. Never an entry in an array. The reason is that a hand-placed tree is a thing the user will select, name, move, parent to a building and possibly attach a script to — everything a node does and nothing an array element does. It costs five lines of TOML each, which is exactly right for the tens of them a scene has, and it survives every later change to the rule because the rule never generated it.

The palette's **Place one** mode (Unreal's single-instance placement) is therefore not a foliage feature at all — it is the existing create-and-drop flow with the field's mesh preselected and `place::resolve`'s drop-on-surface doing the work. Free.

### 6.2 Removing one is a point

A deletion cannot be a node (there is nothing to spawn) and should not be a stroke (a stroke is a disc; erasing one tree from a copse would take its neighbours).

**`Scatter.remove: Vec<[f32; 2]>` — world XZ points, each killing every instance within `spacing * 0.45` of it.**

```toml
remove = [[12.5, -3.0], [41.2, 18.7]]
```

Four properties, and the third is the one that makes it safe:

1. **Diffable and agent-authorable.** Two floats. A hundred removals is three readable lines of TOML.
2. **Robust to rule changes.** A removal names a *place*, not an instance, so raising `density` or changing `seed` leaves it meaning what the human meant — "no tree in front of the door". A stale point kills whatever is now nearest, or nothing.
3. **Provably at most one instance per point.** `loom_scatter` guarantees a *minimum* separation of `spacing` (`lib.rs:113`), so a disc of diameter `0.9 × spacing` cannot contain two accepted instances. The test asserts it, and it is why the radius is `0.45` and not `0.5`.
4. **Bounded influence, so dirty-region regeneration stays correct.** A removal reaches `spacing * 0.45` and no further, so it adds one term to `reach_of` and a regenerated patch is still bit-identical to the same patch inside a full rebuild — the property the whole crate exists to have.

Applied in `region_on` after `kept`, as a filter. Order-independent because it is a set-membership test on position, not a sequential edit.

**Grass gets no `remove` list.** Erasing an individual blade is meaningless; grass erase is the mask, at whatever radius the brush is set to. The asymmetry is not an oversight, it is what the two systems are.

### 6.3 Moving one is both, in one transaction

Dragging a *generated* instance is the gesture Unreal calls "convert to actor", and it falls out of the two primitives above with no third mechanism:

> **`SpliceArray` a point into `Scatter.remove` at the instance's old position, and `SpawnNode` a real node at the new one — one `Transaction`, one label, one Ctrl+Z.** Label: `"Detach and move 1 pine"`.

The user sees a tree they dragged. The file gains two lines and loses none. The rule is untouched. And because it is one transaction, undo puts the generated tree back and deletes the node in one keystroke, which is never-do #16 working exactly as advertised.

### 6.4 What the file looks like at a thousand trees

A 256 m painted forest at 8 m spacing is **326 instances** (§7.2) described by: eight lines of `Scatter`, a stroke list of perhaps forty points (~2 kB), a handful of `remove` points, and however many hand-placed nodes the human wanted. **The instances themselves are never written.** `git diff` on "moved the forest uphill" is one changed number, which is the property the research doc measured Unreal and Blender both failing to have (`loom-pcg-and-editor.md:61`).

---

## 7. LOD, culling, and the cost model at 256 m

This is where the honest answer is uncomfortable, so it goes in numbers.

### 7.1 Grass does not fit, by a factor of thirty-five

Verified constants: `GrassBlade` is three `float4`s = **48 bytes** (`renderer.rs:582`); the buffer is `MAX_BLADES = 262_144` (`renderer.rs:999`, `viewer.rs:436`) = **12.6 MB**; `meadow` is `half_extent = [9, 9]` at `density = 140` = 324 m² × 140 = 45,360 blades, matching the 45,460 CLAUDE.md quotes (the identity `coverage × candidate density = density` is exact by the `LUSH` construction, so area × density is the count).

| Field | Blades at 140/m² | Against the 262,144 buffer |
| --- | --- | --- |
| `meadow`, 18 × 18 m | 45,360 | 17% |
| the largest field that fits | 43 × 43 m | 100% |
| **256 × 256 m** | **9,175,040** | **3,500%** — 440 MB |

`warn_if_grass_truncated` fires and the field is dropped in generation order, which is z-major, so **the user gets a straight horizontal edge across the middle of their landscape** and an `"ok": true` render. The warning exists precisely because this failure used to be silent (`main.rs:1683`).

**And the CPU is already baking twenty-five times what the GPU draws.** `GRASS_FAR = 55.0` (`scene.slang:3390`): every blade past 55 m has been shrunk to a point in the vertex shader. A 256 m field uploads 9.17M blades so that the ~1.33M within the visible disc can be drawn. **The single largest optimisation available is to stop generating what the shader is already deleting**, and it is available because a blade is a pure function of its coordinates.

### 7.2 Mesh scatter fits comfortably, which is the surprise

Matérn type II limiting intensity is `1/(π r²)` with `r = spacing`, stated and pinned by a test in the crate (`loom_scatter/src/lib.rs:38-43`). Over 65,536 m²:

| `spacing` | Instances at 256 m | Against `MAX_OBJECTS = 4096` |
| --- | --- | --- |
| 8 m (trees) | **326** | 8% |
| 4 m (bushes) | **1,304** | 32% |
| 2 m (rocks) | 5,215 | 127% — the buffer grows; the TLAS is the question |

So **painting a forest across a 256 m landscape is a solved problem today** and painting grass across it is not. The 2 m row is the first place a new limit is met, and the right response is a measurement (§7.5), not a design.

### 7.3 The interactive cost, which is the one that will actually hurt

`SceneView::build_cached` calls `scatter_objects` unconditionally (`scene_view.rs:118`), and the field comment records the measurement: **103 ms on `forest.loom`**, which is 9 fps and was reported by a human pressing Play. The comment argues the result is cached for the `SceneView`'s lifetime and a file change builds a new one — which is true, and **a paint stroke *is* a file change**.

So today, every foliage stroke commit costs a full re-place of every scatter field in the scene plus a full grass rebake. That is the thing that makes this feature unusable, and it has two fixes, both of which are patterns already in the repo:

1. **`scatter_key`, in the shape of `grass_key`** (`main.rs:1633`): a string of every `Scatter` component, its node's world translation, every `VoxelVolume`, and the paint hash. Equal key, keep the cached instances. Painting field A stops re-placing field B, and a transform edit elsewhere costs nothing. ~40 lines, mirrors an existing function line for line.
2. **`reach_of`-sized dirty regions.** A stroke's bounding box, grown by `reach_of(layers)`, is the region to re-resolve; everything outside is provably unchanged. The function exists, is documented, is tested, and has no caller. This is the payoff for the whole order-independence argument the crate opens with.

For grass the same shape: `grass_blades` returns blades **grouped by tile** (`BTreeMap<(i32,i32), Range<u32>>` alongside the `Vec`), and a stroke regenerates only the tiles its bounding box touches. Recompaction and a whole-buffer re-upload is the lazy path — at 500k blades that is a 24 MB memmove plus 1.8 ms of PCIe at the measured 13.5 GB/s, call it 4 ms, **paid on mouse-up and not during the drag**. *Upgrade path if 4 ms proves visible: fixed per-tile slots (a tile's candidate count is `side²`, deterministic from `density` before generation) and a partial upload, at ~38% wasted slots.*

### 7.4 Hoist the `GroundGrid`

`grass_blades` bakes a `GroundGrid` per `Grass` node (`main.rs:1928`) and `scatter_objects` bakes one for the union of all `Scatter` nodes (`main.rs:1814`). With species-as-nodes, six grass species over one terrain is six marches of the same SDF at 0.11–0.14 s each. **One grid per `(volume, region)` pair, shared by every field over it**, cached beside `VoxelCache`. The grid is already `loom_voxel::heightfield::HeightField`, already shared with water, and already exists for exactly this reason.

### 7.5 What to measure before building any of the above

Three numbers, none of which exists:

- **A `scatter` row in the frame telemetry**: instance count and TLAS rebuild milliseconds. `LOOM_GPU_TIMING` prints `graph` and explicitly excludes the TLAS rebuild as a separate submit (`renderer.rs:2372`), so the cost of 5,000 scattered instances is currently unknown in both directions.
- **`grass_blades` wall time per tile** at density 140, which sizes the dirty-region rebuild.
- **`Session::apply` wall time on `proving_ground.loom`** with a 200-point stroke in the array — doc 03 §2 asks for this and it has not been taken.

### 7.6 The streaming answer, when it is wanted

Painting grass across a landscape is the forcing function for the deferral CLAUDE.md records: *"they will be wanted when placement has to be dynamic — a field larger than one CPU bake, streaming as the camera moves — which is a scale argument, not a cost one."* The scale argument has now arrived, and **it does not need the compute pass.**

> Generate only the tiles within `GRASS_FAR + margin` of the camera, and **pre-apply the shader's own cull on the CPU**: `grassCullDraw(blade) < falloff(distance)` (`scene.slang:3223`). The Slang uses `loom_hash`, which is `loom_field::noise::hash` from the generated `fields.slang` — **frozen ABI, written in Rust and Slang side by side and compared exactly by the S2 agreement test**. So the CPU can reproduce the test bit for bit, and if it uses a conservative distance (one tile short) it can only ever be a strict subset of what the GPU keeps. **No shader changes, and the two cannot disagree.**

Area-weighted, the falloff keeps an effective 3,695 m² of a 55 m disc, so a 256 m field's resident set is **517,000 blades at 140/m²** (369,000 at 100) against 1.33M without the pre-cull — a 2.6× saving that costs one function. `MAX_BLADES` then wants **524,288 (25 MB)**, a doubling, and the truncation warning stops being reachable by any plausible field.

The ring updates on a camera tile crossing — every 4 m of travel, roughly once a second at a walk — and each update regenerates a handful of tiles. This is the crater path with a moving crater.

**This is ADR 0035 and it is a separate slice with its own gate.** Slices 1 and 2 ship inside today's ceiling with the ceiling made visible (§8), because a 43 m painted meadow is a real deliverable and the streaming work should be paid for by someone who has actually painted past it.

---

## 8. The budget meter, because the ceiling must be visible before it is hit

The Foliage palette shows, per field and live: **`45,360 / 262,144 blades · 17%`**, and for scatter **`326 / 4,096 objects`**. Over 100% the row turns and says what will happen in words — *"this field needs 9,175,040 blades and the buffer holds 262,144; the far half will be cut off in a straight line. Lower `density` to 4, or `half_extent` to 43."* — with the two numbers computed, not described.

This is thirty lines and it converts the one failure a foliage painter is guaranteed to hit from a silent corrupted render into an arithmetic problem with the answer printed.

---

## 9. Sculpting under painted foliage

Three behaviours, and two of them are already free.

**Height follows, on mouse-up.** `grass_key` includes every `VoxelVolume` component (`main.rs:1641`), so a sculpt transaction changes the key and the field regenerates against the new `GroundGrid`. **Grass deliberately does not follow during the drag**: a sculpt preview is about the surface, and 40 ms of blade regeneration per frame under a moving brush buys nothing anyone can see. Regenerate on stroke release, dirty-region only (§7.3), which is also when the sculpt becomes a transaction at all.

**A hole clears its grass for free.** `GroundGrid::at` returns `rock = 1.0` where there is no surface (`main.rs:1508`), `soil` goes to zero, the product goes to zero. No floating blades, no special case, and the same query answers it for scatter.

**Sculpting a painted hillside into a cliff deletes the grass on it, and that is correct and surprising.** The mask is in world XZ and does not move; the *rule* now returns zero there. The editor says it once, in the same words §4's refusal uses: *"Grass cleared from ~12 m² — that ground is now steeper than `slope_cutoff`."* One message, reused, and it is the moment the multiplicative model teaches itself.

The mask never needs remapping under a sculpt because sculpting changes height, not ground plan. **If a future tool ever moves terrain laterally, this breaks**, and the note belongs in §13 rather than in a defensive mechanism nothing needs yet.

---

## 10. The agent's half

Everything above is a component field, so `loom scene --tx` authors all of it the day the components exist — property 2 satisfied with no new verb, exactly as doc 03 §2 argues for splat. `SpliceArray` (ADR 0026) appends a stroke or a `remove` point without rewriting the array.

Two things are worth adding, and only two:

- **`loom foliage stats <scene>`** — per field: blades or instances, the buffer budget, and the fraction of the field's area with any authority. This is how the agent *verifies* that a paint landed, which is the difference between authoring and guessing, and it is the natural home for the assertion a gate wants.
- **`loom validate` warns** when `FoliagePaint` sits on a node carrying neither `Grass` nor `Scatter`. A mask with no consumer is silent and invisible — the exact class of failure the prefab load-path bug (PLAN Stage 0) belongs to.

A `loom foliage paint --node --at --radius --value` convenience appends rather than replaces, and is worth adding the first time an agent gets the replace-the-whole-array dance wrong — not before.

---

## 11. A verified correction to ADR 0027 that will otherwise fail CI

ADR 0027 (S4) says: *"one `BrushParams { radius_m, hardness, strength, flow, spacing }` embedded in every stroke type"*, and doc 03 §14 puts `SplatPaint`/`SplatStroke` in `crates/loom_scene/src/components.rs` while `BrushParams` lives in `loom_asset::paint`.

**That combination cannot compile under the dependency rules.** Verified: `scripts/check-deps.sh:26-31` fails the build if `loom_scene` depends on anything but `loom_reflect`, and `crates/loom_scene/Cargo.toml` lists exactly `blake3`, `loom_reflect`, `schemars`, `serde`, `serde_json`, `toml_edit`. A component embedding a type from `loom_asset` makes `loom_scene → loom_asset` a workspace edge, and green check 1 fails the day it lands.

**Resolution: `BrushParams` lives in `loom_scene::brush`, because it is authored state.** It is serialized into the file, schema-validated, and shown in an inspector; that is the definition of a scene type. `loom_asset::paint` — the rasteriser — gains `loom_scene` as a dependency to read it, which is a new edge, legal under every rule, and the direction PLAN §2.1's own diagram already draws.

*Alternative rejected:* `loom_asset::paint::stamp` taking five scalar arguments and no shared struct. It avoids the edge and re-lists five fields at every call site, which is the layout-described-twice hazard this project names in four places. The edge is cheaper.

---

## 12. ADRs, files, and where this belongs in the plan

### 12.1 New ADRs

**ADR 0033 — A painted foliage mask multiplies the placement rule; it never overrides it.**

> A `FoliagePaint` component carries a stroke list per field, rasterised on load into a CPU-only mask of value and authority. `loom_grass::Ground` and `loom_scatter::Ground` each gain `paint: f32`, default `1.0`, supplied through the closure that already exists — neither crate gains a dependency. `coverage` and `viability` multiply by it. Untouched ground is `lerp(1, v, 0)` = exactly `1.0` and bit-identical; erased ground is exactly `0.0`; and because both rules are products containing a slope and a rock term, **no stroke can place foliage where the rule forbids it** — `slope_cutoff` keeps its documented meaning and `grass_thins_on_a_slope_and_stops_on_rock` and the crater test pass unmodified. `value` is clamped to `LUSH` (1.6), so painting borrows the existing gully headroom and can never exceed the authored `density`, nor violate `Scatter.spacing`. The mask is never uploaded: no bindless slot, no `paint_upload` pass, no barrier, no `ObjectData` field, **no shader change of any kind**. Stroke edges are broken up by modulating the dab radius with frozen low-frequency noise at bake time — not by perturbing the sampled gain, which would break either erase-exactness or unpainted-identity.
>
> *Rejected:* ADR 0028's `lerp` form (it lets a stroke override a hard cutoff, failing an existing test and reopening the floating-blades hole); a mask that *is* the coverage (a procedural rule frozen into a bitmap — never-do #11 with a different noun, and it would make `slope_cutoff` inert on painted ground); a `paint` term applied at `habitable`/`kept` separately rather than inside `viability` (an erased region would thin its own fringe, the shaved-ring artifact from a third direction); a GPU-side mask (placement is CPU work; uploading it would buy nothing and cost a descriptor).

**ADR 0034 — A species is a node; a hand-placed instance is a node; a removed instance is a point.**

> Foliage is stored as three tiers and the instances are never written. The rule is `Grass`/`Scatter`; the mask is `FoliagePaint.strokes`; the exceptions are (a) hand-placed instances, which are ordinary scene nodes spawned by `SpawnNode`, and (b) deletions, which are `Scatter.remove: Vec<[f32;2]>` world-XZ points killing every instance within `spacing * 0.45`. That radius is provably at most one instance, from the crate's guaranteed minimum separation, and its bounded reach adds one term to `reach_of` so dirty-region regeneration stays bit-identical to a full rebuild. Dragging a generated instance is `SpliceArray` into `remove` plus `SpawnNode`, **in one transaction and one Ctrl+Z**. A species is a node rather than an array entry, so it inherits naming, selection, `Material`, prefab overrides and undo; the editor's Foliage palette is a filtered view of the hierarchy, not a second model. Grass gets no `remove` list — erasing a blade is meaningless and the mask is the eraser. `Scatter` gains `align: f32`, default `0.0`, which is byte-identical today and is the surface-normal alignment `scatter_objects` currently has no code for at all.
>
> *Rejected:* a species array inside one node (a second hierarchy nobody can select in, needing its own naming, reorder op and override semantics); storing removals as cell indices `[ix, iz]` (renumbers when `spacing` changes, so every deletion silently points at a different tree); storing baked instance arrays (never-do #11's shape, and the diff goes dark); an erase brush as the only deletion mechanism (cannot remove one tree from a copse).

**ADR 0035 — Grass generation is camera-centred, and the CPU pre-applies the shader's cull.** *(Needed only when slice 3 is built; drafted here so the slice has somewhere to land.)*

> Blades are generated only for tiles within `GRASS_FAR + margin` of the camera, and each candidate is tested against the same `grassCullDraw(blade) < falloff(d)` the vertex shader applies, using `loom_field::noise::hash` — the frozen ABI the S2 agreement test compares exactly — at a conservative distance one tile short, so the CPU's survivors are provably a subset of the GPU's and no blade can pop. `MAX_BLADES` rises to 524,288. **No shader changes.** The ring is regenerated on a camera tile crossing, which is the crater path with a moving crater, and it is correct for the reason the crater path is correct: a tile is a pure function of its coordinates. This resolves the "placement compute pass and indirect draw" deferral in the CPU direction — the deferral's own stated trigger was a scale argument, and a compute pass would additionally require hand-porting Voronoi clumping and the position hash into Slang, which is the CPU/GPU divergence ADR 0006 exists to prevent.

### 12.2 Round-1 ADRs that change

**ADR 0028 gains a scope clause.** Its `lerp(rule, value, authority)` form is correct for the ground-layer **blend weight** and must not be read as the general rule for painted masks. Add: *"This form applies to shading weights. A painted mask over a placement probability composes multiplicatively — see ADR 0033 — because a placement rule carries hard guarantees a blend weight does not."* Also record the consequence that grass is now touched by **two** masks — the splat mask through `Ground.rock` and the foliage mask through `Ground.paint` — and that both are multiplicative, so painting rock and painting grass-erase compose without an ordering question.

**ADR 0027 gains one consequence and one correction.** The consequence: a third stroke type, `FoliageStroke`, sharing the brush, the falloff and the dab walker, whose raster is never uploaded. The correction is §11 — **`BrushParams` must live in `loom_scene`, not `loom_asset::paint`**, or `check-deps.sh` fails.

**ADR 0026 gains two callers** to its list: `FoliagePaint.strokes` and `Scatter.remove` are both `SpliceArray` targets. No change to the decision.

### 12.3 Files touched

| File | What |
| --- | --- |
| `crates/loom_scene/src/brush.rs` | new — `BrushParams` (§11) |
| `crates/loom_scene/src/components.rs` | `FoliagePaint`, `FoliageStroke`; `Scatter.remove`, `Scatter.align`; two lines in `registry()` (`:1713`) |
| `crates/loom_grass/src/lib.rs` | `Ground.paint`, one factor in `coverage`, one test |
| `crates/loom_scatter/src/lib.rs` | `Ground.paint`, one factor in `viability`, the `remove` filter in `region_on`, one term in `reach_of`, two tests |
| `crates/loom_asset/src/paint.rs` | the foliage baker + `FOLIAGE_EDGE_BREAKUP`; `loom_scene` dependency |
| `crates/loom_cli/src/main.rs` | `GroundGrid` carries the mask and fills `Ground.paint`; `grass_blades` returns tiled output; `scatter_objects` honours `remove` and `align`; `scatter_key`; hoisted grid cache |
| `crates/loom_cli/src/scene_view.rs` | `scattered` becomes keyed (`:71`) rather than whole-lifetime |
| `crates/loom_editor/src/tools/foliage.rs` | new — the brush tool, one `Tool` variant, `Outcome::Edit` only |
| `crates/loom_editor/src/panels/foliage.rs` | new — species palette, brush settings, budget meter |
| `crates/loom_render/src/{renderer,viewer}.rs` | `MAX_BLADES` — **slice 3 only** |
| `assets/test/foliage.loom`, `foliage_mesh.loom` | new gated scenes |
| `xtask/src/main.rs` | `SCENES` 48 → 50, `GOLDEN` 32 → 33 |

**Shader entry points touched: none.**

### 12.4 Where this belongs in PLAN.md

**A new Stage 7½ — Foliage, between voxel sculpting (7) and UV painting (8).** A decimal rather than a renumber, because renumbering 8 and 9 invalidates every cross-reference in a 778-line plan for no gain.

It depends on:

- **Stage 1** — `SpliceArray` (both arrays), the inspector (the rule fields are edited there far more often than the mask is painted).
- **Stage 4** — `cursor::under_cursor`'s three-tier raycast, `Tool`/`Outcome`, `COMMANDS`, the palette.
- **Stage 6** — `loom_asset::paint`'s `BrushParams`, falloff, dab walker, `stamp_incremental`, the paint gesture contract (§2.5) and `incremental_painting_equals_a_full_rasterisation`. **The whole mask mechanism is Stage 6's; this stage is its second consumer.**
- **Stage 7** — sculpting, for §9's "sculpt under painted grass and it follows" criterion.

It is independent of Stage 8 (UV painting) and Stage 9 (Windows, docs).

**One piece can be pulled forward into Stage 6 for three lines** if the schedule prefers: `Ground.paint` and the `coverage`/`viability` factor land beside ADR 0028's `Ground.rock` hook, which is the same closure and the same file. The tools, the palette, the `remove` list and the streaming stay here regardless.

**Slices, in order, each ending somewhere runnable:**

1. **The mask and the grass brush.** `FoliagePaint`, the baker, `Ground.paint`, the tool, the palette, the refusal message, the budget meter, `foliage.loom`. Runnable: paint a meadow into existence with the mouse and erase a path through it.
2. **Mesh foliage.** `Scatter.remove`, `Scatter.align`, place-one, detach-and-move, `scatter_key`, `reach_of` dirty regions, `foliage_mesh.loom`. Runnable: paint a copse, delete the tree in the doorway, drag one two metres.
3. **Streaming (ADR 0035), gated on §7.5's three measurements.** Runnable: a 256 m painted landscape.

### 12.5 Gates

All four green checks, unchanged in number. `foliage` and `foliage_mesh` join `SCENES`; `foliage` joins `GOLDEN`.

**`foliage` in `GOLDEN` is an extension of the stated rule and is admitted as one.** It covers no new *rendering* path — it draws through `grassVertexMain` like `meadow`. It covers a new *placement* path, and a golden image is the only gate that can see the mask fail to reach the placement at all. The unit tests cover the arithmetic; the reference covers the wiring, which is where `grass_blades` passing a flat constant `Ground` for two slices went unnoticed.

**Every existing reference must be unmoved**, which is the check that `x * lerp(1, v, 0)` is exactly `x`. A moved reference is a bug in the factor, not a bless.

`cargo xtask flythrough` matters more than the still here, for the reason it always does: a painted boundary is a curve in a density field, and whether §2.3's break-up is enough to stop it reading as a mown edge is a motion judgement no still frame makes. `cargo xtask shimmer` on `foliage` must not be worse than `meadow` at the same density, colour and lighting — **and never compared across a change in any of those** (ADR 0010).

---

## 13. What I could not verify

- **No `cargo` command was run** (design phase, and parallel builds have frozen this machine twice). Nothing here has been compiled, and every cost claim below the ones marked *measured* is arithmetic.
- **The TLAS rebuild cost per instance is unknown**, in both directions. `renderer.rs:2372` states it is a separate submit outside the graph timing, so §7.2's "1,304 instances is fine" rests on the object buffer and the instanced-draw batching, not on the acceleration structure. §7.5 asks for the measurement first.
- **`grass_blades` per-tile wall time is unmeasured.** The 4 ms mouse-up figure in §7.3 is 24 MB of memmove plus 24 MB at the measured 13.5 GB/s PCIe rate, and it assumes tile generation is small beside those. It may not be.
- **The area-weighted falloff integral (3,695 m²) is analytic**, computed from `GRASS_NEAR = 8` and `GRASS_FAR = 55` assuming the kept fraction is linear in distance. The shader's `saturate` makes it piecewise linear, which I integrated, but I did not check it against a blade count from a real render.
- **`meadow`'s 45,360 predicted against 45,460 measured** differ by 0.2%. I believe the difference is boundary blades excluded by the `half_extent` clamp in `grass_blades`, but I did not run it.
- **I did not verify that `loom_field::noise::value` is exposed on the CPU side in the form §2.3 needs** — `coverage` calls it, so it exists, but I did not check whether the baker's call site can reach it without a new dependency on `loom_field` from `loom_asset`. If it cannot, the break-up moves into `loom_cli`'s baker call or `loom_asset` takes the edge.
- **Whether `Scatter.align` is visually right at `1.0` on a 20° slope is a judgement nobody has made**, because the code does not exist. Trees plumb-vertical on a hillside and trees fully normal to it are both wrong; the usable value is probably 0.3–0.5 and that is a human's call at the end of slice 2.
- **The 2 m-spacing row of §7.2 (5,215 instances) exceeds `MAX_OBJECTS`'s initial 4,096.** `reserve_objects` grows the buffer, and I read the growth path, but I did not check whether the TLAS instance buffer grows with it.
- **`texels_per_meter = 2.0` against `CLUMP = 0.5` is reasoning, not measurement.** A mask finer than a clump controls something invisible; whether a mask *coarser* than 2/m is visibly blocky at a stroke edge, given §2.3's break-up, is unknown.
- **The mask does not survive a lateral terrain move.** Nothing moves terrain laterally today. If a future tool does, painted foliage will be left behind and this document does not say what should happen.
- **Doc 03's `SplatPaint` and this document's `FoliagePaint` can coexist on one node**, and the composite is well-defined (they touch different terms), but I did not check whether `loom validate`'s S16 warning about `SplatPaint` + `PaintLayer` should extend to this pair. I believe it should not — they answer different questions — but that is an opinion.
