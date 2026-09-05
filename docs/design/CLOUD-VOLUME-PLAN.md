# The cloud deck becomes a volume: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the sky-plane projection with a raymarched slab, so the deck has
thickness, parallax, a silhouette and an underside — measured against the frame, with a
switch that restores the projection exactly.

**Architecture:** Three layers, and only the middle one is new. `clouds_at` stays the 2D
weather map it already is and the rain keeps reading it. The march invents the vertical
dimension from a height profile and frozen noise. The lighting reuses `smokePhase` and
Beer–Lambert. **No new pass, no new resource, no new dependency, no GPU state** — a frame
stays a pure function of (scene, tick), which is what keeps `cargo xtask repeat` out of this.

**Tech Stack:** Slang in `assets/shaders/scene.slang`; Rust in `loom_scene`, `loom_render`,
`xtask`. Nothing new.

**Spec:** **ADR 0078** `docs/decisions/0078-the-deck-becomes-a-volume-and-the-weather-map-is-already-there.md`,
which is the binding argument — read it first. It amends ADR 0015's third clause and ADR
0016's §"What it does not do" first sentence, and both amendments are narrow. ADR 0016
remains the record for what the field is *for*.

**Branch:** `sea/rebuild`.

**Deliberately out of scope:** cloud shadows on the world; god rays; rain leaving the deck
(ADR 0016 step 5, still unbuilt); distant rain curtains; weather that evolves rather than
advects; any camera above the cloud base. ADR 0078 §6 carries the reopening trigger for
each. Do not build them here and do not half-build them.

## Global Constraints

- **`clouds_at` is not to be edited.** Not its body, not its params, not its output arity.
  The whole argument of ADR 0078 §2 is that the field is already the right shape; a task
  that finds itself wanting to change it has misread the plan and should stop and say so.
  `field_agree` must not move.
- **No new uniform, no struct growth.** Type goes in `cloud.w`, which is documented unused
  at `crates/loom_render/src/renderer.rs:315` and read nowhere. The `EnvironmentGpu` layout
  test must not change by one line.
- **No new dependency, no 3D image, no new pass, no new pipeline.**
- **Rendering only.** Nothing here is readable by `loom sim --assert` or by `rhai`, nothing
  produces a force, nothing stores state. ADR 0045 governs. If a task finds itself adding
  state, it has left the plan.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's, modified and
  uncommitted. Never stage, stash, move or revert them.** Never `git add -A`.
- The fixed step is **60 Hz = 16.67 ms**. Every cost claim is a fraction of that.
- This project documents the *reasoning* behind decisions in the code, not only the
  mechanism.
- **A builder never blesses its own references and never promotes its own ADR.** ADR 0078
  lands at `proposed` and stays there.

## File Structure

| file | responsibility |
|---|---|
| `assets/shaders/scene.slang` | The march. `cloudPlane`/`cloudSample` keep working as the fallback; the volume is a sibling and one switch chooses. |
| `crates/loom_scene/src/components.rs` | `Environment.cloud_type: Option<f32>`, and the load-time refusal. |
| `crates/loom_render/src/renderer.rs` | `CLOUD_TYPE_DERIVE`, the sentinel `cloud.w` carries when a scene authors no type. |
| `crates/loom_cli/src/main.rs` | Pack it: `environment_of_inner`'s `cloud` vec4. |
| `crates/loom_render/src/ablate.rs` | `cloud_volume` row and its mask bit. |
| `xtask/src/main.rs` | The `ABLATE` entry and its floor. |

---

## C1 — Type reaches the shader, and no pixel moves — **DONE**

The plumbing, alone, so that the task which changes the picture changes only the picture.

- [x] `Environment` gains `cloud_type: Option<f32>`. Documented as one scalar driving base
      altitude, thickness and profile shape together, and why those are coupled.
- [x] `CLOUD_TYPE_DERIVE = -1.0` in `loom_render`, written to `cloud.w` when the scene
      authors nothing. **Resolution is the shader's, not the CPU's — corrected while
      building; see below.**
- [x] Load-time refusal on `cloud_type` outside `[0, 1]`, with a test that also pins *why*
      it needs testing: the range attribute is wrapped in an `Option`, and whether it
      survives that is `schemars`' business, not this crate's.
- [x] `cloud_scale <= 0` — **already refused** by `range(min = 50.0, max = 20000.0)`,
      which predates this plan and is stricter. Pinned by a test rather than duplicated into
      a second mechanism that would disagree the first time one of them moved.
- [x] Packed into `cloud.w`. The `EnvironmentGpu` layout test is untouched.

### Correction: the derivation is the shader's job

This task was written as *"derive on the **CPU** at scene load … no sentinel, no per-pixel
branch."* **That is not buildable, and the reason is worth keeping.**

The type has to be derived from the *effective* cover, and cover is not final when the
environment is built. A scene that rains and authors no cover has its deck forced solid in
`rain_at_eye` (`main.rs:3679`) — which returns early for every dry scene, and is called from
three places with no common point after it, one of them `run.rs`, which this plan may not
touch. Deriving in `environment_of_inner` would therefore read cover `0.0` for exactly the
downpours that most want a flat ceiling.

So the CPU carries the authored value or a negative sentinel, and the shader resolves in one
uniform branch against the `cloud.x` it already has. **No twin is created**: nothing on the
CPU reads cloud type — the rain reads cover — so there is no second implementation to keep
in step and no agreement test owed. The `Option` on the component and the sentinel in the
buffer are the same fact written on the two sides of four floats that cannot carry a `None`.

**Consequence for C2:** the cover→type curve now lands there, in Slang, and its proof is a
picture rather than a unit test. C2 owns stating what the curve is and why.

**Green:** `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`;
`cargo xtask validate`; **`cargo xtask image` 60/60 unchanged.** That last one is the point
of the task — nothing reads `.w` yet, so a moved reference here would mean something else
moved, findable now rather than tangled into C2.

---

## C2 — The march, and the switch that undoes it — **DONE**

Where parallax appears.

- [x] `cloud_volume` in `ABLATIONS` at bit 5, with `LOOM_ABLATE_CLOUD_VOLUME = 32u` in
      Slang and a row in `xtask`'s table. Note the gap: bit 16 is `spray_droplets`, which is
      a CPU early return and the one row with no Slang constant.
- [x] `skyColor` routes through `cloudLook`, with `cloudLookFlat` and `cloudLookVolume`
      behind it. **Verified before the volume was trusted:** `LOOM_ABLATE=cloud_volume`
      renders 60/60 against the pre-C2 references, so the restructure moved nothing.
- [x] `cloudType()` beside `cloudCover()`, resolving the sentinel against the effective
      cover. Crossing band 0.5-0.9.
- [x] The slab, entry/exit closed-form, `dir.y > 0`, capped span, `horizonFade` reused, and
      the fallback branch when `eye.y >= base`.
- [x] `density = mask * profile - erosion`, and the ablate floor set from the measurement.
- [x] Beer-Lambert with an early-out; flat height-based lighting, deliberately.

### Four renders, and each wrong one is recorded rather than tidied away

The first three versions were all wrong, in ways worth keeping because two of them were
failures of *this plan's own reasoning*, not of the code.

1. **A flat wash.** `CLOUD_SIGMA = 0.01` made a density-0.2 column 96% opaque, so every
   cloudy pixel saturated. Fixed by deriving the figure instead of picking it.
2. **Vertical streaks.** The ray was capped at 12 km over 28 steps — 429 m per step against
   a 260 m detail scale. More than half that budget was spent on ray `horizonFade` had
   already deleted; the cap is now derived from the fade.
3. **Mush at small `cloud_scale`.** The slab was up to 1600 m thick while `squall`'s masses
   are 260 m across, so every cloud was six times taller than it was wide. **ADR 0078 §6
   called this a C4 re-authoring problem and that was wrong** — thickness follows
   `cloud_scale`, and Addendum 1 records why re-authoring could never have fixed it.
4. **Wash at high cover.** The mask used coverage alone, which saturates. ADR 0015 had
   already written this trap down for the shading tap and this plan did not carry it
   forward. Addendum 2.

### What is still wrong, and is C3's

**`lanternhead` reads worse than the plane deck** — a uniform bright grey. Its failure is
tone, not form, which is what the sun march, the phase function and the powder term exist
to fix. Named here rather than blessed away; the references still hold the plane version.

### The measurement

At 960x640, `--sim 300`. Forward 0.040 -> 0.438 ms on `squall`. **The water pass is 83% of
`mood_deep`'s added cost** (0.191 -> 1.66 ms) because reflections call `skyColor`, so the
march runs per water pixel too — unpredicted, and it changes which escalation ADR 0078 §5
should reach for. See Addendum 3. Roughly +4.7 ms at 1080p.

**Green:** clippy clean; `cargo test --workspace` 46 suites; `validate` 93 runs zero
messages; `repeat` 60/60 byte for byte; `ablate` 6/6 with `cloud_volume` at 67.098% against
a 30% floor; `image` moves ten rows and nothing else. Not blessed.

## C2b — The deck is marched once into a direction-indexed map — **DONE**

Not a task this plan planned. It came out of ADR 0078 Addendum 3's measurement and the
human's instruction to escalate, and it is recorded here so the plan is not read as if
C2 went straight to C3.

- [x] `crates/loom_render/src/cloud_map.rs` — a 1024x512 `R16G16B16A16_SFLOAT` target and
      the pipeline that fills it, from two entry points added to `scene.slang` itself.
- [x] `cloudMapTex` on set 3 binding 2, wrapping in `u` and clamped in `v`.
- [x] `cloud_map` rides one lane of `flow_pad`; the layout pin is still 1168 bytes.
- [x] One `CloudMap` type, **both render paths**, both calling the same `record`.
- [x] The pass is skipped entirely when the volume is ablated.
- [x] Both pinned barrier lists updated, and mutation-checked: removing the `ShaderRead`
      declaration fails `the_water_block_reads_what_the_opaque_half_left`, restoring it
      passes.

**3.0x on `mood_deep` and 2.5x on `squall` at 1080p** — the table is in Addendum 4.

**Green:** clippy; 46 test suites; `validate` 93 runs zero messages; `repeat` 60/60 byte
for byte; `ablate` 6/6 with `cloud_volume` at 67.058%; `image` moves the same ten rows as
C2 and nothing else; `loom run --edit --frames 90` runs clean in the window at 146.8 fps.

---

## C3 — The sun march, the phase function, the powder term

Where it stops being a grey volume.

- [ ] Six exponentially-spaced steps toward the sun from each march step, accumulating
      density; transmittance by Beer's law. Exponential spacing so the near shadow is
      resolved and the far one is cheap.
- [ ] `smokePhase` (`scene.slang:4413`) for Henyey–Greenstein forward scattering — the
      silver lining when the sun is behind a cloud. Reuse it; do not write a second one.
- [ ] The powder term for dark edges. State in the comment that it is an approximation of
      multiple scattering and not a derivation, the way the soot volume's constants do.
- [ ] Retire the single sunward tap at `scene.slang:659` and the `lit` lerp it feeds. Its
      comment calls it *"the highest realism-per-instruction trick available without
      raymarching a volume"* — that condition no longer holds, and the comment should say
      what replaced it rather than being deleted silently.

**Measure again:** the same two scenes. C3's delta over C2 is what the lighting costs, and
it is the number that decides whether the 6 steps stay 6.

**Green:** as C2. `image` moves the same nine and nothing else. Do not bless.

---

## C4 — The scenes, the numbers, and the hand-off

- [ ] Re-author `cloud_scale` where the march has exposed it. `rain_pool` at 90 m and
      `lanternhead`/`squall` at 260 m were tuned to make *rain* vary across a small scene
      and will read as grain once the deck is world-space geometry. **Change the scene, not
      the parameter** — ADR 0078 §6 explains why splitting it is refused, and each scene's
      comment already explains what its number was chosen for, so the edit has to answer
      that comment rather than overwrite it.
- [ ] Author `cloud_type` explicitly only where the derived default is wrong, and say in the
      scene comment why it was wrong. A scene that does not need it does not get it.
- [ ] Add the measured costs to ADR 0078 as an addendum — the C2 and C3 numbers, the step
      counts they were taken at, and whether either escalation in §5 is now triggered.
- [ ] Hand to the human: the nine moved references for blessing, and ADR 0078 for
      promotion. **A builder does neither.**

**Green:** the full six. clippy, `validate`, `test --workspace`, `image` (moving the nine),
`repeat`, `ablate`.
