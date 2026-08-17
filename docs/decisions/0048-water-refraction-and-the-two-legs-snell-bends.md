# ADR 0048 — Screen-space refraction, both legs bent by Snell, and the acceptance test that did not pass

- **Date:** 2026-08-17
- **Status:** **accepted**, with one acceptance criterion **missed and
  reported** rather than met by a constant.
- **Numbering:** above 0047, for the reason ADR 0045 gives. 0046 stays reserved
  for interactive ripples.
- **Governed by:** ADR 0045. Everything here draws and nothing here is read by
  a force, a script or `loom sim --assert`; buoyancy keeps sampling the CPU
  Gerstner tier through `loom_water::sample_water`, untouched. There is no
  readback and no new state, so the `repeat` gate has nothing new to prove.
- **Decision touched:** none of CLAUDE.md's locked decisions.
- **Supersedes in part:** `docs/design/WATER-REFRACTION-PLAN.md`'s
  `WATER_DOWN_SEC = 1.5`, which is derived here instead of picked, and its
  slice-3 offset construction, which marched the wrong distance.

## What shipped, in three commits

1. **The offset.** `waterBehind` takes the refracted direction, probes along
   it, and reports what it found. No strength knob.
2. **Two defects the offset would have amplified:** the caustic web applied
   twice, and an un-fog clamp of `1e-3` where the plan specified `0.15`.
3. **Snell on both absorption legs, and the body term the crest gate had
   killed.**

Slices 1 and 2 of the plan shipped earlier without the ADR the plan asked for.
This document is that ADR as well, which is why the blend algebra and the
`d >= 1.0` guard are restated here rather than left in a design doc.

## Decision 1 — the probe marches the water column, never the air path

> The offset lookup steps along the **refracted** ray by
> `drop / max(-refracted.y, 0.05)` — the distance that descends the column —
> and never by `length(bg - surfacePos)`.

`bg` was found along the *air* ray. On `shore` that ray is bent some 48° away
from the refracted one and is 35 m long against a column of 6, so marching its
length puts the probe metres inside the seabed and the offset comes out
enormous. One fixed-point refinement: the first probe descends by the drop
under the straight ray, the second by the drop it found. Two steps is where a
fixed point of this shape stops moving visibly.

**A hit in front of the water is attenuated, never rejected.** A hard switch
traces the silhouette of anything crossing the surface as an outline that
breathes with the waves, and `loom.depth_target` is a `SAMPLE_ZERO` resolve, so
there is no coverage information to soften it with. `WATER_REFRACT_ACCEPT`
fades the offset back to the straight UV, which can never be wrong: this
fragment *is* water there. Colour, caustic anchor and path all lerp on the same
`accept`, so the bed is never shaded from one place and measured from another.

**No TIR guard**, deliberately: air→water gives `sinT2 <= 1/1.333² = 0.5628` at
every incidence, so `refract` cannot return zero and a guard would advertise a
case that does not exist.

## Decision 2 — both absorption legs take Snell, and that deletes two constants

> `viewLeg = drop / -refracted.y`, `downLeg = bedDepth ·
> loomWaterSecant(sunDirection().y)`, and `loomWaterSecant` is
> `1/sqrt(1 − sin²θ_air/n²)`.

The old down leg divided by `max(sunDirection().y, 0.15)` — the **air**
elevation. On `shore`'s 20° sun that is 17.6 m of water against the 8.5 m the
light actually crosses: a factor of 2.08 in path, and 22× in surviving red. It
is the same mistake the probe above refuses to make, in the other direction,
and it was in the shipped code because the plan wrote `H/cos θ_down` and the
code read the angle in air.

Snell bounds the secant at **1.5124** for any incidence, because the transmitted
angle cannot exceed the critical 48.6°. Two consequences:

- The plan's hand-picked `WATER_DOWN_SEC = 1.5` was the right number and is now
  derived rather than authored — the diff has one fewer magic constant than
  before it.
- The `max(sunDirection().y, 0.15)` clamp is deleted, not moved. It existed
  because the air secant diverges at the horizon; the water secant does not.

**`WATER_CREST_THICK` collapses into `WATER_CREST_THIN = 0.225`** — `0.9/4.0`
written as the ratio it always was — so a scene with no bed comes out bit for
bit as before while the same crest-thinning applies to a measured column.

## Decision 3 — the wrap lobe gates on the column, not on the crest

> `sss` is gated by `max(crest, saturate(1 − column / WATER_GLOW_COLUMN))`,
> where `column` is the two legs `waterBehind` measured.

This is the plan's escape clause, fired. `0651775` deleted
`body * 6.0 * pow(dot(-view, sunDir), 3)` — wrong in mechanism, and carrying
the entire shallow-water look — and replaced it with a term gated on `crest`.
**Shoaling flattens the swell as it comes onto the shelf**, so `in.fold`, and
with it `crest`, goes to zero across exactly the shallow band where a real sea
glows most. The replacement was gated on the one quantity guaranteed to vanish
where it was needed.

`WATER_GLOW_COLUMN = 6.0` is `shore`'s own shelf: the shallow band glows, the
water past the drop-off does not, and a scene with no bed reports
`WATER_NO_BED` and is left to `crest` alone — which is what keeps `ocean` and
`squall` byte-identical. **It is a gate, not a strength**: how much light
survives the crossing is `exp(-WATER_EXTINCT · thick)` and is not tuned.

A bed depth at the `100.0` sentinel reports **no column at all**. The sentinel
means "no voxel volume here", not "a hundred metres of water", and letting
`ocean`'s submerged posts tell the wrap lobe otherwise switched their glow off.

## The acceptance test, and it did not pass

`WATER-REFRACTION-PLAN.md` sets one hard, hand-measured criterion:
`shore.loom`'s shallow band (`190x55+25+108` at 320x200, `--sim 90`) must reach
**G−R ≥ 38 with no compensating constant**, because it was 38 before `0651775`
and is 14 after. Reproduce it with a shipped command:

```
loom render assets/test/shore.loom --out /tmp/s.png --size 320x200 --sim 90
loom compare /tmp/s.png tests/references/shore.png --rect 25,108,190,55
```

| stage | R | G | B | **G−R** | R/B |
| --- | --- | --- | --- | --- | --- |
| before `0651775` (hand-measured, the target) | 86 | 124 | 134 | **38** | 0.644 |
| after `0651775` (hand-measured) | 68 | 82 | 93 | **14** | 0.737 |
| `0561f36`, this branch's baseline | 66.4 | 92.7 | 111.1 | **26.3** | 0.597 |
| + the offset | 66.4 | 91.7 | 109.9 | **25.2** | 0.605 |
| + the two defects | 66.3 | 91.4 | 109.6 | **25.0** | 0.606 |
| + Snell on both legs | 69.6 | 94.1 | 110.3 | **24.5** | 0.631 |
| + the column-gated body term — **shipped** | 74.6 | 99.9 | 114.8 | **25.3** | **0.649** |

**The hue is restored and the brightness is not.** R/B lands at 0.649 against
the 0.644 the band had before the regression and the NOAA storm-sea reference's
0.62 — by mechanism, and the sweep below shows it is not a knob. G−R lands at
25.3 against 38, and **one unit below the baseline it started from**.

**No formulation of the path reaches 38, and that is measured rather than
argued.** G−R has a *maximum* near the true path length, because `shore`'s bed
is warm sand (albedo 0.52/0.45/0.34) and every metre of path removed lets more
red back up:

| view leg | **G−R** | R/B |
| --- | --- | --- |
| both legs vertical (the shortest physical path) | 23.7 | 0.687 |
| half the refracted path | 24.4 | 0.674 |
| **the refracted path — shipped** | **25.3** | **0.649** |

Where the remaining units are, measured by stubbing one term at a time in the
shipped build:

| stub | **G−R** | R/B |
| --- | --- | --- |
| the shipped frame | 25.3 | 0.649 |
| no Fresnel sky reflection | 31.3 | 0.536 |
| no bed transmission (`behind · T`) | 9.6 | 0.809 |
| no volume term (`WATER_DEEP · downwelling · (1−T)`) | 25.2 | 0.650 |
| no foam | 25.3 | 0.649 |
| no fog | 26.8 | 0.629 |

So the sky reflection costs 6.0 units of G−R and is *what brings the hue to
0.649 from an over-blue 0.536*; the volume term and the foam contribute
nothing measurable in this crop. **The gap to 38 is brightness, not hue**: the
target band is a fifth brighter at the same ratio, and the only remaining lever
is a multiplier the plan forbids.

`WATER_GLOW_COLUMN`, swept, for completeness — it is monotone and saturating,
and 6.0 was chosen as the shelf depth rather than for the metric:

| column | 3.0 | **6.0** | 12.0 | 25.0 |
| --- | --- | --- | --- | --- |
| G−R | 24.4 | **25.3** | 27.3 | 28.3 |
| R/B | 0.647 | **0.649** | 0.650 | 0.650 |

**What we are not doing about it.** Not a multiplier, and not
`WATER_BACKSCATTER`: slice 0 already ran that knob and rejected it — at 10× the
blue reflectance is 0.189 against real open ocean's 2–5%. The honest lever is
**per-`WaterBody` optical parameters**, which the plan lists as the largest
remaining gap to Unreal and as out of scope: it is a schema change, an
`EnvironmentData` field and a scene-format decision. The human picks whether
`shore` gets one, or accepts 25.3.

## Consequences

- **`squall` and `underwater` are byte-identical through all three commits**,
  worst channel 0. That is the check the plan named: every formula here touches
  every water fragment, so a sea with nothing behind it moving by one byte
  would mean the `d >= 1.0` guard is wrong and every other number is suspect.
- `ocean`, `water_crate` and `splash` move by up to 24 — their submerged
  geometry, which is the feature working.
- **`beach` reads paler than it did**, and that is the down-leg correction, not
  the offset: 0.5 m of water over bright sand with a correct 1.4× sun path lets
  much more warm sand back up than a 2.9× one did. Physically right, less
  pretty, and the same thing per-water-body turbidity would answer.
- `loom compare --rect x,y,w,h` reports the mean R, G and B of a crop in both
  images. Every number in this document is reproducible with it; before it,
  the acceptance test could only be checked with a private script.
- Salt metric (pixels above 3× their 8-neighbour median) on `beach` and
  `homestead` at 1280x800: **0.0000% at every commit**. A second `SampleLevel`
  of a shaded image is the firefly class and this one has none.

## What this does not settle

Whether `shore`'s shallow band should be brighter, and by what mechanism.
The plan's fallback — "reinstate an ambient-driven body term" — has been taken
as far as physics allows: the term is back, gated on a measured column instead
of a crest that shoaling zeroes. It recovers 0.8 units of the 1.0 the Snell
correction cost, and neither reaches the 38 the band had when the term was a
`* 6.0` constant over a 2×-too-long absorption path.
