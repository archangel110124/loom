# ADR 0046 — Interactive ripples are CPU-authoritative, and anchored to the water

- **Date:** 2026-08-17
- **Status:** **accepted**
- **Supersedes / extends:** nothing. It is the ADR ADR 0045 clause 1 said this
  work would need, because it puts new state on the force path.
- **Applies to:** `loom_water::ripples`, `WaterBody::ripples`,
  `PontoonState::ripple`, `sample_water`'s sixth argument, the two load-time
  refusals in `loom_scene::scene::check_ripples`, and the one-direction upload
  `Renderer::set_ripples` / `Viewer::set_ripples`.
- **Amended:** 2026-08-17, §6 and the first bullet of "what this does not
  settle". The upload was written as a separate slice and had already landed
  when this was first filed; the ADR said it had not, and it did not ratify the
  deviation from the plan's `R16F` image. Both are corrected below.

## Context

Everything in `loom_water` up to now is a closed form in `(x, z, t)`. That is
what makes buoyancy trustworthy: a pontoon can ask for the surface anywhere with
no history, an assertion can name a coordinate the renderer never drew, and the
Slang half is the same function transcribed once.

Interactive ripples cannot be that. A wake is a *record* of what happened, so it
is state, and it is state that feeds a force. ADR 0045 clause 1 admits the shape
by name — "state advanced inside the fixed step, under never-do #7 and #8, is
exactly as reproducible as `rapier3d`, which is itself stepped state" — and its
trap clause names the way to get it wrong.

## Decision

**A `WaterBody` may author one `Ripples` grid: an explicit five-point 2D wave
equation, stepped on the CPU inside the fixed step, whose height is added to the
Gerstner surface for physics, gameplay and rendering alike.**

```text
u' = 2u − u_prev + C² (u_l + u_r + u_u + u_d − 4u),   C = c·dt/h
```

Five properties are what make it admissible, and each is structural rather than
a convention somebody has to remember.

### 1. The domain is anchored to the water node, never the camera

`RippleGrid::new` takes a world centre — the `WaterBody` node's own position —
and keeps it for the run. `Sim` cannot see a camera at all, which is the
structural half.

This is ADR 0045's trap clause and the single easiest way to get W6 wrong.
`VFX-IMPLEMENTATION-REPORT.md` §2.2a prescribes "centre the grid on the camera;
when it scrolls, reproject by an integer-cell shift", which is right for a
*rendering* ripple texture and forbidden here: the buoyant force on a crate
would become a function of where the viewer was standing, so `loom render
--sim N` and `loom run` would compute different physics and the determinism hash
would go viewer-dependent.

The cost of a fixed domain is that ripples exist inside the authored square and
nowhere else. That is stated in the schema rather than hidden, and it is what a
harbour, a pond or a river reach wants anyway. **A moving domain is a separate
decision and needs an answer to what "anchored to sim state" means when the sim
state moves** — tracking the bodies makes the domain jump when one sinks.

### 2. Two-way coupling goes *through* `sample_water`, not around it

`sample_water` gains a sixth argument, `ripple: (height, ∂h/∂x, ∂h/∂z)`,
pre-sampled by the caller for exactly the reason `ground_height` and `flow` are:
the function has to stay a function of its arguments, or the Slang half stops
being the same function. `PontoonState::ripple` carries it per pontoon rather
than per body, and that is not tidiness — a ripple crest passing under one end
of a hull and not the other is a *torque*, which is the thing this feature
exists to produce.

The alternative considered and rejected was adding the ripple height inside
`buoyancy::solve` only. It is one line shorter and it creates a second opinion
about where the surface is: `submersion_at` — which the audio listener and the
eye-underwater flag both read — would disagree with the solver. This crate's
header exists to prevent exactly that.

The slang agreement test now hands both halves a **non-zero** ripple, so a side
that dropped the term fails rather than agreeing by luck.

### 3. The Courant condition is refused at load, with the numbers

The explicit scheme is unconditionally unstable above `c·dt/h ≤ 1/√2`.
`check_ripples` refuses the file and reports the authored speed, the computed
`c·dt/h`, the limit, the timestep, the cell size, and the maximum speed the
authored cell size allows:

```
"constraint": "at most 21.213 m/s: c*dt/h = 1.000 exceeds the 2D limit
               1/sqrt(2) = 0.707 at dt = 0.016666668 s and h = 0.5 m",
"error": "ripple_speed_exceeds_courant",
"hint": "the explicit wave stencil diverges above the Courant limit — the field
         reaches infinity within a second and takes every floating body with
         it. Lower `speed` or widen `cell`."
```

A comment would have been worthless: an agent asked to make the ripples livelier
will raise `speed`, and the symptom is not sluggish water but every buoyant body
leaving the scene.

`RippleGrid::new` clamps as well, for the same reason `sample_water` does
`take(MAX_WAVES)` rather than trusting its caller — a component built in code
never went through the parser. A second refusal caps the grid at 65,536 cells,
because it is stepped on the CPU beside `rapier` and a cell count is a cost an
agent has no feel for.

### 4. The CPU grid is authoritative and nothing is read back

ADR 0045 clause 2, enforced by the dependency rules rather than by review:
`loom_water` is a crate that cannot import `ash`. The GPU gets a copy to
displace the surface with and never writes one back.

### 5. A scene without ripples is bit-identical

`WaterBody::ripples` is `Option` and absent by default. The grid is not built,
`Sim::float` does what it did before, and every pontoon's `ripple` is `[0.0; 3]`
— which `sample_water` adds to a height and two slopes, so it is an exact
no-op. **Both determinism hashes are unmoved.** A scene that *gains* ripples
moves its own hash, and that re-pin belongs in the commit that gains them.

### 6. The copy the GPU gets is a float buffer at a device address, not `R16F`

`VFX-IMPLEMENTATION-REPORT.md` §2.2a and the work order both say the upload is
an `R16F` texture. It is a `float` buffer reached by buffer device address, the
same shape `terrain_heights` beside it already had, and this ratifies that
rather than leaving it as something the next reader discovers.

An image would need a layout, a transition owned by the render graph, a sampler,
a descriptor and a second copy of all of it in `Viewer`. A buffer needs a
`float*` in the environment block and one `write_slice`. The shader wants
*bilinear over four taps it fetches itself* — `loom_ripple_at` returns a height
and two slopes and cannot take a hardware bilinear tap for the slopes anyway —
so the one thing a sampled image would have bought is not bought. The `f32`
costs 256 KB against 128 KB at the cap; that is the whole price.

The two halves are named separately because they are separate risks: the
buffer's contents are a *copy* and the CPU grid stays authoritative (§4), while
the buffer's *format* is an implementation detail this paragraph now owns.

### 7. The upload happens wherever the simulation is stepped

Every path that steps `Sim` hands the grid to the surface: `loom render`'s
still and its fly-through, and `loom run`'s window. The grid is the simulation's
and the renderer is only shown it, so a path that forgets the call draws flat
water over a wake that is nevertheless *felt* — which is the silent-no-op class
this repository keeps finding. `wake.loom` is in `GOLDEN` for that reason and
not because the picture is interesting.

## The two failures this cost, both of which looked like a lively buoy

Recorded at length because the symptom in each case was not "unstable" and no
still image, and no short run, could have caught either.

**A body that reads the ripple it just made is a self-excited oscillator.** The
dent under a floating body lowers its own buoyancy, so it falls further, so the
dent deepens. On `wake.loom` the buoy reached a **6.85 m** limit cycle. It
*saturated* rather than diverging, which is worse: a limit cycle reads as an
energetic float. Cutting the coupling constant twelve-fold only lowered the
plateau to 1.15 m, which is the tell that tuning was never going to fix it. The
cure is to inject the body's velocity **relative to the surface** — once the
water is already moving with the body nothing further goes in, which is also
what the physics says, since a body makes waves by moving *through* water.

**Adding `Δ` to `now` is not a displacement, it is an impulse.** The scheme
infers velocity from `now − prev`, so the surface acquires `Δ/dt`. At 60 Hz that
made a coupling constant of 0.06 a **3.6× velocity amplifier**, and the relative
fix alone still ended in a `NaN` inside a minute of simulated time. Multiplying
the injection by the timestep is what makes `strength` mean what its
documentation claims: the dimensionless fraction of a body's relative motion the
water takes each tick, stable by construction below 1.

With both, `wake.loom`'s buoy decays monotonically — 0.0339 m at 10 s, 0.0056 at
30 s, 1.5e-4 at 60 s, 4.5e-8 at 120 s — against 3.5e-6 with the grid removed.

**The general lesson is the one this project keeps paying for.** A two-way
coupling that adds energy is the failure mode, and its signature is only visible
over tens of seconds. `wake.loom` is authored so that `bob` measures *nothing
else*: the water is flat, the wind is zero, and the buoy is six metres from
anything that touches it, so a growing number can only be the coupling.

## Cost

Measured over 18,000 ticks in release, against the same scene with the table
removed:

| grid | cells | per tick | of a 16.7 ms frame |
|---|---|---|---|
| `wake.loom` (24 m at 0.5 m) | 2,401 | **7.2 µs** | 0.04% |
| at the cap (127 m at 0.5 m) | 65,025 | **167 µs** | 1.0% |

Linear in the cell count, as the stencil says it must be. This is CPU time
inside the fixed step, so it is spent whether or not a frame is drawn — which
is the difference between it and every other number in the VFX overhaul.

## What this does not settle

- ~~**Rendering.** The GPU upload is a separate slice.~~ **Stale, corrected
  2026-08-17.** This was written believing the upload had not landed; the
  shader half and `Renderer::set_ripples` were already in the tree, uncalled,
  and the missing piece was one caller. It is now wired on all three paths —
  see §6 and §7. The ordering the paragraph described was still the right one:
  the force path is the part that needed an ADR, and the picture followed it.
- **A bow wave.** The coupling is velocity-only, so a body moving horizontally
  at constant depth injects nothing. Entries, bobbing and rocking all work,
  which is what two-way coupling needed to demonstrate. Displacement-driven
  injection closes the same feedback loop this ADR is about and would need the
  same relative-velocity treatment plus its own measurement.
- **More than one grid per scene.** One `WaterBody`, one grid. A scene with a
  pond and a harbour is a second `WaterBody`, which the engine does not have.
