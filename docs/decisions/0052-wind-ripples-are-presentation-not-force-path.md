# ADR 0052 — Wind ripples are presentation, and the force-path grid is not their home

- **Date:** 2026-08-17
- **Status:** **accepted**
- **Extends:** ADR 0046, whose grid this declines to force, and ADR 0045, whose
  clause 1 is why the question needed answering at all.
- **Applies to:** `scene.slang`'s `WATER_DETAIL_CALM` / `WATER_DETAIL_COXMUNK` /
  `WATER_DETAIL_DRIFT` and the per-pixel `grassWind` call beside them; and to
  `loom_water::ripples`, which this ADR is about *not* changing.

## The request

> "add the ripples from the wind where they be integrated with what we have
> right now"

Two readings of "integrated with what we have", and they lead to different
engines:

1. Inject wind forcing into the CPU ripple grid of ADR 0046 — the thing already
   called "ripples" — so wind and impacts share one field.
2. Make the term that *already* paints wind-scale texture on every water
   surface actually read the wind, instead of being a constant.

**Reading 2 shipped.** The rest of this file is why 1 is refused, with the
numbers, so it is not relitigated.

## What was already there, and why reading 2 is the honest "integration"

`scene.slang` carried `WATER_DETAIL_STRENGTH = 0.30`, applied **unconditionally
to every water surface in the repository**, with a floor of `max(wind.z, 0.5)`
under its drift. Cox & Munk (1954) invert an RMS slope of 0.30 to a wind of
about 17 m/s — a near gale. So a wind-detail system already existed, was already
the loudest thing on the surface, and was wind-deaf: `pool.loom`, whose header
says "the pool is dead calm on purpose" and authors `Wind.speed = 0.0`, rendered
covered in chop that crawled across it at 3 cm/s.

The "two systems that don't add up" objection to reading 2 is therefore empty.
There is no second system to add; there is one system that was not listening.

## Why wind forcing does not belong on the ripple grid

### 1. It is unevaluable on the two scenes the wind is judged on

Exactly two scenes in the repository author a `[ripples]` table — `pool.loom`
and `wake.loom` — and **both author `Wind.speed = 0.0` for load-bearing
reasons stated in their own headers**: wake's `Buoy.bob` is a direct measurement
of two-way coupling and means nothing if a swell is also moving the buoy.

The wind-driven seas — `ocean.loom`, `spindrift.loom` — **cannot have a grid at
all.** `MAX_RIPPLE_CELLS` is 256², so at the 0.5 m cell those scenes' waves want
the domain caps at 128 m, against a mesh that reaches the horizon (ADR 0013).
A forced grid there is a 128 m square of ruffled water in an otherwise smooth
sea, with a visible boundary. That is not a feature; it is a bug report waiting
to be filed.

So grid wind forcing would ship with no scene that could show it working and no
scene that could show it failing.

### 2. It is the wrong band by two orders of magnitude

Measured on this tree, with the exact stencil from `RippleGrid::step` reproduced
outside the engine and forced with **per-cell white noise every tick** — the
most favourable wind forcing there is, since it puts equal energy into every
representable wavelength. 128² cells, `cell = 0.5`, `speed = 2.4`,
`damping = 0.997` (all `pool.loom`'s authored values), 3,000 ticks:

| | |
|---|---|
| energy-weighted wavelength | **3.08 m** |
| strongest single bin | 14.3 m |

The grid cannot represent anything under 1.0 m at all (two cells), and what it
actually holds is metres. **Wind ripples are 1–10 cm.** What the human asked to
see is capillary texture; what the grid would give back is a lumpy swell.

This is not a resolution knob. `cell` is quadratic in cost and the mesh has its
own floor — a wave shorter than about 2 m fades out as its wavelength approaches
the vertices sampling it (ADR 0012), so even a 5 cm grid would displace a
surface that cannot draw the result.

### 3. There is no saturating term, and one of the legal settings diverges

ADR 0046's coupling is stable because injection is taken **relative to the
surface's own velocity**: once the water moves with the body, nothing further
goes in. Wind forcing has no such term — the wind does not know what the surface
is doing — so it is a pure energy source on a scheme whose only sink is
`damping`.

`damping = 1.0` is legal today. It is the top of the schema's range and its own
doc comment calls it "a perfectly elastic pond". Under white-noise forcing, same
harness as above:

    rms 0.465 m at 25 s  ->  1.688 m at 50 s, still climbing

An unbounded random walk: a metre and a half of pond standing on nothing, and it
takes every floating body with it. Today that setting is merely eccentric,
because nothing injects energy that is not relative. Forcing would make it a
divergence, and the symptom would be ADR 0046's own lesson — a *lively* buoy,
visible only over tens of seconds.

### 4. It would break the one instrument that proves the coupling is safe

`wake.loom`'s 120 s monotone decay — 0.0339 m at 10 s to 4.5e-8 at 120 s — is
the only evidence in the repository that two-way coupling does not add energy.
Its own header says why it works: "the water is flat, the wind is zero, and the
buoy is six metres from anything that touches it, so a growing number can only
be the coupling." Forcing the grid with wind puts a second energy source inside
the measurement and the instrument stops discriminating.

### 5. W10's calming term would read a forced grid as calm

`ripCalm` suppresses the capillary detail in proportion to the grid's local
slope, which is what made the wake visible at all. A *uniformly* forced grid has
slope everywhere, so the detail would be suppressed everywhere inside the
domain — a square of unnaturally glassy water in a textured sea, which is the
opposite of what forcing was for.

## What shipped instead

`sqrt(WATER_DETAIL_CALM + WATER_DETAIL_COXMUNK · U)` with `U` from a **per-pixel**
`grassWind(worldPos)` — the same generated `wind_at` (ADR 0006) the grass bends
to and the particles drift in, so one wind field serves the meadow and the pond.
Per pixel rather than from `wind.z` because a global scalar can only roughen the
whole sea in unison, and a cat's paw is a *patch*: measured on `water_crate` at
y = 0.2 the field reads 2.51 / 3.49 / 4.67 m/s across 60 m at an authored 7.0.

The drift is global while the amplitude is per pixel. Advecting the noise domain
by a spatially varying velocity folds `xz − drift(xz)·t` once `t·∂drift`
approaches the gust wavelength — a smear appearing only after minutes of `loom
run`, that no still can catch — and the pattern is a metre across while the gust
field is a hundred, so it would buy nothing visible for that risk.

Cost: water pass +0.042 to +0.058 ms at 1920x1080. **No sim hash moves and no
`loom water` reading changes.** 22 non-water references are byte-identical; 14
with a water surface in shot move.

## What this does not settle

- **Shelter.** Grass has the same gap: a pond in the lee of a cliff still
  ruffles, because `grassWind` has no S3 exposure term. Fixing it belongs in
  `loom_field::wind` where both would get it, not in a second occlusion query
  (ADR 0007 is explicit about that).
- **Anisotropy.** Wind chop is stretched crosswind. The tool exists —
  `WATER_FOAM_STRETCH = 6.0` does it for foam — and it is a separate slice with
  its own bless, judged on the fly-through for the direction snapping that P1's
  slew rate already had to fix once.
- **Whether the grid is ever forced.** The reopening trigger is a **bounded**
  scene — a harbour, a pond, a canal reach — that wants wind chop to interact
  with wakes *on the force path*, which is the one thing presentation cannot do.
  The shape it would have to take, so nobody redesigns it:
  `Ripples.wind_strength` defaulting to 0 and an exact no-op at wind 0; forcing
  drawn from the frozen `loom_field::noise` hash and never a crate (ADR 0006);
  a load-time refusal of `damping >= 1.0` whenever `wind_strength > 0`; and
  `ripCalm` fixed first, or §5 above lands on the scene that asked for it.
- **A spectrum sea nobody looks at.** Noted in passing while checking coverage:
  `pool` and `wake` report `waves.source: "wind"` with `count: 0`, and every
  scene with real waves hand-authors them. **No golden scene exercises the
  wind-derived wave path at all.** That is a gap in a different feature and it
  is owed a scene.
