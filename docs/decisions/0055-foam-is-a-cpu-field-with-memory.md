# ADR 0055 — Foam is a CPU field with memory, and it must stay on the CPU

- **Date:** 2026-08-18
- **Status:** **proposed** — needs human approval before this branch merges.
- **Governed by:** ADR 0045 (clause 1 admits state advanced inside the fixed
  step; clause 3 is why this is not a GPU accumulator), ADR 0053 §7 (which
  deleted the two shading terms whose mechanism this ADR rehouses).
- **Extends:** ADR 0049, whose "what would change this" section names exactly
  the trigger that fired. Its constants are amended separately, in the
  amendment appended to that file.
- **Decision touched:** none of CLAUDE.md's locked decisions. No tier change:
  this is deterministic-tier water throughout and produces no force.

## Context — the trail can only remember what a wave did

ADR 0049 established that the whitecap trail needs no buffer, because
`F(x,t) = max(f(x,t), d·F(x − v·Δt, t − Δt))` unrolls into a maximum over past
samples of a function that can simply be evaluated. That argument is airtight
and it rests entirely on one property: `fold` — now `mu_max` — is a **pure
function of `(x, t)`**.

The same ADR wrote down what would end it:

> A foam source that is not closed-form in `(x, t)` — a wake behind a moving
> hull, spray landing back on the surface, an interactive ripple (W6). None of
> those can be evaluated backwards in time from a position, so none of them
> unroll, and the first one that ships is the trigger to revisit the buffer with
> ADR 0045 clause 3's checklist in hand.

Three have now shipped or are being asked for at once. `plough.loom` is a hull.
`pool.loom` is an impact. `wake.loom` is an interactive ripple whose foam ADR
0053 §7 deleted from the fragment shader precisely because painting it from the
instantaneous slope gave it no memory — it appeared and vanished with the ring
instead of being left behind by it.

## Decision

> **Foam is a field: a 128×128 grid at half-metre cells, anchored to the water
> node, advanced one fixed tick at a time on the CPU.**
>
> ```text
> F' = decay · advect(F, u)  ⊔  deposits
> decay = 0.5^(Δt / 8 s)
> u     = flow(x) + stokes drift + hull drag
> ```
>
> The shader takes `max(instantaneous, trail, field)`; the field is a third
> floor on the coverage, not a second opinion about it.

Deposits are **maxima, never sums** — so a source cannot dim foam it did not
make, two overlapping sources cannot exceed 1, and the order of deposits inside
a tick cannot change the result. Four of them: crest (a smoothstep on `mu_max`),
hull waterline (speed and submerged fraction), impact disc at 1.5× the
waterplane radius, and the wake ring, which is where `RIPPLE_FOAM_SLOPE` and
`RIPPLE_FOAM_MAX` moved.

**Position is advected, never velocity** (Ihmsen). Foam is a passive tracer
painted on the water; advecting a velocity field would be simulating the water,
which is what the deterministic tier does not do.

## Why it must not be "optimised" onto the GPU

This is the clause most likely to be undone by someone who has read only the
performance numbers, so it is written down first.

**A headless `--sim N` render has to reach the state a live run reaches at tick
N.** ADR 0045 clause 3, as reworded by ADR 0053 §6, asks that catch-up be a
fixed function of N alone. Every GPU-stateful effect in this engine satisfies it
by rolling its recurrence up in registers per thread: the rain buffer and the
particle pool each reach tick N in **one** dispatch, because each element's
state at N is a closed form in N.

**An accumulator with memory has no such form.** Its state at tick N is the
ordered composition of N deposits and N advections, so a GPU version's catch-up
is **K dispatches** — 2,400 of them for `lanternhead`. That is legal under the
reworded clause and it is exactly the cost the clause was shaped to avoid, and
it buys nothing: the CPU steps are 0.3–0.5 ms each and the field is 64 KB.

**And it would take the assertion with it.** `water@x,z.foam` and `loom water
--at` read this field. A GPU float on that path is ADR 0045 clause 2, which
ADR 0053 relaxed only inside the cinematic tier — and this is deterministic
water, on every scene in the repository.

## Measurements

- **Semi-Lagrangian is not good enough, measured.** A four-cell stripe advected
  ten seconds in a uniform 2 m/s current comes out **twelve cells wider**.
  MacCormack with a stencil limiter takes that to **five**. The design's
  two-cell budget is not reachable at this cell size — six hundred bilinear
  interpolations diffuse, and a second correction pass is the last cheap tool.
  The test asserts five, and names twelve.
- **The threshold and the memory are one choice.** Steady mean coverage on
  `whitecaps.loom` after 600 ticks at the 8 s half-life:

      0.33  15.73%    0.44  5.41%    0.50  2.03%
      0.36  12.51%    0.45  4.73%    0.52  1.28%
      0.40   8.70%    0.46  4.10%    0.54  0.70%

  **0.45**, at 4.73%. The shading threshold stays 0.33: where a crest is drawn
  white *now* and where it leaves a raft still there eight seconds later are
  different questions.
- **The decay clause in the brief was self-contradictory.** "Under 0.05 within
  12 s" and an 8 s half-life cannot both hold — 12 s is 0.354 and 0.05 is
  34.6 s. The half-life is what the sweep is calibrated against, so it wins.
- **Advection is real and slow to show.** Stubbing `u = 0` moves **1.34%** of
  `plough.loom` at tick 150, **1.84%** at 300, **2.17%** at 420.
- **Cost, `loom sim --ticks 600`, wall clock:** `whitecaps` 0.00 → 0.31 s
  (0.52 ms/tick), `ocean` 0.00 → 0.32, `pool` 0.02 → 0.20 (0.30 ms/tick),
  `river` 0.27 → 0.55. **Over the 55–130 µs/tick the design budgeted**, and
  that budget was written for a single semi-Lagrangian pass without a crest sum.
  Three things already claw at it and are part of the decision:
  - the crest sum is strided over eight ticks (133 ms against an 8 s half-life);
  - a sea whose `peak_fold` cannot reach the threshold skips the crest sum
    entirely, which is every calm scene in the repository;
  - an empty field skips advection altogether, which is why `mirrorpool`,
    `lanternhead` and `squall` render byte-identical.

## What is deliberately not in it

- **Spray landings do not deposit.** `loom_water::spray`'s population follows
  the eye, by design and legally, because it is drawn and nothing else. Feeding
  it into a field that an assertion can read would make a CPU quantity depend on
  where the viewer stands — ADR 0045's trap clause, which is untouched. A spray
  deposit needs spray to be anchored to sim state first.
- **The field carries no age channel.** Its foam draws *fresh*: a hull's churn
  and an impact's ring are white while they are being made, and what removes
  them is the coverage decaying under the shader's lace erosion, which is how
  foam leaves everywhere else in this shader. The trail is the only term that
  describes foam that is *old*. A second advected channel would double the
  advection cost for a distinction the decay already draws.
- **No MacCormack on the deposits, no sub-cell tracking, no particles.** The
  five cells of residual smearing is the price of a grid, and the tools that
  would remove it are a different feature.

## Consequences

- `plough.loom` joins `SCENES` and `GOLDEN` in the same commit as the feature.
  Nothing else in the list can see the field: every other water scene's foam is
  a closed form.
- Any scene with water now pays 0.3–0.5 ms a tick of CPU inside the fixed step.
  The gate's own runtime grows with it — `lanternhead` at `--sim 2400` pays it
  2,400 times.
- The field is readable by `--assert` and by scripts, and that is a promise:
  moving it anywhere the CPU cannot see breaks two commands, not one.
