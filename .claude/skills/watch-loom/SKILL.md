---
name: watch-loom
description: Use when asked to check how a Loom feature looks or behaves in motion, to verify a visual or physics change, or to debug water, rain, wind, grass, clouds or scatter. Captures a deterministic frame sequence and reads it as one labelled contact sheet plus a per-frame CSV.
---

# Watching Loom in motion

A still cannot show a phase error, a sign flip, a lag, a jitter or a
discontinuity — and those are almost every bug in water, rain, wind, grass and
scatter. This captures a sequence and reads it as **one image**, which costs
about the same in vision tokens as a single screenshot however many frames are
on it.

## Do this

1. **Capture.**

   ```bash
   tools/watch.sh <scene>          # default: homestead
   ```

   Scenes are named without a path: `homestead`, `meadow`, `ocean`, `river`,
   `squall`, `puddles`, `forest`, `rain_impact`, `proving_ground`. A path works
   too.

2. **Read `target/agent/contact_sheet.png`.** It is a grid, read **row-major** —
   left to right, then top to bottom. The yellow number burned into each cell's
   top-left corner is the frame index; trust it rather than counting cells.

3. **Read `target/agent/telemetry.csv`.** One row per frame, `frame` matching
   the burned-in number. Columns depend on which systems the scene has.

4. **Report by frame index, citing both signals.** Not "the rain looks wrong"
   but:

   > `rain_rate` goes to 0 at frame 12 (CSV) and the streaks disappear in cells
   > 12–15 (sheet), while `rain_exposure` stays at 1.0 — so the rate is being
   > zeroed by something other than shelter.

   Two signals agreeing is a finding. One signal alone is a hypothesis.

5. **When verifying a fix**, re-run and compare against the previous sheet and
   CSV. Placement and simulation are deterministic, so anything that changed,
   changed because of the edit.

## Tuning

| Want | Do |
| --- | --- |
| More detail per cell | `FRAMES=9 GRID=3x3 CELL_W=480 tools/watch.sh <scene>` |
| Slower motion sampled | `STEP=40 tools/watch.sh <scene>` (ticks between frames, 60 = 1 s) |
| Settle first | `WARMUP=600 tools/watch.sh <scene>` (ticks before frame 0) |
| Bigger frames | `SIZE=1280x720 tools/watch.sh <scene>` |

`STEP` is the important one. At the default of 6 ticks the twenty frames span
two seconds — right for rain and wind, far too short for a tide or a long
drying curve.

## What the sheet is and is not good for

**Reliable:** gross motion, direction, phase, whether something moves at all,
discontinuities between frames, large artifacts, "it stops at frame 12".

**Unreliable:** individual particles, thin geometry, sub-pixel shimmer, small
text, fine colour differences. The cells are ~320 px wide. If the question is
about detail, drop to `FRAMES=9 GRID=3x3 CELL_W=480`, and if it is about
sub-pixel stability use `loom flicker a.png b.png c.png` instead — the sheet
cannot answer that and will look fine while the artifact is there.

## Notes

- **Deterministic by construction.** Loom's timestep is fixed and its noise is
  position-hashed, so two captures of the same scene at the same `STEP` and
  `WARMUP` are byte-identical. There is no seed flag because there is no global
  RNG to seed; `cargo xtask validate` asserts the simulation hash matches
  between debug and release builds.
- **Headless.** The capture renders offscreen with no window or swapchain, so
  it works over SSH.
- `scatter_placement_hash` in the CSV is the determinism check for scattered
  instances: it must not change between runs of the same scene.
- There is no `rain_active_particles` or `grass_bend_mean`. Those live on the
  GPU and are never read back — the CSV reports the CPU-side inputs they are
  derived from (`rain_rate`, `rain_drops`, `grass_blades`) instead.
