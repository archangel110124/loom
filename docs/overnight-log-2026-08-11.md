# Overnight autonomous run — 2026-08-11

Branch: `overnight/2026-08-11`, forked from `worktree-loom-companion-docs` @ `fd1ae4a`.
Deadline: **07:15 EDT**. Started **23:44 EDT**.

## Governors

**Usage cannot be measured as a percentage.** `/usage` is a Claude Code CLI
built-in and is not reachable from Bash; `ccusage blocks --live` is interactive
and returns nothing non-interactively. `ccusage blocks --json` works and reports
*cost and burn rate*, not percent-of-limit, so it cannot answer the 90/97/98
thresholds the brief specifies.

Per the brief's fallback, this run is therefore **conservative**: sub-agent
fan-out never wider than 2, and a git checkpoint at least every 30 minutes.
`ccusage` cost and burn rate are recorded at each checkpoint as the best
available proxy.

Baseline at 23:44 EDT: active block (23:00–04:00 EDT) at **$7.93**, burn
**$13.00/h**, block projection **$63.40**. Prior block (18:00–23:00 EDT) closed
at $248.30 / 439M tokens.

## Scope note decided without the human

"Stage 2" and "Stage 3" are read as the repo's **P2 (grass)** and **P3 (water)**
from `docs/design/LOOM-IMPLEMENTATION-ORDER.md`, which is the sequencing
document `CLAUDE.md` designates as authoritative after M12.

**Grass shadowing is explicitly out of scope** and is not a gap I will try to
close. This engine has no shadow system at all — shadows, SDFGI and the post
stack are Phase 8, deferred with a stated reason. Building one to shadow grass
would be exactly the "never delete or rewrite existing systems / don't invent
new features" line the brief draws, and it would be a multi-day system, not an
overnight piece. Recorded here so the absence is a decision rather than an
oversight.

---

## Log

### 23:44 — start

Ran `date`, attempted usage measurement (see Governors), created branch.

State inherited: P2 slices 1–6 complete and committed through `fd1ae4a` (MSAA).
Slice 7 (the AA investigation's conclusion) was complete and green in the
working tree but uncommitted at the moment the overnight brief arrived; it is
committed first, below, rather than folded into overnight work, because it was
finished and gated before the brief.

### 23:45 — P2 slice 7 committed: density falloff concludes the AA investigation

Not a builder/critic piece — this was already done, measured and green when the
brief arrived. Recorded for bisect completeness.

Flicker on `meadow` at 4x MSAA, one change per row:

    no cull (every blade)              0.354
    hard cull, 12% surviving at range  0.234
    + soft fade                        0.214
    + fading all the way to none       0.137   <- shipped
    soft fade + alpha-to-coverage      0.212
    soft fade + minimum-width clamp    0.419

**0.354 → 0.137.** Three findings worth carrying forward:

1. The previous round measured a minimum-width clamp and a hard cull *together*,
   got 0.431, and blamed the cull. Separated, the cull is the largest single win
   and the clamp is what nearly doubles flicker. The clamp is now deleted, not
   gated — measured worse twice, decisively, confound removed.
2. Fading to *none* rather than to a floor is most of the win. A sparse scatter
   of surviving sub-pixel blades is noisier than either a full field or none.
3. That only works because the ground under a field is authored grass-coloured.
   `meadow`'s soil was brown and the thinned field read as ploughed earth from
   the flythrough orbit. This is a **scene-authoring rule**, now written into
   `CLAUDE.md`.

Also found and fixed: **`meadow` was not in `GOLDEN`**, so the image gate had
been reporting full passes for two slices without rendering a single blade.
Grass is the only rendering path whose geometry exists solely in the vertex
shader. Now eight scenes.

Gates: clippy clean, 35 test binaries, 18 scene runs / zero validation messages,
determinism `b478ea4ac2622d32` (unchanged — grass is outside the sim hash,
verified rather than assumed), 8/8 images match.

### 23:48 — piece A dispatched: grass on real terrain (builder running)

The gap: `loom_cli::grass_blades` passes `&|_, _| Ground::default()` — a flat,
constant ground — so grass has never responded to terrain at all. Two of Phase
2's five exit criteria depend on it ("thins on steep slopes and thickens in
gullies without any authored mask", "destroying terrain under a patch leaves no
floating blades"). `loom_grass::coverage` already implements the rules and has
tests; only the feed is missing.

### 23:52 — performance: the existing harness cannot measure grass, and there is no evidence of a problem

Marginal per-frame cost at 1920x1080, taken as (time for 33 frames − time for 1
frame) / 32 so device init, the grass bake and PNG encode fall out:

                     run 1              run 2
    meadow       28.14 ms/frame     30.39 ms/frame
    primitives   33.52 ms/frame     30.60 ms/frame

Run 1 was taken while a sub-agent was using the GPU and its ordering was noise.
Run 2 is the result: **~30 ms per frame regardless of what is in the scene.**
`primitives` has no grass, `meadow` has ~45,000 blades at 42 vertices each, and
they cost the same. So this number is entirely the offscreen path's per-frame
readback and stall, and it cannot answer "does a grass field render at target
framerate" in either direction. Reported here as two runs rather than one
because the first, alone, would have supported a confident wrong conclusion.

**Decision made without the human, and the reasoning.** The obvious next Phase 2
item is the placement compute pass plus `vkCmdDrawIndirect`, which the design
doc specifies. I am *not* building it next, for two reasons:

1. **There is no measurement showing it is needed.** Building a GPU culling and
   compaction path to fix an unmeasured cost is the premature optimisation this
   project's style rules exist to prevent. Measure first.
2. **It would create a second placement implementation.** Placement lives in
   `loom_grass` as tested Rust. Hand-porting Voronoi clumping and the position
   hash into Slang is exactly the CPU/GPU divergence that S2 and ADR 0006 were
   built to make impossible, and the `Expr` tree S2 generates from is a scalar
   field language that does not express loops, neighbourhood search or struct
   output. Doing it properly is an architectural piece that deserves an ADR and
   the human's judgement, not an overnight decision.

So piece B is instead **GPU timestamp queries around the render graph's passes**
— the instrument that makes any performance claim honest, that the "renders at
target framerate" exit criterion actually requires, and that UE5 has as its GPU
Visualizer. With real numbers, the culling decision can be made on evidence in
the morning. Written up for the human to overrule.

### 23:51 — Stage 3 (P3 water) is fully specified; no spec-writing needed

Checked, because the brief said to write the spec if Stage 3 was underspecified.
It is not: `LOOM-IMPLEMENTATION-ORDER.md` gives steps W0–W9 and
`docs/design/loom-water-system.md` gives the component schemas, the `sample_water`
signature, the Gerstner formulation and nine named traps. There is **no existing
water code** — one unrelated comment about smoke buoyancy in `loom_particles` is
the only hit in the workspace. Greenfield.

The first three steps are the ones worth doing tonight, because they are pure,
deterministic and independently testable:

  - **W0** `WaterBody` / `WaveSet` / `GerstnerWave` in `loom_scene::components`,
    plus the steepness validator. The doc specifies the exact error JSON shape
    (§5.3) including the computed limit, and caps waves at 16.
  - **W1** `sample_water` in a new `loom_water` crate, with **analytic** normals.
    §5.4 is explicit that finite-differencing is wrong here — Gerstner displaces
    horizontally, so three nearby samples are not at the positions you think.
  - **W2** the Slang twin plus an agreement test.

### 00:12 — piece B built: GPU timestamps. **Grass is cheap, and the plan changes.**

Builder returned; critic dispatched with fresh context and no sight of the
builder's reasoning. Verdict pending. The measurements, subject to that:

    scene         forward pass   readback   blades
    meadow           0.105 ms    0.610 ms   45,460
    primitives       0.050 ms    0.610 ms        0
    meadow minus its Grass component, same camera:
                     0.051 ms    0.610 ms        0

**Grass costs ~0.054 ms** for 45,460 blades at 1920x1080 with 4x MSAA — about
0.3% of a 16.7 ms frame. The entire forward pass of every scene in this project
is 0.05–0.11 ms. 4x MSAA costs ~0.024 ms and almost all of that is sky and
ground fill; the blades cost the same at 1x and 4x, which is what thin
coverage-limited geometry should do.

The builder calibrated the instrument against a quantity predictable from first
principles rather than asserting it: the readback pass is a pure image→host copy,
and 8.29 MB / 0.610 ms = 13.6 GB/s, which is realistic PCIe 4.0 x16. It holds at
13.5 GB/s at 4K and 14.0 GB/s at 960x540, and readback scales 1.00 / 4.12 / 16.5x
against a 1 / 4 / 16x pixel count. A mishandled `timestampPeriod` would have
thrown that off by the same factor and landed somewhere absurd. Good method —
but the critic is verifying it independently, because a confidently-wrong timing
instrument is worse than none.

**What this settles.** The ~30 ms/frame measured earlier is **0.7 ms of GPU work
and ~29.3 ms of CPU and stall.** So:

  - The placement compute pass and `vkCmdDrawIndirect` queued next in the design
    doc are **not justified by GPU cost at this scale**. Density could rise ~10x
    before the grass draw reaches 0.5 ms. Building them tonight would have been
    optimising the one part of the frame that is already free.
  - The thing actually worth instrumenting next is the **CPU** side.

### 00:20 — correction: the ~29 ms is the PNG encoder, not the engine

The builder attributed the non-GPU remainder partly to "per-frame blade
regeneration on the CPU". **That is wrong and I checked it rather than repeating
it.** `loom_cli`'s render loop calls `set_grass` once *before* the frame loop;
blades are baked exactly once per invocation.

Marginal per-frame cost of `meadow` against resolution:

    960x540      4x pixels    11.97 ms/frame
    1920x1080   16x pixels    32.26 ms/frame
    3840x2160   64x pixels    98.29 ms/frame

(A 480x270 point read 119 ms and is discarded as contaminated — sub-agents were
compiling on the same box. Reported rather than silently dropped.)

That fits **~10 ms fixed + ~11 ms per megapixel**. GPU readback is 0.61 ms at
1080p, so the per-megapixel term is not transfer — it is **PNG compression**, in
a loop that writes one image per frame.

So the honest picture is: the offscreen path's frame time is dominated by the
harness being a *testing* tool that encodes a PNG every frame. It never measured
the engine at all, in either direction. Engine cost at 1080p is **0.7 ms of GPU
and a small CPU remainder**. Nothing here indicates a CPU bottleneck in the
engine, and the earlier entry's "the CPU side is the thing worth instrumenting"
should be read as unproven rather than established.

This is the outcome that justifies having built the instrument before the
optimisation, and it is worth noticing that the intuition it overturned was the
design document's own sequencing.

**W2 will follow `loom_field::noise`'s precedent, not S2's `Expr` tree.** The
water doc's recommended Option A is "write it once in Rust, emit the Slang from
`build.rs` as text", which is exactly what `noise::slang()` already does. S2's
`Expr` is a *scalar field* language; Gerstner needs vector output and a loop over
a wave set, which it does not express. Same reasoning that ruled out generating
grass placement, and the same conclusion reached from the design doc's own
recommendation rather than from convenience.
