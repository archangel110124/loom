# ADR 0051 — Ripple injection should be volume-normalised

- **Date:** 2026-08-17
- **Status:** **proposed** — recommended for acceptance, deliberately not built
  in the commit that filed it.
- **Extends:** ADR 0046, whose `push()` this is about.
- **Applies to:** `loom_water::ripples::RippleGrid::push`, and through it the
  sim hashes of every scene that authors a `[ripples]` table.

## The defect

`push` splats `amount` into four cells with bilinear weights that sum to one:

```rust
let amount = relative * TICK_SECONDS * self.strength * scale;
for (dz, wz) in [(0, 1.0 - fz), (1, fz)] {
    for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
        self.now[(z0 + dz) * side + x0 + dx] += amount * wx * wz;
    }
}
```

So the injected *volume* is `amount * cell²`, and nothing in the expression is
the size of the thing doing the pushing. Two consequences, both real:

- **A grid's coupling depends on its own discretisation.** The same body at the
  same speed injects sixteen times the volume into a 2 m grid as into a 0.5 m
  one. `cell` is documented as a resolution knob — "coarser is cheaper and
  blurrier" — and it is silently also a strength knob.
- **A pontoon's footprint is not in it.** `PontoonState::radius` is known at the
  call site and unused. A dinghy and a barge push the same water per pontoon.

The fix is to divide by the cell area and multiply by the pontoon's footprint,
so `strength` keeps meaning what its documentation claims and `cell` goes back
to meaning resolution.

## Why it is not built here

**It moves force-path hashes.** Every scene with a `[ripples]` table changes
what its bodies feel, so `wake.loom` and `pool.loom` both re-pin — and `wake`'s
120 s monotone-decay measurement (0.034 m at 10 s to 4.5e-8 at 120 s) is the
only evidence in the repository that the two-way coupling does not add energy.
Changing the injection scale is exactly the parameter that measurement bounds,
so it has to be re-run, not assumed.

**And it does not do the thing it looks like it does.** It is tempting to file
this as the reason the wake is only centimetres tall. It is not. The coupling is
injected *relative to the surface's own velocity* (ADR 0046, first failure), so
it saturates: once the water is moving with the body, nothing further goes in.
Measured on `pool.loom`, `strength` at 5.6x its authored value moves the
ripple-vs-none ablation mean from 2.29 only to 2.83 — while changing where the
sphere ends up by half a metre. A larger injection constant does not buy a
larger wake; it buys an invisible physics change.

**A probe that drives a constant relative velocity will overstate the
dependence**, because it bypasses that saturation. Any measurement offered in
support of this ADR must be taken with the feedback live — inside a running
scene, not against a synthetic driver.

## What acceptance needs

1. The fix, with `cell` and the pontoon radius both in the expression.
2. `wake.loom`'s 120 s decay re-measured and still monotone.
3. `wake` and `pool` sim hashes re-pinned in the same commit.
4. A test that the same body on two grids of different `cell` injects
   comparable volume — which is the property the defect breaks and the only one
   worth asserting.
