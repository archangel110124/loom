# ADR 0068 — A knob a human turns has two ends, and both are load-bearing

- **Date:** 2026-08-21
- **Status:** **accepted** and built. `CameraSpring` in `crates/loom_cli/src/play.rs`
  is the worked example; the convention is meant to bind the next feel feature
  that ships an environment knob, not only this one.
- **Sits under:** ADR 0045. The camera is render-side, outside `World::state_hash`,
  and none of this is a function of tick. Verified rather than assumed: all 54
  `GOLDEN` rows are byte-identical across a binary with the change and one
  without, and `Play::start` has exactly one non-test caller — the windowed
  viewer at `run.rs:2145` — so `loom render` and `loom sim` cannot reach it.
- **Applies to:** `CameraSpring::MAX_LAG`, `CameraSpring::{WEIGHT,HZ,DAMPING,RESPONSE}_RANGE`,
  `CameraSpring::knob`, and any future feel parameter exposed through the
  environment.

## The request this came out of

> "Research some camera weightiness — something like R.E.P.O. where the movement
> of the camera is a little sluggish, but it still gets to the places where it
> needs to go."

The camera weight itself is not this ADR; it shipped in `40bd02d` as a damped
spring with a feedforward term, and its own reasoning lives in the doc comment
on `CameraSpring`. This ADR is about the two things that turned out to be wrong
with it, which are the same mistake twice: **a taste parameter was left with no
bound at the end nobody was looking at.**

## The two findings

**1. An acceptance criterion measured at one input speed says nothing about the
others.** The flick test bounded overshoot to 2–4° on a 60° flick over twelve
ticks — 300 °/s, roughly a look across a deck. Overshoot in a second-order
system is proportional to the angular velocity it was carrying, and an ordinary
mouse flick is 1000–3000 °/s:

```text
   300 °/s   3.07°     <- the only speed the test saw
  1800 °/s  18.42°
  7200 °/s  47.28°
```

The test's own comment said the upper bound existed because more "reads as a
camera that missed rather than a camera with mass". At the speeds the feature is
actually used at it was six times that, unasserted, and the shipped source
comment on the walk basis quoted "~8°" for a transient that is 4.57° at the
tested speed and unbounded above it.

**2. The knobs that make a feel feature tunable are also the ones that make it
breakable, and the failure mode moves as you fix it.** `LOOM_CAMERA_HZ=12` made
the camera NaN; `=-4` sent the view 2.7e24 degrees round; `LOOM_CAMERA_DAMPING=-1`
diverged. The `hz` cliff is not a constant — semi-implicit Euler at a 60 Hz step
loses stability at 11.41 Hz for ζ = 0.55, 8.04 for ζ = 1 and 4.60 for ζ = 2 — so
the knobs cannot be bounded independently. And the *floor* was worse than the
ceiling in a quieter way: `hz = 0.5, ζ = 0.1` is perfectly stable, converges
exactly, and takes **7.3 seconds** to settle a 60° flick. Stable, correct, and
not a camera.

## Decision

**Three rules. They are about knobs, not about cameras.**

### 1. A feel filter carries a bound on its output, and the bound is a constant

`CameraSpring::MAX_LAG` clamps the picture 6° from the mouse in either
direction, and it is a `const`, deliberately not a fifth environment variable.
A bound is not a taste decision; the moment it becomes one, the range it may be
set to needs a bound of its own and nothing has been gained.

Chosen so the tuned feel is the shipped feel: at 300 °/s the clamped and
unclamped trajectories are **bit-identical**, so the clamp removes only the part
that was never weight. It also *shortens* a fast flick's settle (350 → 283 ms at
1800 °/s) and *reduces* sustained error above 800 °/s, because a bound on the
lag is a bound in both directions. And it sits under the ~13° bucket that
`deeper_player.rhai` quantises its heading caption to, which is the one place a
sprung `forward` reaches gameplay.

The clamp carries a velocity trim at the wall. It was written against a kick
coming off the stop, **and that kick does not exist** — worst error after a hard
stop from 600–7200 °/s is 6.000° with the trim and 6.000° without it, because
bounding the position bounds the error whatever the velocity does. It is kept
for the reason measurement found instead: without it `vel` grows without bound
while pinned and reaches a NaN, and a NaN fails *both* comparisons in a clamp
and walks straight through it. **State the reason a guard survives, not the
reason it was written.**

### 2. Every knob is clamped to a range whose *both* ends are load-bearing

```text
LOOM_CAMERA_HZ        2 – 6      ceiling: stability. floor: usability.
LOOM_CAMERA_DAMPING   0.4 – 1    ceiling: overdamped. floor: it rings for 1.8 s.
LOOM_CAMERA_WEIGHT    0 – 1      0 is off exactly; above 1 doubles the error.
LOOM_CAMERA_RESPONSE  0 – 1      above 1 the view leads the hand.
```

The ceiling is where the numerics break. **The floor is where the feature stops
being the feature**, and it is the end that gets forgotten, because a too-slow
camera is not a crash and not a wrong pixel — it is a correct answer to a
question nobody wanted asked. Every corner of `HZ_RANGE × DAMPING_RANGE` settles
inside 417 ms, and that is asserted, not documented.

**Clamping logs a line.** A tuning knob that quietly disagrees with what you
typed teaches the wrong lesson about the feel, which is the one thing the knob
exists to teach.

### 3. The clamping is separated from the environment read, so it can be tested

`CameraSpring::knob(key, asked, fallback, range)` is a pure function; `from_env`
is the impure shell that supplies `asked`. This is not tidiness. Environment
variables are process-global and the tests run in parallel, so an env-mutating
test flakes against every other test that starts a `Play` — which meant, with
the read and the clamp in one function, that **deleting the clamp passed every
test in the workspace**, on the exact line the whole defect was about. That was
injected as a fault and observed, not reasoned about.

## What this cost, and the rule that came out of it

Three of seven injected faults slipped through the first version of the tests
written for them, and each slip was more informative than the test would have
been:

- A test that the position clamp prevents a velocity kick **cannot fail**, because
  the position clamp prevents it via the position, whatever the velocity is.
- A divergence check (`is_finite`, `< 1e4`) on a filter that *has* a lag clamp
  **cannot fail**, because the clamp converts divergence into chatter: at
  `hz = 12` the view is finite and bounded and jitters 11.6° a tick, settling
  5.6° from where the mouse points. Unusable and stable are not opposites.
- A clamp test that never calls the clamping function cannot fail.

> **A guard makes the test written for it unfalsifiable more often than it makes
> it pass.** Inject the fault before believing the assertion, and when it slips,
> the thing to change is what is asserted — not the threshold.

The tests that replaced them assert the *property*, not the absence of the bug:
the view never leaves the bound **and still lands exactly where the mouse
asked**; every reachable setting **arrives and then holds still**; a knob
**cannot be set outside the box**. All seven faults now fail at least one.

## Consequences

- Anything that clamps a filter's output must assert the endpoint in the same
  test. A bound bought by losing the target is lag wearing a clamp, and it is a
  worse bug than the overshoot it fixes.
- Adding a feel knob means adding a range with a reason at each end, and a test
  that the whole box is usable — not merely that it does not crash.
- `MAX_LAG` at 6° is a *bound*, not a tuning. If the human, in a window, wants
  a longer throw, the number moves deliberately and the tables in the
  `CameraSpring` doc comment move with it.
- **None of this establishes that the camera feels right.** It establishes that
  every setting reachable from the environment is a camera. The question the
  request actually asked can only be answered by a person with a window, and
  the ranges above exist so that person can sweep them without a rebuild and
  without finding a NaN at one end or a seven-second settle at the other.

## Rejected

- **Making `MAX_LAG` a fifth knob.** Bounds that are tunable are not bounds.
- **Scaling `MAX_LAG` with `hz`.** It would defeat the point: the property being
  bought is that the swing is bounded *independently* of how the spring is set.
- **Widening the flick test's bound to whatever a fast flick produces.** That
  documents the behaviour instead of deciding it, which is how 47° would have
  become the specification.
- **Testing `from_env` through the environment.** Correct in isolation, flaky in
  a parallel suite, and the flake would have been blamed on the camera.
