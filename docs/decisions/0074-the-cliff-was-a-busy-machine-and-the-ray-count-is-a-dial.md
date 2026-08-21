# ADR 0074 — The occupancy cliff was a busy machine, and the ray count is a dial

- **Date:** 2026-08-21
- **Status:** **proposed.** The code ships at the default and is measured; what
  needs a human is §"What only the human can decide" — which stop is the
  default, and whether to re-bless.
- **Decision touched:** **ADR 0019's `AO_RAYS = 8` and the occupancy cliff it
  was justified by.** The cliff is retracted; the count becomes a run-time dial
  with a default of 4 per lane, shared across the quad. Also corrects the
  citation of that cliff in **ADR 0061**'s rejected alternatives.
- **Not touched:** every locked decision in CLAUDE.md. No ray-tracing pipeline,
  no shader binding table, no new pass, no new descriptor, no barrier outside
  the render graph. Still one inline ray query per term from `fragmentMain`.

## Context — the human asked for cheaper rays and said it need not be perfect

> "We want to have ray tracing, but very performant ray tracing. If there's some
> way to maybe do, like, half tracing, something like that, so it looks still
> good but it performs better. It maybe doesn't look perfect."

That is permission, and the interesting part of this ADR is that it was not
needed. The half-resolution trace the human was reaching for exists, in a form
that costs nothing in structure: **a fragment quad is four lanes shading four
points on one primitive**, so averaging AO across it is a spatial reconstruction
whose guide is a guarantee rather than a heuristic. Half the rays per lane, four
lanes decorrelated, and the picture gets *better* rather than worse.

ADR 0019 named this cure and left it unbuilt behind two objections, both of
which are now answered. It is corrected in place there.

## What was measured

Three instruments, all of them load-quoted, GPU warmed to boost, minimum of
several **interleaved** repetitions — interleaved because a batched sweep on a
box with anything else running drifts, and this project has published one set of
timings 20× wrong and another 1.8–2.6× high from exactly that.

### 1. The occupancy cliff does not exist

ADR 0019 recorded 0.024 ms per AO ray to eight and **0.101 ms each** from eight
to sixteen — a fourfold jump with nothing changing but the trip count — and
three separate refusals in this codebase were resting on it. Re-measured,
unshared, forward pass at 1920×1080, min of 5 interleaved reps × last 16 of 24
frames, marginal ms per added ray:

| scene | 2 | 4 | 8 | 16 | 2→4 | 4→8 | 8→16 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| lanternhead | 0.574 | 0.663 | 0.864 | 1.293 | .0445 | .0503 | .0536 |
| stoneyard | 0.605 | 0.731 | 0.990 | 1.506 | .0630 | .0648 | .0645 |
| cave | 0.327 | 0.396 | 0.531 | 0.805 | .0345 | .0338 | .0343 |
| materials | 0.239 | 0.283 | 0.375 | 0.555 | .0220 | .0230 | .0225 |
| croft | 1.458 | 1.701 | 2.207 | 3.246 | .1215 | .1265 | .1299 |

**Flat, on five scenes, across the rung the cliff was claimed at**, and measured
independently by two agents on two builds. The mechanism a cliff would have
needed is not present either: the loop is rolled, so the trip count cannot move
the register footprint.

Rays are linear. Eight was still a reasonable count — the *picture* saturates —
but "we cannot afford more rays" was never true, and a rejection resting on a
retracted fact is the trap this project's own rules name.

### 2. The quad share is worth four times its samples

Each lane of a 2×2 seeds its dither from the **quad's** corner and indexes the
sample sequence at `lane * rays + i`, so the four lanes are four disjoint slices
of one stratified set rather than four rotations of one set — four rotations are
four correlated estimates and averaging them buys much less. Costs three integer
ops.

Scored against a converged reference — 64 rays with the share **forced off**, so
neither variant is being marked by its own bias. Mean absolute error over the
frame and `loom salt --thresh 8`, which is the metric that sees grain. 1920×1080:

| scene | OLD (8, no share) worst / mean / salt | NEW (4 + share) worst / mean / salt |
| --- | --- | --- |
| primitives | 47 / .0446 / 8147 | 30 / .0302 / **1396** |
| cave | 18 / .0132 / 1138 | 13 / .0095 / **67** |
| ground | 30 / .0392 / 397 | 27 / .0273 / **1** |
| forest | 33 / .0890 / 339 | 28 / .0602 / **−29** |
| vale | 36 / .0411 / 510 | 36 / .0380 / **25** |
| stoneyard | 41 / .0628 / 469 | 41 / .0482 / 163 |
| croft | 34 / .0379 / 226 | 34 / .0289 / 54 |
| glass | 32 / .0200 / 144 | 32 / .0134 / 43 |
| spruce | 120 / .0132 / 19 | 68 / .0101 / −7 |
| gleamsprat | 49 / .0057 / 60 | 49 / .0051 / 22 |
| materials | 27 / .0076 / 30 | 32 / .0054 / 16 |
| mood_deep | 3 / .0086 / 2 | 3 / .0058 / 1 |

**Mean error falls on twelve of twelve and grain falls on twelve of twelve**,
often by an order of magnitude — while the forward pass falls 23–29%.

**The share is what buys this, not the ray count.** Four rays with *no* share is
the control and is worse than eight on every scene: `primitives` 67 / .0834 /
5989, `cave` 24 / .0236 / 2280, `forest` 29 / .1449 / 678. Halving the rays alone
is a straight loss; halving them and decorrelating the four lanes is a gain.

### 3. The dial, and what each stop costs

`LOOM_AO_RAYS` — rays per lane, default 4, read once, carried in
`EnvironmentData::viewport[2]`. Forward pass at 1920×1080, min of 5 interleaved
reps × last 16 of 24 frames, load 1.6–1.9, ms:

| scene | 8 | **4 \*** | 2 | 1 |
| --- | --- | --- | --- | --- |
| croft | 2.202 | **1.692** | 1.445 | 1.332 |
| stoneyard | 0.982 | **0.725** | 0.595 | 0.536 |
| spruce | 0.964 | **0.692** | 0.561 | 0.492 |
| lanternhead | 0.856 | **0.654** | 0.564 | 0.528 |
| cave | 0.529 | **0.390** | 0.322 | 0.287 |
| materials | 0.371 | **0.278** | 0.236 | 0.213 |
| proving_ground | 0.375 | **0.286** | 0.239 | 0.217 |
| primitives | 0.238 | **0.183** | 0.154 | 0.141 |
| **vs the default** | +30…+39% | — | −14…−19% | −19…−29% |

And in pixels, against the same converged reference:

- **8** — mean error ~30% below the default, grain roughly halves again. A third
  more forward pass and nothing else changes.
- **4 (default)** — closer to converged than anything this engine has shipped.
- **2** — about last week's engine: mean error 5–10% *above* the default, grain
  still 2–4× cleaner than the pre-share shader. This is the stop to reach for
  when a frame is GPU-bound.
- **1** — the first stop worse than anything shipped. `primitives` goes to mean
  .0775 against the old shader's .0446, and the speckle is visible.

**Where it shows, plainly: large untextured matte surfaces, and nowhere else.**
`primitives` is the worst-looking scene in the repository at every stop — a wide
flat blue plane with hard shapes on it and nothing to hide variance behind. Set
beside it at 1:1, `croft` is *indistinguishable across the entire dial*, and
`proving_ground` — an actual game — moves a mean of **0.0004** from converged at
the cheapest stop. Texture hides this completely.

## Why a run-time dial rather than a constant

The loop was already rolled with a run-time trip count (`quadShare` picks between
two values and is a per-material fact), so nothing was unrolled and nothing is
lost: at the default the compiled shader renders the **same pixels** — 55 of 56
GOLDEN rows byte-identical — and the same milliseconds, within 0.4% on six
scenes.

**An environment variable and not a scene field.** How much silicon to spend is a
property of the machine looking at the scene, not of the scene; a scene-authored
value would make the golden images photograph whatever each `.loom` happened to
say and would have to be answered again in every scene ever written. It joins
`LOOM_GPU_TIMING` and `LOOM_CMAA2`, and the table of stops lives with them in
`loom_render`'s crate docs — one place, in English, for someone choosing rather
than reading the shader.

## Determinism

Unchanged, and this is the clause that had to be checked rather than argued.

- The dither is still a pure function of the **integer pixel coordinate** — now
  of the quad's corner rather than the pixel's own, which is the only thing about
  ADR 0019's determinism argument that moves. It is still screen-locked: it sits
  still while the world moves under it.
- `loom flicker` over three frames at a **static** camera with the simulation
  advancing scores `primitives`, `materials`, `cave`, `ground`, `stoneyard` and
  `spruce` at exactly **0.00000** — before, after, and at the cheapest stop.
- Three fresh processes per scene, 18 scenes at 1280x800, produce **one distinct
  hash each**, `alpha_cutout` (the unshared path) and `plough_cinematic_low` (the
  water fix) among them.
- The quad is fixed by the pixel grid, not by scheduling. Anything wider than a
  quad is refused on exactly that ground: the arrangement of quads inside a
  subgroup is implementation-defined.
- **No history buffer, no reprojection, no motion vectors.** This is a purely
  spatial filter inside one frame, which is what ADR 0019 said the cure would
  have to be. (ADR 0073 has since widened the temporal licence; nothing here
  uses it, and nothing here needs to.)

## The hazard, which is real and bit elsewhere

**A lane that has executed `discard` may not be read across the quad.** Slang
lowers `discard` to `OpKill`, not to a demote, and a quad read from a killed lane
is undefined.

`fragmentMain` sidesteps it structurally: its one `discard` is alpha cutout, both
terms of the condition are *material* constants, `material` is `nointerpolation`,
and a quad is one primitive — so every lane agrees and a quad that could contain
a kill simply takes the unshared path, at twice the rays. Verified from the
compiled SPIR-V rather than from the source: of the module's nine `OpKill`s,
`fragmentMain` holds exactly **one**, it calls no other function, and no function
holding a kill is reachable from it. It has three quad ops.
`GroupNonUniformQuad` is declared; `DemoteToHelperInvocation` is **not**, and is
not needed.

`waterFragmentMain` did **not** sidestep it and was reading killed lanes in
shipped code. Its shoreline `discard` is gated on an interpolated varying, so no
branch can be quad-uniform about it and it had to be *measured*: the condition is
now summed across the quad in uniform control flow before anything is killed, and
the reflection share takes that as a second guard — `waterFragmentMain` now
compiles to six quad ops where it had three. Footprint of the fix: **two pixels**
of `plough_cinematic_low` at 320×200, worst channel 3 of 255, with all 55 other
GOLDEN rows byte-identical. That the
driver was returning nearly the right answer from a terminated lane is why it
survived five green gates, not evidence that it was safe.

`GroupNonUniformQuad` is now used by three things, one of them in every scene
with a mesh in it. `device.rs`'s capability check is no longer removable with
cinematic water, and says so.

## Rejected

- **A second, coarser AO pass at half resolution.** This *is* the half-resolution
  trace, at no cost in structure: no extra target, no upsample, no depth-guided
  filter, no bilateral weights, and — critically — no bright halo, because the
  filter provably never crosses a silhouette.
- **A subgroup-wide share.** Wider than a quad means an implementation-defined
  screen-space arrangement, which is a determinism cost this project does not pay.
- **Any temporal denoiser** — ReSTIR, SVGF, reprojection. Not needed: the grain
  it would attack is already below the previous shipped level.
- **A runtime knob per scene, or a `--quality` flag on every command.** One
  environment variable is the whole surface.
- **More stops.** 8/4/2/1 is the whole useful range; the term is converged well
  before 16 and unusable at 0.

## What only the human can decide

1. **Which stop is the default.** 4 is shipped and is both cheaper and prettier
   than what it replaces. 8 is *also* prettier than what it replaces and costs a
   third more forward pass. Both are defensible, which is unusual, and is the
   reason the dial exists rather than a constant.
2. **The re-bless.** 23 GOLDEN rows plus `spruce` move against the pre-share
   references. **Eleven rows are byte-identical and are the discriminator** —
   `ocean whitecaps rain_pool squall campfire water_crate wake splash pool
   pool_jet spindrift`. If any of those md5s moves during a re-bless, the pending
   tonemap change rode along. Bless the tonemap first, alone.

## What would reopen this

- **A GPU where the ray ladder is not linear.** Every number here is one RTX 4090
  at 300 W. If the marginal cost per ray ever measures non-flat again on real
  hardware, the count — not the share — is what to revisit, and the re-measurement
  should be interleaved and load-quoted or it will say whatever the machine was
  doing.
- **Blades entering the TLAS.** AO's exemption from the grass objection is a
  coincidence of grass being vertex-shader geometry. If it stops being one, the
  hemisphere-kernel problem returns in its original form.
- **A `discard` added to `fragmentMain` on anything but a material constant.**
  That would make the sharing guard unsound, and nothing would report it. The
  guard, and why it holds, is a comment at the top of `fragmentMain` for that
  reason.
- **Wanting the last of the grain.** The remaining step is spatial and known: an
  8-ray quad share is 32-ray quality; a further factor wants either more rays
  (now known to be affordable and linear) or a wider share (refused above).
