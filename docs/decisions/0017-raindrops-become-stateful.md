# ADR 0017 — Raindrops become stateful, and what that costs the golden gate

- **Date:** 2026-08-14
- **Status:** **accepted**
- **Supersedes:** ADR 0014's deferral. Its trigger list is what fired; its
  "what the change would look like" section is what was built.
- **Decision touched:** Phase 4 step 2's stateless streak renderer. Does not
  move a locked decision. It does extend the render graph to buffers, which is
  a strengthening of never-do #4 rather than an exception to it.

## What fired the trigger

ADR 0014 deferred the stateful version behind four named triggers. Two of them
were unambiguously true and are what this ADR answers:

- **Trigger 2 — mesh geometry stops no rain.** Occlusion was a test against
  W6's baked terrain height field, which is the topmost solid surface in a
  column of the *voxel* volume. A bridge, a gantry, a mesh roof appears in no
  voxel volume, so it appeared in no height field, so it stopped nothing. Rain
  fell through it.
- **Trigger 3 — splashes landed where exposure said they should.** They were a
  closed form over the rate and the height field: a position the CPU believed a
  drop should have reached. Right on open terrain and wrong everywhere else.

Trigger 4 (sideways-driven rain under a ledge) follows from the same fix and is
now correct as a consequence rather than as a feature.

**Trigger 1 did not fire, and that is the more interesting half of this
document. See "What this does not fix" below — it is the artifact the human
actually reported, it was characterised before any of this was written, and
statefulness does not address it.**

## What was built

`rain_sim.slang`, `crates/loom_render/src/rain.rs`, `loom_rain::collide`, and
buffer support in `loom_render_graph`. Exactly ADR 0014's own sketch:

- **A GPU-resident drop buffer.** 131,072 drops of 32 bytes — position and age,
  velocity and identity. Device-local; the CPU never reads or writes it.
- **One compute dispatch per frame**, integrating and colliding.
- **A splash append buffer feeding an indirect draw.** A splash is a collision
  the simulation resolved, carrying the impact point and the surface normal.
  The count is a GPU fact and stays one: `rainSplashArgsMain` writes the
  `VkDrawIndirectCommand` from the ring's cursor.
- **No readback, ever.** Nothing in this path is visible to the host. The
  determinism hash is unchanged at `b478ea4ac2622d32`.

### The world reaches the GPU as a 3D texture

`loom_voxel::Volume` is a `BTreeMap` of chunks with an edit layer, which no
compute shader can walk. So `loom_rain::collide::bake` rasterises it into a
192 x 64 x 192 `R8_SNORM` image — 2.36 MB, which is ADR 0014's own sizing — and
the shader takes a hardware trilinear fetch. Re-baked when the world changes and
at no other time, on exactly the trigger the terrain height grid already uses,
so carving a roof open in the editor lets rain through on the next frame.

**The bake is the collision world, not the voxels.** Every voxel volume unioned
with every static `BoxCollider`. Baking only the voxels would have carried the
drops onto a better representation of the same world and left trigger 2
unfired. The rule this encodes is one sentence — **rain stops where a body would
stop** — and it is the rule audio's `openness` already follows.

Outside the baked field a drop falls to the terrain height grid instead, so a
scene larger than 48 m does not rain through its own ground.

### One dispatch, not one per tick

Thread `i` reads and writes `drops[i]` and touches no other drop, so the
simulation is 131,072 independent scalar recurrences. Advancing N ticks does not
need N dispatches with N barriers between them; it needs one dispatch with the
recurrence rolled up in registers. Catching a headless `--sim 1800` render up
therefore costs **one** dispatch at 2.25 ms rather than 1,800 of them.

### The density rule, which took three tries

Where a landed drop is reissued is a density argument, and both obvious answers
are wrong:

| rule | what happens |
| --- | --- |
| anywhere in the block | drops over ground are consumed faster than they are replaced; the field thins out as the scene runs |
| the top of its own column | a column with high ground holds the same drops over a shorter fall; rain is densest where the ground is highest |
| **one block-height above the impact** | every cycle is the same distance however high the ground is; density is uniform and stays at its seeded value |

A drop spends the part of its cycle the ground took away sitting above the
block, where the boundary fade is zero. Measured on `rain_impact`: the sky's
rain light is 1.27/255 at tick 20 and 1.37 at tick 1,800, against 1.24 for the
closed-form layer this replaced.

### Barriers went into the graph rather than beside the dispatch

never-do #4 says no barrier lives outside the render graph, and the graph
modelled images only — on the reasoning that "buffers reached by device address
need no layout transitions". True, and it left the other half unhandled: a
compute pass that writes the drop buffer and a vertex shader that reads it in
the same command buffer need an execution and memory dependency or the draw
reads last frame's drops. That is a defect with no layout to give it away and
an almost-right picture, which is the worst profile there is.

So `loom_render_graph` grew `BufferId` and `BufferAccess`, `pass_with`, and
`plan_full` so the buffer barriers are as testable as the image transitions
already were.

## The cost: a render is no longer a pure function of its state

This is the real price, and it is the same objection ADR 0010 used to reject
TAA. The golden-image gate rests on a frame being reproducible; state makes a
frame a function of the *sequence* of ticks the buffer has been advanced
through, not of the tick alone.

**What is preserved, and it is enough for the gate:**

- Seeding is a pure function of the drop index and the eye, off the frozen
  `loom_field::noise` hash.
- A renderer that has never simulated seeds and then advances to the requested
  tick **in one dispatch**. So a headless still at `--sim N` is always
  seed-then-advance-N, from a fresh process, with a fixed camera.
- Nothing is read back, so no CPU decision depends on any of it.
- The splash ring is sized so one tick's landings cannot lap it. The order the
  atomics resolve in is undefined, but the *set* of entries the ring holds is
  not, and the draw is additive, so the image does not depend on the order.

Verified: `loom render assets/test/rain_impact.loom --sim 1800` and
`rain_gantry --sim 600` are **byte-identical** across three separate processes,
and `cargo xtask image` matches its references.

**What is lost, stated plainly:**

1. **The viewer and the offscreen path agree only while the camera is still.**
   A drop is reissued relative to wherever the eye was at that moment, so a
   window whose camera has walked from A to B holds a different arrangement of
   drops than a headless render of tick N with the camera parked at B. Both are
   correct rain; they are not the same rain. Nothing in the gate depends on
   them matching, because every golden render is a fresh process with a fixed
   camera — but "the offscreen render is the frame the viewer would be showing"
   was true before this and is not true now.
2. **A frame depends on the frames before it.** `loom render --frames` is now a
   genuine sequence rather than N independent stills. That is what makes a
   fly-through show rain falling instead of rain being re-hashed, and it is
   also why `Renderer::set_rain_tick` going backwards re-seeds.
3. **The layer can no longer be reasoned about from a still.** A closed-form
   drop could be checked with a pencil. This one has to be measured.

If a future change makes the golden gate flake on rain, the fix is to re-seed
per frame in the headless path rather than to loosen the tolerance.

## What this does not fix, and it is the thing that was reported

The human's report was "it looks better, but when you move, that's when the
illusion is broken". That was characterised before this was built, on
`rain_impact` at 60 fps with the camera walking at 4 m/s:

- **Rain is 97% of all frame-to-frame temporal noise** in the scene: flicker
  1.201 with rain against 0.033 with the same scene dry.
- **The far half of the field is unfiltered sub-pixel noise.** The rain pass
  draws into the *resolved*, single-sample colour target after the MSAA
  resolve, so it is the one thing in the frame with no anti-aliasing at all. At
  640x400 a 0.02 m streak is under a pixel beyond about ten metres, and those
  render as isolated single pixels that reshuffle every frame — visible in the
  frame crops as a field of salt. Rendering at 4x and downsampling removes it
  visually, which is what proves it is aliasing.
- **The near field is grossly over-scaled and pops.** `RAIN_WIDTH` is a
  constant 0.02 m of *world* width, chosen so a distant streak is visible at
  all; the same drop half a metre from the eye draws about 19 px wide and
  240 px long at full additive brightness, and at one drop per cubic metre
  there are about three of those inside a one-metre sphere at any instant. The
  rain layer's total light in the sky band swings 14% frame to frame with
  single-frame spikes of +70%.

**The first two are not statefulness problems and are not fixed here.** Measured
over the same 24-frame walking path, flicker went from 1.201 to 1.405 — slightly
*worse*, because per-drop terminal speeds add genuine variety that the metric
counts. Side-by-side frame crops before and after are indistinguishable in
character.

**The third is fixed**, because it was reported again from a scene
(`homestead`) as bright bands sweeping the frame that read as lens flares, and
because it is four lines. `RAIN_NEAR_MIN`/`RAIN_NEAR_FULL` fade a drop out over
the nearest 0.6–2.5 m. It is defensible independently of the artifact: no real
camera focuses at 20 cm, and it costs about 65 drops of 131,072.

**The boundary fade could never have covered it**, and that is worth writing
down, because it is the obvious thing to reach for: the boundary fade attenuates
a drop near a *face of the block*, and a drop 30 cm from the eye is in the
middle of the block, as far from every face as a drop can be.

Frame-to-frame swing in the sky band's rain light, over the same walking path:

| | mean | rel. sd | range |
| --- | --- | --- | --- |
| stateless | 1.325 | 13.4% | 1.13 – 1.95 |
| stateful, no near fade | 1.428 | 15.4% | 1.09 – 2.02 |
| **stateful + near fade** | 1.316 | **7.4%** | 1.10 – 1.45 |

`cargo xtask shimmer` cannot see any of this either: at its 0.2 s frame step a
drop falls 1.6 m, so consecutive frames share no streaks at all and the number
means nothing for rain. The measurement that works is a 1-tick step, and the
tool for it is `loom render --dolly`, added here — the fly-through could only
*pan*, and a pan is chosen precisely because it produces no parallax, which
makes it the wrong instrument for an artifact that is parallax.

**What is left, and it is the larger half:** a screen-space width floor with
matching alpha reduction, so a sub-pixel streak widens and dims instead of
flickering on and off at the pixel centre. That is the trick P2's grass
investigation found *made things worse on its own* and needs alpha-to-coverage
to work — which needs the rain pass to have samples, and it deliberately has
none, because it draws into the resolved target after the MSAA resolve so that
one pipeline serves both render paths. Changing that is a real piece of work and
it is the next one. It does not need state either.

## Cost

`LOOM_GPU_TIMING=1`, 1920x1080, 4x MSAA, `rain_impact`:

| | before (stateless) | after |
| --- | --- | --- |
| simulate | — | **0.022 ms** |
| splash args | — | 0.002 ms |
| draw | 0.036 ms | 0.033 ms |
| **total** | **0.036 ms** | **0.057 ms** |

Comfortably under the 0.1 ms bar, and ADR 0014's prediction that "a stateful
version would be cheaper than the draw already is" holds. The one-off catch-up
for a headless `--sim 1800` render is 2.25 ms, paid once per process.

## Sources

- ADR 0014, which specified this and named the triggers.
- ADR 0010, for the argument about a frame being a pure function of its state.
- Latta, *Building a Million-Particle System*, GDC 2004 — GPU-resident
  simulation removes the CPU→GPU transfer cap that limits particle counts.
