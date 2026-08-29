# The boat floats where it is painted to: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** JIB VI's mass, its buoyancy model and its paint all derive from one measurement of
the hull, so they cannot disagree again.

**Architecture:** A generator reads the shipped meshes and produces the hull's sectional
area curve. Everything else — displacement, the immersion model, the waterline the antifoul
is painted to — falls out of that one table.

**Tech Stack:** Python (the generator, in the idiom of `tools/mesh/rig/`), Rust
(`loom_water::buoyancy`, `loom_scene`), and the `jib_vi` prefab.

**Spec:** `docs/design/SEA-REBUILD.md` §6, which is this plan's argument, and §2.1, which is
the evidence. Read both.

**Branch:** `sea/rebuild`.

## The defect, measured

Three numbers describe one hull and none of them agree:

    hull displacement below its designed line (y = 0)   57.64 - 60.34 m^3
    mass                                                43,776 kg = 43.78 m^3
    where the antifoul is painted to                    y = -0.060
    where it actually floats                            y = +0.024

`jib_vi_float.loom`'s own header records how: *"the wide hull displaces 57.64 m^3 … a ratio
of 1.152, so `mass` scales 38000 -> 43776"* — mass was scaled by a volume **ratio** instead
of being set to the displacement. **The boat has been about a quarter light ever since.**

Nothing errors, because the twelve pontoons are internally consistent with each other:
`12 × (4/3)π(1.093)³ × ⅔ = 43.78 m³`. The proxy holds the origin at `y ≈ 0` exactly as
designed and the hull it stands in is simply never consulted. That is the failure this whole
project's water docs open by naming — *"neither side errors, neither side is wrong on its
own, and the only symptom is that it never quite lines up."*

And the sea it floats in is now a twenty-foot one, so "roughly right" has stopped being
good enough.

## Global Constraints

- **No new dependencies**, in Rust or in the generator. `tools/mesh/rig/rigkit.py` already
  has `read_obj`, `signed_volume` and `bounds`; reuse them rather than writing a third OBJ
  reader.
- Never `thread_rng`, `HashMap`/`HashSet` iteration, or a wall clock in simulation code.
- **Pontoons are summed in index order** and any new summation on the force path is too —
  float addition is not associative and this project hashes it.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's, modified and
  uncommitted. Never stage, stash, revert or edit them.** Never `git add -A`.
- **Never run `cargo xtask image --bless`.**
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` clean.
- Nine defects and eight false comments have been found on this branch. Verify every claim
  you write against the code.

---

### Task 1: Measure the hull, and settle which number is true

Pure analysis. Touches no engine code and changes no behaviour — its output is a table and
a decision.

**Files:** `tools/mesh/jib_vi/sections.py` (new)

- [ ] **Step 1** Read the shipped `jib_vi_*.obj` material meshes and compute the **sectional
  area curve**: at each of N stations along X, the immersed cross-section area as a function
  of waterline height. Reuse `tools/mesh/rig/rigkit.py`.

- [ ] **Step 2 — settle the 2.70 m³ disagreement.** `SEA-REBUILD.md` §2.1 records that the
  material-set union integrates to 60.340 m³ below `y = 0` while `jib_vi_wide.obj` gives
  57.636, and that **60.34 is an upper bound rather than a measurement** because summing
  per-material shells assumes their union is closed and non-overlapping — `grime` and
  `hull_white` are plausible double-counts against `antifoul`.

  **Check watertightness and report which number is real**, with the evidence. `mass` will
  be set from it, so this is the task's most important output. If the material set is not
  closed, say so and use the hull that is.

- [ ] **Step 3** From the curve, report: displacement at the designed line; **the draft at
  which displacement equals the current 43,776 kg**; and the waterline at which the hull
  displaces its own painted line. Those three numbers are the diagnosis in full.

- [ ] **Step 4** Print the curve as a table and commit the generator with its output in the
  report. **Change nothing else** — the fix is Task 2, and separating them means the
  measurement can be argued with on its own.

---

### Task 2: One source of truth for mass, immersion and paint

**Files:** `assets/prefabs/jib_vi.loom`, `assets/test/jib_vi_*.loom` as needed,
`crates/loom_water/src/buoyancy.rs`, `crates/loom_scene/src/components.rs`

- [ ] **Step 1** Set `mass` from Task 1's measured displacement. Record the number and its
  provenance in the prefab, replacing the ratio arithmetic that caused this.

- [ ] **Step 2 — decide the immersion model, and argue it.** Two options, and the plan does
  not pre-judge:

  **(a) Keep spheres, re-solve them.** Radii and offsets derived from the section curve so
  the sphere set reproduces the hull's displacement *and* its waterplane area *and* its
  longitudinal distribution. Data-only; no engine change; `MAX_PONTOONS` is 16 and twelve
  are in use.

  **(b) `Buoyancy.stations`** — a generated section table, integrated exactly. More faithful,
  and it makes ADR 0063's wake source exact rather than an estimate, but it is new solver
  code on the force path.

  The sphere's virtue is documented and real: *"a sphere crossing the surface gains and loses
  displacement as it rises and falls, and that change is the restoring force"*, where a linear
  ramp oscillates forever. **A real hull section has that property naturally**, because a V or
  U section widens as it rises — so (b) keeps the nonlinearity on physical grounds rather than
  by imitation. Choose, and say why.

- [ ] **Step 3** Move the antifoul's top edge to the resulting waterline, or say why the mesh
  should not change and move the model instead. **The paint and the float must agree**;
  today they differ by 8.4 cm.

- [ ] **Step 4** Re-pin every hash this moves, in the same commit, deliberately.

---

### Task 3: Added mass and directional drag

The sea is now twenty feet. `damp_linear = 4.0` was tuned holding station on flat water and
is currently absorbing, by hand, the error of a hull that carries no added mass.

**Files:** `crates/loom_water/src/buoyancy.rs`, `crates/loom_scene/src/components.rs`

- [ ] **Step 1 — added mass.** A hull heaving drags water with it, of order 1× displacement
  vertically. Without it the heave response is simply too fast. Derive from the section
  table; do not author a coefficient.

- [ ] **Step 2 — directional drag.** A hull resists sideways motion far more than
  fore-and-aft. The section table already knows the lateral and frontal areas, so both
  coefficients are derived rather than chosen.

- [ ] **Step 3** Both are new terms on the force path. Every hash they move gets re-pinned
  deliberately, and **every existing floating scene must be checked** — a crate, a buoy and a
  barge all read this solver.

---

### Task 4: The four acceptance tests

Three are closed-form predictions rather than opinions, which is what makes them worth more
than a look at the render.

- [ ] **1.** The solver's displacement-vs-draft curve matches the mesh's own, computed
  independently by Task 1's generator.
- [ ] **2.** `|settled_y − boot_top| < 0.02 m` — the paint and the float agree.
- [ ] **3.** The heave natural period matches `T = 2π√((m + m_add)/(ρgA_w))` from the section
  table. **This is the test that would have caught the original defect**, and it is also the
  one the FFT ocean makes meaningful: a period is only observable in a sea that has one.
- [ ] **4.** A righting-arm sweep: `GZ` positive through the working range, and **no capsize
  across a long run in the twenty-foot sea** — the test `jib_vi_float.loom` was built to be
  and has only ever been asked on flat water at `Wind.speed = 3.5`.

---

## Self-Review

**Ordering.** Measure (1), fix the disagreement (2), add the terms the new sea demands (3),
then assert (4). Task 1 changes nothing, which is deliberate: its number decides Task 2 and
deserves to be arguable on its own.

**The risk.** Tasks 2 and 3 are both on the force path and both move hashes. Every floating
scene in the repository reads this solver, so "the boat is better" is not sufficient — a
crate in a river and a buoy in a pool must also still behave, and Task 3 Step 3 makes that
explicit rather than hoping.

**Deliberately out of scope:** the hull mesh's shape, the wake, and hull foam. ADR 0063
already reads the immersed section for the wake and will simply get a better number for free.
