# Water Rebuild — Review Panel Ruling

**Date:** 2026-08-18. **HEAD reviewed:** `e551328`. **Panel:** three independent judges, each
rendering from their own fresh release build and judging their own PNGs against reference images
59–65. This document is the chair's synthesis. It is a kick-back, not an approval.

---

## Ruling: PARTIAL — kicked back. Tally: 3–0 PARTIAL.

All three judges ruled PARTIAL independently. No judge ruled PASS; no judge ruled FAIL.

The shape of the verdict is the same in all three: **the cinematic tier's mechanism is real and
the ADR 0053 §5 constraints are honestly implemented** — images 61, 64 and 65 exist in kind for
the first time, the deterministic world is preserved bit-for-bit everywhere sampled, and the
tier reproduces byte-for-byte at its gate ticks. **But the presentation half is unfinished, two
of the five green checks are broken at HEAD, one acceptance claim in ADR 0057 is falsified by
direct measurement, and the flagship feature is invisible in the live viewer.** The quad-boundary
defect class this project has shipped twice before shipped a third time, found the same way the
human found the last two: by moving the camera.

Do not read this as approval-with-notes. The human's instruction was "if you can't replicate
these and have it work in the system, kick it back." Image 63 is not replicated at all, image
59's defining feature is absent everywhere, and "work in the system" fails on the project's own
definition of green. The architecture stays; the named defects go back.

---

## Per-image scorecard

| # | Promised | Actually there | Verdict |
|---|----------|----------------|---------|
| **59** | Many impacts on a pool: crown jets, concentric ring trains, foam | **Ring trains: yes** — dense, world-locked, concentric multi-band packets appearing and expiring across the whole `rain_pool` surface; rain streaks correctly stop at the water. **Crown jets: absent everywhere** — `pool.loom`'s Worthington jet is invisible at the authored camera across ticks 52–84 (swept at up to 1920×1200 with crops; a foam collar and sub-pixel specks, the jet occluded by the floating sphere — a builder-admitted defect), and rain-splash crowns are near-invisible at grazing angle (admitted `rainDepthFade` defect, deferred to "its own slice" that never came). **Foam flecking: not attempted** (admitted). New defect: rain rings clip into **rounded squares** — squircle outlines at the authored camera, the lattice cell boundary showing through. | **FAIL** on its defining feature (all 3 judges) |
| **60** | Stone-spout waterfall: coherent falling sheet with surface structure | Coherent comb-textured sheet, vertical string breakup in the lower third, mist wisps. But the sheet is a **hard-edged rectangle** — ruler-straight sides, and Judge 1 verified at 3× crop that the bottom edge is dead straight (the slice's "fades into the water rather than terminating in a straight line" is false at the authored camera). From `--yaw 40 --pitch 20` the pool is a hard-edged bright square patch on the infinite sea. Reads as a textured card, not the reference's ragged breaking sheet. | **PARTIAL** (3/3) |
| **61** | Niagara-FLIP volumetric liquid ribbon (brought into scope by ADR 0053) | **The mechanism exists.** A genuine volumetric liquid sheet leaves the weir, leans forward, visibly refracts the brick wall behind it, sheds droplets, lands in a churning pool, sustained by inflow; byte-identical across three fresh processes at `--sim 180` (verified by hand by all three judges). Fidelity is well below the reference — marching-tetrahedra terracing visible through the sheet, spray as flat uniform confetti dots. **And the scene fails Vulkan validation** (blocking defect 1 below), so green check 2 cannot pass at HEAD. | **PASS structurally, BLOCKED technically** (3/3 on structure; J2 found the validation failure) |
| **62** | Canyon waterfall: fall, mist mound, reflective pool | Coherent fall all the way down (correctly discriminated from `spout` by discharge), soft mist bank at the foot. **The reflective pool — the reference's defining feature — is absent**: the nappe is not in the TLAS and water reflects only the analytic sky, so nothing of the canyon or the fall appears in the water. The sheet is again a hard rectangle; off-axis (`--yaw 140`) the pool is nested hard-edged rectangles with a bare untextured grey triangle beside it on the sea, plus dense white speckle over the far field. | **FAIL** on its defining feature (3/3) |
| **63** | Interactive dripping water: drips forming, falling, splashing | Only "falling." `dripping.loom --sim 180` at 960×600 is a near-black frame with ~8–9 tiny smeared streaks mid-air. No visible drop formation at the sources, landing zones show 2–3 single-pixel specks with no readable crown, no wet marks, no accumulation (all admitted unbuilt). The physics underneath is real — tests pin Tate's law and fall time — but it does not read as the reference at any camera. | **FAIL** (2/3 outright FAIL, 1 PARTIAL) |
| **64** | Body ploughing a pool: violent foam and spray, advected whitewater | **The hollow is real** — `plough_cinematic --sim 110` shows a bow wave curling off the crate, airborne droplets, and an open dry trough behind the hull with the tiled bed refracted through the water: the shot ADR 0053 was bought for, which could not exist before it, byte-identical across fresh processes at the gate tick. But the whitewater is a smooth opaque meringue mesh and the spray is confetti dots — nothing like the reference's granular churned advected texture. Deterministic `plough --sim 240` reads as a flat static white ice-floe raft, not a furrow. | **PASS in kind, quality kicked back** (3/3) |
| **65** | Bounded 3D liquid with an obstacle: sloshing, real surface | **Closest of the seven.** `slosh --sim 30`: flat continuous marched lid refracting the pillar. `--sim 150`: genuine heaping against the pillar, drained trough behind it, the ball half-out and coupled, torn spray; no leaks or domain-boundary artifacts from any yaw; byte-identical across three fresh processes at 150. **But past the gate tick it fails**: at `--sim 600` — a run ADR 0057's addendum explicitly claims byte-identical — nine renders across two judges, including fully quiet serial runs on an idle GPU, gave **nine distinct hashes** (adjacent pairs 5–10% of pixels, worst channel 94–154, marched triangle counts 20496/20530/20626, divergence bbox exactly the water surface) at ~350 ms/tick against the claimed 8.4. From behind, the surface reads through the tank's scenery wall. | **PASS at the gate tick, FAIL beyond it** (3/3) |

---

## Blocking defects, in priority order

Each entry: symptom, file, reproduction. Ordered by (a) breaks a green check, (b) falsifies a
recorded claim, (c) misses a reference's defining feature, (d) quality/process.

### 1. `ribbon.loom` fails Vulkan validation — green check 2 is broken at HEAD
- **Symptom:** debug render aborts with `VUID-vkCmdFillBuffer-size-00027` on `loom.fluid.vel_u`
  (fill 128448 > buffer 128440). In release the same fill is undefined behaviour.
- **Root cause (identified):** `crates/loom_render/src/fluid.rs` (~line 631) records
  `allocation.size()` — gpu-allocator's **padded** size — and the `fluid_zero` pass fills that
  span instead of the requested buffer size. Overruns whenever the allocator pads; 8 bytes over
  on ribbon's grid dims.
- **Repro:** debug build, `loom render assets/test/ribbon.loom --sim 180`. `cargo xtask validate`
  iterates all 63 `SCENES` in debug and ribbon is in `SCENES`, so check 2 cannot pass.
- **Fix:** one line — record the requested size, not the allocation's.

### 2. `rain_pool --sim 300` (a GOLDEN row) is not byte-reproducible — green check 5 is broken
- **Symptom:** three fresh processes of the same unmodified release binary give up to three
  different sha256s (J1: 3/3 different at 320×200; J2: 2-of-3 identical, ~11 px, worst channel 9
  — sub-tolerance for check 4 but byte-different, which is exactly the property check 5 exists
  to test; J3 reproduced the same pattern). The stateful rain drop buffer predates the cinematic
  tier.
- **History:** reported by slices 4, 5, 6 and 8. Fixed by nobody. It has now been left broken
  across four slices on a gate row.
- **Repro:** render `rain_pool.loom --sim 300` with gate args from three fresh processes; compare
  sha256.
- **Required:** root cause, not tolerance. Check 5 is byte-for-byte by design (ADR 0045); this is
  the licence for every GPU-stateful path in the engine.

### 3. `slosh --sim 600` is nondeterministic and pathological — ADR 0057's addendum is falsified
- **Symptom:** nine renders across two judges — every contention condition including fully quiet
  serial runs on a verified-idle GPU — produced **nine distinct hashes**. Divergence is entirely
  in the water (5–10% of pixels, worst channel 94–154; the marched surface itself differs:
  20496 vs 20530 vs 20626 triangles). Cost is ~211 s for 600 ticks (~350 ms/tick) against the
  slice's claimed 8.4 ms/tick. Separately, the per-tick fence wait is fragile under host CPU
  load: the same `slosh --sim 150` measured 13.5 ms/tick standalone and 222.6 ms/tick while
  clippy loaded the CPU.
- **Mechanism (in the code's own doc comment):** `FLUID_SORT_MAX = 4096` with
  `min(cellCount, 4096)` — past the ceiling the bucket tail keeps scheduler arrival order and
  the gather sums floats in it, **defeating the float-atomic ban by another route**. The unfixed
  volume-compression instability (ADR 0057 failure 3) is what drives buckets past the ceiling,
  and it also presents as the ~40× cost blowup.
- **Repro:** `loom render assets/test/slosh.loom --sim 600` three times from fresh processes on
  an idle GPU; compare hashes; time it.
- **Required:** the addendum's "slosh 600: identical" table row and the `FLUID_SORT_MAX` doc
  comment claiming byte-identity at 600 must come out — they are false on this hardware today —
  and the instability itself needs the ADR's own named next step (ghost-fluid free-surface
  pressure + a real radix sort) or an explicit, documented tick ceiling on the tier's
  reproducibility guarantee.

### 4. `pool_jet`'s GOLDEN row (`pool.loom --sim 70`) contains no visible jet
- **Symptom:** at the authored camera the Worthington jet is occluded by the floating sphere /
  below visibility — verified at up to 1920×1200 with crops across a tick sweep 52–84. The row
  would bless a picture without its subject: the exact "reference outside the feature's window"
  failure this project already documented with grass, about to be re-committed on a new row.
- **Repro:** `loom render assets/test/pool.loom --sim 70`, zoom the impact point.
- **Required:** either move the cavity/jet timing or the camera so the jet is unambiguously in
  frame at the gate tick, or the row does not gate the feature.

### 5. `loom run` draws no cinematic water at all
- **Symptom:** `crates/loom_cli/src/run.rs` contains zero fluid references (code-confirmed by
  two judges; builder-admitted in slice 8). Only headless render and flythrough march the
  surface. Play mode steps the solver but draws no water.
- **Consequence:** the human's stated workflow — finding defects by moving a camera in the live
  viewer — cannot see the flagship feature the entire overhaul was approved for. "Have it work
  in the system" is not met for the tier's own scenes.

### 6. Image 63 is not replicated
- **Symptom:** `dripping.loom --sim 180` is a near-empty dark frame; no drip formation at the
  lips, no readable landing splash, no wet marks, no accumulation.
- **Required:** the wet mark ADR 0054 §8 already specifies; visible drop formation at the
  source; a landing splash that reads at the authored camera; a scene lit brightly enough to
  read at all.

### 7. Hard analytic boundaries — the quad-boundary defect class, third shipping
- **Symptoms, all found by moving the camera off the gate framing:**
  - `spout` and `cascade` sheets are perfect rectangles: dead-straight sides, straight bottom
    edge (spout, verified at 3× crop), straight top.
  - `rain_pool` rings clip into rounded squares (squircle outlines at the authored camera,
    verified at 6× crop) — the ring lattice's cell boundary.
  - The foam field is a blocky hard-edged rectangle on `spout`'s open sea (`--yaw 80`).
  - `cascade`'s pool off-axis is nested hard-edged rectangles plus a bare untextured grey
    triangle on the sea (`--yaw 140 --pitch 18`).
  - `slosh`'s marched surface reads through the tank's scenery wall from behind.
- **Repro:** render each scene at the yaws above. No gate looks at any of these framings, which
  is precisely why the class keeps shipping.

### 8. `cascade`'s pool reflects neither the canyon nor the fall
- **Symptom:** water reflects the analytic sky only; the nappe is vertex-generated and not in
  the TLAS (stated in ADR 0054). Image 62's defining feature. The standing rule is "anything
  that wants to be reflected has to become an `Object`" — the fix path is known (proxy geometry
  in the TLAS, or an explicit ADR accepting the miss), it just was not taken.

### 9. Whitewater reads as solid geometry
- **Symptom:** `plough_cinematic`'s bow mass is a smooth solid-white meringue mesh; cinematic
  spray is flat uniform light-blue confetti; deterministic `plough`'s foam raft is an opaque
  white pancake/ice floe. The 0.5 m foam-field cell cannot express image 64's granular advected
  foam at these scene scales. Image 59's foam flecking was never attempted.

### 10. Process debt the verifier and the human must clear before merge
- ADRs **0054, 0055, 0056, 0057 are all status `proposed`** and unapproved. The force path is
  the one thing this project requires human approval for by name.
- **~9–28 new/moved references are unblessed** (pool, water_crate, wake, splash, spindrift,
  river, homestead, lanternhead, squall, plough, pool_jet, rain_pool, spout, cascade, dripping,
  slosh, ribbon, plough_cinematic, …) — check 4 fails by construction until the verifier
  blesses, deliberately, reading the MANIFEST diff.
- **Nobody has run `cargo xtask validate` or `flythrough`** over the seventeen new fluid
  buffers plus `loom.rain.water` / `loom.wavelets` / `loom.foam` (the panel was barred from the
  xtask gates; defect 1 guarantees validate fails at least once).
- Slice 6's **intermittent `Renderer::new ERROR_UNKNOWN`** under concurrent test binaries is
  unattributed; it did not fire in any judge's runs (three full suite runs total), but nobody
  has root-caused it.
- Judge-measured costs disagree with slice reports beyond the slosh case: ribbon@180 measured
  107 ms/tick against a reported 52.

---

## What the judges verified themselves vs. took on trust

**Verified first-hand (all findings above rest on this):**
- **Own renders.** Every water scene rendered from a fresh release build of HEAD `e551328`
  (one judge rebuilt after touching shaders to defeat the slice-6 stale-SPIR-V hazard), at
  authored cameras plus crops, orbit yaws, and tick sweeps, at 960×600 up to 1920×1200. Every
  pass/fail in the scorecard is from a judge's own PNG, opened, not from a builder's report.
- **Regression.** Three independent byte-checks of untouched GOLDEN rows: 27/27 (J2, against
  `tests/references/MANIFEST.txt` at exact gate args), 12/12 (J3), and 8/8 non-water rows in a
  full A/B against a from-source build of pre-slice commit `78cd934` (J1). **ADR 0053's
  bit-identical acceptance criterion holds on every scene sampled**, including the GPU particle
  pool and the pre-existing rain scenes. The four moved water rows (lanternhead, mirrorpool,
  ocean, whitecaps) are all declared expected movers.
- **Reproducibility, by hand** (the xtask gates belong to the verifier): three fresh processes
  per scene, sha256-compared. `slosh@150`, `ribbon@180`, `plough_cinematic@110`: byte-identical,
  every judge who checked. `rain_pool@300` and `slosh@600`: divergent, every judge who checked.
- **ADR 0053 §5, in the code** (`assets/shaders/fluid_sim.slang`,
  `crates/loom_render/src/fluid.rs`, `crates/loom_cli/src/play.rs`,
  `crates/loom_scene/src/scene.rs`): every solver-path atomic is integer (u32 counts/cursors, a
  fixed-point u32 density splat at `FLUID_DENSITY_SCALE`); P2G is a gather over a counting-sorted
  bucket list with in-bucket re-sort; pressure is a multigrid V-cycle, no PCG, no float
  reduction; particles carry APIC affine `c0/c1/c2`, no PIC/FLIP knob; readback is a fence wait
  inside the fixed step crossing as plain f32; the domain anchors to the water node's transform,
  never the camera; `ash` is confined to `loom_render*` (Cargo.toml grep); all four load-time
  refusals exist with tests, and `cinematic_water_is_not_assertable` was fired live and named
  ADR 0053 §2 on exit 1. **§5 is genuinely obeyed** — with the one caveat that the
  `FLUID_SORT_MAX` overflow path (defect 3) re-admits scheduler-order float summation through
  the back door.
- **Judges' own gates:** `cargo clippy --workspace -- -D warnings` clean; `cargo test
  --workspace` fully passing in three independent runs (release and debug).
- **Validation layers on the cinematic scenes** (J2, debug binary): slosh and plough_cinematic
  clean; ribbon aborts (defect 1).

**Taken on trust / not established:**
- The **xtask gates themselves** (validate, image, repeat, flythrough) were not run — barred by
  the rules of engagement; the verifier owns them. Every claim about them here is an inference
  from hand-equivalents.
- **Builder-admitted defects** (jet occlusion, `rainDepthFade`, no wet marks, no foam, viewer
  gap) were taken at their word only where a judge's own render independently showed the same
  thing — which was every case checked.
- The **intermittent `Renderer::new ERROR_UNKNOWN`** flake: absence of evidence only (did not
  fire in three suite runs), not attribution.
- **Full GOLDEN coverage:** the regression byte-checks sampled 8–27 rows each with heavy
  overlap; the union is near-total but no single judge checked all rows.

---

## What has to change for this to pass

In order. Items 1–2 are single-defect fixes; 3 is the hard one; the rest are scoped. Written so
the next builder starts here without re-deriving anything.

1. **Fix the fill-size bug** (defect 1): `fluid.rs` ~631, record the requested buffer size
   instead of `allocation.size()`. Then debug-render every fluid scene; ribbon must run clean.
2. **Root-cause `rain_pool`'s cross-process divergence** (defect 2). It predates the tier, it is
   ~11 px, and it has survived four slices of being reported. Suspects the slices never
   eliminated: seed path, splash indirect count, first-dispatch catch-up. Check 5 must pass on
   the row byte-for-byte, not within tolerance.
3. **Make the tier's reproducibility claim true, or shrink it until it is** (defect 3). Either
   build ADR 0057's named next step — ghost-fluid free-surface pressure so the volume stops
   compressing, plus a real radix sort so no bucket ever exceeds capacity — or delete the false
   claims (ADR 0057 addendum table row, `FLUID_SORT_MAX` doc comment) and record the honest
   ceiling ("byte-identical through tick N; past N unstable and unreproducible") in the ADR,
   with a load-time refusal or documented budget for scenes that exceed it. A false
   determinism claim in an ADR is worse than a narrow true one.
4. **Put the jet in frame** (defect 4): retime the cavity collapse or move `pool`'s authored
   camera; the `pool_jet` reference must visibly contain a jet before it is blessed. While
   there: make rain crowns readable at grazing angle (the deferred `rainDepthFade` slice) —
   image 59 has no crowns anywhere until one of these lands.
5. **Draw the tier in `loom run`** (defect 5): the viewer already steps the solver in play mode;
   the surface march and the spray draw need wiring into the windowed path, same as headless.
   The human must be able to orbit `slosh` live.
6. **Make `dripping` read** (defect 6): source-side drop formation (the physics already knows
   the timing), a visible landing splash, wet marks per ADR 0054 §8, and enough light to see.
7. **Kill the straight edges** (defect 7): jitter/noise the nappe's side and bottom boundaries
   (the sheets need ragged silhouettes, not rectangles); clip rain rings by ring radius rather
   than lattice cell; fade the foam field's domain edge; bound or fade the cascade pool patches;
   depth-test the marched surface against scenery. Then orbit every water scene before calling
   it done — that camera move is where all three shipped instances of this class were found.
8. **Reflect the falls** (defect 8): proxy geometry for the nappe in the TLAS (it is allowed to
   be crude — a reflection wants the shape, not the comb texture), or an ADR explicitly
   accepting analytic-sky-only for image 62 and saying why.
9. **Texture the whitewater** (defect 9): break the foam isosurface up — advected density
   modulating alpha/normal at sub-cell scale, or spray particles granular instead of uniform
   confetti. Image 64's bar is churned texture, not more foam volume.
10. **Clear the process debt** (defect 10): human approval on ADRs 0054–0057; verifier blesses
    the moved/new references deliberately, reading the MANIFEST diff; full `validate` +
    `flythrough` over the new buffers; attribute or bound the `ERROR_UNKNOWN` flake; re-measure
    and re-record the per-tick costs that were reported from a different machine state than
    they reproduce on.

---

*Chair's note. The panel was unanimous, and unanimous on both halves: the mechanism deserves to
live, and the work is not done. The §5 constraints — the integer-atomic ban, the multigrid, the
APIC transfer, the sim-anchored domain, the synchronous readback, the tier fence — survived
contact with implementation, which is the part of ADR 0053 that could not be recovered if it had
been fumbled. Everything else on this list is ordinary finishing work with a known address. Ship
the fixes; do not re-litigate the architecture.*
