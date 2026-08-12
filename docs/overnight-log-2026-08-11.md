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
