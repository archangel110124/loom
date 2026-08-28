# Wiring the FFT ocean in: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A scene can ask for the FFT ocean, and when it does, the boat floats on the same
surface the player sees.

**Architecture:** Opt-in on `WaterBody`, defaulting off. `sample_water` reads the cascade
for a body that asked; the CPU uploads its tiles and the GPU displaces from them; the
breaking criterion is derived from the same tiles so foam has something to read. Nothing a
scene did not ask for changes.

**Tech Stack:** Rust (`loom_scene`, `loom_water`, `loom_render`), Slang, `xtask`.

**Spec:** ADR 0076 and `docs/design/SEA-REBUILD.md` §3. Read the ADR first — its licence is
that the ocean is a pure function of (scene, tick), and every choice here protects that.

**Branch:** `sea/rebuild`.

## The decision that shapes this plan

**Opt-in, default off.** `WaterBody` gains a field selecting the wave model; absent means
the sixteen-wave sum exactly as today. Follow the precedent `ParticleEmitter.gpu` set —
*"default `false`, so all eight blessed particle references are untouched"*.

This is not timidity, it is what makes the plan reviewable. Twenty-five commits have landed
so far without moving a single pixel or determinism hash, because nothing was wired. The
naive integration ends that in one commit: every water scene's sim hash moves, fourteen
golden references move, and they land on top of the fifty-four already outstanding from the
tonemap re-bless — an unreadable diff, which `docs/rebless-the-tonemap.md` exists precisely
to prevent. **With the opt-in, the only thing that moves is a new scene that did not exist
before.** Converting the shipping scenes is then a separate, deliberate act, one scene at a
time, each with its own bless.

## Global Constraints

- **ADR 0045 clause 2 holds absolutely: no GPU readback, ever.** The CPU tile is the
  authority; the GPU is told what the CPU computed. If any step here would need a value
  back from the device, it is the wrong step.
- **No new dependencies.** Never `thread_rng`, `HashMap`/`HashSet` iteration, or a wall
  clock in simulation code.
- **Never edit `assets/shaders/generated/*.slang`** — emitted from Rust by `build.rs`.
  `assets/shaders/scene.slang` is hand-written and is where shader work goes.
- Never create `VkRenderPass`/`VkFramebuffer`; never allocate per-draw descriptor sets;
  never call `vkAllocateMemory`; **never place a barrier outside the render graph** —
  never-do #4 covers buffers, which the drop buffer forced.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's, modified and
  uncommitted. Never stage, stash, revert or edit them.** Never `git add -A`.
- Green is six checks; check 1 is `cargo clippy --workspace --all-targets -- -D warnings`.
- **`cargo xtask image --bless` is the human's alone.** Produce diffs and numbers; never
  bless.
- The three-way normalisation coupling (unnormalised transform, variance-calibrated
  amplitudes, no `1/N²`) is correct only together. Do not "fix" one.

## File Structure

| file | responsibility |
|---|---|
| `crates/loom_scene/src/components.rs` | the `WaterBody` opt-in field and its load-time validation. |
| `crates/loom_water/src/lib.rs` | `sample_water` dispatches to the cascade when asked. |
| `crates/loom_water/src/ocean.rs` | gains the breaking criterion from its own tiles. |
| `crates/loom_render/src/renderer.rs`, `viewer.rs` | upload the tiles; both stamp sites. |
| `assets/shaders/scene.slang` | `waterVertexMain` displaces from the tiles. |
| `xtask/src/main.rs` | the new scene in `SCENES`, `GOLDEN` and `ABLATE`. |
| `assets/test/ocean_fft.loom` | the scene that opts in. |

---

### Task 1: The opt-in, and proof that nothing else moved

**Files:** `crates/loom_scene/src/components.rs`, `assets/test/ocean_fft.loom`

The field selects the wave model. Absent or `"waves"` is today's behaviour; `"spectrum"`
(or whatever name reads best beside the existing `simulation` field) selects the cascade.
Validate at load: a body asking for the cascade while also authoring an explicit wave list
is a contradiction and should be refused with a message saying which to remove — this
project refuses at load rather than commenting, and there are four such refusals on
`ParticleEmitter` as precedent.

- [ ] **Step 1** Add the field, its schema, and the load-time refusal, with tests for both
  the default and the contradiction.

  **Two things the hardening plan left for this task**, from its Task 3:

  `Cascade` now carries required band fields (`k_lo`, `k_hi`), so the scene layer has to
  express them — either authored per cascade, or derived from the cascade's patch and
  resolution with the derivation written down. **Prefer deriving**: a band an author can
  get wrong is a band an author will get wrong, and a gap silently loses energy while an
  overlap double-counts it.

  And `Ocean::new`'s check that consecutive bands meet is currently a **panic**. Once a
  scene can author cascades that becomes reachable from a `.loom` file, and this project
  refuses at load rather than panicking — `ParticleEmitter` has four such refusals as
  precedent, each naming the required value in its error. Convert it here.
- [ ] **Step 2** Write `assets/test/ocean_fft.loom` — deep water, no bed, `U10 = 18`,
  `fetch = 440000`, an authored camera low to the water where detail is visible.
- [ ] **Step 3** **The proof this task exists for:** `cargo test --workspace` and
  `cargo xtask validate`, and confirm **no determinism hash moved and no golden reference
  moved**. Nothing opts in yet except the new scene, which has no reference. Report both
  results explicitly; a "probably fine" here defeats the whole structure of the plan.
- [ ] **Step 4** Commit.

---

### Task 2: `sample_water` reads the cascade

**Files:** `crates/loom_water/src/lib.rs`, `crates/loom_water/src/ocean.rs`

This is the task that puts the FFT ocean on the force path. After it, buoyancy, `loom sim
--assert`, `rhai` and the sim hash all read the cascade for a body that asked.

- [ ] **Step 1** Decide where the `Ocean` lives and who evolves it. It must be evolved
  **once per tick, inside the fixed step**, and it must anchor to the water body, never to
  the camera (ADR 0045's stated trap: *"a force-producing CPU sim grid anchors to sim
  state, never to the camera"*). Write the ownership down; it is the thing a later reader
  will get wrong.
- [ ] **Step 2** `sample_water` returns the cascade's height, displacement and velocity for
  an opted-in body. **`WaterSample`'s existing fields keep their meanings** — every consumer
  reads them and none should need to know which model produced them.
- [ ] **Step 3** **The breaking criterion, which is new work and was not in ADR 0076's
  estimate.** `WaterSample::mu_max` is the largest eigenvalue of the horizontal Jacobian's
  symmetric part, and the ADR's claim that `mu_max` needs "only a richer field to read" is
  not yet true — `Ocean` exposes no derivative. Derive `Sxx`, `Szz`, `Sxz` by finite
  differences on the displacement tiles, on the CPU, once per tick, and keep `fold` and
  `break_dir` consistent with them. **Without this the FFT sea renders with no foam at all**,
  and Task 4's ablation row would correctly report it drawing nothing.
- [ ] **Step 4** Assert from the CLI: `loom water --at x,z --sim N` on the new scene must
  report a plausible height, a nonzero `mu_max` and — at `U10 = 18` — foam coverage above
  zero. Record the numbers against the pre-FFT baseline in
  `.superpowers/sdd/SEA-FFT-CORE-PLAN/baseline/README.md`, which was captured for this.
- [ ] **Step 5** Re-pin any determinism hash this moves, in the same commit, deliberately.
  **Only the new scene should move.** If an existing scene's hash moves, the opt-in leaks —
  stop and report.

---

### Task 3: The GPU draws what the CPU computed

**Files:** `crates/loom_render/src/renderer.rs`, `crates/loom_render/src/viewer.rs`,
`assets/shaders/scene.slang`

- [ ] **Step 1** Upload the tiles. Three fields per cascade; they are three channels of one
  tile, not three tiles. **The barrier belongs to the render graph** — a CPU-written buffer
  read by `waterVertexMain` needs its dependency declared through
  `BufferId`/`BufferAccess`/`pass_with`, and `plan_full`'s barrier-list test must name it.
  A missing dependency draws last frame's ocean, which looks almost right.
- [ ] **Step 2** `waterVertexMain` displaces from the tiles instead of summing waves, for an
  opted-in body. **Both stamp sites** — `renderer.rs` and `viewer.rs` — must be fed, the
  lesson from `LOOM_ABLATE`: the viewer is the window the human judges water in, and it has
  its own path.
- [ ] **Step 3** **State the interpolation choice and its cost.** The sim evolves the ocean
  once per tick; a frame between ticks either uses the last tick's tile or re-evaluates at
  frame time. Re-evaluating is *legal* precisely because the ocean is stateless — that is a
  real dividend of ADR 0076's argument — but it costs a full `evolve` per frame. Take the
  last tick's tile, measure whether it judders at the authored camera, and write the finding
  down either way.
- [ ] **Step 4** `cargo xtask validate` must report **zero Vulkan validation messages**, and
  `cargo xtask repeat` must show the new scene byte-identical across three processes.
- [ ] **Step 5** Render the new scene and **compare it against the pre-FFT baseline**. A
  whole-frame diff will be near-total and says nothing — the wave field changed. The
  questions the baseline README poses are the ones to answer: did detail density rise, does
  `mu_max` now reach the breaking threshold on a *derived* sea rather than only a
  hand-authored one, and does the sea look like the reference.

---

### Task 4: The gates learn about it

**Files:** `xtask/src/main.rs`

- [ ] **Step 1** Add `ocean_fft` to `SCENES` and `GOLDEN`. *"Adding a rendering path means
  adding a scene to `GOLDEN`, or the gate reports a full pass without ever having looked at
  it."*
- [ ] **Step 2** Add an `ocean_fft` row to `ABLATE`, with its floor **measured at the size
  the task renders**, and the measurement written beside the number.
- [ ] **Step 3** Run `cargo xtask ablate` and prove the new row bites — sever the wiring,
  confirm `DRAWING NOTHING`, restore. The recipe is in `SEA-REBUILD.md` §7.3; **sever the
  wiring, never zero the effect in the shader**, and rebuild on the way out.
- [ ] **Step 4** Produce the reference diffs for the human. **Do not bless.**

---

## Self-Review

**Coverage.** ADR 0076's mechanism reaches the force path (Task 2), the screen (Task 3) and
the gates (Task 4), behind an opt-in (Task 1) that keeps every existing scene bit-identical.

**One item is new work the ADR did not budget**, and it is called out rather than absorbed:
`mu_max` from a tile (Task 2 Step 3) is finite differences and a sampling decision, not the
same closed-form maths. The ADR should be amended when its shape is known.

**Deliberately out of scope:** converting `deeper_demo`, `fishing` or any shipping scene to
the FFT sea; the shoaling bake; breaking; the curl sheet. Each is its own phase in
`SEA-REBUILD.md` §8. This plan makes the ocean *available*, not universal.

**The risk that would sink this plan** is the opt-in leaking — an existing scene picking up
the cascade because a default was misread. Task 1 Step 3 and Task 2 Step 5 both assert it
independently, and both are stop conditions rather than notes.
