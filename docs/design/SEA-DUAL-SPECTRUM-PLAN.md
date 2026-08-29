# A swell and a wind sea: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A sea that breaks. Today the biggest sea in the repository has the least white
water, and the reason is physics rather than a bug.

**Architecture:** Two spectra summed into the same cascades — a **swell**, long and tall,
carrying the height; and a **wind sea**, short and steep, carrying the breaking. This is
what "Dual JONSWAP" means in the reference implementation the human brought.

**Tech Stack:** Rust, `loom_water::spectrum` and `ocean`, `loom_scene`.

**Spec:** ADR 0076's "Two things the grid makes newly available" section, which already
records the argument and the evidence.

**Branch:** `sea/rebuild`.

## The defect, measured twice

`ocean_fft` realises `Hs = 6.100 m` — a genuine twenty-foot sea — and **barely breaks**:

    mu_max over 256 points across 4 km    mean 0.0486   max 0.2516   0.78% past 0.22
    at the scene, per the audio wiring    mu_max 0.032-0.093 against FOAM_CREST_BREAK 0.45
    foam coverage                          mean 0.0008

**Whitecapping is driven by steepness, not height**, and this sea is not steep. At `U10 =
18` with 440 km of fetch the peak wavelength is 226 m, so `Hs/λp ≈ 0.027`. We spent fetch
to buy height, and long fetch buys height by making the sea *longer* — which is precisely
what `spectrum.rs` warns in its own words: *"Raising the wind zooms the sea; it never
roughens it."* The original complaint was answered and its flatness returned in a new form.

### Why a wind sea fixes it, in one calculation

`mu_max` is a compression, and per wave it goes as `k·A` — so **short waves contribute
steepness out of all proportion to their height**:

    swell     A ~ 3.0 m   lambda 226 m   k = 0.0278   k·A = 0.083
    wind sea  A ~ 0.65 m  lambda  31 m   k = 0.203    k·A = 0.132

A wind sea a fifth the height contributes **more** steepness than the swell it rides on.
That is the whole mechanism, and it is why a real gale whitecaps while a long groundswell
does not.

**The cascades are already right for this.** The band structure carries wavelengths down to
0.354 m; the chop cascade exists and is drawn. What is missing is *energy in that band* — a
440 km spectrum puts almost none there. A wind sea does.

## Global Constraints

- **No new dependencies.** No `thread_rng`, no `HashMap`/`HashSet` iteration, no wall clock
  in simulation code.
- **The grid path and the sixteen-wave path must not be able to disagree** about what a sea
  is — that invariant is held by `amplitude_field_agrees_with_wave_set_fetch` and is the
  single most important test in `spectrum.rs`. **A second spectrum changes what that test
  can assert**; decide what it asserts now and say so, rather than letting it quietly become
  a test of the swell alone.
- **Do not touch `wave_set`, `wave_set_fetch`, `bands` or `significant_height`.**
- Anything summed on the force path is summed in a fixed, documented order.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's, modified and
  uncommitted. Never stage, stash, revert or edit them.** Never `git add -A`. Never
  `cargo xtask image --bless`.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` clean.
- Ten defects and nine false comments have been found on this branch. Verify every claim.

---

### Task 1: Two spectra in one field

**Files:** `crates/loom_water/src/spectrum.rs`, `crates/loom_water/src/ocean.rs`

- [ ] **Step 1** `amplitude_field` gains the ability to sum a second spectrum into the same
  grid. **Sum the energies, not the amplitudes** — two independent seas superpose in
  variance, and adding amplitudes would correlate them into a single larger sea with the
  wrong statistics.

- [ ] **Step 2** Keep the Hermitian symmetry, the `k = 0` zeroing and the frozen Nyquist
  cells intact — they are hard-won and there are tests on all three. **The existing pins
  will move**; re-bless them by running the test and pasting the printed value, and **say in
  the commit message why they moved**. That habit was established deliberately.

- [ ] **Step 3 — the acceptance test, and it is the reason for the whole plan.** A swell
  alone and a swell-plus-wind-sea at the same `Hs` must differ in **breaking fraction**, not
  merely in appearance. Assert that adding a wind sea raises the fraction of the surface past
  the breaking threshold by a material factor. Report the before and after.

  **Do not tune the threshold to make this pass.** If the wind sea does not raise breaking,
  that is a finding about the mechanism and the plan is wrong.

---

### Task 2: A scene can author one

**Files:** `crates/loom_scene/src/components.rs`, `assets/test/ocean_fft.loom`

- [ ] **Step 1** `WaterBody` gains an optional swell: its own `u10`, `fetch` and `direction`.
  **The wind sea is the scene's existing `Wind` and `fetch`; the swell is the authored
  remnant of an older wind from somewhere else.** That division is the physical one — a swell
  is weather that has left — and it means the default (no swell authored) is exactly today's
  behaviour, so no existing scene moves.

- [ ] **Step 2** Refuse at load what cannot be meant: a swell with no direction, a swell on a
  Gerstner body, a fetch outside the schema. Name the required value in the error, as
  `ParticleEmitter`'s four refusals do.

- [ ] **Step 3** Give `ocean_fft` a swell crossing its wind sea — **and it must stay inside
  ±90°**, for a reason Task 1 discovered and marked: a grid cell carries one amplitude and
  one travel direction, so a swell authored more than 90° off the wind lands on the right
  axis *travelling the wind's way along it*. A genuinely opposed sea needs two grids or a
  per-cell direction, and neither is in scope here.

  That limit is worth stating plainly because it **weakens** what this step can demonstrate.
  `WaterSample::mu_max` is an eigenvalue rather than a trace precisely to handle crossing
  seas — its docs record that on two 10 m swells **90° apart** the trace calls 12.32% of the
  surface past breaking where the eigenvalue calls 0.00%. At 90° exactly we are at the edge
  of what one grid can represent, so a swell at, say, 50–70° off the wind is the honest test:
  a real crossing, well inside the representable range. Report the trace and the eigenvalue
  both, and say how far apart they are — that comparison is the closest this engine has come
  to exercising a decision it made long before it had a sea to make it on.

- [ ] **Step 4** Re-pin every hash that moves, deliberately, in the same commit.

---

### Task 3: Look at it, and hear it

- [ ] **Step 1** Render `ocean_fft` before and after, and a `--frames 48 --spin 0 --step 4`
  sequence of each. **The still will not settle it** — a crossing sea is a motion phenomenon,
  and the whole reason this repository has a sequence instrument.

- [ ] **Step 2** Re-measure the numbers that defined the defect: `mu_max` mean and max, the
  fraction past threshold, and foam coverage, over the same 4 km sample the baseline used.

- [ ] **Step 3** Re-measure `Hs`. **The swell must not have quietly made the sea bigger than
  twenty feet** — it is added energy, and the top rung is a number the human chose.

- [ ] **Step 4** `loom audio` on the new scene. The bed reads `breaking` directly, so if this
  plan works the sea should audibly gain its hiss — the tilt should rise without the level
  moving much. That is a prediction; report whether it held.

---

## Self-Review

**This is the last thing standing between the sea and the reference footage.** Everything
else — the transform, the spectrum, the cascade, the force path, the GPU, the gates, the
sound — is built and measured. The sea is the right size, in the right place, drawn by the
right pipeline, and it does not break.

**The risk worth naming:** a wind sea adds energy, so it adds height too, and the twenty-foot
target is a number the human chose deliberately after a wrong turn. Task 3 Step 3 exists to
catch that, and if the swell must shrink to keep `Hs` at 6.10 while the wind sea supplies the
steepness, that is the correct trade and should be made explicitly rather than discovered.
