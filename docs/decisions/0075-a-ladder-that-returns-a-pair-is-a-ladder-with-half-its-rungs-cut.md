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
