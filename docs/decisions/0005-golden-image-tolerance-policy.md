# ADR 0005 — Golden-image tolerance policy

- **Date:** 2026-08-11
- **Status:** accepted
- **Decision touched:** new (test policy); implements LOOM-IMPLEMENTATION-ORDER.md S1

## Context

Until S1, `CLAUDE.md`'s definition of green had three checks and a hole. Clippy caught what the
compiler did not, the validation layers caught what clippy did not, and determinism hashes caught a
simulation that drifted. Nothing caught a shader that renders everything slightly wrong. Every
render in this project was verified by a human opening the PNG and looking at it.

That scales to one change at a time. It does not scale to Phases 1–6, which are the four most
visually regression-prone systems the engine will ever have — wind, grass, water, rain — where a
numeric change to a shader is otherwise unverifiable.

A pixel diff needs a tolerance, and the tolerance is the whole decision. Too tight and a driver
update fails the build, which teaches people to bless without reading and converts the gate into a
ritual. Too loose and it reports success while a real change sails through, which is worse: a gate
nobody trusts gets fixed, a gate that lies does not.

## Decision

**Two thresholds, because there are two shapes of regression.** An image fails if *either* enough
pixels differ *or* any single pixel differs enormously. Neither rule alone catches both shapes:

- `fraction: 0.001` — a tenth of a percent of pixels. Catches a wide, shallow change: a lighting
  term that moved slightly across the whole frame.
- `worst: 72` — one pixel differing by more than this fails the image on its own, however few
  there are. Catches a small, blatant artifact: one wrong light, a NaN pixel, a single blown
  highlight. A fraction rule alone rounds these away.
- `channel: 2` — the per-channel delta below which a pixel counts as unchanged at all.

**`channel: 2` is measured, not chosen.** Two independent renders of the same scene on this machine
are bit-identical — worst channel delta zero — so tolerance buys nothing but survival across a
driver update, which shifts a count or two.

The first value was 8, picked because it sounded safely small, and it was useless. Multiplying the
shader's diffuse term by 0.96 — a real one-line change of the exact kind this gate exists to catch —
moves 17% of the pixels in `materials.loom`, and its worst single-channel delta is **4**. At a
threshold of 8 that change was invisible and the harness cheerfully reported everything fine.

The calibration is pinned as a unit test (`a_four_count_shift_across_the_image_is_a_regression`) so
that loosening `channel` past the magnitude a real shader change produces fails the suite rather
than quietly disarming the gate.

**Alpha is compared.** An image that lost its transparency is a regression, and comparing only the
colour channels calls it identical.

**A missing scene fails the gate.** It originally skipped with a warning, which meant renaming a
scene file dropped a whole rendering path out of coverage while the gate still printed success.
Verified by hiding `assets/test/materials.loom`: the gate now exits 1 and names it.

**No Vulkan device skips, honestly.** A machine that cannot answer the question says so and returns
success. A green tick nobody earned is worse than a stated gap, and CI without a GPU is normal.

**Six scenes, chosen for coverage of rendering paths** rather than of content: mesh, bindless
textures, voxels, alpha particles, additive particles, and an authored-dark environment. Adding a
seventh is only worth it for a path none of these exercise.

**320×200.** Large enough that a real change moves far more than the fraction limit, small enough
that six references are ~150 KB total and rendering them all is seconds.

## Deviation from S1 as written

S1 asks for references "as content-hashed artifacts, not in-tree binaries bloating history." The
references are committed in-tree, with `tests/references/MANIFEST.txt` recording each one's SHA-256.

The concern behind the instruction is real but does not apply at this size. Six PNGs at 320×200 are
about 150 KB; an artifact store, its fetch step, and its cache would be more machinery than the
thing it stores, and it would put the gate behind a network dependency for a single-developer
project. The manifest recovers the reviewability that content-hashing was wanted for: a re-blessing
shows up in review as six readable text lines rather than an opaque binary diff, and a changed PNG
*without* a changed manifest line is visibly a mistake.

Revisit if the reference set grows past roughly a hundred images or moves to a resolution where
history size is felt.

## Blessing

`cargo xtask image --bless` is the only way a reference is created or changed, so the intent is
always explicit, and it rewrites the manifest in the same step. A new scene with no reference is
neither a pass nor a failure to be ignored: it fails, naming `--bless` as the fix.

## The fly-through is not a gate

`cargo xtask flythrough` dumps sixteen orbiting frames per scene, advancing the simulation between
them, and asserts nothing. That is deliberate and it is not laziness about the harder half.

The artifacts it exists to surface — shimmer, LOD popping, density popping, unison sway, swimming
vegetation, a wind direction that snaps instead of turning — are recognised by a person watching
motion and cannot be described by a threshold. A still PNG cannot see any of them, and they are the
dominant failure mode of every system queued in Phases 1–6, grass above all. What the task buys is
making the looking cheap: one command, numbered frames, flick through them.

The implementation order calls this "the part that matters most and the part most likely to be
skipped," which is why it is a task with a name rather than a set of flags to remember.

## Consequences

- `CLAUDE.md`'s definition of green is four checks, and `scripts/green.sh` runs all four.
- Every future shader change must either preserve the references or re-bless them deliberately.
  This is the intended friction.
- A driver update will re-bless all six at once. That is a legible commit, and the manifest shows
  it was all six rather than a selective edit.
- The gate cannot catch what it does not render. Adding a rendering path without adding a golden
  scene leaves it unverified, and nothing will complain.

## Human approval

Not required — this adds a test gate rather than changing a locked decision. The tolerance numbers
are the reviewable part and are recorded above with the measurement that produced them.
