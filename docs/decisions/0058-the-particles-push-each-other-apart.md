# ADR 0058 — The particles push each other apart, in position space

- **Date:** 2026-08-19
- **Status:** **proposed** — needs human approval before merge.
- **Implements:** ADR 0057 failure 3, which it closes. Constrained by ADR 0053
  §5 and §6 in full.
- **Scopes:** ADR 0057's rejection of a density correction. See the addendum to
  that ADR filed in the same commit — the rejection stands, and it is a
  rejection of *velocity-space* divergence steering, not of every way of
  reading particle density.

## Context

The human's report was two sentences: *"the performance is terrible, thirty to
forty frames on a forty ninety"* and *"after it sat for forty five seconds, the
water exploded"*. The photograph shows `plough_cinematic` shattered into
fragments and spray, the pool gone, the bed showing through and the crate
sitting on debris.

Forty-five seconds is tick 2,700. Reproduced on a quiet machine at HEAD, the
solver reaches peak 380x rest density at tick 2,800, 99.7% of the domain
occupied, and stays there: the whole tank is marked fluid, there is no free
surface left, and the speed clamp at `0.9·h·60·SUBSTEPS` is the only thing
between it and a NaN. It is an absorbing state.

### The mechanism, in three parts

**A ratchet.** The projection zeroes the divergence of the *velocity field*.
Nothing anywhere in the solver reads *particle* density, and there is no
separation and no resampling, so whatever compression the velocity field leaves
behind at a free surface integrates one way and never comes back. Measured at
HEAD: cells at or above rest density fall by about four a tick from tick zero,
in a motionless tank (`slosh`, ke ≈ 0.0003) exactly as fast as in a ploughed
one. That is what rules out every dynamics-based explanation, and it is why no
`CYCLES` count fixes it — ADR 0057's addendum already records a 2D replica with
an *exact* pressure solve compressing to 4.5x.

**An ignition.** As the body dissolves into sub-isovalue mist, the marker
`cellCount > 0` grants a full incompressibility constraint to thousands of
single-particle cells. The projection is asked to zero the divergence of a
velocity field sampled from one particle — which is noise — and the answer is
handed back to the APIC affine matrix multiplied by `4/h²`.

**A detonation.** Around tick 2,500–2,750 the kinetic energy jumps four orders
of magnitude in under ten ticks.

## Decision

### 1. A position-space particle separation pass, every tick

One new compute kernel, `fluidSeparateMain`. For each particle, over the 27
cells around its own, push away from every neighbour closer than
`FLUID_SEPARATION_RADIUS · h`, by `FLUID_SEPARATION_STRENGTH` of the overlap,
halved because both particles compute the same push from the same pre-pass
positions.

**Positions only, never velocities.** ADR 0057 rejected a density correction and
that rejection stands as written: all three forms it tried steered the *target
divergence* by how over-full a cell was, and a velocity-space correction adds
energy to a settled tank, which is why every one of them broke a picture. A
position projection cannot: a settled tank whose particles are already further
apart than the radius is a fixed point of this pass, and the push is exactly
zero.

### 2. The constants are knobs, chosen by sweep

| | value | why |
| --- | --- | --- |
| `FLUID_SEPARATION_RADIUS` | **0.50** cells | `FLUID_PER_CELL` is 8, which is two particles per axis, so the rest spacing is exactly half a cell. A minimum-spacing constraint at `r` caps density near `(h/r)³` particles a cell; half a cell is the value that caps it at rest density and nothing else does. At 0.35 the cap is 2.9x rest and the tank compresses freely up to it — measured, `wet` volume halving between tick 400 and tick 5,000. |
| `FLUID_SEPARATION_STRENGTH` | **0.5** | Swept. At 0.2 the correction is slower than the compression feeding it (`wet` −16% by tick 6,000); at 1.0 it is a hard projection and rings, taking peak density to 7.9x and `wet` to 10,900. |
| stencil | **27 cells** | Eight — the nearest, on the cell-centre lattice — covers any radius up to half a cell, and half a cell is exactly the radius wanted, which leaves no margin at all. |
| schedule | **every tick** | A function of the tick and of nothing else — ADR 0053 §6. Never residual-triggered. |

The 2D replica that proposed this suggested 0.35 and 0.2. Both are wrong in 3D
and the reason is geometric: the rest spacing at eight particles a cell is a
different number in two dimensions. The numbers above were measured on
`plough_cinematic` and `slosh` at the authored camera.

### 3. Out of place, which is the reproducibility rule

The kernel reads `reordered` — the bucket-ordered copy every gather already
uses — and writes `particles`, which it never reads. `sorted` is a permutation,
so each slot owns exactly one particle index and no two threads write the same
place. The neighbour sum runs over 27 buckets in cell-index order and over each
bucket in slot order: a fixed summation order, with **no atomic, no scan and no
compaction**.

It is recorded immediately after `fluid_reorder`, which is the only point in the
substep where the bucket structure describes exactly the positions being read.
The correction therefore lands before `fluid_g2p` advects from it and reaches
the pressure solve on the next substep's rebuild.

### 4. The marker is read from the density splat, at a quarter of rest

`fluidMarkerMain` used `cellCount > 0`. It now reads the trilinear density field
— the same one the isosurface is extracted from — and calls a cell fluid at a
quarter of rest density. The field is computed inside the substep now, two extra
dispatches, where it previously existed only on the readback path.

Swept on `plough_cinematic`, peak density in units of rest at the authored
camera, quiet machine:

```text
                 tick 400   tick 2400   tick 4000   tick 6000
   1/8 rest         2.0x        4.0x       52.0x        4.3x
   1/4 rest         2.5x        3.4x        4.6x        5.0x
   1/2 rest         3.3x        3.5x         —           —      (drains)
```

An eighth is one particle's whole weight, so a lone droplet on a cell centre
still gets a constraint and the amplifier is only half removed. A half is where
`fluidInstanceMain` culls spray and `fluidProbeMain` finds the free surface, and
it is too high to be a *marker*: the topmost layer of a free surface is partly
filled by definition, so marking at a half puts the pressure boundary a cell
inside the water and the tank drains — occupancy 18,464 falling to 10,493 by
tick 3,200.

### 5. `gatherAxis` clamps to the edge instead of dropping the node

The G2P gather skipped any of its 27 nodes that fell outside the face grid, and
a skipped node takes its weight with it. The 27 weights then no longer sum to
one and the sample is scaled down by whatever fell off the grid — 28% of the
tangential weight against a wall, which fabricates shear out of a uniform field.
The node index is clamped; the node's *world position* is not, so the APIC row
still sees the real geometry.

This is the single largest of the three: with the separation pass and the marker
already in, it takes peak density at tick 10,000 from 50.6x to 3.4x and takes
`over4` — cells past four times rest — to exactly zero for the whole run.

### 6. What was measured and NOT shipped

**Clamping the density splat's clipped boundary weight.** The eight trilinear
weights sum to one, so skipping the ones whose cell is outside the domain throws
away a quarter of a particle's mass per clipped axis and 58% at a corner.
Folding it into the edge cell instead is what conservation says — and it takes
peak density on `plough_cinematic --sim 5000` from 4.6x to 34.7x. A correct,
denser boundary cell is a cell the marker calls fluid and the ratchet then
feeds. The measurement is recorded at the line in the shader. **It should be
revisited once the residual settling in §7 is understood**, because it is
correct and the code is currently relying on it being wrong.

**Raising `CYCLES`.** Not tried here and not wanted: ADR 0057's own 10k soak
shows nonzero drift at every count, which is what a rate knob does to a
structural leak.

**Moving the post-projection CFL clamp.** Left where it is. Its comment
documents rapier-probe protection, and with the separation pass landed, clamp
saturation is unreachable — `vmax` no longer pins.

## What this costs

`plough_cinematic`, 1920x1080 offscreen, quiet machine, wall clock:

```text
                        HEAD          this
   --sim 400           2.05 s        2.05 s      3.6  ms/tick
   --sim 2800         17.51 s        9.83 s      3.30 ms/tick
   --sim 10000            —         32.70 s      3.21 ms/tick
```

The per-tick cost is now **flat**, which is most of the win: at HEAD it doubled
across a run, because P2G and the per-cell sort are superlinear in bucket depth
and the marching cubes is linear in triangles, so the ratchet was paying for
itself twice. The separation pass and the two in-step density dispatches
together cost 0.24 ms/tick on the viewer (3.08 → 3.32).

## The gate

`--sim 10000` on `plough_cinematic` and `slosh`, reading the counters
`LOOM_FLUID_DEBUG=1` prints. The line now carries `wet` (cells at or above the
isovalue the surface is drawn at), `mass` and `ycom` as well as `peak`, because
**peak density cannot see a uniform ratchet**: the tank sat at 3.4x from tick
400 to tick 10,000 while the water under it quietly halved, and only the volume
number showed it.

```text
plough_cinematic            peak     wet     mass    ycom
  tick   400                1.4x   16673    15928    3.38
  tick  2400                1.5x   15991    15818    3.06
  tick  6000                1.5x   15734    15712    2.84
  tick 10000                1.6x   15567    15657    2.76

slosh (a still tank)
  tick   150                1.3x   16177    15936    3.43
  tick 10000                1.5x   15610    15717    2.87
```

Mass is conserved to 1.4%, which is the splat clipping at the boundary and not a
physical loss. Peak is flat. `ycom` settles 16% below the authored fill and
**converges** — 3.05 at 2,400, 2.91 at 6,000, 2.87 at 10,000, so the last 4,000
ticks move it 1.4%. That is a settling, not a ratchet, and §7 records it as
open.

Three fresh processes, `sha256` of the PNG, by hand, per cinematic scene: one
distinct hash each for `plough_cinematic --sim 110`, `slosh --sim 150` and
`ribbon --sim 180`.

## 7. What is still open

**A 16% one-time settling.** A still tank ends about a sixth below its authored
fill height and then holds. It is consistent with the rest packing the hash
seeder produces being slightly looser than the packing the separation pass
relaxes into, and with the 1.4% mass clipped at the boundary. It is not the
ratchet — the ratchet does not converge — but it means `fill = 0.5` does not
draw a surface at exactly `surface_height` after a minute.

**The density splat's boundary weight** (§6) is correct and unshipped.

## Consequences

- One new kernel, one changed kernel, one changed line in a third, two extra
  dispatches a substep.
- The pixels of every cinematic scene move. None of the three has a blessed
  reference, so nothing is re-blessed by this.
- Nothing outside `WaterBody { simulation = "cinematic" }` is touched, and the
  determinism hash is untouched by construction — the tier has never been in it
  (ADR 0053 §3).
