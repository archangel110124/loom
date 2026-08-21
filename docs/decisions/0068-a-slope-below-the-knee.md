# ADR 0068 — A slope below the knee

- **Date:** 2026-08-21
- **Status:** **accepted**
- **Decision touched:** none of CLAUDE.md's locked decisions moves. **ADR 0018's
  shoulder is not amended by a character** — it keeps its knee, its hue
  guarantee and its identity-above-nothing property. What this adds is a term
  *before* it, on scene-referred light.
- **Applies to:** `assets/shaders/tonemap.slang` and every golden reference.

## Context — the operator is a highlight clamp, not a tone curve

The human's word for the look was *meh*. Before proposing anything, what the
tonemap actually does was read: `shoulder()` is **identity below linear 0.76**
and compresses above it. On a third of the golden library no pixel ever reaches
the knee, so on those scenes the tone map **provably never runs**. That, and not
the shape of the shoulder, is why every frame in this project sits in one
midtone band with no blacks and no whites.

Measured over the library: `cave`'s entire range is sRGB 70→161. `lanternhead`'s
1st percentile is 20. Nothing is black because nothing is *made* black.

## Decision

**One term: an exposure-relative contrast power about a mid-grey pivot, applied
to scene-referred light before the untouched shoulder.**

    c = PIVOT * pow(max(c / PIVOT, 1e-8), CONTRAST)      PIVOT 0.30, CONTRAST 1.30

Order is `exposure → contrast → shoulder`, all in linear light. Before the
shoulder, because a curve applied after it moves shadows and nothing else — the
shoulder has already spent the top of the range. The `_SRGB` attachment does the
encode in hardware exactly as it always has; there is no encode or decode in
this shader, and there is none anywhere in `assets/shaders/`.

### Why a power and not a toe

Khronos PBR Neutral's toe — a fixed −0.04 subtraction in linear — was measured
and **refused at every strength**. It is exposure-*absolute*, so it removes a
fifth of a bright frame's shadows and nearly all of a dark one's: 56% of
`campfire` and 68% of `emberfall` below luminance 16 at full strength, off a
cliff between 0.75 and 0.80 of it. It is also not the hue-preserving operation
its name implies — a constant subtracted from all three channels raises
saturation wherever the min channel is small, which is every shadow in this
library. A power scales with how the scene was lit and can do neither. Pinning a
constant one rung below a measured cliff is a setting that breaks on the next
scene someone authors.

PBR Neutral's highlight bleach is refused separately: it costs 16–36% of
`campfire`'s flame-core saturation, in the month whose whole deliverable is
fire, smoke and water, and it is the only candidate that pushes rows past the
gate's own threshold *for reasons of its own*. It is eight lines and can arrive
later as its own commit with its own bless; nothing here forecloses it.

### Why the pivot is 0.30 and not 0.18

Pooled over every pixel of all 54 references the median linear value is 0.19 — a
good number for a library nobody is looking at. `deeper_demo`, the game the
human *is* looking at, sits at 0.36, and at a pivot of 0.18 its opening frame
gets **brighter**, immediately after the human said "not too bright". The
alternative was to cancel that lift inside `deeper_demo`'s own grade, which is
two terms fighting in one file, invisible to whoever authors the third scene.

**PIVOT is the brightness knob.** If an opening frame reads hot, raise it; do
not patch it downstream.

### Why the slope is 1.30

Measured on the library at each value. At 1.30, shadows fall and highlights do
not move: `lanternhead` p01 20.1 → 7.5, `ocean` 37.1 → 22.2, frame saturation up
about a quarter, 99th percentiles within 11 codes. **1.45 was measured too and
is where it starts to harden** — `campfire`'s p01 reaches 0 and the sky
posterises. `cave` moves 90.6 → 77.6 and still has no blacks, because *its*
flatness is a lighting fact and no curve fixes it.

## What it does to the gate, measured before it was written

`tools/postpredict/predict.py` inverts the shipped shoulder analytically,
applies a candidate to all 54 references in float64 and re-encodes, so **the
whole re-bless is reviewable before the change exists**. Its `selftest`
round-trips bit-exact (54 rows, worst channel 0) and carries its own fault
injection.

It predicted a max worst channel of **26** against the gate's threshold of 72.
The real GPU render returned **26**. Every row moves, on `fraction`; that is the
intent of a global curve.

## The trap this cost an hour to find, and it is the reason to write it down

**`LOOM_CMAA2` defaults to *on*** (`cmaa2.rs:109` reads it as opt-*out*), so an
edge-directed anti-aliasing pass runs **downstream of the tonemap**. It is
contrast-sensitive by construction. Raise the contrast and it finds edges it
used to ignore and blends across them — on `campfire`'s black log against lit
ground that dilates the bright side by two pixels and reports a `worst` channel
of **103** where the model predicts 26.

Two doc comments in `renderer.rs` and `viewer.rs` stated the opposite default.
Both are corrected in this commit.

Measured, one A/B with only the tonemap swapped:

    LOOM_CMAA2 on  (the gate's default)   residual worst 105, 23 px off by >20
    LOOM_CMAA2=0   (the operator alone)   residual worst   1,  0 px off by >8

**So there are two different questions and they need two different renders.**
`LOOM_CMAA2=0` answers "is the operator what I published" — and it answers it
to within **two codes on 53 of 54 rows**. The default answers "what will the
gate say". A model of the tonemap alone can only ever answer the first, and
believing it answers the second is how an hour goes.

The three rows the gate will report above `worst: 72` — `campfire` 103,
`slosh` 85, `plume_roof` 76 — are all hard silhouettes being anti-aliased
correctly for the first time. They are not regressions.

## Consequences

- **All 54 golden references move and must be re-blessed, in one call.**
  `image(bless)` has no row filter and `write_manifest` re-hashes the directory;
  the review is per-row, the write is atomic.
- `viewport_rect` moves on **0.3749** of its pixels, not 1.0 — the tonemap is
  scissored to the placement rect, so the editor chrome is never graded. That
  number is the guard, together with `lib.rs`'s pinned chrome corner.
- **The sim hash cannot move.** `Play::state_hash` is `physics.state_hash()` and
  `fn sim` never constructs a `Renderer`.
- **`cargo xtask repeat` sees nothing new.** The operator is a pure function of
  one texel and the push constants — no clock, no RNG, no readback, no history.
- **`cargo xtask shimmer`'s three exactly-0.000 controls survive**, because the
  operator has no screen-space term. That is also why there is no dither and no
  film grain here: both would raise the flicker floor on every scene, and those
  zeros are the instrument the open AA question depends on.

## Refused, each on its own evidence

Bloom (every variant measured lifts p1 and drops std — veiling glare, in a game
that needs blacks), chromatic aberration (−18% acutance on a game made of thin
geometry), DOF, motion blur, film grain, a vignette, dither, 3D LUTs (32,768
lines of floats defeats property #1), AgX (+4.9° mean hue rotation, which makes
every authored grade number a moving target), ACES (−68% of bright-region
saturation), an operator enum, a `PostEffect` trait or registry (never-do #12
is aimed exactly here), and any new pass, image, descriptor or barrier.

Also refused, and flagged rather than fixed: `scene.slang:611` draws the sun
disc as `float3(1.0, 0.94, 0.82) * (glow + disc)` and never multiplies by
`sun_strength` or `sun_color`. That is the real root cause of "the sun is a grey
smudge", it is one line, and it carries its own re-bless and its own ADR. This
work must not smuggle it in.
