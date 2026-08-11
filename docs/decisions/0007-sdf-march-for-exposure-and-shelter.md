# ADR 0007 — SDF march, not ray query, for exposure and shelter

- **Date:** 2026-08-11
- **Status:** accepted
- **Decision touched:** new (simulation query); implements LOOM-IMPLEMENTATION-ORDER.md S3

## Context

Three systems want the same number and would each have invented it. Rain needs "is this point
exposed to the sky". Wind needs "how sheltered is this point". Audio needs "interior or exterior".
Written three times they drift, and the drift has no symptom worth noticing: rain falls through a
roof that muffles sound and blocks wind, nothing errors, and no test fails.

The engine already has ray queries — ray-traced acoustics and blast-cover both use rapier against
the collision world.

## Decision

**One function, marching the voxel SDF on the CPU:** `loom_voxel::exposure`.

Ray queries were rejected for this, for four reasons that all point the same way:

1. **Binary.** A ray hits or it does not. Exposure has to be a fraction, or an entity walking under
   a ledge pops between two states and takes the wind, rain and reverb with it.
2. **Not in the simulation.** A GPU ray query cannot feed a deterministic tick.
3. **Destructible by construction.** The march reads the current field, so carving an overhang
   raises exposure in the same tick with nothing to invalidate. A ray query needs an acceleration
   structure rebuilt after every edit.
4. **Deterministic.** A fixed sample count of `f32` arithmetic is reproducible; a BVH traversal is
   not something to stake the sim hash on.

Ray queries remain the better answer for a **visual-only** GPU occlusion pass, if per-drop rain
precision is ever wanted. That is a different question with different requirements.

### Shape

```
exposure(volume, from, direction) -> f32   // 0 sealed .. 1 clear
sky_exposure(volume, from) -> f32          // straight up
```

- **`EXPOSURE_STEPS = 128` samples, `STEP = 0.5` voxels, reach 64 voxels.** A count, never a
  tolerance: a march that stopped on a float comparison could take a different number of steps on
  a different machine, and this value is destined for the simulation hash.
- **No early exit**, though one is obviously available once a sample is fully solid and could not
  change the result. Cost then varies with geometry; a fixed cost is worth more here.
- **Trilinear sampling.** Nearest-neighbour makes the answer jump as an entity moves a fraction of
  a voxel — the popping the whole module exists to prevent.
- **The most obstructed sample, not accumulated absorption.** Two roofs are still one roof.
- A roof beyond reach reads as open sky, and outside the volume is air. Both are documented
  properties with tests, not accidents.

### Two numbers the tests chose, not the design

**`STEP = 0.5` voxels.** One voxel — the thinnest representable feature — looks like the obvious
step and is wrong. A sample can land half a step off the deepest point of a one-voxel slab, read
it as barely outside, and report a roof 84% blocked instead of sealed. The symptom was a roof that
sheltered at some heights and leaked at others. It is the ordinary sampling-rate argument: to
never miss a feature of width *w*, sample at *w/2*.

**`occupancy` is asymmetric.** The natural ramp is centred on the surface, `0.5 - d`, and it only
reaches fully-blocked deep inside solid — so a one-voxel roof never gets a sample deep enough and
leaks about 10% of the sky. Anything at or below the surface is blocked; softening applies only to
near misses outside.

Both were found by a test failing, not by reasoning, which is the argument for having written the
tests first.

### The soft edge is half a voxel wide, and that is a real limit

`SDF_SCALE` spreads the whole `i8` range across ±1 voxel, so the field saturates a voxel from any
surface and no wider gradient is representable. A single ray therefore gives a narrow penumbra. A
genuine "how much of the sky can this point see" needs a **cone of rays averaged**, at N times the
cost. Not built — nothing has asked for it — and this is where to start when something does.

The same saturation is why this marches a fixed step instead of sphere tracing. There is no
long-range distance here to take a big step on, and the reflexive "it's an SDF, so sphere trace
it" would step straight through walls.

### Audio is deliberately not unified with this

S3 says rain, wind and audio "are one function". For rain and wind they are. Audio's `openness` is
**not** replaced, because it casts against the **collision world** — boxes, meshes, everything —
while this marches only the voxel volume. Swapping it would make every scene without voxel terrain
read as wide open, including the test room built from box colliders.

They answer a similar question over different worlds. Unifying them means first making the march
see non-voxel geometry, which is a larger change than S3 and has no caller yet.

## Consequences

- Rain (P4) and wind (P1) take their sheltering from here. Neither may grow its own.
- **It enters the simulation hash when a consumer in the sim uses it.** Nothing consumes it yet, so
  it is not in the hash today; adding a consumer purely to satisfy that would be building P4 early.
  The function is deterministic and tested for bit-identical repeats, which is the part that has to
  be true first.
- **No CLI surface yet.** `loom measure --exposure` is the obvious home and is deliberately not
  built: with no caller, the shape of the arguments — which volume, what direction, one point or
  many — would be guesswork. Add it alongside the first real consumer.
- Cost is 128 samples × 8 voxel reads per query. Fine per entity per tick; not fine per particle.
  A per-drop rain effect wants the GPU pass mentioned above, not this in a loop.

## Human approval

Not required — this adds a query to `loom_voxel` rather than changing a locked decision.
