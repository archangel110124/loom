# ADR 0011 — Water depth comes from one baked terrain height field

- **Date:** 2026-08-12
- **Status:** accepted
- **Decision touched:** new (water); implements W6 of `docs/design/loom-water-system.md`
  §6. Follows ADR 0006's rule about one implementation, and applies it one layer
  down from ADR 0006's own mechanism.

## Context

W6 needs `depth = surface_height − terrain_height(x, z)` on **both sides**. The
CPU needs it because buoyancy is a deterministic simulation input and because
waves flatten in the shallows, which changes where a floating body sits. The GPU
needs it per vertex to attenuate those same waves, per fragment to tint the
shallows, and per fragment to cut the surface off at the shoreline.

That is the two-implementation hazard S2 and ADR 0006 exist to prevent, and here
it is worse than for a field: **a fragment shader cannot march a voxel SDF.** The
volume is a sparse `BTreeMap` of `i8` chunks with an edit layer over it, reached
by a pointer chase and a bisection loop. There is no honest way to transcribe
`loom_cli`'s `surface_height` into Slang, so the ADR 0006 answer — write it once,
generate the other half — does not apply to the query itself.

`loom_cli` already had the shape of the answer for an unrelated reason. Grass
does not march per blade either: it bakes a grid once (measured 27× cheaper than
per-blade marching) and does a bilinear lookup per blade.

## Decision

**Bake once on the CPU; sample the same array on both sides with the same
bilinear.**

- `loom_voxel::heightfield::HeightField` is the grid: world origin, spacing,
  side, and `side²` heights in world Y. `bake` is the SDF march, moved verbatim
  out of `loom_cli` — there is one march in the engine, and grass and water are
  both windows onto it.
- `HeightField::at` is the lookup, and `loom_voxel::heightfield::slang()` is its
  twin, emitted into `assets/shaders/generated/water.slang` by
  `loom_render/build.rs` beside `loom_water::slang()`. The two are one function
  written twice, in one file, and `loom_water`'s agreement test compiles the
  Slang through `slangc -target cpp` and compares both the ground height and the
  depth against the Rust at 512 points. **Worst absolute difference: exactly
  0.0.** It was 4.8e-7 until the Rust lerp stopped using `mul_add` — an fma
  rounds once where the shader rounds twice — which is the entire argument for
  measuring rather than asserting.
- The grid reaches the GPU as a `float*` in the environment buffer, uploaded by
  `set_terrain` when the scene loads or the terrain changes, never per frame.
  The push block was already at 124 of its 128 bytes.
- A column with no surface is `NO_GROUND = −1e9`, **not a `NaN`**: the bilinear
  is plain arithmetic with no branch, so the sentinel has to survive being
  interpolated with a real height, and a `NaN` in a shader is a silent failure of
  every comparison it touches. Downstream it reads as "unfathomably deep", which
  is the right answer for a sea with no terrain authored under it.

### Attenuation: `tanh(k·d) / tanh(k·D)`

`WaveSet::attenuation_depth` (`D`) finally drives something. Per wave, amplitude
is scaled by `clamp(tanh(k·d) / tanh(k·D), 0, 1)`; `D ≤ 0` disables it, which is
the schema default and what keeps every scene written before this identical.

`tanh(k·d)` is linear wave theory's depth dependence — the same factor in
`ω² = g·k·tanh(k·d)` that deep water already assumes away. Normalising by
`tanh(k·D)` makes `D` mean what the schema says: full height at and beyond it,
flattening below it, zero at the shoreline. Long swell feels the bottom in
deeper water than short chop does, which comes free from `k` being in there.

Three real things are **not** modelled, deliberately:

- **No shoaling amplification.** Real waves grow before they break. The taper is
  monotone into the shore and never exceeds 1, and that is what makes the fold
  limit free: the validator proves `Σ Q·k·A ≤ 1`, the condition is linear in `A`,
  so scaling every amplitude by a factor in `[0, 1]` cannot break a bound that
  already holds. Amplification would need its own clamp and its own proof, and it
  belongs with breaking and foam in W7.
- **No refraction.** Crests do not turn to run parallel to the beach; that needs
  a direction that varies with position, which makes the phase a path integral
  and the surface no longer an analytic function of `(x, z, t)`.
- **No wavelength shortening.** `ω` stays `sqrt(g·k)`, for the same reason.

The one place shoaling *can* fold a surface is a bed steep enough that the taper
itself varies fast with x, adding a `dA/dx` term the flat-bed argument does not
cover. Measured: **between 1:1 and 1.1:1**, and only for a wave set sitting
exactly on the fold limit. Real beaches are 1:20 to 1:100 and the gate's scene is
about 1:4. It is a documented bound with a test on it
(`a_steep_enough_bed_is_where_shoaling_could_fold`), not a fix.

### The shoreline

The fragment shader discards where the interpolated depth is negative. A discard
rather than a fade because the surface is opaque and writes depth; a fade would
need blending, a sort against the terrain behind it, and an answer for what a
half-transparent sea reflects. The cut is not raw, though: the taper flattens the
waves to nothing as they reach it, so the line runs along still water rather than
through a moving crest.

## Consequences

**Destructible terrain (§5.2) rebakes on the edit, not on the frame, and not
inside a running sim.** The viewer keys the upload on the volume's op list
(`terrain_key`), so the transaction that blows a crater under a lake rebakes the
grid and the shoreline moves with it. `Sim` bakes once at `new` and never again:
carve mid-run and the water sees the old bed until the scene reloads. Rebaking
costs the march — 193² samples on the gate's scene, tenths of a second — which is
edit-time work, not frame work, and there is no incremental path yet. The
smallest honest one when it is needed: `Volume::edit` already returns the chunks
it touched, so only the columns over those chunks need re-marching.

**Water is still a plane that terrain does not drain.** Depth changes how big the
waves are and where the surface stops; it does not empty a lake through a hole in
its bed. That remains the stated v1 limitation.

**A grid boundary is a depth discontinuity, and it is visible.** Outside the
baked window there is no terrain and the water is bottomless, so anything that
reads depth has to be *saturated* at the volume's edge or the boundary draws
itself in the sea. The first version of `shore.loom` put its floor four metres
down — exactly `attenuation_depth`, which was enough for the waves and not for
the tint, whose ramp is five — and a top-down shot showed a hard-edged rectangle
around the terrain. The rule is therefore stronger than "at or below `D`": **the
bed at the volume's edge must be deeper than everything that reads depth**, which
today is `max(attenuation_depth, WATER_TINT_DEPTH)`. `shore.loom` uses six metres
and says why at the top of the file.

**The sentinel must not reach the fragment stage.** `depth` is an interpolated
vertex attribute, and a triangle with one vertex on the grid and one off it
perspective-divides a 6 against a 10⁹. That loses the small end badly enough to
come out negative on scattered pixels, the shoreline discard ate them, and the
grid's outline appeared in the water as a dotted line — a second, subtler version
of the same rectangle. `waterVertexMain` clamps the attribute to 100 m, which is
far past any use of the value and interpolates exactly. Found by looking at a
render, which is the only thing that would have found it.

**It found a half-voxel error in the mesher, and fixing it moved two
references.** `Volume::world_of` places voxel `k` at `(k + 0.5) · voxel_size`,
`solid_cells` hands parry the same convention, and `exposure::sample` inverts it
— but `SurfaceNets::mesh_chunk` mapped sampled index `p` to `(p − 1) · size`
instead of `(p − 0.5) · size`, so **every voxel mesh in the project was drawn
half a voxel low on all three axes** while its collider and its SDF sat where the
author put them. Measured against an authored box top at y = 4.0: field 4.0002,
collider 4.0000, mesh 3.8750 at 0.25 m voxels and 3.7500 at 0.5 m.

Nothing had drawn two conventions into one pixel before, so nothing showed it.
W6 does: the sea was cut where the SDF says the beach breaks the surface, and
against a mesh 0.15 m low that left 0.57 m of dry sand the water never reached.
The fix is one line in `mesh.rs`, pinned by
`the_mesh_the_collider_and_the_field_place_a_surface_in_the_same_place`, and it
also puts grass back on the ground it is draped over and the drawn geometry back
where the collider is. `grass_slope` and `cave` moved as a result — grass_slope
visibly, cave inside the tolerance — and were re-blessed deliberately. **Nothing
else in W6 depends on that one line**, if it is wanted as its own commit.

**Grass and water bake separately.** They want different windows: grass a margin
around one field, water the volume's whole footprint. Two marches of one
implementation, rather than one march of the coarser of the two.

**256² is the ceiling.** `MAX_SIDE` caps the grid and the GPU buffer at 256 KB;
past it the bake coarsens rather than growing, so a 500 m terrain gets 2 m
samples and a mushier shoreline. Raising it is a constant and a buffer size.

## Human approval

Not required: nothing in CLAUDE.md's locked table changes. The locked decisions
this leans on — descriptor indexing and buffer device address, no readback for
physics, op-list voxels — are all followed rather than amended.
