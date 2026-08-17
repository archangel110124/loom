# Decisions taken without you, 17 Aug 2026

**Why this file exists.** You set a goal — work until 07:15, don't stop, and where a decision
needs you, make the recommendation and proceed rather than block. This is the ledger of those
calls. Each entry says what was decided, what the alternative was, how confident I am, and what
it would cost to reverse. **Nothing here is settled; every row is yours to overturn.**

Sorted by how much I'd want you to look at it, not chronologically.

---

## ⚠ Worth your attention first

### D1 — The report's milestone 4 (buoyancy readback) is refused, and milestones 3–4 are already shipped

**Decided:** The VFX report's recipe — "buoyancy sampling that reads back the height field" — is
rejected outright, not adapted. A GPU→CPU readback feeding the fixed physics step would make the
simulation depend on GPU float behaviour and frame timing, which destroys the determinism the
engine's assertions rest on.

**Why it barely cost anything:** the judge verified that buoyancy is *already* built correctly
on CPU pontoons (`crates/loom_cli/src/play.rs:549`) reading the closed-form Gerstner
`sample_water`, and `crates/loom_water/src/lib.rs:11-18` already says "Never GPU readback"
verbatim. So the report's milestones 3 and 4 describe work this engine has shipped. The roadmap
is shorter than the report implies.

**Confidence: high.** Reversing it means reopening a locked property.

### D2 — The FFT ocean is deferred behind evidence, not scheduled

**Decided:** No unwindowed Tessendorf FFT as the swimmable sea. If an FFT tier is ever built it
may exist *only* as a force-free detail band windowed from the same Pierson–Moskowitz spectrum
above an ω_split, so `m0_low + m0_high = m0` is a CPU-checkable test and buoyancy keeps reading
the Gerstner tier unchanged. Building it needs its own ADR **and** flythrough evidence at
authored cameras that the Gerstner sea visibly tiles.

**The alternative** was scheduling FFT early as the headline feature. Rejected because a boat
floating on a surface it is not drawn on is precisely the defect `loom_water`'s own header
exists to prevent — and because "does the current sea actually tile badly enough to matter" is a
question the flythrough instrument can answer, and hasn't been asked.

**Confidence: medium-high.** This is the one most likely to be worth overturning if you look at
the water and think it reads as flat. Say so and it gets built.

### D3 — A new gate, `cargo xtask repeat`, before any second GPU-stateful path

**Decided:** ADR 0017 claims rain's byte-identity holds because "the draw is additive, so the
image does not depend on the order". That is **measured on one RTX 4090, not guaranteed** —
`InterlockedAdd` decides slot order, slot decides seed, and float addition is not associative.
A new gate runs three fresh processes and SHA-256 compares, and it lands before a second
GPU-stateful effect does.

**Confidence: high.** This was the sharpest finding in either design and it is cheap.

---

### D10 — `beach` got *less* pretty, and I blessed it anyway

**Decided:** the W1 water work makes `assets/test/beach.loom` visibly paler with dimmer caustics
than the reference it replaced. I read both images, judged the change correct, and blessed.

**Why the old one looked better:** it had two bugs.

1. **The caustic web was applied twice** — once at `scene.slang:2285` and again at `:4316`. The
   code at 2285 carries a comment describing an `eyeUnderwater()` guard that was never actually
   there, so every bed seen through the surface got the web doubled. Fixing it makes the web
   *dimmer*, which is the correct direction and looks worse.
2. **The sun's down-leg path was ~2.9× where physics says ~1.4×.** Snell on both legs replaced a
   `/ max(sunDirection().y, 0.15)` with a bounded secant, and the diff has one fewer magic number
   after it than before.

So the reference was prettier because light was being counted twice through half a metre of
water over bright sand.

**Why I blessed rather than reverted:** keeping a known double-application to preserve a nicer
picture is exactly the kind of debt that makes every later water change unjudgeable — you would
be tuning against a bug. And the beauty is recoverable by *authoring*: the honest lever is
per-`WaterBody` optical parameters (scattering and absorption coefficients), which is how a real
engine art-directs water without lying about the physics.

**This is the row most likely to be worth your disagreement.** You asked for VFX to be a selling
point, and I have just made one scene less striking on correctness grounds. If you look at
`beach` and want the old look back, the answer is to author it — not to restore the double
caustic. **Confidence: high on the fix, medium on the aesthetics.**

### D11 — W1's acceptance test fails at 25.3 against 38, and it is unreachable

**Decided:** ship the mechanism and the measurement table rather than hit the number.

`WATER-REFRACTION-PLAN.md` set a hard acceptance test: restore `shore`'s shallow-band G−R to
≥ 38 with no compensating constant. **The shipped result is 25.3.** It does not pass, and no
constant was reached for.

**It is not reachable by refraction, and that was measured three ways rather than argued.** Both
legs fully vertical — the shortest physical path there is — gives G−R 23.7; half the refracted
path 24.4; the shipped path 25.3. G−R has a *maximum* near the true path, because `shore`'s bed
is warm sand (albedo 0.52/0.45/0.34), so every metre of path removed lets more red back up.
Shortening the optical path, which is what refraction does, moves the number the wrong way.

**What did work:** the hue is restored by mechanism. R/B went 0.597 → **0.649** against the
0.644 the band measured before the regression. The colour is right; the brightness is not.

**My recommendation, and what the ADR records:** the remaining units are only reachable by a
multiplier the plan explicitly forbids, so the honest lever is per-`WaterBody` optical
parameters — the same lever D10 names. That makes shallow-water brightness an authoring
decision instead of a global constant, which is what it should have been.

**Correction to my own earlier note:** I told you the baseline was G−R 20.3. That was measured on
a `--sim 0` render; `shore`'s golden row is `--sim 90`. The correct baseline is **26.3**, which
is what both designs and the judge used. My point that the plan's "14" was stale still holds; my
number did not.

### D12 — W3/W4's build agent crashed *after* committing, and I verified the commit instead

**What happened:** the W3/W4 workflow's build phase hit `StructuredOutput retry cap (5) exceeded`
— five attempts to serialise its final report, none valid. The design and judge phases succeeded.

**Why it did not cost the work:** the agent had already committed. `e2af1b7` was on the branch
with a clean tree, and its commit message carries everything the structured report would have —
the measurements, the cost figures, the scene list, and an explicit note that blessing is the
verifier's job. I verified the commit directly rather than re-running the builder.

**Decided:** do not re-run. Re-running would have rebuilt work that already exists and burned
the remaining time. The lesson for future workflows is that the report schema was too demanding
— seven required fields including three long prose ones — and a smaller schema would have
survived. **Confidence: high.**

### D13 — A real Vulkan bug was found, and the scene that catches it did not exist

**Recorded because it is the best argument for this project's own discipline.**

Once `waterFragmentMain` statically references `sceneTLAS`, a scene containing water and **no
mesh** draws with descriptor set 0 unbound — VUID-vkCmdDraw-None-08600. `build_instances` now
always builds a TLAS, and set 0 is bound only when `ready()`.

**No scene in the repo covered that case.** `assets/test/bare_sea.loom` was written to, and the
fix was verified the right way round: reverting it fires the VUID on `bare_sea` and on no other
scene. That is a latent crash-class defect that would have shipped and then appeared the first
time somebody authored an ocean with nothing floating in it.

### D14 — Two smoke constants were wrong for reasons a still image cannot show

**`SMOKE_HZ` was 2.6, which is 1.4 noise cycles across the plume's width.** One blob of noise
over an envelope with a hard boundary, so whether the silhouette read as smoke was a lottery on
which region of the noise field it happened to land in. It was found by *adding seed
decorrelation*, which turned a wispy column into a solid oval — the decorrelation did not break
it, it revealed that the original was luck. Now 6.5, which erodes the cone everywhere and renders
as the same kind of thing at offsets 0, 8 and 32.

**`SMOKE_STEPS = 20` was chosen on the flicker floor, not on the still.** An under-resolved
volumetric march does not blur — it *crawls*, and only a moving instrument sees that.

Both are the same lesson this project keeps relearning: the still image is the weakest instrument
it owns, and the constants that matter are the ones a still cannot judge.

### D15 — Smoke is the first effect whose cost scales with coverage, not population

0.35 ms at `plume`'s camera; **~2.8 ms extrapolated to full-screen coverage.** Every other effect
here is priced by how many things exist — 45,000 grass blades at 0.054 ms, 131,072 rain drops at
0.022 ms. A volumetric march is priced by how much of the screen it covers, so a camera walked
into the plume costs an order of magnitude more than one looking at it from across the yard.

**No budget decision was taken on this** and none should be until somebody walks a camera into a
plume and looks at the frame time. Flagging it because it is the first time the engine's cost
model changes shape, and the existing `LOOM_GPU_TIMING` numbers in CLAUDE.md will read as
misleadingly cheap next to it.

## Routine calls, logged for completeness

### D4 — Subagents no longer run `cargo xtask` gates

Your instruction, implemented: workflow builders run clippy, tests, `cargo check` and
`check-deps.sh` (cheap, lock-free) and never the xtask gates. I run those during verification.
Three queued `validate` runs had already cost ~30 minutes of wall clock on this branch.

### D5 — The editor is paused *in the document*, not just in conversation

`docs/design/editor/PLAN.md` gained an "ON HOLD" section at the top naming where it stopped and
pointing at `MANUAL-CHECKS.md`. A hold that exists only in a chat log is not a hold.

### D6 — No new crates yet

The report proposes `loom-vfx-core`, `loom-vfx-render`, `loom-water`, `loom-fluids`,
`loom-noise`. Two problems: the workspace uses underscores, and a crate with one caller is a
module. New crates wait for a second caller. Noise stays in `loom_field` regardless — ADR 0006
says a crate bump must never be able to move the sim hash.

### D7 — ADR numbering starts at 0045

`docs/decisions/` runs to 0044 with gaps, and the editor plan reserves 0023–0042. VFX ADRs take
numbers above 0044 to avoid a collision with editor work that may still land. (The two competing
ADR 0022/0023 drafts rescued from deleted worktrees are still unfiled — see below.)

---

### D8 — The report's milestone order was replaced

**Decided:** the nine milestones in `VFX-IMPLEMENTATION-REPORT.md` are re-ordered as W0–W8, and
the change is large enough to name. The report front-loads the GPU particle core and schedules
the FFT ocean fifth. The judged order is:

| | | |
|---|---|---|
| W0 | `cargo xtask repeat` | ✅ built |
| W1 | screen-space refraction | in progress |
| W2 | whitecaps from `fold` | in progress |
| W3 | one traced reflection ray on water | queued |
| W4 | smoke as a marched soot volume | queued |
| W5 | splash/spray from fold + submersion | queued |
| W6 | interactive ripples, CPU-authoritative, own ADR | queued |
| W7 | the GPU particle pool | ✅ built |
| W8 | windowed FFT detail tier | deferred behind evidence (D2) |

**Why:** the report is engine-agnostic and assumes a greenfield. This engine has already
shipped several of its milestones — buoyancy on CPU pontoons, splash machinery, a line-integral
fire march. Refraction is cheapest-visible-win first because it has a *written, hand-measured
acceptance test* already in the tree (`WATER-REFRACTION-PLAN.md`: `shore` shallow band
G−R ≥ 38), and because a previous commit deleted the term that was carrying the shallow-water
look. Refraction is not a new feature there — it is the honest replacement for a regression.

**Confidence: high** on the ordering, **medium** on W4 (marched soot rather than a real gas
solver). W4 generalises ADR 0020's shipped line-integral fire instead of building Navier–Stokes.
If you look at the smoke and it reads as a procedural blob rather than a plume, that is the row
to revisit — a grid solver is the report's answer and it was priced, not refused.

### D9 — The GPU pool was built out of trigger order, and the trigger was satisfied honestly

**Decided:** the judged design gated the GPU particle pool behind a trigger (a scene needing
>65,536 particles, or a measured CPU tick >0.2 ms) rather than scheduling it. I asked for it
first anyway, because you asked for milestone 1 and everything in the report hangs off it.

**What makes that defensible rather than sloppy:** the builder did not skip the trigger, it
*satisfied* it — authored `assets/test/emberfall.loom` at 16,200 slots and measured the
difference (0.61 s against 5.55 s for `--sim 600`). So the pool exists because a scene needs it,
which is the condition the gate was asking for.

**Confidence: high**, and the caveat is now closed. I ran `cargo xtask repeat` across all 32
golden scenes: **every one reproduces byte for byte across three fresh processes.** No
pre-existing scene fails the new check, which was the open risk. All five gates pass on the
pool — clippy, 46 test binaries, 32 golden images, 56 scene runs with zero validation messages,
and the new repeat gate — with the determinism hashes unmoved.

**What that does not prove:** reproducibility on *other* hardware. The original concern was that
additive-blend order-independence is not guaranteed across GPUs, and no gate running on one
RTX 4090 can settle that. The gate catches regressions here, which is worth having and is less
than the question asked.

## Still owed to you, unresolved

- **Two rival ADR 0023 drafts for the fire work**, rescued from throwaway worktrees to
  `$CLAUDE_JOB_DIR/tmp/rescued/`: `the-plume-is-carried-by-a-curl` and
  `the-fire-is-art-directed-not-simulated`. The fire feature shipped to master; its ADR never
  did. Which reading was right is a judgement I did not make for you.
- **Two rival ADR 0022 drafts** for the reflected-hit work, same provenance. The winner shipped
  as 0044; these are the losing variants.
- **`PLAN.md` says the token table has 25 entries; `DARK` has 23.** Code is the truth, the plan
  is stale. Not silently "fixed" either way.
- **Everything in `editor/MANUAL-CHECKS.md`** — no gate in this project has ever seen a pixel of
  the editor.
