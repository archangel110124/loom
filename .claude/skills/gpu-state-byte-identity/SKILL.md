---
name: gpu-state-byte-identity
description: Use when cargo xtask repeat reports DIFFER, when two renders of the same scene and tick differ, or before writing a compute pass that holds state across ticks, uses an atomic, sorts, or allocates slots. Covers the arrival-order failure class and how to verify a reproducibility claim.
---

# GPU state that renders the same twice

`cargo xtask repeat` renders every `GOLDEN` scene in three fresh processes and
compares the PNGs **byte for byte, no tolerance**. That property is the entire
licence for GPU-stateful rendering in this engine (ADR 0045 clause 3). It is
the one thing `cargo xtask image` structurally cannot ask: `rain_pool --sim 300`
diverged by ~11 px with worst channel 9 — comfortably *inside* the image
tolerance — while being a different picture every run.

Three shipped bugs, one shape:

- `d4b30b8` — `slosh --sim 600` gave **nine distinct hashes in nine renders on
  an idle GPU**. `fluidSortCellMain` sorted `min(occupancy, FLUID_SORT_MAX)`
  while the P2G gather summed the *whole* bucket, so the tail past the ceiling
  kept the order `InterlockedAdd` handed out and floats were summed in it.
  The float-atomic ban was defeated **with no atomic at the point of failure**.
  The ceiling had already gone 128 → 1024 → 4096, a scene reaching each
  (`2dd3176` is the second time). The fix removed the ceiling, it did not raise
  it.
- `555f4c0` — `rain_pool --sim 300`, a GOLDEN row in the deterministic tier,
  rendered four different PNGs in five processes. A 16,384-entry splash ring
  filled by `InterlockedAdd` lapped, because "550 drops per tick" was a property
  of one scene's geometry. **Reported by slices 4, 5, 6 and 8, and fixed by
  none of them.**
- Found alongside: two device-local buffers read through a device address and
  never declared to the render graph, and fresh device memory read before being
  written.

## The rule

**Anywhere arrival order can select a slot, a seed, or a summation order is
nondeterminism — whether or not there is an atomic in the failing kernel.**

Atomics are admissible only for a count that nothing in the same dispatch reads
(`rainSplashArgsMain` is the legitimate case: it produces an indirect draw
count).

**Any capacity constant is a correctness constant.** A ceiling, ring size or
bucket cap whose bound was derived from one scene's numbers is a latent nine-
hash bug. Name the scene that would exceed it, or remove the ceiling.

## Write it this way instead

- **The slot is a function of the ordinal, not of arrival.** ADR 0047's
  particle pool: slot `i` holds the largest birth ordinal `n < births_by(tick)`
  with `n ≡ i (mod N)` — arithmetic, no free list, no atomic on the seed path.
  `555f4c0` fixed the splash ring the same way: the slot is the drop's own
  index. Round-robin recycling beats a free list, because a drain needs
  compaction needs an atomic append.
- **Seeding is a pure function of (scene, index).** Never a running counter.
- **Catch-up to `--sim N` is a fixed function of N alone** — the same sequence
  of steps regardless of frames drawn, wall clock or camera position (ADR 0053
  §6, which reworded ADR 0045's "one dispatch" because shipped code already
  contradicted it).
- **Zero every device-local buffer in the first submit**, and declare every
  buffer the graph's passes touch, including ones reached through a device
  address. A barrier that arrives as a side effect of a neighbouring buffer is
  a latent bug (never-do #4 covers buffers).
- Comparison sorts with distinct keys have one answer, so swapping insertion
  for Shell sort kept every previously-passing bucket bit-for-bit.

## Verifying a claim

**Three fresh processes, at the row's exact gate arguments — and well past the
gate tick.**

```bash
bash tools/repeatcheck.sh slosh assets/test/slosh.loom --sim 600
```

(Three renders at `320x200`, md5-compared, without the cross-worktree gate
lock. Builders may run this; they may not run `cargo xtask repeat`.)

`slosh` was byte-identical at tick 150 and nine-way divergent at 600, and
**ADR 0057's claim of the latter was written from the former.** Test past where
the gate looks.

## Diagnosing a DIFFER

In order:

1. **Bbox the divergent region.** `loom compare a.png b.png --rect x,y,w,h`.
   Are the moving pixels the new feature's?
2. Does any output depend on **which thread got there first** rather than on an
   ordinal?
3. Does any float reduction happen in a container whose order is
   scheduler-decided — a sort with a ceiling, a bucket tail, an atomic append?
4. Is any buffer written past its requested size? (see
   `loom-vulkan-resource-safety`)
5. Is seeding a pure function of (scene, tick)?

Then **bisect the mechanism rather than guessing.** `555f4c0` proved the ring
by setting `SPLASH_STEPS = 1` (identical), then a 16× ring (identical), then
the original (not) — and the 16× run doubled as proof the fix lost nothing,
because it rendered the same hash.

**The fix is always a root cause, never a threshold.**

## Honesty clause

If you cannot make the claim true, **shrink it rather than falsify it**:
"byte-identical through tick N, unstable past it" in the ADR, with a load-time
refusal or a documented budget for scenes that exceed N. *A false determinism
claim in an ADR is worse than a narrow true one* (`WATER-REBUILD-REVIEW.md`,
required change 3). Strike the false claim through in place with a pointer
forward — do not delete it (ADR 0057's addendum table).

## Scope

Three runs on one RTX 4090 says nothing about other hardware
(`OVERNIGHT-DECISIONS.md` D9). Cinematic-tier scenes are reproducible on this
device alone and are barred from `DETERMINISM_SCENES` and the pinned-hash tests
(ADR 0053 §3) — see the `determinism-tier-routing` skill.
