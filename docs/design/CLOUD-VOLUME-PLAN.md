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
| `crates/loom_scene/src/components.rs` | `Sky.cloud_type: Option<f32>`, and the two load-time refusals. |
| `crates/loom_render/src/renderer.rs` | Derive type from cover when `None`; pack into `cloud.w`. |
| `crates/loom_render/src/ablate.rs` | `cloud_volume` row and its mask bit. |
| `xtask/src/main.rs` | The `ABLATE` entry and its floor. |

---

## C1 — Type reaches the shader, and no pixel moves

The plumbing, alone, so that the task which changes the picture changes only the picture.

- [ ] `Sky` gains `cloud_type: Option<f32>`, mirroring the `Option<f32>` override pattern
      `components.rs:1163` already uses for `cloud_cover`. Document it as one scalar driving
      base altitude, thickness and profile shape together, and say why those are coupled.
- [ ] When `None`, derive on the **CPU at scene load** from cover: low cover is a cumulus
      field, high cover is an overcast ceiling. The shader gets a plain float — no sentinel,
      no per-pixel branch. Unit-test the mapping's endpoints, the way `coverage_shape` pins
      the coverage curve's.
- [ ] Two load-time refusals, each naming the authored value in the message (ADR 0078 §5):
      `cloud_type` outside `[0, 1]`; `cloud_scale <= 0`. Test both refuse.
- [ ] Pack into `cloud.w`. Assert the `EnvironmentGpu` layout test is untouched.

**Green:** `cargo clippy --workspace -- -D warnings`; `cargo test --workspace`;
`cargo xtask validate`; **`cargo xtask image` 60/60 unchanged.** That last one is the point
of the task — nothing reads `.w` yet, so a moved reference here means something else moved
and the cause is findable now rather than tangled into C2.

---

## C2 — The march, and the switch that undoes it

Where parallax appears.

- [ ] `cloud_volume` joins `ABLATIONS` in `ablate.rs` with the next free bit, and gets its
      `LOOM_ABLATE_CLOUD_VOLUME` Slang constant — unlike `spray_droplets`, this one is a
      shader switch, so it follows the ordinary pattern rather than that row's exception.
- [ ] `skyColor` routes cloud sampling through one call site with two implementations.
      Ablated, the projection path runs and must be **byte-identical to today** on all nine
      cloud scenes. Verify that before building the volume, not after.
- [ ] The slab: `base` and `top` from type per ADR 0078 §3, entry/exit closed-form,
      `dir.y > 0` only, total march length capped, `horizonFade` reused unchanged.
      One branch falls back to the projection when `eye.y >= base` (ADR 0078 §4).
- [ ] `density(p) = coverage(p.xz) · profile(h, type) · erosion(p)`. `coverage` is
      `clouds_at` at the step's world `xz`. `profile` lerps the stratus and cumulus height
      gradients by type. `erosion` is ridged fBm from `loom_value_noise`, advected on the
      same wind the deck drifts on, biting hardest where the profile is already weak.
- [ ] Accumulate transmittance with Beer–Lambert and an early-out, following
      `SMOKE_STEPS`/`SMOKE_T_MIN`. Light it with the existing `CLOUD_DARK`/`CLOUD_LIT` pair
      driven by accumulated density — **flat lighting on purpose**; the sun march is C3, and
      keeping them apart is what makes it possible to say which one bought the look.
- [ ] Set the ablate floor from the measured change, not from a guess.

**Measure here and write the number down:** frame time on `squall` and `mood_deep`, with and
without `LOOM_ABLATE=cloud_volume`. That difference is the feature's cost and it is the
number ADR 0015 said this engine did not have.

**Green:** clippy; `cargo test --workspace`; `validate`; `ablate` with the new row passing;
`repeat` byte-identical across three fresh processes. **`image` will move the nine cloud
scenes and must move nothing else** — that containment is the check. Do not bless.

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
