# ADR 0050 — Water reflects the scene, and smoke is a marched volume

- **Date:** 2026-08-17
- **Status:** **accepted**
- **Numbering:** above 0049, for the reason ADR 0045 gives. 0046 stays reserved
  for interactive ripples.
- **Governed by:** ADR 0045. Both items are rendering-only. Neither produces a
  force, neither is readable by `loom sim --assert` or by `rhai`, and neither
  adds state — a frame stays a pure function of (scene, tick), so **neither
  adds an obligation to `cargo xtask repeat`**. Byte-identity across three fresh
  processes was checked anyway, by hand, for both new scenes.
- **Extends:** ADR 0019 (W3 — the inline ray query, its sky fallback and its
  radiance clamp) and ADR 0020 (W4 — the line-integral volume).
- **Decision touched:** none of CLAUDE.md's locked decisions.

Two milestones, one record, because they turn out to share one fact: **the TLAS
holds meshes only.**

---

## W3 — one traced reflection ray on water

### Context

ADR 0019 put `tracedEnvironment` in `scene.slang` — an inline ray query from the
fragment shader against the existing TLAS, falling back to an analytic sky on a
miss, clamped by `REFLECT_MAX_RADIANCE`. Opaque surfaces have used it since.
Water did not, "deliberately", and the recorded reason was that a sea reflects
the sky. That is true of open sea and false of every scene with a bank, a hull
or a quay in it: `water_crate.loom` had a post standing in water that reflected
the weather and not the post.

### Decision

`waterFragmentMain`'s reflection term is one call to the same function. Fresnel,
the refracted `through` term, subsurface scattering and the specular sun
highlight are all untouched; only what goes into the fresnel `lerp` changes.

**`tracedEnvironment` gains one parameter, `float3 miss`,** replacing its
hard-coded `skyGradient(dir)`. The opaque caller passes `skyGradient(lobe)` and
is bit-identical — `materials`, `primitives`, `cave`, `glass`, `forest`,
`stoneyard` and `proving_ground` all compare at worst-channel **0** against
their committed references. Water passes `skyColor(mirror)`, because the sun's
glitter path on a sea *is* the reflected sun disc and the gradient has no disc,
no glow and no cloud deck in it. Same function, two skies, one line.

**Three water-specific guards, each measured rather than assumed.**

1. **The mirror is clamped into the mesh normal's hemisphere.** The capillary
   ripples perturb the shading normal several degrees; a mirror direction
   reflected off the perturbed normal can point *below* the true surface, so the
   ray starts inside the seabed and returns a lit sand sample — one bright pixel
   among dark ones, which is what a firefly is. `flatN` is captured before the
   ripple block and the mirror is folded back into its hemisphere.
2. **`WATER_REFLECT_BIAS = 0.02` m**, not the opaque path's millimetre. The
   water mesh is vertex-displaced from a wave sum, so an interpolated fragment
   position sits off the true surface by up to half a cell's curvature and the
   near ring's cell is 0.5 m.
3. **The lobe widens with distance** — `saturate(WATER_REFLECT_ROUGH +
   length(toEye) / WATER_REFLECT_RANGE)`, with `WATER_REFLECT_ROUGH = 0.06` and
   `WATER_REFLECT_RANGE = 25.0`. This is the firefly fix and the table below is
   why.

**`WATER_CLARITY_ROUGH` (0.22) is deliberately not reused as the trace
roughness.** `tracedEnvironment` uses roughness twice — once to widen the lobe
and once to `lerp` the traced answer back to the miss colour, reaching it
exactly at 1.0 — so passing 0.22 would dilute every traced reflection 22% toward
sky. The sea's broadening comes from the ripples perturbing the normal per
pixel, which is geometry; paying for it a second time as a blend washes the
reflection out.

### The fireflies are geometric, and the clamp is not the tool

ADR 0019's fireflies were radiometric: a ray landing near a point light returned
two orders of magnitude above its neighbours, and `REFLECT_MAX_RADIANCE` fixed
it. **The water ones are not that.** `shore.loom` contains no point light at all
and still gains hundreds of salt pixels from a flat lobe. The variance is
geometric — sub-pixel wave crests scattering the mirror direction, so
neighbouring pixels sample unrelated parts of the scene.

Salt (pixels more than 24/255 from the median of their eight neighbours — ADR
0019's instrument), at 960x600, added over a build with the trace stubbed back
to `skyColor(mirror)`; `worst` is the worst channel against that same stub:

| `WATER_REFLECT_RANGE` | shore | shore worst | homestead | mirrorpool |
| --- | --- | --- | --- | --- |
| ∞ (flat lobe) | +443 | **101** | +184 | +1144 |
| 200 m | +427 | 92 | +139 | +975 |
| 60 m | +334 | 75 | +82 | +616 |
| **25 m** | **+51** | **51** | **+45** | **+84** |
| 12 m | −8 | 21 | +0 | +100 |

**A flat lobe puts `shore` at worst-channel 101, which is over the golden gate's
own 72** — an unclamped-shaped failure arriving by a different route. 25 m is
the knee: it is where the far chop stops contributing twinkle, and the far chop
was contributing no picture. 12 m has begun deleting the reflection instead of
de-noising it.

`REFLECT_MAX_RADIANCE = 4.0` is inherited unchanged. **Never lower it globally**
to chase these — it is the wrong instrument for a geometric firefly, and
clamping at the water call site would clip the reflected sun disc on a *miss*,
which is the one thing this reflection most needs to keep.

### Cost

Water pass, 1920x1080, median of five, traced against stubbed:

| scene | stub | traced | delta |
| --- | --- | --- | --- |
| ocean | 0.253 | 0.282 | **+0.029** |
| mirrorpool | 0.161 | 0.190 | +0.029 |
| homestead | 0.268 | 0.294 | +0.026 |
| shore | 0.268 | 0.301 | +0.033 |

**+0.03 ms, flat.** No fog gate, no roughness cutoff, no contingency.

### The Rust change, which is a validation requirement

`Raytracer::build_instances` no longer early-returns on an empty scene: a
zero-instance TLAS is always built, so every ray simply misses.

This is not tidiness. `renderer.rs:1911` binds descriptor set 0 only when
`Raytracer::ready()`, and `ready()` is `tlas.is_some()`. That was safe for
exactly as long as every pipeline *statically using* `sceneTLAS` also had a mesh
in the frame. The water shader now statically uses it, so a `WaterBody` with no
`MeshRenderer` draws with set 0 unbound — `VUID-vkCmdDraw-None-08600`.

**No scene in the repository was covering that**, so the validate gate would
have passed with the bug present. `assets/test/bare_sea.loom` is added to
`SCENES` for it and for nothing else; reverting the fix fires the VUID on that
scene and on no other, which is how the guard is known to be one. The same fix
closes a pre-existing hole: `cutout` objects are deliberately kept out of the
TLAS, so a scene of nothing but alpha-cutout meshes had the identical shape and
there is no such scene either.

### Not extended, on purpose

- **The underwater TIR branch.** It needs `hitT` returned from
  `tracedEnvironment` to attenuate the reflected leg through the water, and
  without it the mirrored seabed renders *brighter* than the direct view of the
  same bed through the same water.
- **The puddle path**, which is a separate measurement.

---

## W4 — smoke becomes a marched soot volume

### Context

ADR 0020 made a flame one quad with a line integral across it: `FIRE_STEPS`
samples along the view ray through an emitting field, overdraw of exactly one,
no population. Smoke in this engine is still sprite billboards. The research
doc's §3 asks for a marched soot field; the report's §3.1 gas solver was priced
and refused (OVERNIGHT-DECISIONS D8).

### Decision

**One quad per plume, selected by `flame = true, additive = false`.** Both
varyings already exist — the sign of the authored red carries `flame`, the sign
of the radius carries `additive` — so the selector costs no schema, no new
packed bit and no new varying. A marched field that does not emit is a field
that only absorbs, which is what soot is.

**The pair is authored by no existing scene**: all three `flame = true` scenes
in the repository are additive. That is not an argument, it is a containment
proof, and it is measured — `smoke`, `campfire`, `emberfall`, `explosion`,
`windy` and `materials` all compare at worst-channel **0**.

`smokeColor` marches `SMOKE_STEPS` samples along the analytic chord of an
ellipsoid (`SMOKE_ASPECT = 0.55`, solved in unit-sphere space where the ellipsoid
is a sphere and `t` stays in world metres), trapezoid rule, both Nyquist bounds
combined by `min` — all of it verbatim from `flameColor`, including the reason
the trapezoid is not a nicety.

- **`sootEnvelope`** is the column before any noise: foot ramp, `h^0.75` flare,
  top dissipation, and a downwind bend as `h^1.6` driven by P1's wind out of the
  environment buffer. Superlinear because drag on a rising parcel has had longer
  to act on it; a linear lean is a tilted cylinder.
- **`sootDensity`** evaluates `fireFbm` on a *material* coordinate —
  `p.y − SMOKE_RISE·t` — so a feature is born at the source and carried up
  rather than the field sliding through a stationary envelope.
- **The warp is the perpendicular gradient of one scalar stream function** in
  the meridional plane, three fbm taps. `fireField`'s two-field warp is not
  reused, and that is the point: two independent noise displacements have
  sources and sinks, which over a flame's ~0.2 s residence time is invisible and
  over a plume's several seconds pulls density into knots — the procedural-blob
  failure D8 names. A stream function's perpendicular gradient is
  divergence-free in the plane it is taken in, which is the plane a plume's
  vortex rings actually roll in.
- **Lighting per sample:** Beer–Lambert transmittance, Henyey–Greenstein at
  `SMOKE_G = 0.42` against the sun, sky ambient floored by envelope depth, and
  `pointLights` so the fire lights the underside of its own plume.
- **Self-shadow is two density taps** along the analytic chord from the sample
  to where the sun leaves the ellipsoid — no second march. The chord is closed
  form, so the cost is two density evaluations, and the envelope is a good model
  of the column doing the occluding, which is what makes two taps enough. This
  is the thing ADR 0020 priced at an offscreen R16F optical-depth target and
  refused.

### `SMOKE_HZ` was 2.6, and that was the bug the whole item nearly shipped with

`SMOKE_HZ` is cycles of the coarsest octave across the plume's **half-height**,
and `SMOKE_ASPECT` makes the column 0.55 as wide as it is tall. At 2.6 that is
**1.4 cycles across the entire width** — one blob of noise laid over the
envelope. The envelope's radial term is a linear cone with a definite boundary,
and with a single blob to erode it, **whether the silhouette read as smoke or as
a hard-edged ellipsoid was a lottery on which region of the noise the plume
happened to land in.**

It was found by fixing something else. Decorrelating two plumes (below) moves
the field by a bounded offset and nothing more — and a wispy column became a
solid oval with a crisp arc down its right side. A parameter that has no
business changing the *kind* of thing being drawn had changed it, which is the
tell that the shape was resting on a coincidence.

At **6.5** there are ~3.6 cycles across the width, the noise erodes the cone
everywhere, and the silhouette belongs to the field rather than to the envelope.
Rendered at offsets 0, 8 and 32 it reads as the same kind of thing three times,
which is the property that was missing. It is scale-invariant by construction —
the field is sampled at `SMOKE_HZ / R` — so it is a statement about how detailed
soot is and not a per-scene tune, and it costs nothing: the same three octaves,
with the step bound retiring the finest to ~0.33 in exchange.

**The general lesson is the one this project keeps relearning.** A procedural
look that is a draw from a distribution rather than a property of the
construction will pass every gate on the day it is blessed and change character
the first time anything perturbs its input. The instrument is not a metric — it
is rendering the same thing under three offsets and asking whether it is
recognisably the same thing.

### Two plumes in one scene must not be one plume twice

Everything in `sootDensity` is a function of plume-local position and time, so a
second emitter marched a bit-identical field. `sootOffset(in.seed)` shifts where
in the noise each plume reads.

**Bounded, and that is the whole design of it.** `in.seed` is `float(index)`,
which reaches 16,200 on `emberfall`; `seed * scale` at that magnitude lands the
lattice coordinate where a `float` has no fractional bits left and the field
degenerates. `frac` of an irrational multiple is equidistributed over `[0, 1)`
and never leaves it. Three different irrationals, so the offset is a point in
the volume and not a slide along its diagonal. **The unbounded form is live one
function over** — `in.seed * 7.31` in the sprite flipbook path — and is safe only
because those populations have stayed small.

**The offset is added at the noise lookup, never to the coordinate.** The twist
rotates the material coordinate about the column's axis by an angle that grows
with height, so an offset carried through it becomes a second, enormous twist;
and the warp takes its radial direction from `m.xz`, so an offset of tens of
units there swamps the radius and leaves the direction nearly constant, which
destroys the meridional-plane property the stream function is built on.

### `SMOKE_STEPS = 20`, and the still is the wrong instrument for it

`plume` at 1920x1080, forward pass, against a 64-step march as the converged
reference; flicker at the scene's own camera held still, 12 ticks apart:

| steps | 4 | 6 | 8 | 12 | 16 | **20** | 28 | 64 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| forward ms | 0.518 | 0.554 | 0.583 | 0.660 | 0.735 | **0.799** | 0.940 | 1.70 |
| mean diff | 0.661 | 0.366 | 0.246 | 0.155 | 0.110 | **0.083** | 0.052 | — |
| picture/ms | — | 8.4 | 4.0 | 1.22 | 0.60 | **0.42** | 0.22 | — |
| flicker | — | — | 0.1223 | 0.1152 | 0.1129 | **0.1111** | 0.1122 | — |

Picture-per-millisecond has a knee between 8 and 12 and is flat after it, so a
still alone argues for 12. **Flicker is what argues for 20.** The sample
positions slide along the ray as the camera or the field moves, so an
under-resolved march does not look soft — it *crawls*. Flicker falls to a floor
at 16–20 and 28 does not beat 20; that last pair is the noise floor, which is
how the floor is known to be one. A 12-step build is a legitimate quality
setting, 0.14 ms cheaper, reaching the same still.

### Cost, and overdraw is the risk fire did not have

1920x1080, median of five. Baseline is the fixed cost read off the step sweep's
own slope (0.0176 ms/step), which agrees with a stubbed build:

| | forward ms | plume's share |
| --- | --- | --- |
| baseline (no volume) | 0.45 | — |
| authored camera, ~7% coverage | 0.80 | 0.35 |
| walked in, ~45% coverage | 1.73 | 1.28 |

Extrapolated to full-screen that is **~2.8 ms**, which is the budget line to
quote: this is the first effect in the engine whose cost is set by *coverage*
rather than by population. One quad per plume means overdraw of exactly one, so
the number is `covered pixels x steps` and both terms are visible constants —
which is the thing a sprite plume cannot say, because thirty overlapping
translucent puffs each restart their own work.

**A close-up plume is softer than a distant one, and that is the step bound
working.** Walking in makes the screen-space Nyquist term go to 1 while the
chord — and therefore `dt` — grows, so the field's fine octaves retire. The
alternative is a march that crawls. It is the ceiling on this shape and the
lever is `SMOKE_STEPS`.

---

## Consequences

- **The TLAS holds meshes only, and this is now load-bearing in two places.**
  Grass, water, rain, fire and smoke are all generated from `SV_VertexID`, so
  none of them is in the acceleration structure and none can be hit by any ray.
  **A reflected flame or plume does not appear** — `plume`'s own fire is not in
  its reflection anywhere, and `mirrorpool` reflects the shed and the drum and
  nothing generated. Their *light* does reach the water, through `pointLights`,
  which is the term that makes the omission survivable. **Anything that wants to
  be reflected has to become an `Object`.**
- **`SCENES` 46 → 49, `GOLDEN` 33 → 35.** `plume` (`--sim 200`) and
  `mirrorpool` (`--sim 90`) are new references; `bare_sea` is in `SCENES` only.
- **Eight water references move and every non-water reference does not.** At the
  gate's own tolerance: `ocean` (3.3% of pixels, worst 41), `river` (5.6%, 38),
  `splash` (5.9%, 60), `water_crate` (5.6%, 53), `shore` (1.8%, 45), `homestead`
  (1.6%, 15), `beach` (0.8%, 36), `whitecaps` (0.8%, 29). `lanternhead` moves 7
  pixels at worst 4 and still passes. Worst channel across all of them is 60,
  under the gate's 72, so none of them is moving on noise.
- **Neither item is stateful.** No new render-graph `Access` variant, no buffer,
  no barrier outside the graph, no readback, no grid. `plume` and `mirrorpool`
  each render byte-identically across three fresh processes.
- Nothing here is readable by `loom sim --assert`, by design — ADR 0045
  clause 1.

## What this does not settle

- The underwater TIR reflection and the puddle reflection, both named above.
- Whether a plume the camera walks *into* wants a chord-proportional step count.
- Whether soot should occlude the flame it rises from more strongly than
  alpha-over-additive sorting gives it. `plume` is the only frame in the
  repository with a marched additive quad and a marched alpha quad in it, so it
  is where that question will be asked.
