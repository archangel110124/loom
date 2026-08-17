# ADR 0049 — The foam trail is closed-form, so there is no foam buffer

- **Date:** 2026-08-17
- **Status:** **accepted**
- **Numbering:** above 0048, for the reason ADR 0045 gives. 0046 stays reserved
  for interactive ripples.
- **Governed by:** ADR 0045. Its consequences section says a foam *accumulator*
  "is new state and rendering-only, and takes the `repeat` gate". This ADR is
  the finding that W2 does not need one.
- **Decision touched:** none of CLAUDE.md's locked decisions. No FFT (clause 4
  is untouched and out of scope).

## Context — whitecaps are a record, and a still frame is not

`WaterSample::fold` is `Σ Q·k·A·sin φ`, already computed, already documented as
a plain scalar, already the whitecap signal. W2 renders it; it invents nothing.

But foam painted from the *instantaneous* fold is a highlight welded to the
wave: it appears when the crest steepens, vanishes the instant the crest
passes, and never sits on the water. Real whitecaps are entrained air. The
crest makes them in about a second and they take ten to disperse, drifting
downwind while they do. **A foam field is a record of where crests have been**,
and that record is the whole visual difference between a sea and a wave list.

`VFX-IMPLEMENTATION-REPORT.md` §2.1c writes the standard answer as a
recurrence: a foam texture, seeded by the Jacobian each frame and decayed each
frame.

## Decision

> **Do not build a foam accumulation buffer. Unroll the recurrence.**
>
> `F(x,t) = max(f(x,t), d · F(x − v·Δt, t − Δt))` expands to
> `max_k d^k · f(x − v·kΔt, t − kΔt)`, and every term of that is a function
> this shader can simply evaluate, because `fold` is a pure function of
> `(x, t)` with no history. Three taps at 1.1 s give a 3.3 s trail.

The taps are per water *vertex*, beside the one `loom_sample_water` the vertex
shader already does, and the result rides out as one more varying.

**What a buffer would have cost, and it is all of ADR 0045 clause 3.** Seeding
as a pure function of (scene, index); catch-up to `--sim N` in one dispatch, so
a headless still never depends on how many frames were drawn; no atomic on the
seed path; byte-identity across three processes, proven by `cargo xtask
repeat`; plus a warm-up, because a decayed buffer's first frame is empty and
`--sim 300` would have to mean 300 dispatches or one contrived one. That is the
whole burden rain and the particle pool each carry — and both of them carry it
because their state is *genuinely* not a function of the tick. Foam's is.

**It also stays a pure function of its tick**, which is the property ADR 0048's
offset already had and ADR 0010 used to reject TAA. Nothing in W1 or W2 makes a
frame depend on the frames before it.

## Measurements

- **Stubbing the trail (`FOAM_TRAIL_DECAY = 0.0`) moves 20.2% of
  `whitecaps.loom`** at 320x200, against a 0.1% tolerance — and reproduces
  `ocean`, `squall`, `shore`, `homestead` and `river` byte for byte, which is
  what proves the mutation isolates the trail and nothing else.
- **The trail shows the past, and it peaks where it should.** Of the foam the
  trail adds to `whitecaps` at tick 300, **4.4%** is foam the sea has *now*,
  **25.0%** was foam 1.1 s ago and **14.8%** was foam 2.2 s ago — one tap and
  two taps back. A term that was merely brightening the existing whitecaps
  would score highest in the first column.
- **Cost, water pass, 1920x1080, median of eight:** `ocean` 0.229 → 0.254 ms,
  `whitecaps` 0.288 → 0.303 ms. Under the +0.03 ms budget. The taps are per
  vertex on a mesh whose vertex count is fixed by `WATER_RES`/`WATER_LEVELS`,
  so the cost does not scale with the sea state or the resolution.

## The constants, and what they are not

`FOAM_TRAIL_STEP = 1.1 s`, `FOAM_TRAIL_TAPS = 3`, `FOAM_TRAIL_DECAY = 0.60`,
`FOAM_TRAIL_DRIFT = 0.02` of the wind speed.

The step is bounded from above by correlation, not by taste: past about two
seconds a Gerstner crest has travelled further than a foam patch is wide, and
the taps stop reading as one trail and start reading as three whitecaps. It is
bounded from below by the trail's total length — at 0.5 s the three taps cover
1.5 s and the effect is a slightly fatter crest.

**`fold` is still never normalised.** ADR-adjacent, and worth restating because
the trail is a new consumer of it: `loom_water` forbids dividing `fold` by
`Σ Q·k·A`, because 1.0 is a cusp and the validator caps the sea below it, so an
absolute threshold means the same nearness-to-breaking on every scene. A glassy
sea gets no foam and therefore no trail, which is correct and is the property a
normalised fold would destroy.

## Consequences

- `whitecaps.loom` joins `SCENES` and `GOLDEN` in the same commit as the
  feature. Adding a rendering path means adding a scene, and this project has
  reported a full pass on an absent feature three times.
- The trail is the coverage's **floor**, not an addition to it: a fresh
  whitecap is exactly as bright as it was before W2, and only the water behind
  it changes. Summing them would make a crest crossing its own wake brighter
  than paper.
- The taps reuse this column's ground height rather than re-marching the height
  grid three times. They are under a metre away and the shoaling difference is
  nothing; on `whitecaps` there is no voxel volume at all.

## What would change this

**A foam source that is not closed-form in `(x, t)`** — a wake behind a moving
hull, spray landing back on the surface, an interactive ripple (W6). None of
those can be evaluated backwards in time from a position, so none of them
unroll, and the first one that ships is the trigger to revisit the buffer with
ADR 0045 clause 3's checklist in hand.
