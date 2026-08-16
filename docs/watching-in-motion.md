# Watching Loom in motion

A still image cannot show a phase error, a sign flip, a lag or a discontinuity,
and those are most of the bugs in water, rain, wind, grass, clouds and scatter.
This is how to look at motion without paying for twenty screenshots.

```bash
tools/watch.sh homestead
# wrote target/agent/contact_sheet.png (20 frames, 4x5 grid, 1248 KB)
# wrote target/agent/telemetry.csv (20 rows)
```

One labelled contact sheet and one CSV. The sheet answers *does it look right*;
the CSV answers *what are the numbers doing*. A finding cites both, by frame
index — "`rain_rate` goes to zero at frame 12 and the streaks disappear in cells
12–15" — which is a claim someone else can check.

A single image costs roughly the same in vision tokens whatever its resolution,
so twenty frames on one sheet cost about what one screenshot does.

## Knobs

| Variable | Default | Meaning |
| --- | --- | --- |
| `FRAMES` | 20 | frames captured |
| `GRID` | `4x5` | tiling, columns × rows |
| `CELL_W` | 320 | cell width in pixels |
| `STEP` | 6 | **simulation ticks between frames** — 60 is one second |
| `WARMUP` | 0 | ticks simulated before frame 0 |
| `SIZE` | `960x540` | render size before tiling |

`STEP` is the one that matters most. Twenty frames six ticks apart span two
seconds — right for rain and wind, useless for a tide or a drying curve.

For detail rather than sweep: `FRAMES=9 GRID=3x3 CELL_W=480`.

## What it is built on

Nothing new. `loom render --frames N --step T` already dumped deterministic
numbered frames offscreen, which is the whole capture; the script drives it and
tiles the result, and the engine gained only the telemetry CSV and the
directory handling around it.

## Determinism

Two captures of the same scene with the same `STEP` and `WARMUP` are
byte-identical — **with one exception, found by the acceptance test for this
tool and not yet fixed.**

**The first ~4 frames of a scene with rain do not reproduce.** Measured on
`rain_impact` with no warmup, two runs differ at frames 0-3 by 5268, 20367,
9766 and 578 pixels, and every frame from 4 onward is bit-identical.
`WARMUP=300` is clean throughout. `meadow` — grass and wind, no rain — is
bit-identical from frame 0.

The drop layer is stateful and GPU-resident (ADR 0017). Its opening frames
depend on state that is not reproducibly seeded, and it washes out within about
four frames as drops are re-issued. ADR 0017 verified byte-identical rain
across three processes, but at a *single* `--sim N` — one dispatch — which is
exactly the case this does not cover.

`tools/watch.sh` warns when a rain scene is captured with `WARMUP` under 240.
Until it is fixed, **use `WARMUP=300` whenever comparing two captures of a rain
scene**.

**The CSV is not byte-identical either, by design**: it carries
`frame_time_ms`, which is wall-clock. Every other column reproduces exactly.

**There is no `--seed` flag, because there is no global RNG to seed.** Loom's
randomness is position-hashed off a frozen integer hash (ADR 0006), clippy
forbids `thread_rng` in simulation code, and `cargo xtask validate` asserts the
simulation hash matches between debug and release builds. Determinism here is a
property of the design rather than of a flag.

**There is no `--dt` flag either.** A fixed timestep is a locked decision —
the simulation never sees a variable `dt` — so what a capture chooses is not the
step size but how many steps apart its samples are, which is `--step`.

## Telemetry columns

Written to `target/agent/telemetry.csv`, one row per frame, four decimal places.
Columns depend on what the scene has:

| System | Columns |
| --- | --- |
| always | `frame`, `sim_time`, `frame_time_ms`, `draw_calls` |
| wind | `wind_x`, `wind_y`, `wind_z`, `wind_magnitude` |
| water | `water_height_min`, `water_height_max`, `water_height_mean`, `water_energy` |
| rain | `rain_rate`, `rain_exposure`, `rain_drops` |
| grass | `grass_blades` |
| scatter | `scatter_instance_count`, `scatter_placement_hash` |

**No `rain_active_particles` and no `grass_bend_mean`.** Those live entirely on
the GPU and are never read back — ADR 0014 is explicit that a readback would
destroy the determinism this whole tool rests on. What the CSV reports instead
are the CPU-side inputs those numbers are derived from.

`scatter_placement_hash` is the determinism check for scattered instances: it
must not change between two runs of the same scene.

Adding a column means implementing `Probe` in `crates/loom_cli/src/telemetry.rs`
and adding it to `probes()`. The writer takes its header from whatever the
probes return.

## Automation

`.claude/settings.json` carries an opt-in `PostToolUse` hook that regenerates
the sheet after an edit to a `.slang` or `.loom` file, so an unattended run
always has a current one. **Off by default**: a capture is about five seconds of
GPU work, and firing it on every file write would cost more than the work it is
watching.

```bash
export LOOM_AUTO_WATCH=1
export LOOM_WATCH_SCENE=meadow   # optional, defaults to homestead
```

## Limits

The cells are about 320 px wide. The sheet is reliable for gross motion —
direction, phase, whether something moves at all, discontinuities, large
artifacts — and unreliable for individual particles, thin geometry, sub-pixel
shimmer and fine colour. For sub-pixel stability use `loom flicker a b c`, which
measures what the sheet cannot see.
