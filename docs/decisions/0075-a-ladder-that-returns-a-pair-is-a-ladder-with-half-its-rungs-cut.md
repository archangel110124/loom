# ADR 0075 — A ladder that returns a pair is a ladder with half its rungs cut

- **Date:** 2026-08-21
- **Status:** **accepted** and built. Completes ADR 0069's mood ladder for
  weather, and depends on ADR 0072's `WaterBody.fetch`; replaces neither.
- **Sits under:** ADR 0045. The wind half is inside the determinism hash
  (waves push bodies); the rain half is render-side. No GPU float crosses the
  line, no readback was added, and none of the five pinned hash literals in
  `crates/loom_cli/src/main.rs` moved.
- **Adds:** three free functions, one setter, one CLI flag, two gate rows and
  two tests. No pass, image, descriptor, pipeline, barrier or trait.

## The decisions

**1. Weather escalates on the mood ladder's scalar and on no other.** A scene
that wants worse weather further out authors `wind_speed` and `rain_intensity`
on the `Environment.stages` it already authors a look on. There is no `Weather`
component and no second scalar. Weather that escalates with distance and a mood
that escalates with distance are the same shape, and two ladders on one axis
can disagree — silently, and only in the window.

**2. A resolver that returns several things must be consumed as one value.**
`mood_weather_of` returns `(wind_speed, rain_intensity)`. Four call sites wrote
`.0`; the second element was never read anywhere in the workspace. Every path
that reads the weather now goes through **one** function, `weather_at`, which
hands back the wind and the rain together. A caller cannot take half.

**3. The ceiling on a demo's weather is measured on the organ that gives out,
and named.** Not on "the boat completed the trip".

## Why — the field that validated, documented itself, and did nothing

`MoodStage::rain_intensity` shipped inert. It was range-checked at `0..100` by
`loom validate`, described by `loom describe`, documented in the component's
own doc comment beside `wind_speed` — and no code read it.

Measured before the fix on a two-rung ladder eased by a rules script, wind as
the control:

    tick            1     300     600     900
    dread       0.002   0.500   1.000   1.000
    wind         1.64    5.89   10.70    7.97     <- ramps
    rain         6.00    6.00    6.00    6.00     <- the component's own number

The calm rung says `rain_intensity = 0.0` and it rained anyway; the storm rung
says `40.0` and it rained the same. After:

    rain         0.07   20.00   40.00   40.00

**This is the S4 prefab trap in a new place and worse than it.** There, a
parser ignored a key it did not understand, and the instance drew nothing —
loudly, eventually. Here the entire toolchain advertises the key and honours
none of it, so an author has no way to discover the truth except by measuring
the thing they just authored.

**The general rule this pays for:** a test on the blend passes on the broken
code. `the_ladder_moves_the_rain_as_well_as_the_wind` asserts through
`weather_at`, the plumbing, and not through `mood_weather_of`, the arithmetic —
because the arithmetic was correct throughout.

### The second silence, in the same four call sites

`environment_with_mood` substituted the ramped wind for the sea **privately**
and handed nothing back. Every consumer standing beside it kept the file's
wind: `rain_at_eye`, `submerge_eye`, the spray, the telemetry row. So the sea
got up while the rain went on slanting at the berth's angle over it.

Measured, in the picture, on the demo at `dread = 1` with the rain held at
30 mm/h and only the top rung's `wind_speed` changed — gradient anisotropy in
the sky band, `sd(d/dx) / sd(d/dy)`, which is large for a vertical streak:

    wind 3.0    1.419      wind 16.0    0.521

Before the fix both frames got the file's 3.0 and the two numbers were the same
one. `shots/lean_crop.png`: near-vertical against hard-slanted.

Two more consumers were moored in the same berth and are fixed with it.
`deck_of`, which decides *where* it rains (ADR 0016 step 3), read `cloud_cover`
off the raw component, so a ladder could thicken its overcast in the picture
while the term gating the rain stayed at the file's number. And `Sound` read
the rain rate once at construction, so the bed stayed at the berth's silence
through the whole shower.

## What the demo does now

    stage        dread   wind   rain   Hs      at tick   what it is
    inshore       0.00    3.0    0.0   0.17 m        1   a flat calm morning
    underway      0.25    4.5    1.5   0.26 m     ~1660   a breeze, first spits
    outofsight    0.50    7.0    6.0   0.40 m     ~2100   steady rain, chop
    grounds       0.80   11.5   16.0   0.66 m     ~2650   heavy rain, a swell
    theedge       1.00   16.0   30.0   0.91 m     ~3400   a gale and a downpour

`Hs` read off the scene with `loom water --at 40,-10 --sim <tick> --hold
move_z=1`. **That flag is new and its absence is why nobody measured this
before**: `loom water --sim` ran the game with zero input for every tick, so on
a scene whose weather is a function of where the boat is, it reported the
berth's sea however many ticks it was given — the one answer `--sim 0` already
gives. Two previous rounds of this work substituted a closed form for it.

**Rain leads the wind**, which is the causal order the request named. It spits
at the second rung while the wind is still a breeze.

## 4. The ceiling is 16 m/s and it is the man's footing

The rung said 5.0, and the sweep behind that number varied two things. It set
every stage **flat** to one speed and read the long-turn row:

    flat rung   3.0  3.5  4.0  4.5  5.0  5.5  6.0
    full wheel  PASS PASS PASS PASS PASS FAIL FAIL

A flat rung also raises the **berth** wind — and the berth is welded to
`WaterBody.fetch` and to thirty tapes calibrated on a boarding walk that grinds
on that exact sea. The failures were the helmsman reaching the wheel at a
different tick, not the sea taking him off the mat. Raising only the top rung,
berth held at 3.0, the same three rows verbatim from `green.sh`:

    top rung        5.0  8.0 10.0 12.0 16.0 20.0
    turn, 3200     PASS PASS PASS PASS PASS PASS
    wind, 3000     PASS PASS PASS PASS PASS PASS
    steam, 1800    PASS PASS PASS PASS PASS PASS

**Two variables measured as one is not a measurement**, and this project has
now recorded that mistake four times.

There is a real ceiling and the gate cannot see it, because it sits past the
longest tape. Holding W to the far edge of the played water:

    top rung    16      17      18      19      20
    at_helm      1       0       0       0       0    at tick 3400
    boat x     153     148     145     145     127

`at_helm` going to zero is the helmsman sliding out of the mat rectangle **in
the boat's frame** (`on_mat_now`, `deeper_player.rhai`), with the hull inside a
few centimetres of still water throughout. It is not the boat failing. At 16 he
holds the wheel the whole trip and comes off at tick 3600, by which point she
has stopped at x = 158 anyway.

**A second instrument, on another scene, lands in the same place.**
`weather_ramp.loom` is this ladder over the five-station carry rig, and ADR
0060's acceptance is 30 mm peak-to-peak in the boat frame. Its riders swept
over ticks 1800–2100, by top rung:

    Wind.speed   10     12     14     16     18     20
    worst p-p  12.9   19.2   23.6   23.9   33.6   38.5  mm

Monotone, crossing between 16 and 18; net drift never above 19.5 mm of a 50 mm
budget. Two measurements of one organ — a passenger drifting on a heaving deck
— from two directions, agreeing.

## What this does NOT fix, with numbers

**a. Platform carry is upward-only and ratchets — ADR 0060's own defect.** It
is what sets the ceiling above and it is untouched here. Fixing it is a
character-controller change, not a weather change, and it wants its own ADR.
Until then 16 m/s is the cap and any raise is a raise on a passenger who will
slide.

**b. The seven-knot wall — ADR 0063.** `Buoyancy` damps all three axes with the
heave coefficient and thrust is body-frame; at high thrust the hull submarines
rather than accelerating. Not reachable at the demo's rungs — the hull sits
within 12 cm of still water at every one — and untouched. It gets worse with
bigger seas and it needs its own ADR.

**c. There is no white water at any wind speed this engine supports, and that
is half of "the look of the waves".** `FOAM_CREST_BREAK` is 0.45 and
`SPRAY_BREAK` is 0.33, both on realised `mu_max`, and both were calibrated
against `whitecaps.loom`'s hand-authored wave list (4.73% steady-state
coverage). A **wind-derived** sea does not get near them. Measured off the
engine's own `foam.mu_max`, 41×41 grid over 400 m at the demo's fetch:

    Wind.speed    3.0    4.5    5.0    7.0   11.5   16.0   20.0
    Hs (m)      0.166  0.256  0.284  0.398  0.654  0.909  1.137
    mu_max      0.162  0.165  0.181  0.176  0.210  0.183  0.223

`Hs` rises 6.8× and steepness barely moves — it never reaches even the spray
threshold, let alone the foam one. A sixteen-component linear superposition
spreads its energy over frequency and rarely stacks a crest past 0.33. Grid
convergence checked at four densities (11², 21², 41² over 100 m, and 41² over
400 m) at three wind speeds: the number is real, not undersampled. **The thresholds are right for an authored wave list and wrong
for a derived one**, and closing that is a rendering-look decision that moves
the sim hash on every wind-derived water scene. It is the next thing.

**d. The far sea races while the ladder ramps.** `phase = k·(d·x) − ω·t`, and
the wave set is re-derived every tick, so a change in `k` shifts phase by
`δk·x` — proportional to distance from the water origin. Mean per-tick surface
step on the demo, `--hold move_z=1`, ramping window against saturated:

                        x=0    x=100   x=400   x=1000   (mm/tick)
    ladder 3.0..5.0      —       —       —      10.84 / 2.71
    ladder 3.0..16.0    1.22    0.83    4.88   33.75 / 5.64

**This work made it worse** — a four-times-steeper ramp costs three times the
far-field motion at 1 km, and the excess ratio goes 4× → 6×. It is confined to
about a kilometre: at 400 m the ramping window is not measurably busier than
the saturated one, and the demo's top rung fogs the world out at forty metres.
A previous session could not detect it with `loom flicker` even at a thirty
times steeper ramp. **Reported as a measured mechanism with an unproven visual
consequence.** The cure, if it is ever wanted, is to hold `k` fixed and ramp
amplitude alone — which is not the physics, and moves the hash.

**e. `mood_deep` is in `GOLDEN` with no reference PNG.** 55 rows, 54
references. It is the row that exists to catch a broken mood blend, and
`bracket()` has now been refactored twice under no pixel gate. Pre-existing,
blocked on the pending tonemap re-bless, and nothing here blessed anything.

## The gate

Two rows, both fault-injected, sharing runs that already existed:

- the berth is dry (`rain@3,3,-200.rate == 0.0` at tick 60) — fails when the
  calm rung is made wet;
- the offshore run asserts the rain as well as the wind — fails when either
  half of the ladder is deleted, and the rain half is the one nothing in
  `green.sh` could see before.

`the_water_probe_takes_a_tape_and_the_weather_rides_it` asserts the rise rather
than the numbers: `deeper_demo` is not in `GOLDEN` and every rung is free to
retune. What must never come back is the held and unheld answers being equal.

## Alternatives rejected

**A `Weather` component with its own scalar.** Two ladders on one axis, which
can disagree. Rejected on decision 1.

**Letting a mood stage conjure rain in a scene with no `Rain` component.** The
symmetric thing, since `wind_from` does synthesize a default `Wind`. Rejected:
conjured rain arrives with no `duration`, no audio bed and no collision bake —
three quiet failures in place of one loud absence. A scene with no `Rain` stays
dry whatever the ladder says, which is the contract the field already
documented, now pinned by a test.

**Shortening the demo's fetch to make the storm steeper.** `fetch = 3000`
halves the sea at the quay and thirty rows of `green.sh` §6 fail on a re-timed
boarding walk — traced to the exact row in a previous session. The berth's wind
and the fetch are one number in two places and must move together.

**A seventh argument to `Sound::update`.** clippy is right about the arity, and
a field written immediately before the read it feeds has none of the staleness
that made this a bug.

## Addendum 1 — the organ that gave out was the instrument, not the man

**Date:** 2026-09-06. Decision 3 stands; the measurement behind its ceiling does
not, and `green.sh:314` was failing on it.

### What was failing

`scripts/green.sh:314` runs `deeper_demo` to tick 3000 holding W and asserts
`wind@3,3,-200.speed > 8.0` and `rain@3,3,-200.rate > 20.0`. Its own comment
records the table it was written against — tick 3000, dread 0.942, wind 11.372,
rain 25.928. Measured before this addendum: **wind 5.286, rain 0.210, dread
0.487**. Both assertions fail.

`dread` freezes because the boat stops, and the boat stops because `at_helm`
goes 1 → 0 between ticks 1800 and 2400:

    tick        600    1200    1800    2400    3000
    at_helm     1.0     1.0     1.0     0.0     0.0
    dread     0.000   0.075   0.312   0.487   0.487

**This predates the session that found it, and the work in that session made it
better rather than worse.** At `8886281` the helm was already gone *before* tick
1800 and `dread` froze at 0.312; the clumping and cloud work moved the loss from
before 1800 to 2400 and the freeze from 0.312 to 0.487. Both assertions were
already failing on HEAD.

### The rung is not the variable, which is what made this worth chasing

This file's `theedge` note derives a footing ceiling between top rung 16 and 17,
and the obvious reading was that the ceiling had moved. It has not. Halving the
top rung changes nothing at all:

    top rung        16.0        8.0
    at_helm @2400    0.0        0.0
    dread   @2400   0.487      0.487

Identical to three decimals. At tick 1800 the wind is about 5 m/s — the
`underway`/`outofsight` region, nowhere near 16 — so whatever takes him off the
mat is not the sea state the ceiling was measured on.

### The mechanism: a feedback loop over a category error

Instrumented by emitting `stand_local` per tick (temporary; reverted). Against a
`HELM_X_MAX` of −6.10:

    tick     1900    2000    2040    2060    2080    2100    2140
    x      −6.317  −6.344  −6.340  −6.318  −6.158  −5.790  −5.620
    at_helm     1       1       1       1       1       0       0

He stands 0.2 m inside the edge and moves **0.02 m in 140 ticks**. He crosses,
and then moves **0.37 m in the next 20** — walking pace. That is the loop: off
the mat, the held W is no longer the throttle but his legs, which walk him
further out, which keeps him off. By 2400 he is off the boat entirely under
`WALK FORWARD TO THE BOAT`, and `events.station` counts 11 where it should count
about 4 — the same strobe `deeper_player.rhai`'s `ABOARD_LATCH` comment was
written about, one rectangle further in.

**A 90-tick latch on the rectangle was built first and is not enough.** It keeps
the boat — `aboard` is 1 at tick 3600 where it was 0 — but the wheel still goes
by 2400, because underneath the loop the deck is genuinely sliding him out.
Recorded because it was tried and because it is the obvious fix.

The category error is underneath both. **At the helm his legs do not exist** —
W is the throttle, not a step — so the only thing that can move him off the mat
is the boat. Re-testing his footing against the rectangle there is testing for a
bug, not for intent. So the mat is latched until *he* lets go: Space, stepping
off the boat, or going in the water, all of which stay instant.

    tick          2400    3000    3600
    at_helm        1.0     1.0     0.0
    dread        0.617   0.880   0.990

He comes off at 3600, which is what this file's own `theedge` note says happens
at rung 16 — *"he comes off at tick 3600, by which point she has stopped at
x = 158 anyway"*. `green.sh:314` passes on both assertions with its probe point
and thresholds unchanged.

### Two things this does not settle

**Why the deck slides him at all.** `de97324` corrected the hull's mass from
43,776 kg to **57,636 kg** — the old figure was a guessed 38,000 scaled by a
previous hull's volume, and 24% light — and it landed after this ADR derived
its ceiling. A 32% heavier hull is a sufficient mechanism for the rest of the
damage found alongside this, and that half is measured: `rig_drive` makes 21.8 m
astern in 900 ticks where the gate wanted 23, and the demo's return leg stalls
16 m short of home on a tape that allows it 400 ticks. **The slide itself was
not bisected**, so the mass is a named mechanism rather than a proven one for
that specific symptom. Nothing here changes the slide; it stops the slide from
costing him the wheel.

**Decision 3's ceiling is now measured on a different organ.** The 16/17
crossing was read off a footing failure that was partly this loop, so the sweep
behind it should be re-run before that number is quoted again.

### What this repaired in `green.sh`, and what it did not

Collected by running §6 and §7 to completion with `set -e` off, on this change
and on the unmodified script, so the two lists are comparable. **The mat hold
repairs five rows and breaks none**: the offshore wind-and-rain pair, the
`rig_drive` astern row, both halves of the mirrored turn pair — which had been
reporting `at_helm 0`, z −39.882 and x 7.201, a boat that never turned because
nobody was steering it — and one row in the stall block.

Three further rows were stale rather than broken and are re-pinned in place with
their measurements: the wheel answers at tick **191** and not 356 (the walk got
faster; both old rows read 1, so the negative control failed honestly while the
positive one passed for the wrong reason), astern clears **−18.0** and not −20.0,
and the turn windows move from 11 m of swing either side to 15.4 / 16.1 m.

**Ten rows still failed at that point, and every one of them was the hull's
corrected mass arriving somewhere nobody had re-measured.** `de97324` took her
from 43,776 kg to 57,636 kg — the old figure a guessed 38,000 scaled by a
previous hull's volume, and 24% light — and the demo's tapes, thresholds and
teaching text were all calibrated on the light boat. All ten are now closed:

- **The return leg never berthed her.** 400 ticks of astern (2200–2600) left her
  at 0 kn 16 m short, so ALONGSIDE never fired and everything after it read
  BLOCKED. Astern runs to 2850 now; she is alongside at 2800 and against the
  berth from 3000. The walk to the crate is `move_z=-1` alone — the berth moved,
  and the old westward walk put him in the water.
- **Four stale numbers** re-pinned with their measurements beside them: the
  crate 12 m rather than 5, HOME 25 m rather than 19, HOME 115 m at 4 kn rather
  than 122 at 5, and the engine note rising to −1.4 rather than clearing −1.0.
- **`pinned` had gone dead, and that is the one worth reading.** It exists
  because the stall limiter *cannot* see a hull pivoting on a mark: the helmsman
  stands seven metres off her centre, so he keeps walking and `jammed` never
  latches. Its one-knot gate was measured on a hull that pivoted at 0.89 down to
  0.58 kn. The corrected one pivots at 1.508 decaying to 1.036 and never crosses
  it — 1,000 ticks of running into `Mark5` with the caption still reading
  `AHEAD   wheel to port`, which is the first-timer that block was written for,
  told nothing again. The gate is 2 kn now: open sea on the mirrored input is
  4.960 kn, she crosses 2 within about a dozen ticks of the throttle going
  ahead, and astern is excluded by `drive`'s sign rather than by the number.
- **The bait-box caption missed its own zone by a centimetre.** `baffled` is a
  box in the boat's frame; the deck moved under it and the bait box now stops
  him at `plx` −6.79 against an edge of −6.80, so the caption fell through to
  the branch that names neither the obstacle nor the hand. Edge moved to −6.70;
  the helm mat is 38 cm clear of it, and the west edge is untouched because the
  wedge at −9.13 is a different collider and *should* get the unnamed sentence.
- **`rig_trip`'s loop stowed nothing**, because the fish was in his creel rather
  than his hands by the time the old tape reached the box — `stow` cannot reach
  it there. The box comes first now, at tick 1330, and dropping the old zigzag
  is most of why she gets home sooner rather than later.

**A rule the ten of them share.** Every one was a number describing the boat,
written down once, in a place that could not see the boat change: a tick in a
tape, an edge in a zone, a threshold in a detector, a distance in a caption. The
mass commit was careful and right, and it moved all of them at once. Where a
number like that survives, it is worth asking what it is a function of.

One fragility, unrelated and untouched: several rows pipe `loom sim` into
`grep -q`, which exits on first match and SIGPIPEs the writer. Under
`set -o pipefail` that panics the pipeline on output large enough to outlive the
match. It is latent, load-dependent, and it masked the real failures above until
`pipefail` was dropped to see past them.
