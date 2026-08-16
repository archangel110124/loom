# ADR 0018 — The frame is computed in float and collapsed once

- **Date:** 2026-08-15
- **Status:** **accepted**
- **Decision touched:** ADR 0010's "no post-process before Phase 8". This is the
  second move of that boundary; CMAA2 was the first. It does not move a locked
  decision in CLAUDE.md.
- **Plan:** `docs/design/POST-STACK-PLAN.md`, which carries the rejected
  alternatives and the pipeline census in full. **Its ADR number is 0019 and is
  wrong** — 0017 was the highest on disk.

## Context — the renderer's dynamic range has been choosing the art

Every colour target in this engine was `R8G8B8A8_SRGB`: fixed point, so every
additive blend clamped at 1.0 with no rolloff anywhere, and every value the
lighting produced above 1.0 was destroyed at the moment it was written.

That ceiling has been making art decisions for a long time, and the fire work
is only where it became undeniable:

- `fireRamp`'s top rung is **capped below 1.0 on purpose**, with a comment
  saying so, because putting red at 1.0 partway up the ramp meant any second
  light source rotated orange toward yellow toward white.
- The measured overdraw on the sprite path pinned red after **2.6 overlapping
  particles** against about thirty alive, which is why fire is a level set of a
  noise field on a single quad rather than a sum of sprites.
- The campfire's `Light.intensity` sits at 1.35 against a component doc that
  says the useful range is 100–800, because at anything physical the ground
  around it was a white disc.

And it was never only fire. Decoded from `tests/references/` before the change:
`proving_ground` had **18,585 pixels with at least one channel at 254 or above,
29% of the frame**, of which 4,336 were pure white. Those pixels carried no
information — not a bright surface, an absent one.

## The decision

**The frame is computed in `R16G16B16A16_SFLOAT` and collapsed to eight bits
exactly once, in a fragment pass at the end of the graph.**

- The multisampled pair, the opaque resolve targets and the scene target are all
  half float. `Msaa::new` lost its `format` parameter as a result: it used to
  need one because the resolve destination was `COLOR_FORMAT` offscreen and the
  swapchain's in the window, and now there is one answer.
- Fourteen pipeline-creation sites take the new format — seven in `renderer.rs`,
  seven in `viewer.rs`. Dynamic rendering bakes the attachment format into the
  pipeline, so there was never a shortcut here.
- `tonemap.slang` reads the scene once, multiplies by `Environment.exposure`,
  applies a shoulder, and writes an `_SRGB` attachment so the hardware does the
  encode exactly as it always has. **What moved is where, not whether.**

**Half float rather than `B10G11R11`.** The smaller format is half the bandwidth
and was rejected on two counts: no alpha, which the particle and rain blends
need, and five bits of blue, which is visible banding in exactly the dark
blue-grey a night sky is made of. Format support was read out of `vulkaninfo` on
this GPU rather than recalled — `COLOR_ATTACHMENT_BLEND_BIT` present, and a
`framebufferColorSampleCounts` that includes 4.

### The curve: a pure shoulder, and the alternatives were measured

Identity below `KNEE = 0.76` linear; above it the whole triple is scaled by one
factor, so hue and saturation survive and only magnitude is compressed.

**ACES's full fit and Khronos PBR Neutral were both measured on `fireRamp`'s
amber rung and both rejected: each desaturates a fire FASTER than clipping does**
— 0.213 against 0.400 at sixteen times overdrive — because each lerps toward
white once the peak passes its knee. That is precisely the failure `fireRamp`'s
capped top rung exists to prevent, so adopting either would have spent an ADR to
reintroduce the bug the art was shaped around. The shoulder holds hue to 0.3
degrees across six stops.

A flame's white core comes from the ramp's own top rung. **The art decides what
is white**, not the curve.

### Ordering: tonemap before CMAA2, and CMAA2 does not change

CMAA2 is a display-referred filter — its edge threshold is a fraction of a luma
it assumes is bounded — so handing it linear values with no ceiling would make
that threshold mean something different in every part of the frame. The chain is
forward → tonemap → UI → CMAA2 → present, and the readback follows the last pass
that wrote a pixel, as it already did.

### Exposure is authored, never automatic

`Environment.exposure`, default 1.0, in the light block's existing padding so no
offset in the environment buffer moved. **Auto-exposure is refused**: adapting to
what the camera sees makes a frame a function of the frames before it, which is
the property ADR 0010 rejected TAA over and the one that makes a golden image
reproducible.

## What was measured

**1. The bottom of the range — and the prediction was wrong.**

The plan predicted that identity below the knee makes six references
(`cave`, `grass_slope`, `meadow`, `primitives`, `smoke`, `windy`) **bit-identical**.
All six moved, and the reason is worth more than the prediction was.

The multisampled colour image used to be `_SRGB`, so **each of the four samples
was quantised to eight bits and the resolve averaged encoded values**. It
averages linear light now. That is the physically correct resolve and the one
every anti-aliased edge in this project was previously getting wrong.

Measured on `primitives`, classifying by local gradient in the reference:

| | pixels | of which differ |
|---|---|---|
| flat (neighbour delta ≤ 2) | 59,564 | 249, **every one by exactly 1** |
| edge | 3,400 | 484, with a tail to 25 |

The ±1 everywhere is half-float quantisation and is under the gate's `channel: 2`
tolerance. The tail is entirely at edges, brighter, in the direction concavity of
the sRGB encode predicts.

**2. The top of the range.** Pixels with at least one channel at 254 or above:

| scene | before | after |
|---|---|---|
| proving_ground | 18,585 (4,336 pure white) | 192 (0) |
| explosion | 823 | 0 |
| homestead | 871 | 103 |
| ocean | 247 | 4 |
| shore | 121 | 7 |
| campfire | 41 | 0 |

**3. Whether MSAA still works — the risk fired.**

Absolute `shimmer` numbers are void across a colour change, so the question is
answered by the 4×/1× flicker ratio measured **entirely within one build**:

| | 1× | 4× | ratio | MSAA's reduction |
|---|---|---|---|---|
| `meadow` before | 3.888 | 2.712 | 0.698 | 30.2% |
| `meadow` after | 3.616 | 2.787 | **0.771** | **22.9%** |
| `grass_slope` after | 2.277 | 1.603 | 0.704 | 29.6% |

**About a quarter of MSAA's effectiveness on `meadow` is gone**, which is exactly
what the plan named as this change's single biggest risk: a bright edge covering
one sample of four used to swing ~83 codes and now swings more.

`primitives`, `materials`, `cave` and `ground` still score **exactly 0.000**, so
the instrument and the static camera are sound and this is a real reading.

**The plan's proposed mitigation does not fit the scene that regressed.** It
proposed a per-fragment ceiling on the emissive/specular term; `meadow` is
diffuse-lit grass against sky with no emissive term at all. The mechanism here is
more likely the resolve moving from encoded to linear averaging, which changes
how coverage maps into code space — compressed at the dark end, spread at the
bright end. **That makes the mitigation an open question rather than a queued
task**, and it gets its own commit and its own number when it is answered.

Accepted anyway: the clamp was destroying energy, which is the entire point of
the change, and a gamma-incorrect resolve is not a defensible thing to keep for
a metric.

## Alternatives rejected

- **Leave it at eight bits and keep tuning constants against the ceiling.** This
  is the status quo, and it has already produced a capped fire ramp, a campfire
  light 18× below physical, and a whole game scene 29% blown out.
- **ACES / Khronos PBR Neutral.** Measured; see above.
- **Auto-exposure.** Makes a frame a function of its predecessors.
- **Bloom, dither, grain, chromatic aberration, motion blur, depth of field,
  vignette, LUT grading.** All refused with reasons in the plan. **Bloom's is the
  one worth repeating here**, because it is the obvious next request: its radius
  is a fraction of screen height, so a resolution-derived mip chain is 3 levels
  at 1080p and **1 level at the gate's fixed 320×200** — the gate would validate
  a different effect from the one shipped, which is the measure-where-the-filter-
  is-not defect for a third time. It needs its own ADR and an answer to that
  first.
- **A `LOOM_TONEMAP` uniform.** A permanent dead branch in the hottest
  full-screen shader for the sake of a measurement.

## Consequences

- All 25 references re-blessed. `MANIFEST.txt` records each hash, so the
  re-bless is a readable diff.
- The viewer's scene image is now unconditional. It used to exist only when
  CMAA2 did, so the forward pass wrote a different destination depending on an
  environment variable — **in the window, which is where the human judges
  everything**, and that class of offscreen/window divergence has cost this
  project three defects.
- The render graph owns two more transitions; the barrier-list test names all
  eleven.
- Determinism is untouched: the hash is still `b478ea4ac2622d32`. Rendering was
  never in it.
- Anything authored against the ceiling is now free to be physical, and S2 of
  the plan is where that is spent, one line and one reason at a time.
