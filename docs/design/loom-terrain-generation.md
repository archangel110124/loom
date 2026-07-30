# Loom: AI-Authored Terrain Generation

**Fourth companion doc.** How the agent generates terrain, given that the runtime representation is
destructible SDF voxels (see `loom-voxel-system.md`).

---

## 0. The one principle everything follows from

**The heightmap is an output, never an input.** The agent never writes pixels, never uploads an
image, never edits a height array. It writes a **recipe** — a text document describing how the
terrain is produced — and the heightmap is a derived, cached artifact.

This is the same decision as the voxel op list, for the same reasons, and it's worth stating as a
pipeline:

```
recipe (.toml)          authored by agent or human — text, diffable, reviewable, tiny
   ↓  evaluate + bake
heightmap (R32F)        derived artifact, cached by content hash, never in version control
   ↓  sdf = y - h(x,z)
SDF voxel field         the runtime representation — destructible, supports caves
   ↓  CSG ops
final volume            overhangs, caves, arches the heightmap can't express
```

Everything below is about making that first line something a language model can write correctly on
the first attempt.

---

## 1. Why bother with a heightmap when the runtime is voxels?

Because they're good at different things and the split is clean.

| | Heightmap | SDF voxels |
| --- | --- | --- |
| Macro landform | Excellent — 2D, cheap, one value per column | Wasteful — 99% of voxels are far from surface |
| Erosion simulation | Native — all the literature is heightfield-based | No practical algorithms |
| Analysis (slope, flow, buildability) | Trivial | Awkward |
| Caves, overhangs, arches | Impossible | Native |
| Runtime destruction | Impossible | Native |
| Memory | ~4 bytes per column | ~1 byte per voxel |

So: heightmap generates the base field; voxel CSG carves what a heightmap can't express. One
function bridges them, and it's three lines:

```rust
/// Base SDF from heightmap: signed distance to the terrain surface along Y.
/// Approximate but correct in sign, and redistancing (§5.3 of voxel doc) fixes magnitude.
fn base_sdf(p: Vec3, height: &Heightfield) -> f32 {
    p.y - height.sample_bilinear(p.x, p.z)
}
```

Then cave ops subtract from it. The heightmap also has a second life at distance: an earlier finding
worth repeating — Relic avoided applying LOD to volumetric terrain at all by using a **heightmap
representation for distant terrain**, since volumetric detail only matters where the player can
actually see into caves and under overhangs.

---

## 2. Recipe format: a layer stack, not a node graph

The reference implementations here are Gaea and World Machine. Gaea is instructive because it offers
**three workflows** — Layers, Graph, and Sculpt — and they interoperate: layer masks get converted
into nodes within the graph, and individual nodes can be modulated through an adjustment-layer-like
Post Process Stack. Its design goal is also directly relevant to your problem. QuadSpinner's
co-founder framed it as a step away from starting every terrain with Perlin noise and then adapting
it toward a shape you might like, toward being able to **art-direct from the start** — with mountain
drawing, erosion sculpting, and primitive-based construction.

That's exactly what an agent needs. "Make a valley running north–south with a plateau on the east
side" is art direction; it is not a noise seed.

### Why a linear stack beats a DAG for agent authoring

A node graph is more expressive. It's also where an agent will make most of its errors: node IDs,
port names, edge wiring, and accidental cycles are all bookkeeping the model has to get right before
any of the terrain logic matters.

A **linear layer stack with named outputs** is nearly as expressive for terrain specifically —
because terrain generation is overwhelmingly a sequence of "add this, then mask it by that" — and
it's dramatically easier to write correctly. Allow named references for reuse and you get back the
DAG for the cases that need it, without making every simple case pay the wiring cost.

```toml
[terrain]
size        = [2048, 2048]      # heightmap resolution
world_scale = [4096.0, 4096.0]  # world units covered
height_range = [0.0, 900.0]
seed = 41337

# ── macro landform ────────────────────────────────────────
[[layer]]
name = "continent"
kind = "fbm"
octaves = 5; lacunarity = 2.0; gain = 0.5; frequency = 0.0004
warp = { strength = 180.0, octaves = 2 }   # domain warping — see §3.2

[[layer]]
name = "ridges"
kind = "ridged_multifractal"
octaves = 7; frequency = 0.0016
blend = { mode = "add", amount = 0.55, mask = "continent" }

# ── art direction: this is the part that matters ──────────
[[layer]]
name = "main_valley"
kind = "spline_carve"
points = [[400, 120], [980, 700], [1500, 1400]]
width = 220.0; depth = 90.0; falloff = "smoothstep"

[[layer]]
name = "fort_plateau"
kind = "flatten_disc"
center = [1180, 640]; radius = 130.0; blend_radius = 90.0
target = "auto"        # sample existing height at center

# ── physical simulation ───────────────────────────────────
[[layer]]
name = "erosion"
kind = "hydraulic_particle"
droplets = 400_000; inertia = 0.05; capacity = 4.0
erode_rate = 0.3; deposit_rate = 0.3; evaporate = 0.02

[[layer]]
kind = "thermal"
talus_angle = 38.0; iterations = 60

# ── outputs the rest of the engine consumes ───────────────
[outputs]
height = "height.r32f"
masks  = { rock = "slope > 42", grass = "slope < 22 && height < 400", snow = "height > 620" }
```

Two things to notice. **Art direction sits between the noise and the erosion**, which is the correct
order — you place the landforms you need for gameplay, then let physics make them look real. And
**the masks are declared in the recipe**, so material assignment is derived from the same document
rather than being a separate hand-authored thing that drifts.

---

## 3. Generator vocabulary

### 3.1 The fBm family

Standard fractal sum: octaves of coherent noise at doubling frequency and halving amplitude.
Musgrave's terminology is the one to use in your parameter names, since it's what every artist and
every reference already uses — **frequency** is the lateral size of the features, **amplitude**
their height, and **lacunarity** the factor by which frequency changes per octave (Latin for "gap").

Variants worth exposing as distinct `kind` values rather than flags:

- **Ridged** — take the absolute value of each layer before summing and invert the result, which
  produces mountain ridges. (Turbulence is the same abs trick without the inversion, giving
  ridge-like creases.)
- **Multifractal** — each octave's contribution depends on what previous octaves produced, so areas
  that are already high ridges receive more detail while flat areas and valleys receive less. This
  is what makes commercial terrain software look better than a naive fBm, and multifractal
  simulations are common in exactly that software.

### 3.2 Domain warping — cheapest large quality win

Domain warping distorts the *input coordinates* of the noise function using the output of another
noise evaluation, rather than modifying the summation — `f(p + f(p + f(p)))`. It was popularized by
Inigo Quilez's articles and ShaderToy work, though coordinate perturbation goes back to Perlin's
original 1985 procedural-textures paper.

The reason to care: it produces terrain that reads as **eroded, with river valleys and steep peaks**,
for essentially the cost of extra noise samples. One implementer describes exactly that result after
adding it to their tiled terrain generator. Expose it as an optional `warp = { strength, octaves }`
on every noise layer and the agent will use it correctly, because there are only two parameters.

### 3.3 Noise derivatives — the underrated one

Quilez's other technique: inject the noise **derivatives** into the core of the fBm construction.
This simulates erosion-like effects and produces a much richer variety of shapes, with genuinely
flat areas alongside rough ones — a much nicer variety than regular fBm. Two bonuses: analytical
derivative computation is faster and more accurate than central differences, and depending on the
fractal sum function you can compute **analytical normals for the complete heightmap**.

Free correct normals for your terrain shading, with no finite-difference artifacts. Worth
implementing.

### 3.4 The honest caveat about all of it

From a well-regarded comparison of three erosion approaches: these fBm variants look iteratively
more convincing, but if you compare them against real elevation maps they look **nothing like real
terrain**, because the fractal shapes in real landscapes are driven by erosion — principally
hydraulic erosion, the displacement of terrain by water.

Which is why §4 isn't optional.

---

## 4. Erosion: the step that makes terrain look real

Two families, and you want one now and the other later.

### 4.1 Particle / droplet erosion — the default

Spawn particles (typically on the order of 10,000 for a modest map, scaling with size) at random
positions. Each has a floating-point position over the grid, computes the slope beneath it, and
follows the path of least resistance downhill, eroding and depositing sediment along the way until
it evaporates or runs off the edge. Because the particle position is continuous while the map is a
grid, you bilinearly interpolate the gradient from the four surrounding corners.

The property that makes this the right default: **performance is tied to particle count, not map
size**, because only the affected parts of the map get computed. And it parallelizes beautifully —
one GPU implementation simulates a million droplets in about ten seconds.

It also produces recognizable real features on its own: gullies on mountainsides, sediment-filled
valleys, and alluvial fans where water drains from a narrow passage into a wider area.

**Its limitation, stated plainly in the practitioner discussion:** the particles don't interact with
each other, so this approach can't form lakes and doesn't form rivers. It erodes, but it isn't a
fluid.

### 4.2 Shallow water — when you want rivers and lakes

The grid-based alternative: no particles, instead a water height per cell flowing to and from
adjacent cells, with flow proportional to the height difference because of the pressure from the
water column. The canonical reference is Mei, Decaudin & Hu, *Fast Hydraulic Erosion Simulation and
Visualization on GPU* (2007), extended by Jákó & Tóth's *Fast Hydraulic and Thermal Erosion on the
GPU*, with Šťava et al. on interactive terrain modeling via erosion. Multiple open implementations
exist in both compute shaders and CPU/OpenMP.

Add this as a second `kind` when the game needs water features. It's more expensive and has more
parameters to get wrong, which is why it isn't the default.

### 4.3 Thermal erosion — cheap, do it immediately

Material above a talus angle slides downhill. Two parameters, trivial to implement, and it fixes the
implausibly steep slopes that noise produces. Both GPU erosion references above pair it with
hydraulic.

### 4.4 Determinism: bake and hash, don't re-simulate **[important]**

Erosion is the one place where the recipe-as-source-of-truth model strains. GPU erosion with atomic
accumulation is order-dependent, so it is not reliably bit-reproducible across runs or hardware —
which collides with the determinism requirement from §A.3 of the graphics doc.

Resolve it structurally rather than fighting it:

1. Erosion runs **once, at bake time**, not at load.
2. The output heightmap is a **cached artifact keyed by a hash of the recipe** — same content-hash
   asset pipeline as §2.6 of the main doc.
3. That artifact is what ships and what loads. Runtime never re-simulates.
4. The recipe stays in version control; the baked heightmap does not.

So the recipe is authoritative for *authoring*, and the baked artifact is authoritative for
*determinism*. Both properties preserved, and rebake is an explicit action with a visible diff in
the analysis outputs (§7).

---

## 5. Art direction layers: what the agent actually reaches for

An agent authoring terrain is almost never trying to make pretty noise. It's trying to satisfy
gameplay constraints: a buildable plateau for the fort, a valley the player walks up, a ridge on the
north horizon for silhouette, a river the bridge crosses.

So the highest-value layer kinds aren't noise variants — they're these:

```rust
pub enum ArtLayer {
    /// Carve or raise along a path. Rivers, valleys, roads, ridgelines.
    SplineCarve { points: Vec<Vec2>, width: f32, depth: f32, falloff: Falloff },
    /// Flatten a disc or polygon toward a target height. Buildable ground.
    Flatten     { region: Region, target: Target, blend: f32 },
    /// Raise a peak with a profile curve. Named landmarks.
    Peak        { at: Vec2, height: f32, radius: f32, profile: Profile },
    /// Cliff along a line — a hard discontinuity noise can't make.
    Escarpment  { line: Vec<Vec2>, drop: f32, sharpness: f32 },
    /// Constrain a whole region to a height band.
    Clamp       { region: Region, min: f32, max: f32, blend: f32 },
    /// Guarantee a traversable path between two points.
    Corridor    { from: Vec2, to: Vec2, max_slope: f32, width: f32 },
}
```

`Corridor` is the one that doesn't exist in commercial terrain tools and should exist here. "There
must be a walkable route from the spawn to the fort" is a *gameplay* constraint, and having the
generator guarantee it beats having the agent iterate blindly until a slope check passes.

Worth noting that the ML research converged on almost exactly this vocabulary for human control:
TerraFusion's sketch conditioning uses **red lines for valleys, green for ridgelines, and blue for
cliffs**. Three primitives — valley path, ridge path, cliff line. That convention is also very
agent-friendly, since emitting a short list of colored polylines is far easier than emitting a
heightmap.

---

## 6. The ML option, which is now genuinely usable

This moved from research to available-off-the-shelf within the last year, and it's worth knowing
precisely what's there.

- **`terrain-diffusion` / InfiniteDiffusion** (Goslin, SIGGRAPH '26) — converts sketches into
  high-resolution heightmaps at around 30m/pixel, with models published on Hugging Face. Notably it
  ships **two model variants with an explicit games recommendation**: one with finer local controls
  allowing more local variation and detail, recommended for games and interactive experiences; and a
  more coherent, expansive one for large-scale worldbuilding, which the authors themselves note is
  often too expansive. It also exposes an API that can be queried for elevation and climate data.
  The paper's framing — bridging learned fidelity and procedural utility — is the same problem you
  have.
- **TerraFusion** (arXiv 2505.04050, code and pretrained models public) — jointly generates
  heightmap *and* texture in a fused latent space using a latent diffusion model with a
  heightmap-specific VAE, plus a supervised adapter for sketch control. Their motivating insight is
  worth internalizing even if you never use the model: geometry and surface appearance are
  correlated, since steep regions tend to be rocky or forested with patterns following the
  contours — so generating them jointly preserves a relationship that two-stage pipelines lose.
- **Pixels2Peaks** (ACM TOG, July 2026) — converting terrain images to heightmaps.
- **Earthbender** (SIGGRAPH MIG 2025) — stylistic heightmap generation via a guided diffusion model.
- **MESA** — text-driven terrain generation from a latent diffusion model trained on global terrain.

**How to use this without wrecking the architecture:** make it a layer kind, not a replacement.

```toml
[[layer]]
name = "macro"
kind = "diffusion"
model = "terrain-diffusion-30m"
sketch = [
  { kind = "ridgeline", points = [[200,100],[700,400]] },
  { kind = "valley",    points = [[300,600],[900,900]] },
]
seed = 41337
# output cached by hash of (model, sketch, seed) — see §4.4
```

The recipe stays text. The output is a cached artifact. Determinism is preserved by caching rather
than by trusting the model. And you can layer procedural detail on top, which you'll need to:
**30m/pixel is far too coarse for a playable level** — it's a continent-shape tool, not a
level-design tool. Use it for the macro, procedural for the meso, voxel CSG for the micro.

Costs to weigh honestly: a Python/torch dependency in an otherwise self-contained Rust engine, model
weights to distribute, licensing to check per model, and inference latency that makes it a bake-time
tool only. Defer it. But design the layer-kind extension point now so it slots in without a rewrite.

---

## 7. Analysis: the agent's feedback channel **[the novel part]**

`render_preview` verifies placement. `run_scene` verifies behavior. Terrain needs its own channel,
and it's mostly *not* visual — most terrain mistakes are invisible in a render and obvious in a
slope map.

```
terrain_analyze(recipe | baked) → {
    hillshade:      PNG,     # what it looks like from above
    slope:          PNG,     # where it's walkable — the most useful single output
    flow:           PNG,     # flow accumulation: where water goes, i.e. where rivers belong
    buildable_mask: PNG,     # slope < threshold, contiguous area > threshold
    stats: {
        height: { min, max, mean, p95 },
        slope:  { mean, pct_over_45 },
        buildable_area_pct: f32,
        largest_flat_region: { center, area },
        reachable_from: Option<Vec2>,      # flood fill under max_slope
    }
}

terrain_query(x, z) → { height, slope, normal, flow, mask_hits }
terrain_path(from, to, max_slope) → Option<Vec<Vec2>>
```

Three notes on why this shape:

- **Return small PNGs, not arrays.** The agent reads images competently and reads a 2048×2048 float
  array not at all. A 256×256 slope map answers "is this terrain playable" in one glance.
- **`buildable_area_pct` and `largest_flat_region` are what the agent actually iterates on.** A
  gorgeous mountain range with 3% buildable ground is a failed level, and nothing in a hillshade
  reveals that.
- **`terrain_path` closes the loop on the `Corridor` layer.** Generate, verify traversability, adjust.
  That's an agent working on gameplay rather than on aesthetics, which is the whole point.

Perceptual realism metrics for terrain do exist as research (PTRM), and you could imagine scoring
generated terrain against them. Interesting; don't build it.

---

## 8. Build order

Slots alongside the voxel work, after the agent loop exists.

| Step | Work | Exit criterion |
| --- | --- | --- |
| T0 | Recipe parse; fBm + ridged + multifractal; bake to R32F with content hash | A recipe produces a heightmap deterministically |
| T1 | `base_sdf` bridge into the voxel field; terrain renders | Walk on generated terrain |
| T2 | Domain warping + noise derivatives (analytical normals) | Terrain reads as eroded; no finite-difference normal artifacts |
| T3 | Art layers: spline carve, flatten, peak, escarpment | Hand-write a recipe that makes a specific named valley |
| T4 | Particle hydraulic + thermal erosion, CPU, `rayon`, baked | Gullies, valley sediment, alluvial fans appear |
| T5 | `terrain_analyze` + `terrain_query` + `terrain_path` | Slope and flow maps returned as PNGs |
| T6 | MCP: `terrain_author`, `terrain_analyze`; recipe schema in the registry | **Gate:** *"Generate a mountain valley with a buildable plateau for a fort and a walkable path from the south entrance"* — agent authors it, reads its own slope map, adjusts, and verifies the path |
| T7 | `Corridor` layer with guaranteed traversability | Path constraint satisfied by construction, not by iteration |
| T8 | Cave/overhang CSG ops driven by flow map (caves where water went) | Terrain has caves that make geological sense |
| T9 | Shallow-water erosion as an optional kind | Lakes and rivers |
| T10 | Diffusion layer kind, cached | Sketch → macro heightmap, procedural detail on top |

**T6 is the gate.** Note what it tests: not "does the terrain look good" but "can the agent tell
whether the terrain works, and fix it when it doesn't." Same principle as every other gate in this
project.

T8 is the sleeper: using the flow accumulation map to place caves means the caves appear where water
would actually have carved them. That's a small amount of code for a large amount of apparent
geological intent, and it's only possible because you kept the heightmap analysis around.

---

## 9. Crates and layout

```
loom_terrain/          # recipe parse, layer evaluation, bake, content hashing
loom_terrain_erode/    # particle + thermal (CPU/rayon first, wgpu compute later)
loom_terrain_analyze/  # slope, flow accumulation, buildability, pathfinding, PNG output
```

Dependencies: a noise library (`noise` or `fastnoise-lite` are the usual Rust choices — verify the
current state before committing, and prefer one where you can get analytical derivatives for §3.3),
`rayon` for erosion, `image` for the analysis PNGs, `wgpu` later if erosion becomes a bottleneck.

Write your own fBm/ridged/multifractal composition on top of a raw noise basis rather than using a
library's prefab fractal types — you need derivative access and exact control over the octave
combination, and prefab fractals hide both.

---

## Sources

Inigo Quilez, *fBm and noise derivatives* (iquilezles.org/articles/morenoise) and the domain warping
articles · F. Kenton Musgrave, *Procedural Fractal Terrains* · Red Blob Games, *Making maps with
noise functions* · The Book of Shaders ch. 13 · 3DWorld blog on domain warping · mysimulator.uk on
domain warping history · dandrino/terrain-erosion-3-ways · Mei, Decaudin & Hu, *Fast Hydraulic
Erosion Simulation and Visualization on GPU* (PG'07) · Jákó & Tóth, *Fast Hydraulic and Thermal
Erosion on the GPU* · Šťava et al., *Interactive terrain modeling using hydraulic erosion* ·
bshishov/UnityTerrainErosionGPU · karhu/terrain-erosion · Vehxx/Rainfall · van der Veen, *Improved
terrain generation using hydraulic erosion* · GameDev.net, real-time hydraulic erosion with compute
shaders · QuadSpinner Gaea documentation and CG Channel / befores & afters coverage ·
xandergos/terrain-diffusion and Goslin, *InfiniteDiffusion* (SIGGRAPH '26) · Higo et al.,
*TerraFusion* (arXiv 2505.04050) · Jain, Gain & Cordonnier, *Pixels2Peaks* (ACM TOG 45:4, 2026) ·
Barazandeh & Zachmann, *Earthbender* (SIGGRAPH MIG 2025) · *Terrain Diffusion Network* (AAAI 2024) ·
*PTRM: Perceived Terrain Realism Metrics* (arXiv 1909.04610).
