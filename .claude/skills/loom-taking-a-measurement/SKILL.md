---
name: loom-taking-a-measurement
description: Use before quoting, recording or acting on any number about Loom — ms/frame, ms/tick, fps, flicker, shimmer, pixel fractions, a cost, an "N× faster" claim — including a number already written in an ADR, a design doc or a commit message.
---

# Taking a measurement in Loom

Every wrong number in this project's history was internally consistent and
plausible. They fail in five shapes, and each has fired more than once.

## 1. The subject was not in the frame

The most expensive shape, three times over:

- A smoke cost was reported **four times** on `plume.loom` rendered without
  `--sim`, so the frame contained no plume — including "0.371 ms at the
  authored camera, confirming the builder's 0.35 ms" told to the human. The
  real figure is 0.779 ms (`OVERNIGHT-DECISIONS.md` D21). *"What actually
  caught it, every time, was opening the image."*
- `cargo xtask shimmer` auto-framed `meadow` ~38 m back; the 55 m density
  falloff had deleted the entire field. A night of AA measurements ranked
  variants by **how fast they removed the subject** (CLAUDE.md, P2 slice 7).
- `pool_jet`'s proposed GOLDEN row had the jet inside a floating sphere
  (`WATER-REBUILD-REVIEW.md` defect 4).

**So: open the PNG before you quote the number.** Check the scene's row in
`xtask/src/main.rs` for its `--sim` — any scene whose subject is a particle,
plume, splash or fluid is empty at tick 0. Use the **authored** camera; a
metric that frames a scene automatically will silently stop containing its
subject and then reward whatever removes the subject fastest.

Suspicious agreement between variants (D21's four near-identical rows) means
look at the picture, not at the value.

## 2. The machine was not quiet

`fb9ea5d` — six agents ran gates at once and `validate` reported `lanternhead`
at 44.844 ms against a 25.2 ms unloaded truth. Its subject line: *"the gates are
a singleton, because six of them ran at once."* **This is the third time
contention has been mistaken for a regression here.**

The converse trap: the review panel's "16× host-load fragility" (`slosh`
13.5 → 222.6 ms/tick) did **not** reproduce — 1.41× under 24 spinners (ADR 0057
addendum 3). The swing was an O(k²) sort, and a wrong mechanism was nearly
designed against it.

Check for other agents and builds. The GPU is shared with Ollama — `ollama ps`.
Gates serialise on a lock; ad-hoc `loom render` does not. Take min-of-N. Either
measure on a quiet machine, or measure under both loads and say which.

## 3. The unit was wrong, or the timer covered more than its name

`1c25f38` — *"the surface timer named one third of what it measured."* The line
`cinematic surface: … marched in 19.2 ms` timed a density readback + a CPU
march + a spray readback; `march` was the smallest of the three, and that one
word *"sent the previous investigation at the marching cubes while two thirds of
the number was a shader waiting on PCIe"* (`573cbca`). The same commit quoted
`ms/tick` as `ms/frame`, which *"put cinematic water inside a 60 Hz budget on
paper while the window ran at two frames a second."*

Always state: per tick / per frame drawn / per run; the scene; the tick; the
resolution; debug or release. **Split any timer that spans more than one
mechanism before quoting it.**

## 4. The recorded number was not the current number

ADR 0057's body says `ribbon@180` is 52 ms/tick; the review panel measured 107;
HEAD measures 14.78. `faf34ad` corrects six such claims at once, two of them
wrong particle counts inside a GOLDEN row's own comment.

**A number in a document is a measurement someone took on a machine state they
may not reproduce.** Re-measure before you repeat it.

## 5. The comparison was not a comparison

- **One variable per row.** A min-width clamp and a hard cull were measured
  together (0.431), the wrong tool was blamed, and the entire AA investigation
  had to be re-run (CLAUDE.md, P2 slice 7).
- **Have a control.** `cave`, `primitives` and `materials` scoring exactly
  0.000 on `shimmer` is what proves the instrument, not the setting.
- **Never compare two AA/flicker numbers across a change in colour, lighting,
  resolution or a bless** (ADR 0010). Normalising flicker by mean brightness
  was tried and reverted — it still scales, more steeply.

## Attribute by ablation, and check the ablation is legal

`a6c491b` found the cost by suppressing spray (141.7 fps) and suppressing the
surface (37.6 fps) — two runs, no guessing. Guessing had already produced a fix
on the wrong copy of the code with **no measurable change at all**.

The counter-case (ADR 0058 §6b): removing the V-cycle changes the state it is
measuring, so the ablation runs *slower* than the real thing and cannot answer
the question. Finding that out is the result.

Extrapolation is a hypothesis, not a measurement: D15 extrapolated smoke to
~2.8 ms full-screen, D20 measured 0.85–0.9 ms and retracted the conclusion.

**A shelf is not a verdict** (ADR 0058 §6): the density-splat clamp measured
4.6× → 34.7× and was written off; on top of the later gather fix it is correct
and cheap. Re-measure shelved changes after the next structural fix lands.

## The instruments, and what each one excludes

Read their docs at the source rather than trusting this list — instrument
names move, the discipline does not.

| Instrument | Measures | Blind to |
| --- | --- | --- |
| `LOOM_GPU_TIMING=1` + `Renderer::last_pass_times` | per-render-graph-pass GPU time, offscreen path | prints `graph` (~2% of a frame), **not `total`** — the TLAS rebuild is a separate submit, the PNG encode is CPU. The `Viewer` is deliberately uninstrumented. |
| `loom run --frames n [--play]` | per-frame `cpu`/`draw` in the window; `--play` is the only way to get a running game's real per-frame work | GPU pass breakdown |
| `loom flicker a.png b.png c.png` | temporal noise `\|b-(a+c)/2\|` | cannot tell "wider" from "less stable" — the rain width floor was settled by a salt metric over flicker's objection (`RAIN_MIN_PIXELS`) |
| `cargo xtask shimmer` | twinkle at rest | ratios **within one scene only**, strongly resolution-dependent, and it auto-frames |
| `loom render --frames/--spin/--step/--dolly` | motion sequences; `--dolly` is the only one producing parallax | see the `watch-loom` skill for reading them |
| `loom compare --rect x,y,w,h` | fraction/worst plus mean RGB of a crop in both images | — |
| `LOOM_FLUID_DEBUG` | solver occupancy | — |

Two named traps: the offscreen harness's ~30 ms/frame is **the PNG encoder**
(~10 ms fixed + ~11 ms/megapixel; GPU readback is 0.61 ms of it) — it never
measured the engine in either direction. And `cargo xtask shimmer` steps 0.2 s
between frames, so a rain drop falls 1.6 m and consecutive frames share no
streaks: it cannot see rain at all.

## The closing rule

A number saying the cost is already ~0.3% of a frame is a reason **not to
build** the optimisation. GPU timestamps said grass costs 0.054 ms and deleted
the planned placement compute pass and indirect draw from the phase
(`519eff8`).
