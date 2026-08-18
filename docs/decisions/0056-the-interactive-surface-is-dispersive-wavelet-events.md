# ADR 0056 — The interactive surface is dispersive wavelet events

- **Date:** 2026-08-18
- **Status:** **proposed** — the force path is the one place this project asks
  for human approval by name, and this changes it. The code that implements it
  is written and measured; merging it is the decision being asked for.
- **Supersedes:** ADR 0046 (the CPU ripple grid) in full. `ripples.rs`, the
  `Ripples` component, `COURANT_LIMIT`, `MAX_RIPPLE_CELLS`, `RIPPLE_EDGE_CELLS`,
  `ripple_side` and the edge sponge are deleted in the commit that lands this.
- **Closes by deletion:** ADR 0051 (ripple injection should be volume
  normalised). There is no cell area anywhere in the new injection: `V` is a
  physical volume in m³ and the pontoon's own waterplane radius is what sets it.
  The defect that ADR was filed against cannot be expressed here.
- **Absorbs:** ADR 0052's refusals, which survive their subject (§8).
- **Applies to:** `loom_water::wavelet`, `sample_water`'s sixth argument,
  `PontoonState::wavelet`, `loom_cli::play`'s shed and impact sites,
  `Renderer::set_wavelets` / `Viewer::set_wavelets`, and `scene.slang`'s
  `waterVertexMain` / `waterFragmentMain`.

## The decision

**A disturbance in the water is an *event* — a point, a time, a displaced
volume and a radius — and the surface it makes is the deep-water
Cauchy–Poisson stationary-phase solution, evaluated in closed form on both the
CPU and the GPU. There is no grid, nothing to author, and nothing to step
except a list.**

```text
tau = t - t0
k*  = g·tau²/4r²          the wavenumber whose group velocity is r/tau
w*  = g·tau/2r            its frequency
th  = g·tau²/4r           the phase there
a   = V·g·tau² / (4·sqrt(2)·pi·r³)
W_s = exp(-k*²·sigma²/4)                          the source's own size
W_n = smoothstep(2·CELL, 4·CELL, 2·pi/k*)         Nyquist, CELL = 0.5 m
D   = exp(-tau/4 s)                               everything else
h      = -a·W_s·W_n·D·cos(th)
dh/dr  = -a·W_s·W_n·D·k*·sin(th)
u_e    = w*·h·rhat
```

Cut inside `r_min = tau·sqrt(g·sigma)/4` and outside the radius where the
envelope falls under 1 mm. Both cuts are written without a square root or a
cube root on the right-hand side, so the two halves compare the same numbers:
`16r² < tau²·g·sigma`, and `a < H_MIN`.

## Why wavelets *now*, when a GPU solver has just been made legal

ADR 0053 removed the constraint that made a CPU closed form the only option, so
the choice has to be re-argued rather than inherited. It survives, and not
narrowly:

- **The deterministic tier is every scene's default.** ADR 0053 buys an opt-in
  look upgrade for a `WaterBody` that asks for it. It does not buy a new default,
  and 44 of the 46 golden scenes are outside the tier.
- **`loom sim` runs with no GPU device at all.** The interactive surface is on
  the force path: buoyancy, drag, `--assert` and `rhai` all read it. A headless
  determinism run on a machine with no device must still get an answer.
- **Pinned hashes and replays are cross-machine.** ADR 0053 §3 is explicit that
  a GPU result is reproducible on *this* device, driver and dispatch order and
  nowhere else. The four pinned water hashes, `proving_ground`'s event log and
  any future networked play all need the default tier to stay portable.
- **Cost.** A device sync per tick is the price ADR 0053 accepts for cinematic
  water. Here the whole interactive surface is a loop over at most 128 records:
  measured below at under 10 µs a tick, and zero when nothing has happened.

The two coexist. A `WaterBody` is deterministic or cinematic, never both, and
this ADR is entirely about the first.

## Why this shape rather than the alternatives

### It is dispersive, and that is not a detail

`u_tt = c²∇²u` — ADR 0046's stencil — carries every wavelength at one speed.
The consequence is visible and wrong: its wake is a **Mach cone** whose
half-angle is `arcsin(c/U)` and which *narrows as the body speeds up*. Deep
water does not do that. The Kelvin wedge is 19.47° at every speed, because the
faster body makes longer waves whose group velocity keeps up with it exactly.

That is one test, and it discriminates every wave model surveyed for this
work. Measured through the shipped `at()` and the shipped shed construction:

| | half-angle |
|---|---|
| 3 m/s | **26.0°** |
| 8 m/s | **17.0°** |
| Kelvin | 19.47° |
| a Mach cone through the 3 m/s reading | **9.9°** at 8 m/s |

The pattern narrows by a factor of 1.5 where a non-dispersive medium narrows by
2.7. It is emergent, not drawn: nothing in the code contains 19.47° or any
angle at all.

**It does not meet the ±2–3° the panel's acceptance asked for, and the reason
is measured** (`the_kelvin_wedge_does_not_narrow_with_speed` carries this):
the 1 mm amplitude floor truncates exactly the old, wide rings the fast wake's
wedge needs. Lifting the floor offline moves the 8 m/s reading to 19.0° and
leaves 3 m/s at 22.8°. What is asserted in the test is what was measured, with
the Mach comparison carrying the discrimination.

### The compact convolution kernel (iWave), and a claim not reproduced

The alternative that was on file was Tessendorf's iWave: a linear surface with
the `|k|` vertical-derivative operator applied as a small convolution kernel,
which gives dispersion on a *grid* at radius-P cost. The panel's brief records a
measured truncation table of `c/c_exact` between **0.41 and 0.80** across
λ = 2–32 m at P = 6, and concludes the compact kernel is permanently dead.

**Re-measured here, that table does not reproduce.** The symbol of a radius-6
stencil at a 0.5 m cell, isotropically averaged over nine directions, against
the exact `sqrt(g/k)`:

| λ | truncated inverse transform | least-squares stencil |
|---|---|---|
| 2 m | 0.999 | 1.000 |
| 4 m | 1.004 | 0.999 |
| 8 m | 0.978 | 0.998 |
| 16 m | 1.046 | 0.999 |
| 32 m | **1.312** | 1.017 |

So a rectangular truncation is wrong by 31% at 32 m — in the *fast* direction,
not the slow one — and a least-squares stencil over the same band is wrong by
under 2%, though its coefficients are large and its conditioning was not
studied. Whatever instrument produced 0.41–0.80, it was not this one.

**The claim is therefore not carried into this ADR as fact, and the decision
does not rest on it.** iWave is declined for reasons that do not need a
truncation table: it is a *grid*, so it reintroduces `extent` and an anchor and
a boundary, it costs `side²` per tick whether or not anything has happened, and
it cannot be evaluated at a point the way a pontoon and an assertion want. If
anyone reopens it, re-measure rather than quoting either table.

### The domain-anchoring trap is vacuous here

ADR 0045's trap clause — a force-producing sim grid must anchor to sim state,
never to the camera — was the single easiest way to get ADR 0046 wrong, and it
has no purchase on this design. **There is no domain.** An event is a point in
the world with a time on it; the surface is a function of `(x, z, t)` and the
list. There is no origin to choose, so there is no wrong origin to choose.

## The anti-feedback hole, proved rather than believed

`r_min = tau·sqrt(g·sigma)/4` is the radius at which `k*·sigma = 4`, so
`W_s = e⁻⁴` and the packet is already down to 1.8%. Inside it the `1/r³` in the
amplitude runs away against a weight that has already killed the term, and —
this is the point — a body that has just shed a packet sits exactly there.

`a_floater_never_reads_its_own_packet` sheds at the origin every four ticks for
ten seconds and asserts the height at the origin never reaches a millimetre.

**One feedback path did survive the hole, and `wake.loom` caught it.** Shedding
was gated on a hull's *full* speed. A body floating at rest reports 0.103 m/s
from `velocity_at_point`, because the fixed step applies gravity before the
buoyancy force cancels it — so every floating body in the repository shed a
packet every four ticks forever, and `wake.loom`'s 120-second decay measurement
floored at 1e-5 m and *rose*: 9.63e-6 at 30 s, 9.68e-6 at 60 s, 1.02e-5 at
120 s. Gating on **horizontal** speed — which is what Havelock's construction
actually is — takes it to exactly zero. See §"What was measured".

That measurement is the reason ADR 0052 §4 protected it and the reason this ADR
re-ran it before and after rather than assuming it.

## The event cap, and what it was chosen by

The GPU evaluates the pool per vertex, so the cap is the shader's loop bound. It
was measured before it was chosen: a dummy per-event loop in `waterVertexMain`
on `ocean.loom`, the largest water mesh in the repository, at 1920x1080, water
pass, three runs each, first-run outliers discarded:

```
  0 events  0.564 ms       64 events  0.620 ms
128 events  0.652 ms      256 events  0.722 ms
```

About **0.7 µs an event** on a horizon-filling sea. **128 shipped**: +0.088 ms
on the worst case in the repository, half a percent of a 16.7 ms frame, paid
only while the pool is full, and a 4 KB upload against the 256 KB the ripple
grid needed. On `wake.loom`'s mesh the same 128-event probe cost +0.070 ms.

**The limitation this puts on the record**: at `SHED_TICKS = 4` a moving hull
sheds 15 packets a second, so a full pool is **8.5 s of wake** — a little over
two decay constants, by which point the oldest packet is at 13%. A wake longer
than that is not available at any price short of a bigger loop.

## What was measured

**The 120-second decay, `wake.loom`, `Harbour/Buoy.bob` over the last 300 ticks
of progressively longer runs.** Re-run before and after, never assumed:

| | 10 s | 30 s | 60 s | 120 s |
|---|---|---|---|---|
| ADR 0046's grid (before) | 0.0339 | 0.00563 | 0.000145 | 4.47e-8 |
| **wavelet events (after)** | **0.0375** | **0.0** | **0.0** | **0.0** |
| no coupling at all | 3.5e-6 | 0.0 | 0.0 | 0.0 |

Monotone, and to zero rather than to a floor: an explicit stencil rings in its
own domain forever, and there is no field here left to ring.

**Dispersion.** `the_zero_crossings_are_where_the_phase_says` sweeps `r` over
1–20 m at three ages and asserts every sign change lands on `th = (n+½)π`
within 2%.

**The slope is the height's derivative.** The design note this was built from
gave `dh/dr = +a·W·k*·sin(th)`, which is the derivative of `+a·cos(th)` — one
of its two lines had a sign slip, and a slope that disagrees with its own height
tilts the surface the wrong way and lights it wrong.
`the_slope_is_the_derivative_of_the_height` finite-differences the shipped
`at()` and would fail on either error. The envelope term `A'(r)` is dropped, as
the note asked: it varies over a packet rather than over a wavelength, and what
is left agrees with the finite difference to 0.02.

**CPU and GPU agree exactly.** `wavelet agreement: worst absolute difference
0e0 over 512 samples (5 values each, 325 inside a packet)` — five values,
because the orbital velocity reaches drag and a side that dropped it would
otherwise agree for free.

**The closed form against the exact Cauchy–Poisson integral.** The amplitude
constant is derived rather than taken: stationary phase on
`(V/2π)∫k·J0(kr)·exp(-k²σ²/4)·cos(sqrt(gk)t)dk` gives `4·sqrt(2)·π·r³` in the
denominator, and at V = 0.262 m³ (the hemisphere a 0.5 m sphere displaces),
r = 3 m, τ = 2 s it reads **1.97 cm undamped** against the panel's worked
example of 2.3 cm, which is reproduced exactly at V = 0.30 m³. The panel's V is
not recorded; the band is the same. Read as `4·sqrt(2π)` instead — the other
parse of the brief's `4√2πr³` — the same expression is 1.77x too large.

Against that integral evaluated numerically over r ∈ [1.5, 10] m, the closed
form's **best-fit relative RMS error is 0.44 at τ = 1 s, 0.12 at τ = 2 s and
0.17 at τ = 4 s**, and the fitted phase offset is zero within 0.08π at every
age. That is the expected behaviour of an asymptotic: `th = g·tau²/4r` is the
large parameter, and at τ = 1 s and r = 10 m it is 0.25, where stationary phase
has no business being accurate. It is accurate where the rings are.

**The sign is a statement about the source, and it is worth being explicit.**
The fit prefers `+a·cos(th)` — an initial *elevation* of volume V. What ships is
`-a·cos(th)`, which is the same solution for a **cavity**: an impact craters the
surface and a hull pushes water aside, so the disturbance starts as a hole and
the leading long wave is a depression. Reading the shipped sign as an error
against the fit is the mistake this paragraph exists to prevent; the two differ
by the sign of `V` and nothing else.

**Ring counts depend on the source radius, which is why the panel's table could
not be reproduced directly.** Zero crossings inside `[r_min, r_max]` at
τ = 0.5/1/2/4 s: **1/1/3/5** at σ = 0.5 m, and **1/2/5/9** — the panel's own
numbers — at σ = 0.175 m. Same model, their source radius.

**Cost, `loom sim wake.loom --ticks 1800`, wall clock minus the 60-tick
baseline:** 322 µs/tick after against 339 µs/tick before, which is noise. The
per-tick budget in the brief was 250 µs and **the scene was already over it
before this work**: the foam field's 16,384-cell advection (ADR 0055) is what
that time is, not the water surface. The wavelet pool's own contribution is
under 10 µs a tick and is zero when the pool is empty.

## What this does not do, and the trigger for each

- **No obstruction and no reflection.** A packet passes through a quay, a hull
  and an island as if they were not there. The closed form has no way to know
  about them: an event knows a point and a time, and geometry is not in the
  expression.

  **The trigger to reopen** is a scene that authors waves which must visibly
  reflect off a quay or a hull — a harbour, a lock, a canal reach. The design
  that would be built then is on file and is *not* this one: a **band bank**
  (a small set of retained per-wavelength fields, each stepped by its own
  dispersion relation) plus an **obstruction mask** baked from the collision
  world, on a retained grid; with `damping < 1.0` refused at load whenever
  anything forces it (ADR 0052 §3's divergence, which a forced retained grid
  reintroduces) and with ADR 0051's volume normalisation applied to the
  injection, since a grid brings the cell area back with it. That is a slice of
  its own and it needs its own ADR.

- **No foam from the events' orbital velocity.** `FoamField::velocity_at` does
  *not* sum `u_e`, and the reason is arithmetic before it is aesthetic: it is
  `side²` event sums a tick — 16,384 cells against a 128-event pool is two
  million evaluations inside the fixed step, against a whole-tick budget of a
  quarter of a millisecond. And an orbital velocity is a circle: its mean over a
  cycle is zero, so what it would buy is foam trembling in place, while the
  transport foam actually rides is the Stokes drift already in the field.
  `deposit_ripples` is deleted with the grid it walked; the hull and impact
  deposits are where a wake's foam is made, and they remember.

- **No wavelets on the foam trail's past taps.** This is now a cost decision
  rather than an impossibility, and that is a real gain worth recording: the
  grid had no past to ask about, while every packet here is a closed form in
  `(r, tau)` and `t - tau` is exactly as cheap as `t`. It is FOAM_TRAIL_TAPS more
  pool sums a vertex, for foam the deposits already lay.

- **Rain is excluded by arithmetic, not by policy.** A 3 mm drop displaces about
  1.4e-8 m³; at a metre and a second that is a ring 5e-9 m high. The slice-3
  rain-ring lattice is rain's renderer and stays.

- **A plunging cascade sheds nothing.** `loom_water::nappe` knows the discharge
  and the impact point, so the wiring is short, but the volume per shed interval
  at a canyon fall's discharge is two orders of magnitude above anything
  measured here and it would be a knob nobody has looked at. Left unwired
  deliberately.

## Consequences

**All four pinned water hashes move**, including `river` and `water_crate`,
which never authored a `[ripples]` table: every floating body now sheds into and
reads back from the pool, so the force path changed for all of them. They are
re-pinned in the same commit, which is the rule the wind hash already follows.

**Seven golden references move** — `pool`, `pool_jet`, `wake`, `water_crate`,
`splash`, `river`, `plough` — and 38 of 46 are byte-identical. Every scene that
moves has a `Buoyancy` body in it, which is the whole of what changed.

**Five authored numbers are gone from the schema and nothing replaces them.**
`extent`, `cell`, `speed`, `damping`, `strength`. A file that still carries the
table is refused at parse rather than ignored, which is the S4 lesson: a key the
parser does not understand is a key it drops silently.
