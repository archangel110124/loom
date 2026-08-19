---
name: loom-gate-a-rendering-path
description: Use when a rendering feature has landed and needs a golden-image row, when choosing the scene/camera/--sim for a reference, or when references have moved and someone is about to bless them. Covers proving a row can actually fail, and why blessing is the verifier's act.
---

# Gating a rendering path

CLAUDE.md gives the rule — *adding a rendering path means adding a scene to
`GOLDEN`*. This is the procedure, and **the procedure is where every failure
happened.**

The dominant failure here is not a false alarm. It is a **row that photographs
a frame the feature is not in**, which then reports a pass forever:

- `meadow` was outside `GOLDEN` for two grass slices, `grass_slope` for one.
  The gate reported full passes without rendering a single blade.
- `pool_jet`'s proposed row (`pool.loom --sim 70`) contains **no visible jet** —
  it is inside the floating sphere at that tick (`WATER-REBUILD-REVIEW.md`
  defect 4).
- The soot rim was wrong **twice**, and every `plume*` row was blessed on the
  broken picture both times. The human found it by opening the viewer
  (`xtask/src/main.rs`, the `plume_close` comment).
- A whole night of AA numbers was taken on a frame with no grass in it
  (CLAUDE.md, P2 slice 7).

## Do this

1. **Decide which list the scene belongs on.** Both lists are in
   `xtask/src/main.rs`: `SCENES` (loads, bakes, validates, ~64 entries) and
   `GOLDEN` (renders and pixel-compares, 50 entries).

   A scene earns a `GOLDEN` row only if it is the **only** picture of some
   rendering path. Otherwise `SCENES` only. Otherwise neither — and if it is
   neither, say so out loud, because nothing whatsoever then checks that file.

2. **Choose `--sim` by diff sweep, not by taste.** Render the scene with the
   feature removed and with it present, at several ticks, and take the tick
   where the difference is largest. The `wake` row is the worked example, and
   its reasoning is in its comment: 2.8% of pixels at tick 45, **26.6% at 200**,
   4.8% at 900 — so the row says `--sim 200`.

   Corollary and standing trap: **a scene whose subject is a particle, plume,
   splash or fluid is empty at tick 0.** Read the row's `--sim` before you
   render anything or claim anything about the picture.

3. **Render at the reference size and open the PNG.**

   ```bash
   ./target/release/loom render <scene> --out /tmp/g.png --size 320x200 --sim <n>
   ```

   `GOLDEN_SIZE` is `320x200`. At that size a feature can be a few dozen pixels
   — crop and zoom rather than squint. *Is the subject visibly in this frame?*
   If not, stop; you are about to bless a reference without its subject.

4. **Prove the row can fail.** Stub the feature (return early, multiply by
   zero, revert the constant), render again, and measure:

   ```bash
   ./target/release/loom compare /tmp/with.png /tmp/without.png
   ```

   Record the fraction that moves. The `SMOKE_RIM` row does exactly this
   (plume 3.70% / plume_gale 1.76% / plume_roof 6.53% / **plume_close 15.21%**
   — which is why `plume_close` exists); the puddles row moves 0.8% against a
   0.1% tolerance. **A row that does not move when the feature is deleted is
   not a gate.**

5. **Register it, and check the other lists.** Add to `SCENES` as well as
   `GOLDEN`. Ask whether it belongs in the `--play` list (per-frame CPU budget).
   A cinematic-tier scene may **never** enter `DETERMINISM_SCENES` — `xtask`
   refuses it by name, citing ADR 0053 §3.

6. **Write the comment in the house style.** `xtask/src/main.rs` is ~2,000
   lines of mostly rationale, and that is the gate's real documentation. The
   comment says: which rendering path this is the only picture of, and what
   would keep matching if the feature were deleted.

7. **Then look off the gate framing.** A gate is a fixed camera.

   ```bash
   ./target/release/loom render <scene> --out /tmp/o.png --yaw 40 --pitch 15 --sim <n>
   ```

   All three shipped instances of the hard-analytic-boundary defect class were
   found by moving the camera, never by a gate (`WATER-REBUILD-REVIEW.md`
   defect 7). For motion, use the `watch-loom` skill.

## Blessing

**Blessing is the verifier's act, not the builder's.** A builder does not
bless its own references (`WATER-REBUILD-REVIEW.md` defect 10 filed it as
process debt).

- **`--bless` copies without comparing.** In `xtask/src/main.rs` it is a bare
  `std::fs::copy` of rendered over reference, per scene, with no diff printed
  and **no per-scene filter** — one invocation overwrites all 50 references.
  So run `cargo xtask image` *first*, read every failing diff, and only then
  bless.
- Record the expected movers in the commit message, the way this repo already
  does: *"References expected to move: spout, cascade… everything else
  byte-identical, hand-checked by sha256 at gate args"* (`98ab3be`).
- Blessing something **uglier** is legitimate when the change is correct and
  you say why — `beach` went paler because a double-applied caustic web was
  fixed (`OVERNIGHT-DECISIONS.md` D10). Blessing to make a failure go away, or
  blessing a picture whose subject is absent, is the prohibited use.
- `tests/references/MANIFEST.txt` records each reference's hash. A changed PNG
  with no MANIFEST line is a mistake.
- **After a bless, every AA/flicker/shimmer number taken before it is void.**
  Flicker is not invariant to brightness (ADR 0010).

## Hand-equivalents, when you must not run the gate

Builders do not run `cargo xtask` gates (`OVERNIGHT-DECISIONS.md` D4 — three
queued `validate` runs cost ~30 minutes of wall clock, and the gate is a
cross-worktree singleton). These do the same two commands without the lock:

```bash
bash tools/goldcheck.sh <name> <scene> --sim 200   # render + compare at gate args
bash tools/bytecheck.sh <name>                     # md5 that render against the reference
```

## Notes

- Tolerance lives in ADR 0005 (`fraction 0.001`, `worst 72`, `channel 2`, size
  320x200). `channel: 2` is pinned by a unit test — loosening it fails the
  suite rather than quietly disarming the gate. Do not copy those numbers into
  new code; reference the ADR.
- **A no-GPU machine skips honestly and prints success.** "Green" from a box
  without a device proves nothing.
- A missing scene file fails the gate by design.
