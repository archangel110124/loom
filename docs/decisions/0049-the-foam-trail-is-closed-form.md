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

---

# Amendment, 2026-08-18 — the taps, the threshold, and where the trail stops

- **Status:** **proposed** — needs human approval before this branch merges.
- **Amends:** the constants section above, and one sentence of the decision.
- **Companion:** ADR 0055, which is the buffer this ADR declined to build,
  built for the sources this ADR's own "what would change this" section names.

## The step was set by an argument and the argument was wrong

The original text bounds the step from above by tap correlation, reasoning that
"past about two seconds a Gerstner crest has travelled further than a foam patch
is wide". Measured on `whitecaps.loom`'s own wave set — the correlation between
the coverage now and the coverage at the same drifting point `tau` earlier:

    tau   0.1s  0.91     0.4s  0.36     0.7s  -0.00
          0.2s  0.73     0.5s  0.20     1.0s  -0.14
          0.3s  0.54     0.6s  0.08     1.1s  -0.14

**It crosses zero at 0.70 s**, well inside the shipped 1.1 s step. So the three
taps were three independent draws of the same field rather than a trail. Against
the recurrence they unroll — stepped one fixed tick at a time out to 3.3 s:

     3 taps at 1.10s   recovers 33.7%   misses 45.2%
     5 taps at 0.66s   recovers 56.7%   misses 22.3%
     6 taps at 0.55s   recovers 63.5%   misses 17.3%
     8 taps at 0.41s   recovers 73.0%   misses 11.5%
    10 taps at 0.33s   recovers 78.6%   misses  8.5%   <- shipped
    16 taps at 0.21s   recovers 87.1%   misses  4.5%

"Misses" is the fraction of points carrying real trail that the taps give
nothing at all — a point that was white 0.6 s ago and is not white now got
nothing from any of the three old taps.

**`FOAM_TRAIL_STEP` is 0.33 s, `FOAM_TRAIL_TAPS` is 10, `FOAM_TRAIL_DECAY` is
0.858.** The trail is the same 3.3 s long and the half-life does not move: 0.858
per 0.33 s is 0.600 per 1.1 s, exactly the old decay. Sixteen taps is where the
curve flattens and is not worth 60% more samples.

**Cost, water pass at 1920x1080, six runs:** `whitecaps` 0.402 → 0.509 ms,
`ocean` 0.336 → 0.586 ms. The design predicted +0.06 ms; the true figure is
+0.11 and +0.25, because the taps run the scene's whole wave loop and `ocean`
has seven waves to `whitecaps`' five. It is the largest single addition the
water pass has taken, and it is paid per water vertex rather than per pixel, so
it does not scale with resolution.

## The criterion the taps threshold is no longer the trace

`WaterSample::fold` is `Σ Q·k·A·sin φ`, which is the **trace** of the horizontal
compression — the sum of both principal compressions. Two swells crossing at 90°
add their compressions and read as broken while neither axis has folded.
Measured on exactly that pair, two 10 m swells at `Q·k·A = 0.32` each: the trace
calls **12.32%** of the surface past breaking, the largest eigenvalue **0.00%**.

Every foam threshold now reads `mu_max`. On a single-direction sea — which is
most of this repository — the two agree to 1.5e-8, so this is a change only
where the sea crosses.

## `WATER_FOAM_STRETCH` is `WATER_FOAM_STREAK`

The streaks lay across the *global wind*, which is right only for a pure wind
sea and was invisible on `whitecaps.loom` precisely because that scene authors
every wave within 20° of the wind. They now lie along `break_dir`, the local
compression's eigenvector — ADR 0053 §7.

### AMENDED 2026-08-30: reverted. The frame is the global wind again.

**This section is withdrawn.** It was right about the physics and wrong about
what a *per-fragment rotating domain* does to everything downstream of it, and
three separate defects were traced back to that one term:

1. **The band-limit ladder could not see the frame's own rotation.** The screen
   derivative of `R(wd)·wp` is `R·dwp + dR·wp`, and only the first term was ever
   measured, so octaves were declared fully resolved at **2x to 50x** their true
   frequency and the near-binary erosion threshold turned that into a
   rectilinear lattice. `178da5b` fixed the measurement — see
   `.superpowers/sdd/foam-lattice-report.md`.
2. **The coordinate is translation-variant.** `R(x)·x` for a non-gradient
   direction field `R` cannot be made translation-invariant by any choice of
   constants. Measured: offsetting the same sea by 300 m turns its lace into
   white noise. `178da5b` recorded this as latent, because every scene in the
   repository is near the world origin.
3. **`ddx` of an interpolated varying is piecewise constant on each triangle.**
   This is what the honest measurement in (1) then exposed: `foamFrameFoot`
   differentiates the frame, `break_dir` arrives as a per-vertex varying, so
   every octave weight and the erosion band with them became **per-triangle
   constants** and jumped at every triangle edge. The sea beside `ocean_fft`'s
   buoy was painted in the water mesh's own triangulation — hard-edged faceted
   shards, photographed by the human. `.superpowers/sdd/foam-field-report.md`
   has the repro, the probes and the fix.

A global uniform has none of the three: `dR` is exactly zero, so the footprint
is the honest world footprint, it is smooth across triangle edges, and it is
translation-invariant. It is also what every readable production ocean does —
Crest, gasgiant's `Ocean_FoamTrailDirection0/1`, Houdini's "Streak Direction"
and WaveWorks all comb foam by a **uniform**, and Tessendorf's own minimum
eigenvector is universally used as a *scalar* and discarded as a *direction*
(`foam-rebuild-report.md` §1.2 has the sources).

**The cost is exactly the one this section named**, and it is accepted: one
direction is right only for a pure wind sea, so a swell crossing the wind has
its foam combed the wrong way. That is a wrong *angle* on a soft texture. What
it replaces is a wrong *shape*, with the mesh showing through it.

`WaterSample::break_dir` is untouched — it is still computed, still compared by
the CPU/GPU agreement test, and still available to anything that wants a local
breaking axis. What is gone is the water vertex shader's `breakDir` varying,
which had this as its only consumer.

## Foam ages

`age = foamHist / max(instantaneous, foamHist)`, free, because both numbers were
already in hand. It drives the albedo 0.72 → 0.28 and the substitution opacity
1.0 → 0.55: Koepke's whitecap reflectance falls 55% → 3–10% over about ten
seconds as the raft drains. The epsilon in the denominator is not tidiness —
without it the expression is 0/0 wherever the foam came from ADR 0055's field
alone, and a NaN age lerps the shaded result to NaN, which resolves to black.
Measured: `whitecaps` came back with the field's footprint punched out of it in
solid black.

## What is unchanged

The decision itself. The trail is still closed-form, still takes no buffer,
still costs ADR 0045 clause 3 nothing. ADR 0055 does not replace it: the trail
is what remembers a *wave*, everywhere on an unbounded sea, and the field is
what remembers a hull inside a 63.5 m domain. The shader takes the maximum.
