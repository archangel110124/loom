# ADR 0061 — One jittered reflection ray, shared across the quad

- **Date:** 2026-08-19
- **Status:** **proposed** — needs human approval before merge.
- **Extends:** ADR 0019 (secondary rays). It adds no pipeline, no pass and no
  barrier, and it does not change the ray count. It applies to the cinematic
  tier's marched free surface and to nothing else.

## Context

A human standing at the side of `plough_cinematic.loom`'s tank reported "a
reflection at a weird angle — a different reflection, in a different direction
from the rest of the pool", with hard straight edges, visible from a low camera
near the ramp and invisible from the scene's authored top-down one.

The mechanism was already written down, in `WATER_CINEMATIC_REFLECT_ROUGH`'s
own docstring. `fluid_surface::march` samples a 10 cm lattice and then runs two
Jacobi sweeps over the fraction field, so the mesh that reaches the shader is
smooth below about 30 cm. Real water is never smooth at that scale, and the
reflection is the one term that notices: neighbouring pixels send their single
mirror ray at the *same* triangle of the *same* box, a whole region agrees on
one flat-shaded colour, and the boundary of that region is that box's
silhouette. Fresnel is why it is only visible at grazing incidence.

That docstring also named the fix and deferred it:

> The honest fix is a wider lobe — more rays (ADR 0019's occupancy cliff says
> no) or a screen-locked jitter of the ray inside the lobe, which is the trick
> AO and the soft shadows already use here and which would break the
> *coherence* rather than paint over it. That is the upgrade path, and it wants
> a salt measurement before it lands.

This is that upgrade, and the salt measurement now exists as `loom salt`.

## Decision

**Jitter the one mirror ray inside a 4° lobe from a screen-locked dither, and
average the result across the 2×2 quad.**

- The lobe is `WATER_CINEMATIC_LOBE = tan(4°) = 0.0699`, sampled uniformly over
  the disc with `sqrt(ξ₁)` — the reason `sunVisibility` gives.
- The dither is `rayDither(in.clip.xy + float2(19, 7))`, R2-rotated by
  `raySample`. The offset exists because `ambientVisibility` offsets by
  `(37, 11)`: two consumers of one dither correlate, and ADR 0019 paid for that
  lesson already.
- The rotated ray is re-clamped into the surface's own hemisphere, the same way
  the unjittered one is.
- The result is averaged with `QuadReadAcrossX`, `QuadReadAcrossY` and
  `QuadReadAcrossDiagonal`.
- All of it is inside the existing `in.nappe.x < -0.5` lane, so nothing outside
  the cinematic tier's marched surface is touched.

**`WATER_CINEMATIC_REFLECT_ROUGH` stays at 0.55.** The two are not
alternatives: the dilution is the variance reduction the jitter is affordable
on top of. Its docstring's "the honest fix is a wider lobe" paragraph is
rewritten to say so.

**The quad share is not optional.** Unshared, the jitter trades a coherent
polygon for per-pixel stipple, which is a worse artifact and a much noisier
frame. Four samples for the price of one is the whole reason the design is
affordable, and the quad is already resident and already shading.

## Consequences

**A new hardware requirement.** Nothing else in this engine uses a subgroup
operation. `select_physical_device` now rejects a device whose
`VkPhysicalDeviceSubgroupProperties` lacks `SUBGROUP_FEATURE_QUAD` or lacks
`FRAGMENT` in `supportedStages`, naming the device — the same shape as every
other missing-feature rejection, rather than a `vkCreateShaderModule` failure
carrying a capability number. Both halves are checked: some drivers advertise
subgroup operations for compute alone. Fault-injected to prove it reads real
data: inverted, it rejects this RTX 4090 and the llvmpipe fallback by name.

**Determinism is unaffected**, and this is the clause that matters most. Every
jitter input is `in.clip.xy` — integer pixel coordinates. No clock, no frame
counter, no accumulation; the frame stays a function of its tick, which is what
ADR 0010 refused TAA to protect. `QuadReadAcross*` reads lanes of a 2×2 fixed
by the pixel grid rather than by scheduling. The branch is quad-uniform by
construction: `nappe.x` is a per-vertex constant of −1.0 across the whole fluid
mesh, so every lane of a quad on this surface takes the same path, helper lanes
included. Verified as three fresh processes rendering byte-identical PNGs at
the gate ticks and at `--sim 3000`.

**Cost is ~0.003 ms.** Measured on `plough_cinematic_low --sim 3000` at
1920×1080 with `LOOM_GPU_TIMING=1`, on a machine carrying another session's
build: water pass 0.155 → 0.158 ms; the tick cost is unmoved at 3.80 → 3.83
ms/tick. The ray count did not change, so this is the arithmetic and the three
quad reads and nothing else.

**Salt is what licensed it, and salt is a ceiling rather than a verdict.**
Against a build whose cinematic reflection is stubbed out to plain sky, at
960×600, `plough_cinematic_low --sim 3000`:

```text
    before this ADR      salt added  +30
    after                salt added  −19
```

So the jittered, quad-shared reflection is *quieter* than the coherent one it
replaces. The bar set in advance was ≤ +60; the number that would have refused
the design is an unshared jitter, which is where the fifty-times figure comes
from.

**But salt cannot see the defect this fixes**, and that has to be said plainly.
A flat straight-edged patch is a hundred neighbouring pixels agreeing, which is
the opposite of an isolated pixel; salt prices the *cure* and is nearly blind
to the disease. The picture is the verdict, and the acceptance therefore also
required the frame to move: `loom compare` reports 4.20% of pixels differing at
tick 3000 and 3.32% at 250, worst channel 52 and 42 — under the gate's own 72,
which is the second clause.

**Two traps, recorded so they are not re-derived.**

1. **The hemisphere clamp re-correlates at wide lobes.** A sample rotated below
   the surface is clamped back into the plane, so past roughly 0.25 tan the
   clamped samples pile onto the horizon and agree again. At 4° almost nothing
   is clamped. If the lobe is ever widened materially, the clamp has to become
   a rejection.
2. **A low camera is a precondition for judging any of this.** From the
   authored top-down camera Fresnel is about 0.1 and the term is invisible
   whatever it says. `plough_cinematic_low.loom` exists for that, and the whole
   defect survived five green gates because no gate had one.

## Rejected

- **More rays.** ~~ADR 0019's occupancy cliff: 0.024 ms each to eight and 0.101
  ms each after.~~ **That cliff has been retracted — ADR 0074, and the
  correction is in ADR 0019 beside the original table.** Rays are linear. The
  rejection stands on the measurement that actually chose this design and is two
  bullets down: an unshared jitter is ~50× the added salt, and the share buys
  the variance reduction for one ray's traversal. What is now *false* is that
  more rays would have been unaffordable — four unshared rays here would cost
  about four rays, and would still be the wrong shape, because the defect is
  coherence between neighbouring pixels and not variance within one.
- **Dropping `WATER_CINEMATIC_REFLECT_ROUGH` toward the analytic sea's 0.06.**
  Measured with the jitter present: the flat patches return and the added salt
  goes up by an order of magnitude.
- **An unshared jitter.** Per-pixel stipple, ~50× the added salt.
- **Temporal accumulation of any kind.** ADR 0010, unchanged.
