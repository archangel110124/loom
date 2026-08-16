# Advanced shapes and landscapes in Loom — research report

*Verified against the tree at `62f9ebe` + `a96f809` (`feat(voxel): 3D noise, and primitives that can be displaced by it`). Every claim marked **[verified]** was checked in the source this session; **[measured]** was measured on this machine; **[derived]** is arithmetic from a measured number.*

---

## 1. The one-paragraph answer

Loom's shape vocabulary is not limited by its CSG, its mesher, or its voxel resolution — it is limited by **frequency content and orientation**. Four analytic primitives combined with a hard `min` produce a field that is smooth everywhere except at creases, so every shape has detail at exactly one scale (the size of the pieces you placed) and every joint is infinitely sharp. `Displace` (landed in `a96f809`) fixes the frequency half for *objects*: one ridged-displaced sphere scores 49.2% concave surface area against a photogrammetry boulder's 55.8%, where a 49-primitive union blob scores 8.9%. What it does not fix is **landscape**, and the reason is not missing technology — it is a disconnected cable. `loom_terrain` contains a complete, tested, deterministic terrain pipeline (fBm, ridged, domain warp, spline carve, flatten disc, peak, corridor guarantee, hydraulic and thermal erosion, content hashing, slope/buildable/reachability analysis) and **nothing in `loom_voxel` calls any of it** [verified] — `VoxelOp::Heightfield` reaches straight past the recipe system for `loom_terrain::noise::fbm` at `lib.rs:331`. So `valley.toml` is an eroded landscape that cannot be placed in a game, and every voxel landscape in the library is un-eroded fBm. Secondarily, `Box` has no rotation [verified], which is why every building, quay, shed and plinth in the library is axis-aligned — the author had no choice. Fix the cable and the rotation and Loom can author landscapes today; almost everything else on the researchers' list is either already there, or is a 25–260× bake cost for a smaller gain.

---

## 2. Ship first

### 2.0 — Prerequisite: the half-day that makes the rest safe

Do not skip this. It is three small commits and two of the three are live defects *right now*, since `Displace` landed.

**(a) `Volume::edit` does not apply `lipschitz()`.** [verified] `bake` at `lib.rs:671` multiplies its early-out threshold by `max_ops(lipschitz)`; `edit` at `lib.rs:805` tests the raw chunk radius:

```rust
// crates/loom_voxel/src/lib.rs:805 — today
if op.distance(centre).abs() > chunk_radius { continue; }
// the fix
if op.distance(centre).abs() > chunk_radius * op.lipschitz() { continue; }
```

The contract is: for `|∇f| ≤ L`, a chunk of bounding radius R is provably surface-free only when `|f(centre)| > L·R`. A displaced sphere used as a runtime carve — `loom place --op` — skips chunks that hold surface, and the symptom is an un-remeshed chunk at the crater edge: a floating slab or a seam. Measure the ~0.1 ms crater before and after; the second per-chunk distance filter still rejects most of the widened span, so the regression should be small.

**(b) The noise gradient constant is understated for the 3D path.** [measured, this session] `Displace::gradient_bound()` and `VoxelOp::lipschitz()` both use `3.0` as the per-octave slope bound, with a comment saying "the smoothstep used to interpolate it peaks at 1.5, over a range of 2". That is correct for `loom_field::noise`, which uses Hermite smoothstep `3t²−2t³` [verified, `loom_field/src/noise.rs:83`]. It is **not** correct for `loom_terrain::noise`, which interpolates with *smootherstep* `t³(6t²−15t+10)` [verified, `noise.rs:57`], whose derivative peaks at **1.875**, giving 3.75 per axis over a `[-1,1]` range. Measured max `|∇value3|` over 24 seeds and a 61³ grid inside one cell:

```
max |grad value3| = 3.249      <- 3D, used by Displace          constant in use: 3.000
max |grad value|  = 2.409      <- 2D, used by Heightfield
```

So the heightfield is safe with room to spare, and **the displacement path is already understating its own bound by at least 8% on a sampled maximum**, with an analytic per-axis worst case of 3.75. Under-stating is the hole-punching direction — the exact failure whose measurement is already in the tree ("across seven terrain configurations an unwidened early-out wrongly skipped between 9 and 44 chunks each"). Change the constant in `Displace::gradient_bound` to `4.0`, leave the heightfield's at 3.0, and note in the comment that the two noises have different interpolants. Cost is bake time only.

**(c) Two gates cannot see what they claim to.** [verified] `filter_map(|v| serde_json::from_value(v.clone()).ok())` at `loom_cli/src/main.rs:1926` and `:3525` **silently drops an op it does not recognise** — the volume bakes short, produces a plausible surface, and validates clean. This is the prefab defect verbatim, and it is worse under a cost lens: a dropped op makes the bake *faster*, so a performance suite reports an improvement on the commit that broke the scene. One shared `parse_ops() -> Result<Vec<VoxelOp>, …>` at both sites, hard-erroring with the offending `kind` named. Separately, `determinism_holds` hardcodes `assets/test/tower.loom`, which contains **zero** `VoxelVolume` nodes [verified], and `b478ea4ac2622d32` appears in **zero `.rs` files** anywhere in the repo [verified] — it lives only in prose. So every "the sim hash is unchanged" claim in the proposal set is unverified, not verified. Add a small voxel scene (`rain_gantry`, [2,1,2] = 8 chunks) to the determinism gate; 8 chunks proves bit-reproducibility as well as 2048 and costs ~50 ms of debug bake.

**Cost:** hours. **Verified by:** bake a volume with the early-out enabled and again with it forced off, assert the fields are byte-identical — with a displaced op and with two of them. That formulation, not sign agreement, is the one that catches a wrong bound; the existing test's docstring records that two earlier versions passed with the bug present.

---

### 2.1 — `VoxelOp::Terrain`: wire the recipe into the SDF

**What it is.** One new op kind that names a `loom_terrain` recipe and a world rect. At bake, `Recipe::bake()` runs once into a `Heightmap`; the op then answers exactly like `Heightfield` does, through the same per-column hoist.

```rust
VoxelOp::Terrain { recipe: String, rect: [f32; 4], base_y: f32, mode: CsgMode }

// height_at(x, z) — the hoisted half, one bilinear lookup:
base_y + map.sample((x - rect[0]) / mx, (z - rect[1]) / mz)
// distance(p) = p[1] - height_at(p[0], p[2])     — identical to the Heightfield arm
```

**Where it goes.** `crates/loom_voxel/src/lib.rs` — one enum variant, one arm each in `distance`, `bounds`, `lipschitz`, `height_at`; plus the shared `parse_ops` from 2.0(c), plus `loom terrain <scene.loom>` in `loom_cli/src/main.rs:2955` resolving a scene's recipe op rather than only a bare `.toml` path.

**What it unlocks.** Everything `loom_terrain` already does, in a playable 3D world: domain-warped ridged mountains, spline-carved valleys, flattened build pads, a **corridor traversability guarantee**, hydraulic gullies and thermal talus. And the thing that matters more than any of it — **the only assertable feedback channel a landscape has in this engine**. `loom terrain` reports `buildable_pct`, `slope_mean`, `slope_over_45_pct`, `largest_flat` and a boolean `reachable` [verified, `main.rs:2993–3025`]; `loom measure` reports node bounding boxes and overlaps and nothing else. First-try correctness follows from assertable output, and only one of the two authoring models here has any. "A gorgeous mountain range with 3% buildable ground is a failed level, and nothing in a hillshade reveals that" is already written in the CLI's own docstring.

**What it costs.** Recipe bake, measured: **231 ms at 256²** (6 noise + 183 hydraulic + 42 thermal), **417 ms at 512²** (25 + 198 + 193). The voxel bake itself gets *cheaper* — one bilinear sample (~5 ns) replaces five octaves of fBm (~100 ns) in the hoisted column, so `terrain_stress`-class bakes drop from 118.6 ms toward ~100 ms [derived]. The gate pays 231 ms × the scenes that adopt it; the image gate currently runs 22.5 s over 26 golden scenes [measured], so a handful of adopters is +5–10%.

Two cost facts the researchers disagreed about, and my reading: one proposed "size the recipe to the feature, not the voxels" as the cheap fix; the cost lens measured that **hydraulic erosion is essentially flat in map size** (175.8 ms at 128² to 215.4 ms at 1024²) because it is O(droplets), while thermal is linear in cells and base noise is quadratic in side. So sizing down from 512² to 256² saves ~186 ms of thermal and noise and **~0 ms of hydraulic**, which is 79% of the pass at 256². The cost lens is right. Size the recipe to the feature *and* accept ~230 ms, or cache — but the cache is the thing that carries the determinism risk, so prefer 256² and no cache until `loom run --watch`'s 4 Hz re-bake actually hurts.

**How it is verified.**
- The world-rect mapping is the silent failure: the recipe's `world_scale` and the volume's extent are two independent coordinate systems, and getting it wrong renders a plausible landscape at the wrong scale. Assert at load that the rect covers the volume's XZ footprint, naming both numbers, the way `Recipe::from_toml` names the MB when it rejects an oversized map.
- **Do not put the content hash in the scene file as an authored field.** An agent cannot compute a blake3 without shelling out, so it goes stale on every recipe edit and the outcome is either an unfixable hard error or a silent wrong bake. `Recipe::content_hash()` already exists; derive it at load, use it only for caching.
- `lipschitz()` becomes measured rather than derived — sweep the baked map's gradient. **Take a high percentile (99.9) plus a safety factor, not the raw max**: one spline-carved cliff cell sets the max, and a raw max of 5 takes `terrain_stress`'s reach from 12.8 m to 34 m, admitting all 2048 chunks instead of 854 — a 2.4× on the whole bake, from the pass meant to speed it up. Also note the sample is bilinear between grid cells, whose gradient can exceed the max cell-to-cell slope.
- Outside the rect, **fade toward `height_range[0]`**, not `Heightmap::get`'s clamp — clamping extrudes the boundary row to infinity, giving infinite parallel ridges at the frame edge in any orbiting flythrough.
- Byte-identity test: bake with the early-out and with it forced off.

---

### 2.2 — Yaw, rounding, elongation (three one-liners)

**What it is.** Three transforms that cost 2–6 flops each, are exactly distance-preserving or exactly offsetting, and need no change to `accumulate`, the mesher, the collider or `reach`.

```
// yaw — hoisted per op, not per voxel
cy = cos(-yaw); sy = sin(-yaw)
d3 = p - center
q  = [cy*d3.x - sy*d3.z, d3.y, sy*d3.x + cy*d3.z]

// bounds(), closed form, no corner loop
ex' = |cy|*ex + |sy|*ez        ez' = |sy|*ex + |cy|*ez

// rounding — and shrink the source extents by r, or the box silently grows
round(d, r)    = d - r
// elongation — sphere(elongate(p,[h,0,0]), r) IS a capsule, exactly
elongate(p, h) = p - clamp(p, -h, h)
```

**Where it goes.** `VoxelOp::distance` / `bounds` in `loom_voxel/src/lib.rs`, `#[serde(default)]` on each field so every existing `.loom` is byte-identical.

**What it unlocks.** A wall that is not axis-aligned. Loom's vocabulary is inconsistent today in a way an authoring agent will trip over: `Capsule` gets arbitrary orientation free from its two endpoints, so the engine can angle a *tunnel* but not a *wall*. `lanternhead`'s whole built environment is square because of it. Rounding gives a fillet on a single primitive for one number (which is what most people actually want when they reach for a smooth minimum — see §4). Elongation turns four primitives into a much larger family for zero new variants: a stadium prism, a capsule-loop, a slab with rounded ends.

**What it costs.** Yaw ~1.5 ns/op-voxel against a 4.4 ns baseline [measured], and zero if you branch on `yaw != 0.0`. Rounding and elongation are unmeasurable. No `lipschitz()` change. No reach change.

**How it is verified.** The one thing that breaks, and breaks quietly, is `bounds()`. `Volume::edit` culls chunk spans by it, so an under-computed AABB makes a runtime crater miss chunks entirely — a seam or a floating slab at the crater edge. **Property-test over randomised yaws and randomised extents**, asserting containment of transformed support points; testing eight hand-listed corners at yaw 0 and 45° (the two values anyone writes by hand) passes trivially. Compose with the displacement grow already in `bounds()` rather than replacing it, and assert `height_at()` returns `None` for every rotatable variant.

**Amendment worth taking:** name it `yaw_degrees` and convert at parse. `ops` is `Vec<serde_json::Value>` with no field-level schema [verified, `components.rs:323`], so `yaw = 45` meaning degrees where the code reads radians is a factor of 57 with no error and a plausible-looking render. Put the unit in the file the agent writes, not the docstring it may not read.

**Skip onion (`abs(d) - t`).** It only means "shell of thickness t" on a *true* distance field, and the schema gives an agent no way to know which ops are true distances — the heightfield, and anything displaced, are bounds. Its worst case (onion-then-subtract carving two concentric surfaces) reads as a mesher bug from inside a hill and is reachable by no gate here.

---

### 2.3 — Calibrate `Displace`, and give it a number to be judged by

**What it is.** Two hours of rules plus a report-only measurement tool. `Displace` already exists and works; what is missing is any way to pick its four numbers other than render-and-look, which is the worst iteration loop an agent can be given.

The calibration, measured:

```
A ≈ 0.25·R      f ≈ 0.6/R      ridged = true
o* = ceil(log2(A / voxel_size)) + 2          <- cap octaves here
```

Concave-area fraction on a 3 m boulder at `voxel_size = 0.03`, by octave count: **×3 39.6%, ×4 42.2%, ×5 44.8%, ×7 46.2%** — and `o*` for that configuration is exactly 5, where the curve flattens. Cost of ignoring it, measured on this machine: 4 octaves **289.1 ms**, 7 octaves **531.2 ms** (+84%) for +4.0 points. At `voxel_size = 0.02`: 634.9 vs 1269.1 ms. And the saving is larger than the table shows, because `gradient_bound` grows **linearly** in octaves (lacunarity 2, gain 0.5 ⇒ every octave has the same `amp·freq` product), so capping octaves also caps `reach`, which shrinks the admitted-chunk count on top of the per-voxel cost.

The tool: `loom measure --shape <scene|obj>` printing **concave-area fraction** and **log₁₀|H| percentiles** from the cotangent Laplacian (`H(v) = |Δ_cot x(v)| / (4·A_voronoi(v))`, signed by `sign(Δx·n)`, normalised by mean radius). The separating measurement:

```
photogrammetry boulder (rock_boulder_a.obj)   concave 55.8%   spread 1.33 decades
one ridged-displaced sphere                            49.2%          1.48
49-op union + fbm displacement                         29.4%          1.18
49-op union, hard min (today)                       8.9–12.4%    0.41–0.56
```

The radial power spectrum does **not** separate these (log-log slopes −1.99 / −1.82 / −2.02); curvature sign and spread do, decisively.

**Where it goes.** `Displace::at`'s docstring and a clamp-with-warning at load for `o*`; the metric in `xtask` beside `shimmer`/`flythrough`, or as `loom measure --shape`.

**What it costs.** Hours. The metric is ~2 ms on a 62,660-triangle boulder, 10–30 ms on `terrain_stress`, several hundred ms on `terrain_billion` [derived].

**How it is verified — and the one hard rule.** **Report-only. Never a fifth green check with a threshold.** Curvature on a surface-nets mesh is voxel-size dependent, so a fixed threshold would move every time `voxel_size`, the mesher, or Dual Contouring changes — and a *finer* bake would "win" purely by resolving quantisation noise. Carry "only compare a scene against itself at equal `voxel_size`" in the tool's own output header, exactly the way the flicker metric carries its rule. Emit JSON with scene, node and `voxel_size` so the comparison rule is machine-enforceable.

---

## 3. Worth doing after

Ordered by what has to be true first.

| Item | Prerequisite | Why it waits |
| --- | --- | --- |
| **Parallelise the bake with rayon** | The byte-identical vertex-buffer assertion, written *before* the parallel code | 24 cores, zero threading anywhere in the workspace. `terrain_billion`'s 1938.6 ms bake and 3042.7 ms mesh → ~0.2 / ~0.3 s [measured/derived]. Biggest single lever on the authoring loop, and it is what makes several items above affordable. Collect into a `Vec` indexed by chunk; never `Mutex<Vec>` push — a permuted merge gives a mesh that renders identical pixels and hashes differently, which **no existing gate can catch**. |
| **Parallelise the *gate* at the process level first** | Nothing | Smaller diff than adding a dependency, zero determinism reasoning (each render is already an independent process writing its own PNG), and it takes the 22.5 s image gate to ~4 s. That is what protects the gate from every cost increase on this list. |
| **`droplets_per_cell`, and delete `evaporation`** | 2.1 | `droplets = 50_000` is a *count*, so droplets/cell falls as 1/side²: 3.05 at 128², 0.76 at 256², **0.048 at 1024²**. The same recipe means something different at every resolution, with no error. And `evaporation` provably cannot bind — 1.0 decaying 2%/step to 0.01 needs `ln(0.01)/ln(0.98)` = 228 steps against `MAX_STEPS = 64`. A parameter that cannot affect output is worse than none. Ship with a hard cap on the derived count (~400k, ~1.6 s at 3.97 µs/droplet), since 2/cell at 1024² is 8.3 s. |
| **A mask on every recipe layer** | 2.1 | `loom_terrain`'s own header says terrain generation is "add this, then mask it by that" and the stack has no way to mask anything, which is why one world cannot hold mountains *and* plains. Four keys in the same diff hunk as the layer they modify. **Perturb the threshold, do not widen the band** — that is the lesson P2 slice 9 already paid for. Make it `Option<Mask>` where `None` short-circuits the multiply, and **allow the mask to be a shape** (disc/rect/spline, reusing `FlattenDisc`'s path) — a noise-thresholded mask gives regional *variety* the agent cannot *place*. |
| **Differential erosion (hardness bands)** | 2.1 + `droplets_per_cell` | Three lines in two already-debugged functions gives mesas, hoodoos and scree. Per-band (not per-point) and absolute world Y (not depth) are the two decisions that make it read as a landscape rather than speckle. **The byte-identity claim in the proposal is wrong**: `* (1.0 - 0.9*hardness)` equals 1.0 only at hardness 0, so a "neutral" default silently re-erodes every recipe. Ship as `Option<Strata>` where `None` skips the term. And it needs 2/cell + ~100 thermal iterations to show, which is the 6.6× the proposal did not price — size to 256² (≈670 ms) rather than 512² (≈2.75 s). |
| **Drainage: hoist the one flow accumulation** | 2.1 | `loom_water::flow::accumulate` at `flow.rs:415` is already correct D8 with the diagonal discounted and a deterministic tie-break [verified]. Move it to `loom_terrain` (dependency direction already works) and let both callers use it — two flow implementations is ADR 0006's failure in a non-`Expr` guise. **Lead with the droplet seeding, not the carving**: spawning by `sqrt(accumulation)` costs four lines and makes 50k droplets do the work 500k were being asked for, which directly buys back the item above. Carving by threshold is unaimable (it says how much river, never where) — expose the accumulation in `loom terrain`'s maps first so an agent can *see* where water goes and then place a `SplineCarve`. |
| **Glacial valley profile** | 2.1 | `carve(d) = depth·(1 - (d/width)^p)`; `p=1` V-shaped fluvial, `p=2` the parabola every glaciology text gives, `p≥4` box canyon. The brief's own example — "a mountain range with a glacial valley" — cannot be authored correctly today. `profile = 2.0  # glacial` is a value an agent selects correctly from a docstring; "make the smoothstep steeper" is not. **The default must be *absent*, preserving the existing smoothstep** — the current falloff is `t*t*(3-2*t)` at `lib.rs:492` and `p=1.0` is a *linear* ramp, so "default 1.0 keeps every recipe byte-identical" is false as written. |
| **Anisotropic displacement as a single `bedding` ratio** | 2.0(b) | Squashing the noise domain in Y stretches every feature horizontally and reads as sedimentary bedding — the most recognisable rock cue after silhouette, for one type change. Author it as one ratio, **not a frequency triple**: nothing in the file says which axis is up, so `[f,3f,f]` and `[3f,f,3f]` are interchangeable-looking and mean horizontal bedding versus vertical striping. Note it is free per voxel and **3× on admitted chunks** at the recommended ratio, since `gradient_bound` takes the max component. |
| **A `Plane` half-space, with an extent** | 2.2 | One dot product, exactly 1-Lipschitz, and it finally gives `Intersect` — implemented, correct, used by zero scenes — something worth intersecting with. Cut-plane fracture: a displaced sphere plus 3–6 subtract planes whose normals are spread by a golden-angle sequence (random normals cluster and eat the rock). **Give it an extent rather than unbounded bounds**, or every scene carrying one pays whole-volume cost for every runtime crater. Also: `bake` seeds the accumulator at `f32::MAX`, so an `Intersect` placed *first* is a silent no-op — reject that at load. Surface nets rounds the arris by ~one voxel regardless; that is a Phase 8 (DC) ceiling, not a bug to chase. |
| **One octave of Worley F1 on small ops** | 2.0(a,b) + the metric | The facets ridged value noise cannot make. F1 is *exactly* 1-Lipschitz — it is a distance function — which makes it the safest family for the early-out, with no bound-guessing. But the cost lens is right and the original estimate was low by ~3×: 27 cells × 3 hashes is **81 hashes + 27 distance computations ≈ 230 ns/octave/voxel** against value noise's 20–27 ns. One octave, layered on ridged at ~0.6 amplitude, only on ops whose bounds imply under ~1e6 surface voxels (≈200 ms on a boulder). Compare *squared* distances through the whole reduction, one `sqrt` at the end. Never on a `Heightfield`. And pin it numerically the day it lands. |
| **Surface detail in the shader, with the crack network baked to a texture** | A GOLDEN entry in the *same commit* | Rung one of the ladder and the reason belongs on record: at 0.045 m voxels surface nets cannot represent a feature below ~0.18 m, so the bottom two octaves of "stone" are not geometry at any sane bake cost. Triplanar fBm normal perturbation costs zero bake, zero chunk memory, zero determinism surface. **But drop the runtime Worley**: 81 hashes per fragment at 2.07M pixels is 1–3 ms against a forward pass measured at 0.05–0.11 ms for *every scene in this project*. Bake the crack network to a tiling texture — one fetch per projection. Mip-style frequency falloff is not optional, and judge it on `shimmer` **at the scene's authored camera**. |
| **Nyquist warnings** | Nothing, but test against the boundary scenes first | Octave *i* is representable only while `amplitude_i > voxel_size` and `wavelength_i > 4·voxel_size`. Emit a validate warning naming the first octave below the limit **and the milliseconds it costs** — a warning with a millisecond attached gets acted on, one naming a wavelength gets read past. Same for the two-voxel minimum feature rule, which is currently written in prose in two scene files and checked by nothing. A warning that fires wrongly on `terrain_stress` (octave 4 at 0.258 m against 0.25 m voxels) trains the agent to ignore the channel, so assert silence on both boundary scenes as a test. |
| **LOD by re-baking coarse, per *group* of chunks** | rayon; an explicit collider-volume choice | Neither deferred Phase 8 item (DC, LOD octree) is a prerequisite: LOD levels come from re-baking the same op list at `voxel_size · 2^k`, which costs nothing to invent because never-do #11 already made the recipe resolution-independent, and cracks are fixed by a one-coarse-voxel skirt (~15 lines), not stitching. **But do not go to one draw per 32³ chunk** — `terrain_billion` has 13,670 surface chunks, which at 1–2 µs of CPU per draw is 14–27 ms against a 16.7 ms budget; the merge being "deleted" is the only reason that scene renders. Merge into 4×4×4 groups: 214 draws, frustum culling still useful. And `scene_volume()` returns the **first** `VoxelVolume` node found [verified, `main.rs:1219`], so which tier the collider reads is decided by node order in the file — fix that in the same commit or a player falls through a hillside. |

---

## 4. Rejected, and why

**Smooth minimum, as a rock tool — rejected on measurement. As a joint fillet — unproven, and the two researchers who fought over it measured different things.**
Proposal 44 measured the *exponential* form `-k·ln(Σ exp(-d/k))` and concluded "blending makes the blob more bubble-like, not less". Its own table contradicts that at the recommended radius: hard min 12.4% → smin k=0.1 **16.3%**; hard min + fbm 29.4% → smin + fbm **37.3%**. Both improve; only k=0.30 regresses (4.2%). So "measured worse" is one row out of three, and the function it measured is the one proposal 1 explicitly disqualifies (the exponential form perturbs the field everywhere, never exactly equals `min`, and needs `exp`/`log2` whose rounding lands in geometry). **My reading:** the *conclusion* survives, the *argument* does not. What the data actually supports is that **displacement dominates smin as a lever** — one displaced sphere at 49.2% beats the best blended blob at 37.3% — so smin is a small independent gain, and the blast radius is on `accumulate`, which every op, every edit, and the collider route through. Two concrete costs that decide it: a smoothly-subtracted opening is **narrower than authored by up to k/4**, which eats `lanternhead`'s documented 0.85 m wall margin and would soften its two shelter assertions *without failing them* — the agent's own verification channel degrading silently; and smin is non-associative, making the op list order-sensitive in one more way than the existing `// Order is not commutative` comment warns about. **Lens that killed it: cost-of-measured-regression plus verification-channel damage.** If it is ever re-opened, re-measure the *polynomial* form on a *joint* (a plinth meeting ground at a stated k), because concave-area over a whole boulder does not frame that subject at all — and §2.2's per-op rounding covers most of what people reach for smin to get.

**A global `VoxelOp::Displace` modifier — rejected, and the per-op form already exists.**
`bounds()` returning the whole world defeats `Volume::edit`'s chunk-span cull, which is the mechanism that makes destruction affordable: a crater in `terrain_stress` touches 4 chunks and costs 0.1 ms [measured]; with a world-bounds op in the list the span becomes all 2048 chunks, ≈200–300 ms per crater — destruction at 3–5 fps. Worse, it is additive and mode-free, which breaks the invariant `bake`'s own comment rests on ("min/max combination cannot make the result vary faster than its fastest input"): `reach` is a **max** over per-op lipschitz, and a max does not bound a **sum** once two additive ops exist. The per-op `Option<Displace>` already in the tree has finite bounds already grown by amplitude [verified, `lib.rs:325`]. **Lens: determinism-and-gates + cost.** Same verdict applies to `CsgMode::Displace` for strata — make it a field on an op, not a new mode.

**Iterated domain warping in a voxel bake — rejected outright.**
Two compounding costs. Sample rate: 12 noise samples per voxel with no column hoist against `Heightfield`'s 5 per *column* (5/32 per voxel) is a **77× increase in sample rate** before any Lipschitz effect. Reach: `|∇(f∘g)| ≤ |∇f|·(1 + A·f·L_w)` multiplies once per level, ~25× at two levels with gain 4, which takes `terrain_stress`'s reach from 12.8 m to **320 m against a 128×64×128 m volume** — the early-out never fires again at any size. 67.1M voxels × 12 samples × 20 ns = **16.1 s against a measured 118.6 ms bake**. **Lens: cost.** Two forms survive and are already covered above: the 2D warp inside the heightfield's per-column hoist, which is *free* and already implemented (`loom_terrain::noise::warp`), and a single-level 3D warp on a bounded op with one shared warp sample rather than three.

**3D noise as a density field — caves, overhangs, undercuts — rejected.**
It is "more noise" in its purest form: `|n3(p)| < t` puts caves *somewhere*, and an agent cannot say "a cave here" — which is exactly the statement the engine already answers well, since `terrain_stress` bores a tunnel with one aimed capsule Subtract in four lines. Both failure modes (swiss cheese, floating islands) render fine, collide fine, and are caught by no gate — `cargo xtask image` would happily bless a hovering island. The cost: losing the column hoist is **32× more noise evaluations**, taking `terrain_billion`'s measured 1938.6 ms bake to ~35.8 s [derived]; `bounds()` unbounded in all three axes kills the chunk-span cull; and caves reachable from the surface change `loom_voxel::exposure` answers, which changes wind and rain sheltering, which silently moves every existing `loom sim --assert` in scenes that never opted in. **Lenses: cost and authorability, both.** If it is ever wanted, the only tolerable form is *bounded in Y* — the proposal's own `max(0, 1 - (y-h)/H)` falloff already vanishes above the surface, so make that a `bounds()` fact and cap H at ~2 chunks (≈256 chunks instead of 2048 on `terrain_stress`).

**Twist, bend, taper — rejected on authorability.**
`k` has no unit an author can predict: "twist k = 0.4" means different things for a 0.3 m pillar and a 3 m one, because the result depends on `r_max` and the primitive's height. So the agent cannot state the intent it actually has ("a quarter turn over this column") and is left with render-look-change-number, with no metric behind it, for several days plus three Lipschitz bounds that must be *measured* to the 9-to-44-wrongly-skipped-chunks standard. Secondary: `sin`/`cos` per voxel with position-dependent arguments cannot be hoisted, and `loom_field::noise`'s own header rejects `sin` because "sin of a large argument depends on the argument reduction, which differs between libm and a GPU" — this result reaches geometry → collider → sim. **Lens: authorability.** Reparameterised as `twist_degrees` over the primitive's own extent, `bend_radius` in metres and `taper_ratio`, it becomes arguable; until then it is unaimable and the cost argues for deferring anyway.

**Gradient (Perlin) noise as a second frozen primitive — rejected.**
It adds a choice with no criterion: the difference from value noise is a faint axis-aligned quilt, invisible in every parameter and measurable by nothing in the project. It permanently doubles the frozen-ABI surface (a 12-entry gradient table that is itself interface — reordering it moves every hash silently) for an unmeasured defect. And the repo already carries **three** value-noise implementations — `loom_field::value` (3D, seedless, pinned), `loom_terrain::value` (2D, unpinned), `loom_terrain::value3` (3D, unpinned) — of which exactly one has a numeric ABI test. Adding a fourth ABI before pinning the third is the smell. **Lenses: authorability and cost.** *One correction to the record:* the critique called `hash3` "a u64 splitmix mixer entirely unlike loom_field's lowbias32". It is not — it is deliberately the *same* murmur-finalizer mixer as `loom_terrain`'s existing 2D `hash`, with a third multiplier, and its docstring says why [verified, `noise.rs:172`]. The real gap is that it is unpinned, and that is a `the_hash_is_frozen`-style three-output test, not a rewrite.

**Biome as a `loom_field` `Expr` — rejected.**
A `Field` is authored in Rust source and emitted by `build.rs`; `all()` is a **build-time** list [verified, `lib.rs:412`]. An agent can fill `Param` values and nothing else. So "biome" would become one fixed formula for every scene in the project with a handful of scalars, and the thing it is meant to serve — sand at the coast, moss in the valley — is *placement*, which the field cannot express. The proposal also specifies `seed ^ mask_seed`, and `loom_field::noise` is **seedless** by design; adding a seed argument is an ABI change to a primitive inside the pinned wind hash `0x413c_4a61_3c8c_eb4d`. **Lens: authorability.** Placement belongs in scene-authored masks (§3); `loom_field` should carry only a shared *blend function* if and when a CPU and a GPU consumer must agree on the same scalar.

**`Expr::to_rust()` as a third backend — rejected.**
The headline "200× too slow" is wrong by two orders of magnitude and inverts the conclusion: the proposal's own body measures **205 ns against 104 ns**, which is 2×. A 2× is not a reason to build a compiler; it is a reason not to put the voxel field in `Expr` at all, which is what the codebase already does. A third backend also means the same tree must produce identical results through three paths while the S2 agreement test compares two — reintroducing the divergence ADR 0006 exists to prevent, by the mechanism meant to enforce it. **Lens: cost.** The one useful finding survives on its own terms: `Sqrt` is the single missing node for expressing a sphere or capsule SDF, three lines, defensible. `pow`, `floor` and `select` are how this becomes a compiler nobody asked for.

**`VoxelOp::Mesh` (bake an imported boulder into the SDF) — deferred with an explicit trigger, not built.**
The sign computation as specified is not affordable: a naive generalised winding number is O(triangles) per query, and a 3 m boulder at 0.045 m is 852k narrow-band voxels × 30,223 triangles = 2.6e10 evaluations — minutes, not a week. Even with a BVH for the unsigned distance it is 170–425 ms before sign. And the payoff is measured to be a **melted copy of a mesh that already existed**: the imported LOD1 resolves at radius/34 while 0.045 m voxels sample at radius/33, and the output is 62,660 triangles reproducing a 30,223-triangle input. The cache is also a new determinism surface — and the specified key `hash(asset id, transform, voxel_size)` **omits the asset content**, so editing the OBJ silently reuses a stale grid that feeds the collider. **Lens: cost.** Trigger for revisiting: a rock that must be destructible, CSG-joined to terrain, or share the terrain's collider and `exposure` field. Then: sign by flood fill from the AABB exterior (one pass, not 852k tree queries), key on content hash, and verify byte-identical across three processes before anything in the sim hash reads it.

**The grid pipe model (Mei et al. hydraulic erosion) — do not build.**
~6 array passes per iteration × 300–1000 iterations. Extrapolating from the measured 6.5 ms per full-grid pass at 512² (thermal, ×100 = 645.5 ms), that is **12–39 s**, not the 1–3 s the proposal estimated, against the droplet model's measured 196 ms. It scales with *area* where droplets scale with *droplets*, it has eight tuned constants against five, and its payoff (lakes) is already served by an authored water plane. It also requires double-buffering everywhere, while `thermal()` here is deliberately in-place for a measured reason its comment records — two adjacent passes with opposite update disciplines is how someone later "harmonises" thermal and silently regresses its max-slope behaviour from 55° to 75°. **Preserve that comment verbatim.**

**Coastal notch — do not build.**
Its central parameter, sea level, is authored in a second file that has no idea where the scene's water is: the `.loom` carries the ocean op's still-water line, the recipe carries `level` and `height_range`, and nothing checks them. A beach three metres above the sea renders fine and looks wrong for a reason nobody can point at — the class of failure no gate here can catch. Two shore scenes already look right and are golden. Hand-placing a sphere is fewer moving parts than a four-parameter layer. **Lens: authorability.**

**A node graph for terrain authoring — already rejected in `loom_terrain`'s own header, and correctly.** Node ids, port names, edge wiring and cycle detection are bookkeeping to get right before any terrain logic matters, and a graph diff is an adjacency-list diff where inserting one node renumbers everything. The layer stack wins because **order is the semantics and order is visible in the file**.

**A per-voxel material attribute (`i8` #2) — Phase 8.** The objection is not memory: a second `i8` takes `terrain_billion` from 171.2 MB to 342 MB, which is fine on 64 GB and someone will notice and re-open the question. The cost is *passes* — `solid_cells` already walks 16.8M voxels for 81 ms, the mesher walks a padded 34³ per chunk, and a second attribute adds a full traversal to both plus a vertex-format change on an 11M-triangle mesh. Banded colour from a world-position shading term (the puddles precedent) is the whole win for a fraction of the cost.

**An `f32` sidecar field to escape the `i8` — rejected on a false premise.** `SDF_SCALE = 1/127` stores distance **in voxels, not world units** [verified, `lib.rs:598`], so the field spans exactly ±1.0 voxel at 1/127 resolution regardless of pitch — 2.0 mm at `terrain_stress`, 3.1 mm at `lanternhead`. That is not a limit on smooth CSG, displacement, or fillets, all of which act *near* the surface. What the saturation genuinely forbids is anything reading stored distance beyond one voxel: sphere tracing (ADR 0007 says so outright), a wide penumbra, any redistancing pass. A sidecar is 4× memory (171.2 → 684.8 MB) — affordable, and unnecessary.

---

## 5. The landscape recipe

What "incredible" looks like once §2 exists. A worked scene: **a glacial vale, 256 × 64 × 256 m at 0.5 m voxels**, `chunks = [16, 4, 16]` (the schema caps `chunks` at 64/axis and `voxel_size` at 4.0 [verified]).

### The landform lives in a recipe

`assets/games/vale/vale.toml` — 256² over 256 m is **1.0 m/pixel**, which resolves the features that matter (a gully is 3–10 m wide) and is where the cost sits comfortably. Size the recipe to the *feature*, never to the voxels.

```toml
size = 256
world_scale = [256.0, 256.0]
height_range = [0.0, 90.0]
seed = 20260816

[[layer]] # the mass: broad, warped, unremarkable on its own
type = "fbm"      amplitude = 1.0   frequency = 0.004  octaves = 5   warp = 0.35

[[layer]] # the ridgeline. Ridged weights each octave by the last, so crests
          # get detail and troughs stay smooth — the thing that separates
          # commercial terrain from a sum of bumps.
type = "ridged"   amplitude = 0.6   frequency = 0.006  octaves = 6   warp = 0.25

[[layer]] # the trough. profile = 2.0 once §3's exponent lands: a parabola with
          # a break of slope at the lip. That break is the trimline, and it is
          # what makes it read as glacial rather than fluvial.
type = "spline_carve"  points = [[40,20],[110,90],[150,190],[170,240]]
                       width = 34   depth = 26   profile = 2.0   floor_fraction = 0.35

[[layer]] # somewhere to build. This is the layer an agent reaches for when a
          # fort needs to sit down.
type = "flatten_disc"  center = [150,190]  radius = 22  blend = 0.6

[[layer]] # the guarantee no commercial tool offers: a walkable route exists.
type = "corridor"      from = [40,20]  to = [150,190]  width = 6  max_slope = 18

[[layer]] type = "hydraulic"  droplets_per_cell = 2.0  erode_rate = 0.3  deposit_rate = 0.3
[[layer]] type = "thermal"    talus_degrees = 36       iterations = 60
```

Cost, measured/derived: 6 ms noise + 131,072 droplets × 3.97 µs = **520 ms** + thermal ~100 ms ≈ **0.63 s**, once, at load. `loom terrain assets/games/vale/vale.loom --from 40,20 --to 150,190` then answers, without rendering anything: `buildable_pct`, `slope_mean`, `slope_over_45_pct`, `largest_flat`, `reachable: true`. **That is the loop.** Change a number, re-run, read a number — not change a seed, render, squint.

### The scene composes on top, in order

```toml
[node.components.VoxelVolume]
voxel_size = 0.5
chunks = [16, 4, 16]        # 256 x 64 x 256 m

# 1. THE LANDFORM. One op. Every 2D algorithm at full quality, including
#    the two erosion passes a 3D field cannot run at all.
[[ops]] kind = "terrain"  recipe = "vale.toml"  rect = [0,0,256,256]
        base_y = 0.0  mode = "union"

# 2. WHAT A HEIGHTFIELD CANNOT SAY. A height is one value per column, so it
#    has no overhang, no undercut, no cave — ever. These three ops are the
#    entire 3D half, they are AIMED, and they are four lines each.
[[ops]] kind = "capsule"  a = [96,14,120]  b = [128,11,152]  radius = 4.5  mode = "subtract"
[[ops]] kind = "capsule"  a = [128,11,152] b = [150,10,178] radius = 3.2  mode = "subtract"
[[ops]] kind = "sphere"   center = [112,20,136]  radius = 9.0  mode = "subtract"   # the undercut

# 3. THE BUILT THING, finally not axis-aligned (§2.2). At 0.5 m voxels a wall
#    must be >= 1.0 m: surface nets needs two voxels or a sheet vanishes.
[[ops]] kind = "box"  center = [150,12,190]  half_extents = [7.0,3.0,5.0]
        yaw_degrees = 27.0  round = 0.35  mode = "union"
[[ops]] kind = "box"  center = [150,12,190]  half_extents = [6.0,2.6,4.0]
        yaw_degrees = 27.0  mode = "subtract"

# 4. ERRATICS. Silhouette only at this resolution — see below.
[[ops]] kind = "sphere"  center = [88,26,104]  radius = 1.6  mode = "union"
        displace = { amplitude = 0.40, frequency = 0.38, octaves = 2, ridged = true, seed = 91 }
```

Bake: ~380 surface chunks of 1024, ~50 ms bake and ~110 ms mesh [derived from `terrain_stress`'s measured 118.6 / 196.6 ms at 854 surface chunks]. Grass, water and rain sit on top unchanged — the grass field reads the terrain through `GroundGrid`'s SDF march, which already works and does not care where the height came from.

### What it looks like, and the one honest limit

A warped ridgeline with real drainage — gullies where water actually ran, alluvial fans where it stopped, talus at 36° where the noise wanted 70°. A parabolic trough with a visible break of slope where the ice stopped, a flat build pad that was *asserted* buildable rather than eyeballed, and a guaranteed walkable route to it. A cave that goes *through* the moraine and an undercut above it — the two things the whole 2D pipeline structurally cannot make, supplied by three aimed capsule subtracts. A rotated, rounded, hollow structure on the pad.

**And the erratics read as lumps, not stone.** That is arithmetic, not taste: `o* = ceil(log2(A / voxel_size)) + 2` gives `ceil(log2(0.40/0.5)) + 2 = 2` octaves at 0.5 m voxels, and two octaves of ridged displacement is a potato with a crease. Rock reads as rock at ~0.05 m, and 0.05 m over 256 m is 5,120 voxels/axis = 160 chunks/axis, **2.5× over the schema cap of 64**. So detail rock at landscape scale is structurally not a single-volume problem. Two honest routes, and picking between them is a rule worth writing down:

- **Imported mesh** for scenery rock — `props.loom` already does this, zero engine work, 30,223 triangles at 55.8% concave with UVs and a tangent-space normal map, versus 318,840 triangles at 50.6% and 618–1,244 ms of bake to do worse. **Mesh is the default.**
- **A second, small `VoxelVolume` at 0.05 m** for the rock the player can crater, CSG-join to the ground, or that must share the terrain's collider and `exposure` field. Those three are the *only* things voxels win, and they are worth the 2× triangles when you need them.

The seam where a mesh boulder meets voxel ground is real and no material work hides it under a low sun; seat it with a `Subtract` sphere in the terrain, or scatter debris on the line. If neither is acceptable, that is the trigger for `VoxelOp::Mesh` — and only then.

---

## 6. Open questions

**Is the `3.0` gradient constant already holing terrain in a shipped scene?** Measured max `|∇value3|` is 3.249 against a bound of 3.000, so the guarantee is broken *in principle* today. Whether any authored `Displace` actually lands in a chunk where it matters is unknown — the volumes carrying displaced ops are small, and the early-out fires on far chunks. **Experiment:** bake every scene in `SCENES` with the early-out enabled and again with it forced off, assert byte-identical fields. That is a one-evening xtask target and it settles the question for every op at once, not just this one. It should probably become permanent.

**What is `reach` really costing, per scene?** Nothing in this project measures bake time [verified — no gate reports it], so every cost claim in this report about admitted-chunk counts is derived from one measured configuration (`terrain_stress`: 854 of 2048 chunks at lipschitz 1.843). The per-chunk `reach` narrowing (max over ops whose `bounds()` intersect the chunk, instead of one global max) is worth 2.4× on paper and is unmeasured. **Experiment:** print per-scene bake milliseconds in `xtask validate` and record them MANIFEST-style, so a regression is a readable diff line. Until that exists, this cost class is permanently ungated and a 26× regression passes all four green checks in silence.

**Does a fillet at a joint help, at all?** The smin argument was settled on a metric (concave area over a boulder) that does not frame a joint. Unknown whether a k = 0.15 m fillet where a plinth meets ground reads as better or as mush. **Experiment:** one scene, one plinth, three values of k, judged on `loom measure --shape` restricted to the joint region *and* on a flythrough — and only after the shape metric exists, so it is a number rather than an opinion. Low priority; §2.2's per-op rounding may satisfy the actual want.

**How much does the 2D/3D noise split cost in practice?** Voxel geometry will use `loom_terrain`'s seeded 3D value noise while wind and clouds use `loom_field`'s seedless frozen one, permanently — the nesting order that makes `loom_field::lattice` correct is hostile to `bake_chunk`'s z→x→y loop, and reordering it would move the pinned wind hash. So a scene's rock and its weather draw from different noise forever. Nobody has established that this matters visually. **Experiment:** none needed to *decide* — write it as an ADR with the reason, declare `loom_terrain::noise` the CPU-only voxel noise and `loom_field::noise` the two-sided one, and forbid a third. But pin `hash3`/`value3` numerically first: it feeds geometry → `solid_cells()` → collider with no frozen-ABI test.

**Where exactly does the mesh-versus-voxel line fall for a rock the player interacts with?** The measured comparison is decisive for scenery (mesh wins at every resolution, on fidelity and cost). It is *not* decided for a rock that is destructible but rarely destroyed. **Experiment:** author the same 3 m boulder both ways in one scene, measure load-time bake, resident field, collider cells (11.4 bytes/solid cell measured), triangle count and concave-area, and look at the ground seam under a low sun. That is a half-day and it produces the rule that stops this being re-argued per rock.

**Does the flicker instrument even apply to landscapes?** `cargo xtask shimmer` and `flythrough` frame whole-scene bounds, which is how the density-falloff table came to measure an empty field. A 256 m landscape framed from its bounds is a green slab. Every AA-adjacent claim about terrain silhouettes — layered strata, LOD transitions, distant displaced rock — needs the authored-camera treatment before any of it is trusted. **Experiment:** extend the authored-camera path (already built for `shimmer`) to `flythrough`, and make the first LOD and first strata scenes frame their subject explicitly, at an authored camera, with a hard edge in the frame.