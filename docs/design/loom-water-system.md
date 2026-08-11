# Loom: Water

**Sixth companion doc.** How Unreal's water system actually works, which parts of it survive
contact with Loom's constraints, and how to build it.

Read `docs/design/README.md` first for doc precedence. This one assumes the Vulkan backend and the
determinism requirement, both of which change the answer materially.

---

## 1. How Unreal actually does it

Worth understanding properly, because the architecture is good and the parts that don't fit Loom
don't fit for reasons specific to your engine rather than because UE got them wrong.

### 1.1 The plugin's shape

<cite index="3-1">The Water plugin provides infinite ocean planes with Gerstner waves, enclosed
polygon lakes, and spline-driven rivers, all sharing the same underlying `AWaterBody` base class and
rendering through the deferred water pipeline. It ships with UE5 but is disabled by
default.</cite> Water bodies also <cite index="8-1">carve the terrain beneath them via a Landscape
Brush</cite>, and rendering uses <cite index="8-1">the SingleLayerWater shading model — physically
based, one pass, with depth-based color, real-time reflections, refraction, and shoreline
foam.</cite>

Three body types, one base class. That's the right factoring and Loom should copy it.

### 1.2 Waves: Gerstner, and only Gerstner

<cite index="6-1">Water bodies get a wave simulation model through a Water Waves Asset slot, and the
default uses the Gerstner model. Unreal ships only Gerstner; other models are possible but you write
them yourself in Blueprint or C++.</cite> <cite index="2-1">By default 16 waves are summed — fewer is
more performant but gives less randomness where waves collide.</cite>

Crucially, this is **not fluid simulation**. <cite index="8-1">It's the Gerstner wave algorithm for
plausible, efficient wave patterns, adjustable by length, amplitude, direction and steepness — not
full fluid dynamics.</cite> That's the key architectural fact about UE water, and the reason it
performs well: the ocean is an analytic function of position and time, not a simulation with state.

Rivers are the exception and work differently: <cite index="5-1">rivers don't drive surface motion
with waves at all — they use velocity from individual points along the spline, written to a flow map
that visually drives water along the surface in the spline's direction. Rivers act as connections
between other water bodies, and where they intersect lakes or oceans a transitional material blends
them.</cite>

### 1.3 The mesh: quadtree LOD

<cite index="19-1">LOD is handled by traversing a quadtree each frame to generate an optimized set of
visible tiles, collapsed per level where possible. Each LOD is a concentric ring around the camera,
each lower level farther out and containing half the vertices of the one before it.</cite>
<cite index="23-1">The `UWaterMeshComponent` owns that quadtree and generates one continuous water
mesh across all water body actors in the world.</cite>

One continuous mesh for all bodies — not a mesh per lake. That's what makes river-to-lake transitions
seamless, and it's worth copying.

### 1.4 Buoyancy: spheres, not volumes

<cite index="9-1">The Buoyancy Component uses spheres — "pontoons" — as a simplistic volumetric
approximation of the floating object. It's a low-cost solution supporting many simultaneous
objects. Their example cube uses four pontoons to sit upright.</cite>

The query interface is the important part. <cite index="10-1">`GetLastWaterSurfaceInfo` returns water
plane location, plane normal, surface position, water depth, water body index, and water
velocity.</cite> That's the complete set of things a buoyancy solver needs from a water system, and
it's a good API to copy verbatim.

The tuning parameters reveal what actually goes wrong in practice:
<cite index="13-1">`buoyancy_coefficient` scales the force per pontoon; `buoyancy_damp` and
`buoyancy_damp2` are first- and second-order damping on Z velocity; and a ramp
(`buoyancy_ramp_min_velocity` / `max_velocity` / `max`) limits buoyancy at speed.</cite> Rivers get
their own: <cite index="12-1">`water_velocity_strength` for push force, `water_shore_push_factor` for
nudging objects toward shore — or negative to push toward the center — and a
`river_traversal_path_width`.</cite>

Two things to read out of that list. **Damping is not optional** — without it objects bob forever,
because an analytic wave field injects energy with nothing to dissipate it. And the **velocity ramp
exists because fast-moving objects at the surface get unstable**; the fix is capping buoyant force
by speed rather than solving it properly.

### 1.5 Interactive ripples

Separate system. <cite index="36-1">Two-way interaction is done with dynamic fluid simulation render
textures: when an object hits the water its position and velocity are drawn into a ripple map
texture.</cite> UE layers a Niagara-based grid sim on top of the analytic waves for this.

And here is the trap that decides Loom's architecture:
<cite index="36-1">for simulation-driven water, copying the math to the CPU is too expensive, so
developers render water heights to a displacement texture on the GPU and read it back to the CPU —
which gives accurate physics alignment but introduces a frame delay from the GPU→CPU
transfer.</cite>

**Loom cannot do that.** More on this in §5.1.

---

## 2. What changes for Loom

Four constraints of yours that UE doesn't have, each of which pushes the design somewhere specific.

| Loom constraint | Consequence for water |
| --- | --- |
| **Determinism** — `--assert` must be trustworthy | The CPU owns the wave field. No GPU readback, ever. Analytic waves become mandatory rather than merely efficient. |
| **Text scenes** | A water body is a handful of TOML lines the agent writes directly. No spline editor required, unlike UE. |
| **Destructible SDF voxel terrain** | Water depth is a query against a field that *changes at runtime*. UE bakes terrain carving at edit time; you can't. |
| **`loom_terrain` already computes flow accumulation** | Rivers can be placed where water would actually flow. UE makes the artist guess. |

That last one is a genuine advantage and §6 builds on it.

The determinism constraint is the load-bearing one, and it's worth seeing that it makes your life
*easier*, not harder. UE's architecture is complicated by having two sources of truth for surface
height — the GPU displacement for rendering and a CPU approximation for physics — with all the
readback latency and divergence that implies. Loom gets one source of truth by construction.

---

## 3. Architecture

### 3.1 Components

```rust
/// The water surface. Analytic, stateless, deterministic.
#[derive(Component, JsonSchema)]
pub struct WaterBody {
    pub kind: WaterKind,          // Ocean | Lake | River
    pub surface_height: f32,      // still-water level, world Y
    pub waves: WaveSet,
    pub flow: Option<FlowField>,  // rivers only
    pub density: f32,             // kg/m³ — 1000.0 fresh, 1025.0 salt
    pub drag: f32,                // linear drag coefficient underwater
    pub material: AssetRef,
}

#[derive(JsonSchema)]
pub struct WaveSet {
    pub waves: Vec<GerstnerWave>,   // cap at 16 (§5.3)
    pub attenuation_depth: f32,     // waves flatten in shallow water
    pub max_height: f32,            // clamp, for the mesh bounds and the validator
}

#[derive(JsonSchema)]
pub struct GerstnerWave {
    pub wavelength: f32,
    pub amplitude: f32,
    pub steepness: f32,   // Q — see the trap in §5.3
    pub direction: [f32; 2],
    pub speed_scale: f32, // 1.0 = physically correct phase speed
}

/// Makes a rigid body float. Spheres, following UE.
#[derive(Component, JsonSchema)]
pub struct Buoyancy {
    pub pontoons: Vec<Pontoon>,     // local-space sphere centers + radii
    pub coefficient: f32,           // force multiplier, default 1.0
    pub damp_linear: f32,
    pub damp_quadratic: f32,
    pub max_speed_ramp: Option<Ramp>, // caps buoyancy at speed (§1.4)
}
```

Everything the agent needs is a scalar or a short array. No splines, no assets to create, no editor
interaction. A lake is six lines of TOML.

### 3.2 The one function that matters

```rust
/// THE authoritative water query. Everything — buoyancy, rendering, gameplay,
/// the CLI, the agent's measure tool — goes through this.
pub fn sample_water(body: &WaterBody, world_xz: Vec2, t: f64) -> WaterSample;

pub struct WaterSample {
    pub height: f32,        // world Y of the surface
    pub normal: Vec3,       // analytic, not finite-differenced (§5.4)
    pub displacement: Vec3, // full Gerstner offset, for rendering
    pub velocity: Vec3,     // orbital + flow, for drag and rivers
    pub depth: f32,         // surface height minus terrain height
}
```

That mirrors UE's `GetLastWaterSurfaceInfo` field for field, which is a good sign — it's the set of
things that turns out to be needed.

The Gerstner evaluation itself, from the standard formulation:

```rust
// phi = k * dot(d, xz) - omega * t
// x  += Q * A * d.x * cos(phi)
// z  += Q * A * d.y * cos(phi)
// y   = A * sin(phi)
// where k = 2*PI / wavelength, omega = sqrt(g * k) for deep water
```

<cite index="28-1">This is the standard trochoidal form: `Q` is steepness, `A` amplitude, `D` the
wave direction, `k = 2π/wavelength`, and `ω = sqrt(g·k)` for deep water.</cite> Summing several
waves with different wavelengths and directions is what breaks up the repetition.

### 3.3 The buoyancy solver

Per pontoon, per fixed step:

1. `sample_water` at the pontoon's world XZ.
2. Compute submerged fraction of the sphere against the sampled surface height. A sphere-plane
   spherical-cap volume is exact and cheap; do not approximate it with a linear ramp, because the
   nonlinearity is what makes objects settle rather than oscillate.
3. `F_buoy = ρ_water * V_submerged * g`, applied upward at the pontoon's world position.
4. Damping: `-damp_linear * v - damp_quadratic * v * |v|` on the pontoon's velocity. Apply the
   quadratic term only underwater.
5. Drag against relative flow: `F_drag = drag * (v_water - v_body)`. This is what makes rivers carry
   things, and what makes UE's `water_velocity_strength` exist.

Applying force at each pontoon's offset position is what produces torque — that's why four pontoons
make a cube sit upright, and one pontoon makes it spin. This is the whole reason for the pontoon
model rather than a single center-of-mass force.

**Order of operations matters for determinism:** iterate pontoons in a stable index order, accumulate
into a single force/torque pair, and apply once. Accumulating float additions in a different order on
a different run is exactly the class of bug `clippy.toml` already guards against elsewhere.

---

## 4. The two-implementation problem, and how to not have it

The CPU needs wave heights for physics. The GPU needs the same wave heights for rendering. If those
are two hand-written implementations — Rust and Slang — **they will diverge**, and the symptom is a
boat floating visibly above or below the surface, with no error anywhere.

This is a real risk and worth solving structurally rather than with discipline. Three options,
ordered by how well they fit what you've already built:

**Option A — generate the Slang from the Rust (recommended).** The wave evaluation is ~30 lines of
pure arithmetic. Write it once in Rust, emit the Slang function from `build.rs` as text. You already
run `slangc` and `spirv-val` there, so this is one more step in a pipeline that exists.

**Option B — generate the Rust from the Slang.** Same idea, inverted. Worse, because the Rust side is
the authoritative one and should be the source.

**Option C — write both, test them against each other.** A test that samples both implementations at
a few hundred (x, z, t) points and asserts agreement within tolerance. This requires GPU readback in
the *test* only, which is fine — determinism matters in the sim, not in a test asserting equality.

Do A, and add C as a regression test regardless. If the numbers ever disagree, you want to know from
CI rather than from a screenshot.

---

## 5. Traps

### 5.1 GPU readback destroys determinism — never do it for physics

The industry-standard approach is <cite index="36-1">rendering water heights to a displacement
texture and reading it back to the CPU, accepting a frame of latency</cite>. For Loom this is
disqualifying on two independent grounds: readback timing is not reproducible, so `loom sim --assert`
becomes flaky; and a frame of latency makes buoyancy lag the surface visibly.

**The rule: physics never reads GPU memory.** The CPU evaluates waves analytically; the GPU evaluates
the same function independently for rendering. They agree because they're generated from one source
(§4), not because one copies the other.

This is also why interactive ripples (§7) must be a CPU simulation if they are to affect physics at
all, and why the default should be that they *don't*.

### 5.2 Water depth over destructible terrain is not a constant

UE bakes terrain carving at edit time. Your terrain is a runtime-mutable SDF, so:

- **Depth must be a query, not a cached field.** `depth = surface_height - terrain_height(x, z)`,
  evaluated per sample against the current voxel state.
- **A crater below the waterline is a hole that should fill.** With analytic water it silently
  won't — the plane just passes through. Decide deliberately: either water is a plane that ignores
  terrain changes (cheap, correct for oceans, wrong for a dam), or you run a shallow-water sim (§7).
  Do not leave this undecided, because the agent will blow a hole in a lake bed and file a bug.
- **Wave attenuation needs depth.** UE has `attenuation_depth` for exactly this — waves flatten as
  the bottom rises, which is what makes shorelines look right. Since your terrain moves, so does the
  attenuation.

The honest v1 answer: **water is a plane; terrain destruction below it does not drain it.** Document
that in the format spec as a known limitation rather than letting it be discovered.

### 5.3 Gerstner steepness explodes without a constraint

<cite index="26-1">If Q is too large the wave loops into itself</cite> — the surface self-intersects
and you get a visibly broken, folded mesh. The standard constraint is that summed steepness across
all waves must satisfy `Q_total ≤ 1 / (k * A)` per wave, or in the multi-wave case, normalize by wave
count.

**Put this in the schema validator.** An agent tuning "make the waves choppier" will push steepness
until it breaks, and the failure looks like a rendering bug rather than a parameter error. The
validator should reject it with the computed limit in the message:

```json
{
  "error": "wave_steepness_exceeds_limit",
  "node": "Ocean",
  "wave_index": 2,
  "steepness": 1.4,
  "limit": 0.83,
  "hint": "Q*k*A must stay under 1/N for N waves or the surface self-intersects. Reduce steepness or amplitude."
}
```

Also cap wave count. <cite index="2-1">UE defaults to 16 and notes fewer is more
performant</cite>; make 16 the schema maximum, because per-vertex cost is linear in wave count and
an agent has no intuition for that.

### 5.4 Finite-differenced normals will look wrong

Tempting to compute the surface normal by sampling `sample_water` at three nearby points. Don't —
Gerstner displaces horizontally as well as vertically, so the three samples aren't at the positions
you think they are, and the resulting normal is subtly wrong everywhere and badly wrong at crests.

Derive the normal analytically: the partial derivatives of the Gerstner sum have closed form, and
computing them costs one extra `sin`/`cos` pair you already have from the position evaluation. This
is the same lesson as the terrain doc's analytical-derivative recommendation, and for the same
reason.

### 5.5 Buoyancy without damping oscillates forever

An analytic wave field is an infinite energy source. A pontoon with no damping will gain amplitude
until the object launches. UE ships *three* damping controls plus a velocity ramp because this bites
in practice.

Ship damping on by default with sane values, and add a sim assertion to the test suite:

```
loom sim assets/test/water_crate.loom --ticks 1800 \
    --assert "positions.Crate.y < 2.0" \
    --assert "positions.Crate.y > -0.5"
```

A crate that's still within bounds after 30 seconds of simulated time is a crate that isn't
resonating. Run it in CI; this is exactly the failure mode that a render can't show you (a screenshot
at t=5s looks fine) and a deterministic sim can.

### 5.6 The pontoon count trap

One pontoon gives no torque, so the object spins freely and looks wrong. Too many pontoons is a cost
multiplier — every pontoon is a full `sample_water` plus a force application, per step.

<cite index="9-1">UE's own example uses four pontoons on a cube to keep it upright.</cite> Make 4 the
documented default, validate that a `Buoyancy` component with 1 pontoon on a non-spherical mesh emits
a warning, and cap at something like 16.

Related: **pontoon radius should sum to roughly the object's displaced volume**, not its bounding
volume. An agent will place pontoons at the corners of a bounding box with radii that total 3× the
object's actual volume and then wonder why the crate floats like a cork. Add a validator check
comparing total pontoon volume against the collider volume and warn on a large mismatch.

### 5.7 Underwater is a gameplay state, not just a shader

Once there is water, several systems need to know about submersion, and if you don't plan for it each
one becomes a special case later:

- **Character controller** — swimming, buoyancy on the player capsule, exiting at shorelines
- **Camera** — underwater post-process, which you deferred past M12 so this may just be a color tint
- **Audio** — `loom_audio` traces real geometry for acoustics, so submersion should low-pass; this is
  a small change with a large perceived quality effect
- **Scripts** — `is_submerged(node)` and `water_depth_at(pos)` in the Rhai API, or every game script
  reimplements it badly
- **Particles** — splash on entry, which is `loom_particles` plus an event

Expose one `Submersion` component the systems read, updated once per step, rather than five
independent depth queries.

### 5.8 Rivers need a flow field, and yours should come from the terrain

<cite index="5-1">UE rivers use velocity from spline points written to a flow map</cite> — the artist
draws the river path and the engine derives flow from the spline.

You already compute flow accumulation in `loom_terrain` for erosion. **Derive the river flow field
from that instead of from a hand-drawn spline.** A river then runs where water would actually run,
its velocity follows the real terrain gradient, and the agent authors a river by naming a region
rather than by placing spline control points — which it cannot do well anyway.

This is the one place where Loom's architecture is straightforwardly better than UE's, and it's
almost free because the analysis code exists.

### 5.9 Don't reach for SPH

`salva` is real — <cite index="47-1">2D and 3D SPH fluid simulation in Rust, with optional two-way
coupling to rapier rigid bodies</cite>. It is the wrong tool here. Particle fluids don't scale to a
lake, don't render as a surface without meshing work, and their coupling with rapier is not on
rapier's determinism guarantees.

Height-field water is what games ship. Keep SPH in mind only for small, contained effects (a bucket,
a fountain) and treat it as a separate system if you ever want it.

---

## 6. Build order

Slots after the current work. Each step ends in something runnable, per the brief's rule.

| Step | Work | Exit criterion |
| --- | --- | --- |
| W0 | `WaterBody` component, `WaveSet` schema, validator incl. the steepness limit (§5.3) | `loom validate` accepts a lake, rejects an over-steep ocean with the computed limit |
| W1 | `sample_water` in Rust; analytic normals; unit tests against known values | Determinism hash stable across runs; normals match analytic reference |
| W2 | Slang generation from the Rust source (§4) + CPU/GPU agreement test | Test asserts agreement within tolerance at 500 sample points |
| W3 | Water mesh: quadtree LOD tiles, camera-centered, one mesh for all bodies | `loom render ocean.loom` shows waves; no seams between LOD rings |
| W4 | `Buoyancy` component, pontoon solver, damping, drag | `loom sim water_crate.loom --ticks 1800` — crate floats, doesn't resonate (§5.5) |
| W5 | Terrain depth queries, wave attenuation in shallows, shoreline | Waves flatten approaching a voxel beach |
| W6 | `Submersion` component; Rhai API; audio low-pass; splash particles | A script reads `is_submerged`; entering water is audible and visible |
| W7 | Rivers from `loom_terrain` flow accumulation (§5.8) | A river follows real terrain drainage; a crate dropped upstream arrives downstream |
| W8 | CLI + MCP: `loom water <scene> --at x,z` returns a `WaterSample` as JSON | The agent can ask where the surface is before placing a dock |
| W9 *(optional)* | CPU shallow-water ripple sim, rendering-only by default (§7) | Ripples from a dropped crate, sim hash unchanged with ripples enabled |

W0–W4 is the useful core: authorable water that things float on, verified headlessly. W5–W8 is what
makes it feel like part of the engine rather than a plane with a shader.

---

## 7. Interactive ripples — the optional part

If you want wakes and impact ripples, the height-field wave equation is the standard approach.
<cite index="42-1">The shallow-water height field evolves as `ü = c²∇²u + cα∇²u̇`, where `u` is
height, `c` a wave speed and `α` a damping coefficient, discretized with finite differences into an
explicit update over a grid.</cite> That's a five-tap Laplacian per cell per step — trivially
parallel and cheap at modest resolution.

Two ways to wire it in, and the choice matters:

**Rendering-only (recommended default).** Run the ripple grid on the GPU, add it to the analytic
surface for display, and *don't* feed it back into physics. Physics still sees only the analytic
waves. Determinism is untouched, the visual gain is most of what you wanted, and the cost is that a
boat doesn't ride its own wake. Nobody notices.

**Physics-coupled (opt-in, deterministic).** Run the grid on the CPU at low resolution (128² is
plenty for a local area), inside the fixed timestep, with the state hashed like everything else. Now
ripples affect buoyancy and the sim stays reproducible. Costs a few hundred microseconds per step and
needs the grid to follow the camera without introducing a discontinuity when it moves.

Make it a per-water-body flag with rendering-only as the default, and make the physics-coupled path
prove itself against a determinism test before it ships.

---

## 8. Notes against the current repo

Reading the README, a few concrete integration points:

- **`loom_physics` already runs `enhanced-determinism`** — buoyancy forces must be applied inside the
  same fixed step, before `PhysicsPipeline::step`, in stable order.
- **`loom_measure` is the model for `loom water`** — bounds and overlaps without a render. Water
  sampling is the same shape of tool and should return the same JSON-per-line format.
- **No prefab system yet.** A floating crate is a node with `MeshRenderer` + collider + `Buoyancy`,
  written out per instance. Fine for now, verbose later; this is a small argument for the prefab work
  the format spec already anticipates.
- **`loom_ecs` is `Vec<Option<T>>`, not archetypes.** Buoyancy iterates a small set of entities, so
  this is a non-issue at current scale — don't let water be the reason to rewrite storage.
- **`loom_audio` traces real geometry.** Submersion low-pass is a genuinely cheap win given what's
  already there.
- **Golden-image comparison is still a known gap.** Water is the most visually regression-prone
  system you'll add — small numeric changes produce large visible differences. This may be the
  feature that justifies finally adding pixel diffs to `cargo test`.

---

## Sources

Epic: Water System, Water Body Actors, Water Buoyancy Component, Water Meshing System and Surface
Rendering, Simulating Waves Using the Water Waves Asset, `UWaterMeshComponent`, and the
`BuoyancyComponent` / `BuoyancyData` / `WaterZone` Python API references · GPU Gems 1 ch. 1
(Gerstner/trochoidal waves) and the standard trochoidal formulation · Tessendorf, *Simulating Ocean
Water* (FFT, for context) · DiffTaichi appendix E.4 (shallow-water height-field discretization) ·
Solarflare shallow-water GPU demo · dimforge `rapier3d` and `salva` documentation.
