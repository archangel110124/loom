# ADR 0019 — Soft shadows, ambient occlusion and reflections, all as ray queries

- **Date:** 2026-08-15
- **Status:** **accepted**
- **Decision touched:** `docs/design/POST-STACK-PLAN.md`'s SSAO deferral, which
  asked for "its own ADR, and an answer to what a 45,000-blade depth hairball
  does to a hemisphere kernel". This is that ADR and the answer is that the
  kernel is not a kernel. No locked decision in CLAUDE.md moves.
- **Also touched:** ADR 0010 and ADR 0018's rule that a frame is a function of
  its tick. This ADR keeps it; §"Determinism" says how.

## Context — the hardware was there and one ray was using it

The renderer has had `VK_KHR_ray_query`, an acceleration structure and a TLAS
rebuilt every frame since before M12, and it used all of it for exactly one
thing: `sunVisibility`, one hard shadow ray per lit pixel. Three lighting terms
in this shader were being approximated with something worse *while an
acceleration structure sat next to them*:

- **Shadow edges were binary.** A shadow either fell on a pixel or did not,
  which is the shadow a point-source sun casts and no real one does.
- **There was no ambient occlusion at all**, and the code said so in two
  places: `scene.slang` fakes it for grass with a fixed base darken and the
  comment calls it "the stand-in for the ambient occlusion this renderer does
  not have", and the post-stack plan deferred SSAO as "the one item that is a
  missing *lighting term* rather than a look".
- **Reflections were an analytic sky gradient**, so a mirror in a room
  reflected the weather through the ceiling, and `lanternhead`'s wet quay — the
  scene composed around what wet stone does under a low sun — reflected
  everything about the sky and nothing about the quay.

## Decision

**All three are inline ray queries from the existing fragment shader, against
the existing TLAS, through the existing descriptor set. No new pass, no new
descriptor, no new buffer, no barrier.** The whole feature is `scene.slang` plus
one field of one struct in `raytrace.rs`.

### Why ray query and not a ray-tracing pipeline

`VK_KHR_ray_tracing_pipeline` is not enabled and is not wanted. Every ray here
is secondary — the shading position is already known, from a rasterised
fragment — so there is nothing for a ray-generation shader to generate. The
pipeline variant would add a shader binding table, three shader stages, a second
pipeline object and a second dispatch to arrive at the same visibility answers,
and it would move shading out of the pass that already has the material, the
normal, the roughness and the light list in registers. `raytrace.rs`'s module
header already said this for shadows; it holds for the other two, and more
strongly, because reflections want `pointLights` and `hemisphereAmbient` — code
that lives in the fragment shader and would have to be duplicated into a hit
shader.

The cost of that choice is that these rays are traced at raster rates: one set
per *fragment*, including fragments that are later overdrawn. Measured against
it is that the whole thing is 0.16–0.43 ms at 1920x1080.

### Soft sun shadows

`SUN_RAYS = 4` samples of a disc of angular radius `SUN_ANGLE_DEGREES = 2.0`,
uniformly over the disc, jittered per pixel.

**The angle is a look parameter and the measurement is what establishes that.**
It is an angular *radius*, not a diameter: the sun's disc is 0.53° across, so
its radius is 0.265°, and the smallest row measured is 0.5° — already nearly
twice the real sun. Even there, a one-ray render of `materials` differs from a
converged 64-ray one in 0.13% of pixels, against the golden gate's own 0.1%
threshold for "a different image". There is no penumbra to resolve at anything
physical, and four rays there would be three rays spent on nothing.

So what is measured instead is how far *past* physical the source has to go
before a penumbra exists at all; converged renders of `materials` at 640x400,
each against the 0.5° one:

| radius | mean | worst | pixels past tolerance |
| --- | --- | --- | --- |
| 2.0° | 0.0139 | 44 | 0.28% |
| 4.0° | 0.0404 | 58 | 0.58% |

2° is the smallest widening the project's own gate would call a different
picture, and gives a penumbra of about 3.5 cm per metre of blocker distance —
narrow enough that a contact shadow stays one.

**The ray count was chosen on the cost curve, not the noise curve.** Noise
against a 64-ray reference at 2° on `materials`, and cost on `lanternhead` at
1920x1080 with the other two features off:

| rays | 1 | 2 | 4 | 8 | 16 |
| --- | --- | --- | --- | --- | --- |
| worst channel | 77 | 43 | 36 | 28 | 26 |
| mean | .0257 | .0160 | .0115 | .0071 | .0047 |
| forward ms | 0.452 | 0.456 | 0.467 | 0.559 | 0.851 |

Rays two through four cost **0.005 ms each** — sun rays are coherent, every
pixel sending one in nearly the same direction — and rays five through eight
cost 0.023 ms each, nine through sixteen 0.037 ms each. The cheap half of the
curve ends exactly where the worst-channel error has already dropped from 77
(above the gate's 72) to 36 (well under it).

### Ray-traced ambient occlusion, and the SSAO deferral

`AO_RAYS = 8` cosine-weighted samples of the hemisphere, `AO_RANGE = 1.0` m,
multiplying the sky ambient term and nothing else.

**The grass objection dissolves rather than being solved.** The post-stack plan
deferred SSAO because "45,000 blades produce a depth hairball a hemisphere
kernel will read as one enormous concavity, which is a design problem, not a
tuning one". Every word of that is about the *depth buffer*. A ray does not read
one. Grass is generated in the vertex shader from `SV_VertexID` with no vertex
or index buffer, so it is not in the acceleration structure, cannot be hit, and
occludes nothing — which is exactly the behaviour SSAO could not be made to
have. `meadow` and `grass_slope` are two of the ten golden references that do
**not** move past tolerance. `grass_slope` does change bytes, and correctly so:
its *terrain* is a voxel mesh and is in the TLAS, so the ground under the field
occludes even though the field does not.

That is a coincidence of the implementation and not a physical claim: real grass
does occlude, and if blades ever enter the TLAS the objection comes back in its
original form. The blade-root darkening in `grassFragmentMain` stays exactly as
it is — it is a blade shaded by its neighbours inside a clump, which is a
different fact from the field shading the ground and is not something this term
could ever supply.

**The range was chosen where two saturation curves diverge.** Against a stubbed
build on `primitives` at 640x400 with 32 rays:

| range | 0.25 | 0.5 | 1.0 | 2.0 | 4.0 | ∞ |
| --- | --- | --- | --- | --- | --- | --- |
| worst (deepest contact) | 32 | 70 | 80 | 84 | 84 | 86 |
| mean (whole-frame dimming) | .0056 | .0536 | .1506 | .2796 | .3839 | .4023 |

At 1 m the deepest contact is at 93% of its unbounded value while the overall
dimming is at 37% of it. 2 m buys 5% more contact for double the flat dimming;
0.5 m gives up 12% of the contact for very little. The gap between those two
columns *is* ambient occlusion — a term that darkens contacts and open ground
equally is not AO, it is exposure.

**Eight rays, and the reason is an occupancy cliff.** Noise against a 64-ray
reference on `primitives`, cost on `lanternhead` at 1920x1080 over the 4-ray
shadow:

| rays | 2 | 4 | 8 | 16 | 32 |
| --- | --- | --- | --- | --- | --- |
| worst channel | 84 | 64 | 41 | 24 | 24 |
| mean | .1201 | .0712 | .0415 | .0250 | .0146 |
| forward ms | 0.511 | 0.605 | 0.806 | 1.618 | 2.100 |

Rays cost 0.022–0.025 ms each up to eight and then **0.101 ms each** from eight
to sixteen, with nothing changing in the shader but the trip count. Sixteen rays
would cost more than the entire rest of the frame to move the worst-channel
error from 41 to 24, both well inside the gate's 72.

An AO ray costs five times a sun ray at the same count, and the reason is
coherence: sun rays share their traversal, hemisphere samples do not.

### Reflections, and the geometry that is not there

One ray along the roughness-widened lobe. On a miss it returns the sky gradient
— which is the term it replaces, so a scene with nothing to reflect is
unchanged. On a hit it shades the surface with that object's albedo, the sky
hemisphere, the point lights, and one hard sun-visibility ray, then fogs it over
the distance travelled and blends toward the sky by roughness.

**The missing-geometry problem, which is the interesting one.** The TLAS holds
one instance per `Object` — meshes only. Grass, water, rain, fire and smoke are
all vertex-shader geometry with no buffers, so none of them is in it. In
`lanternhead` this is not hypothetical: the subject is a wet quay with a brazier
two metres from the camera, and a reflective quay that shows the shed but not
the flame could easily read as worse than the sky-only reflection it replaces.

It was measured before it was believed, and three things came out of it:

1. **The fire's *light* does arrive.** The reflected hit is shaded by the same
   `pointLights` the primary surface is, so the brazier's pool of orange on the
   quay appears in the quay's reflection. A brazier's contribution to a
   reflection at these roughnesses is mostly its pool, not its flame.
2. **The flame's own absence is hard to see** because the quay's normal is
   ripple-perturbed and the whole term is weighted by `f0` = 0.04 for a
   dielectric. What the reflection actually contributes on this scene is the
   shed's dark mass on the right-hand quay and a general scene-coloured tint,
   both of which read as wet stone.
3. **It produced fireflies, and that was the real problem.** Neighbouring
   pixels on rippled wet stone send their rays to quite different places, and a
   ray landing near a lantern comes back two orders of magnitude above its
   neighbours — one sample of a high-variance integrand. Isolated orange pixels
   across the quay, worst-channel error 173 against a stubbed build, more than
   twice the gate's 72. An unclamped build would have failed the image gate on
   noise rather than on content.

The two standard cures are both closed here — more samples costs, and temporal
accumulation is what ADR 0010 and ADR 0018 forbid — so the third is used:
`REFLECT_MAX_RADIANCE`, one `min`, the same shape as the existing `LIGHT_MAX`.
Chosen by counting salt (pixels more than 24/255 from the median of their eight
neighbours) on `lanternhead` at 960x600, over a stubbed build's own 8,174:

| clamp | 1 | 2 | 4 | 8 | 16 | 64 | none |
| --- | --- | --- | --- | --- | --- | --- | --- |
| salt added | 0 | 3 | 10 | 37 | 39 | 39 | 39 |
| worst channel | 36 | 36 | 36 | 56 | 86 | 166 | 173 |
| mean | .0954 | .0970 | .0973 | .0979 | .0990 | .1010 | .1006 |

**4.0** — the last value at the worst-channel floor of 36, adding 10 salt pixels
in 576,000 and keeping 97% of the term's brightness.

**A roughness cutoff was measured and removed.** The obvious shape is "trace
below some roughness, sky above it", and a single mirror ray genuinely is a poor
sample of a wide lobe. But the roughness blend already weights the traced answer
to zero at roughness 1, so a cutoff is a cost argument — and the cost is not
there. On `lanternhead` at 1920x1080 against a stub at 0.782 ms:

| cutoff | 0.10 | 0.20 | 0.35 | 0.50 | none |
| --- | --- | --- | --- | --- | --- |
| forward ms | 0.776 | 0.801 | 0.839 | 0.857 | 0.885 |
| pixels changed | 0% | 0.002% | 1.51% | 3.00% | 4.39% |
| salt added | 0 | 0 | 3 | −4 | 20 |

Picture per millisecond is flat or rising across every band (2.5, 3.3, 3.5), so
there is no knee to put a cutoff at, and salt says the wide-lobe samples are not
blotching. Removing it deletes a constant and a branch.

**`REFLECT_RANGE = 60` m** because fog makes a bound free: 60 m is bit for bit
the unbounded answer on both `lanternhead` and `materials` (30 m is one channel
off in one pixel, 15 m is visibly short). It is per-scene in a way the constant
does not admit — it is really a function of `fogDensity()` — and the honest
version is worth writing when a clear-air scene with a mirror exists.

**The reflected hit's normal is the reversed ray direction.** Exact when the
surface faces the ray and wrong by the angle of incidence otherwise, so it is
right at the centre of a reflection and degrades at its edges. The correct
normal needs the triangle, which needs the index buffer, a per-mesh first index,
and three vertex fetches per reflected pixel — and the push block is at 124 of
its 128 bytes, so the index pointer would have to move into the environment
buffer. That is a real change and it is not this one.

### Object identity, and the one Rust-side change

`build_instances` now packs the object's index into the TLAS instance's
`instanceCustomIndex`, where it was 0. `CommittedInstanceID()` reads it back and
indexes `push.objects` directly. This is the entire channel from traversal to
shading, and it works because `pack_objects` and `build_instances` are handed
the same mesh-sorted slice, in the same order, by both the offscreen and the
windowed path. Nothing validates that pairing; the two calls sit next to each
other in `render` for that reason and the comment says so.

## Determinism

**A frame is still a function of its tick, and this was a constraint on the
design rather than a property checked afterwards.**

Every sample direction is a function of the pixel's integer coordinate and the
sample index and nothing else: `rayDither` folds the pixel through `loom_hash`
— the project's one integer hash, frozen ABI, the same one grass places blades
with — and `raySample` advances the R2 low-discrepancy sequence from there.
There is no wall clock, no frame counter, and no history buffer. Re-rendering
the same tick twice gives the same bits, which is what the golden gate requires
and what ADR 0010 rejected TAA over.

R2 rather than white noise because four white-noise samples cluster; a
stratified four has roughly the variance of an unstratified sixteen, which is
the difference between four rays being usable and not.

Simulation is untouched. These are fragment-shader terms; nothing here is read
by `loom_ecs`, `loom_script` or `play.rs`, and the pinned hash `b478ea4ac2622d32`
is unchanged in debug and release.

**Measured rather than asserted.** `cargo xtask shimmer` holds the camera at the
scene's authored eye and advances the simulation between frames, so it scores
exactly the thing a per-pixel dither would break. `primitives`, `materials`,
`cave` and `ground` all score **0.000** — and `primitives` and `materials` are
the two scenes ambient occlusion changes *most*, so that zero is load-bearing
rather than vacuous. Nothing here twinkles at rest.

## The noise that is left, stated rather than hidden

Eight AO rays leave visible grain in the occluded band. The number is the worst
channel against a 64-ray reference: **41**, inside the gate's 72, and at 1080p
it reads as fine grain confined to the contact rather than as an artifact — but
it is there, and at 3x magnification it is obvious.

The dither is a function of the pixel coordinate, so it is **screen-locked**: it
does not swim with the surface under a moving camera, it sits still while the
world moves under it. That is the honest trade for refusing history.

The cure that was not built is a **spatial** filter, which is what the noise
budget allows and temporal accumulation is not. The cheap form is a quad-wide
share — give each pixel of a 2x2 quad a different rotation and average across
the quad with `QuadReadAcross*`, which is 16-ray quality at the 8-ray price and
sidesteps the occupancy cliff entirely. Two things stopped it here: it needs
`GroupNonUniformQuad` in the fragment stage, which this engine neither queries
nor declares, so it would be a device-capability dependency with no fallback;
and a helper lane at a geometry edge shades a point slightly off the surface,
which bleeds occlusion across exactly the silhouettes AO is drawing. Both are
answerable, and neither is answerable in this change.

## Cost

Forward pass, 1920x1080, median of three, `LOOM_GPU_TIMING=1`:

| scene | before | +soft shadows | +AO | +reflections | total added |
| --- | --- | --- | --- | --- | --- |
| lanternhead `--sim 2400` | 0.465 | 0.473 | 0.786 | 0.898 | +0.433 |
| proving_ground `--sim 150` | 0.124 | 0.196 | 0.381 | 0.420 | +0.296 |
| materials | 0.120 | 0.187 | 0.373 | 0.404 | +0.284 |
| meadow | 0.267 | 0.350 | 0.547 | 0.575 | +0.308 |
| primitives | 0.103 | 0.140 | 0.247 | 0.264 | +0.161 |

**AO is two thirds of it** on every scene. Soft shadows are nearly free on
`lanternhead` (+0.008 ms) because its sun is behind a cloud deck and most pixels
never trace, and cost about 0.07 ms on the sunlit scenes.

For scale: the entire post stack is 0.131 ms, so this is three times the post
stack and it is the largest single addition to the forward pass the project has
made. It is a lighting term rather than a look, which is the argument for paying
it, and 0.9 ms of a 16.7 ms frame is not the constraint.

## Consequences

- **16 of the 26 golden references moved past tolerance**, re-blessed
  deliberately across three commits: `lanternhead`, `primitives`, `materials`,
  `cave`, `explosion`, `windy`, `proving_ground`, `shore`, `underwater`,
  `river`, `rain_overhang`, `rain_impact`, `rain_gantry`, `forest`, `ground`,
  `homestead`. Many more change bytes below tolerance, because a penumbra and a
  hemisphere estimate both move a great many pixels by less than one channel;
  `--bless` rewrites all 26 either way, so the MANIFEST diff is larger than the
  list above.
- Two scenes now exceed the gate's *worst-channel* tolerance of 72 against their
  old references (`primitives` 102, `materials` 88). Both are the deepest
  contact-occlusion pixels, and the diff images are clean coherent bands under
  objects rather than scattered noise.
- **Water still reflects the analytic sky.** `waterFragmentMain` has its own
  Fresnel path and was deliberately not changed: the sea is most of the screen
  in four golden scenes, a mirror at that scale is a different cost argument,
  and the water surface is itself vertex-shader geometry that no ray can hit —
  so a sea reflecting a quay would not reflect the sea.
- **Nothing else in the TLAS changed**, so the missing-geometry limitation
  applies to every future consumer of these rays too. Anything that wants to be
  reflected has to become an `Object`.
- The next thing this asks for, if reflections are pushed further, is the index
  buffer reachable from the fragment shader — which is the push block's last
  four bytes and belongs in its own commit with its own measurement.

## Human approval

Not required: no locked decision in CLAUDE.md moves, and the deferral this
resolves was recorded as "deferred, not refused, and it needs its own ADR"
rather than as a locked choice.
