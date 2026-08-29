# The sea has a sound: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A sea that is audible, and audibly different at the berth and at the edge.

**Architecture:** A synthesised bed, driven by the numbers the water already computes,
rendered offline and measured — exactly the shape `loom_rain`'s audio takes. No recording,
no new dependency, no second opinion about what the sea is doing.

**Tech Stack:** Rust, `loom_audio`, `loom_water`.

**Spec:** `docs/design/SEA-REBUILD.md`. This is not in its §8 phase list — it was asked for
directly on 2026-08-29, alongside spray.

**Branch:** `sea/rebuild`.

## What exists, and what does not

`crates/loom_audio/` has ray-traced acoustics (*"what a place sounds like, worked out by
casting"*), a mixer, a device layer, and `rain.rs` — a synthesised rain bed with a recorded
fallback, renderable offline by `loom audio` which reports **rms, peak and tilt** (high-band
over low-band energy, *"the number that says darker"*).

**There is no wave, sea or surf sound of any kind.** Grepped: no function in `loom_audio`
names one. This is unbuilt rather than half-built, which makes it a clean addition.

`loom_audio::lib.rs:82` already handles submersion — *"Submersion enters where every other
acoustic fact does"* — so an ear going under water is somebody else's solved problem.

## The idea, in one paragraph

A sea's sound is not one noise. It is a **low rumble** from swell moving water, and a
**broadband hiss** from breaking crests — and the ratio between them is what tells you,
with your eyes shut, whether it is a swell or a gale. Those are exactly the two quantities
this engine already computes per tick: significant wave height, and the fraction of the
surface past `WATER_FOAM_BREAK`. So the bed is derived, not authored, for the same reason
`spectrum.rs` refuses to let anybody author sixteen amplitudes: *"a sea has one honest input
… and every other number about it follows."*

## Global Constraints

- **No new dependencies.** No `thread_rng`, no `HashMap`/`HashSet` iteration, no wall clock
  in simulation code.
- **Deterministic and offline-renderable.** `loom audio` must produce the same wave file
  twice; rain's audio is reproducible and this must be too. That means a seeded, fixed-order
  synthesis — the same discipline `loom_field::noise` exists for.
- **Read the water's own numbers; do not re-derive them.** `sample_water` and
  `Ocean::significant_height` are the authority. A second opinion about how rough the sea is
  would be free to disagree with the one the boat floats on, which is this project's
  signature failure.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's, modified and
  uncommitted. Never stage, stash, revert or edit them.** Never `git add -A`.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` clean.
- Eight false comments have been found on this branch. Verify every claim against the code.

---

### Task 1: The bed

**Files:** `crates/loom_audio/src/sea.rs` (new), `crates/loom_audio/src/lib.rs` (declare it)

**Interfaces produced:**
- `pub struct SeaState { pub hs: f32, pub breaking: f32, pub wind: f32 }` — what the sound
  is a function of, and nothing else
- `pub fn bed(state: SeaState, seconds: f32, sample_rate: u32, seed: u32) -> Vec<f32>`

- [ ] **Step 1: Write the failing tests.** These are the contract:

  - **Determinism.** Same state, same seed, byte-identical samples — `to_bits()` equality,
    not approximate. `loom audio` renders offline and its output is compared; a bed that
    drifts is not renderable.
  - **A calm sea is quieter than a gale.** rms rises monotonically with `hs` across at least
    four steps.
  - **A breaking sea is *brighter*, not merely louder.** This is the load-bearing test and
    the one the feature exists for: hold `hs` fixed, raise `breaking` from 0 to 0.2, and
    assert the **tilt** rises — high-band over low-band energy, the same measure
    `loom audio` already reports for rain. Without this, "breaking" is just a volume knob
    and the sea sounds like one thing at every state.
  - **Silence is silent.** `hs = 0, breaking = 0` produces samples that are all zero, not a
    quiet hiss. `pool.loom` authors `Wind.speed = 0` and must not whisper.
  - **No clipping.** Peak stays inside ±1.0 across the legal range of states, including the
    top rung (`hs` 6.1, `breaking` at whatever the FFT sea reaches).

- [ ] **Step 2: Run them, watch them fail.**

- [ ] **Step 3: Implement.** Two shaped noise sources summed in a fixed order:
  a low band for the swell rumble scaled by `hs`, a high band for the breaking hiss scaled
  by `breaking`. Noise comes from `loom_field::noise`'s frozen hash, not a crate and not
  `thread_rng` — the same rule the whole engine follows, and here it is also what makes the
  determinism test passable.

  **Document the mapping from state to level with its reasoning**, and if a constant is
  chosen by ear rather than derived, say so at the constant. This project's rule is that a
  number without a referent is a defect; "chosen by ear, and here is what it sounded like"
  is an honest referent for audio and an unmarked magic number is not.

- [ ] **Step 4: Verify, and listen.** Render a few seconds at three sea states with
  `loom audio`, report **rms, peak and tilt** for each, and put the numbers in the report.
  Three states that produce three clearly different rows is the acceptance.

- [ ] **Step 5: Commit.**

---

### Task 2: The sea drives it

**Files:** `crates/loom_cli/src/sound.rs` and/or `weather.rs` — wherever the rain bed is
fed from; follow that path rather than inventing a second one.

- [ ] **Step 1** Find how the rain bed reaches the mixer and mirror it. Rain is
  `intensity × cloud_cover × shelter`; the sea's equivalent is its own state, and
  **`Ocean::significant_height` and the foam coverage are already computed once per tick** —
  read them, do not recompute.

- [ ] **Step 2** A `WaterBody` in the scene means the sea is audible; no water means silence.
  Follow the precedent that *"a raining scene that authors no cover gets a solid deck"* — the
  default should be the one that keeps every existing scene sounding exactly as it does now.
  **Confirm that: no existing scene's audio may change.** `loom audio` on a scene without
  water must produce a byte-identical wave file before and after this task.

- [ ] **Step 3** Assert it from the CLI: `loom audio assets/test/ocean_fft.loom` reports a
  higher rms and a higher tilt than the same command on a calm-water scene. Record both.

- [ ] **Step 4** Commit.

---

## Self-Review

**Scope.** Two tasks: a bed that is a pure function of sea state, and the wiring that feeds
it. Deliberately excluded: positional sea sound (a shoreline louder than open water), the
Doppler of a hull under way, and any recorded sample. The first two are real features and
belong with the shore work; the third would need an asset and a licence.

**The risk.** Audio has no golden-image gate and no ablation harness — `loom audio`'s rms,
peak and tilt are the only instruments, and a bed that is *present but wrong* would pass
every one of them. That is the same failure class this branch has spent its whole life on,
so the tilt test in Task 1 Step 1 is doing more work than it looks: it is the only assertion
that distinguishes "the sea makes a sound" from "the sea makes the *right* sound".
