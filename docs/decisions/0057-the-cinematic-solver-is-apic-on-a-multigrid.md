# ADR 0057 — The cinematic solver is APIC on a multigrid, and the sort is the reproducibility

- **Date:** 2026-08-18
- **Status:** **proposed** — needs human approval before merge.
- **Implements:** ADR 0053, which is the permission. This ADR asks for none: it
  records the choices made inside the space 0053 already opened, and it exists
  because those choices are what decide whether the tier keeps the one property
  0053 kept.
- **Constrained by:** ADR 0053 §5 in full. Every clause below cites the one it
  serves.

## Context

ADR 0053 allows a `WaterBody` with `simulation = "cinematic"` to be
GPU-stateful, to be read back synchronously inside the fixed step, and to push
`rapier3d` bodies. It also says what that must not cost:

> Same machine, repeated runs: still reproducible. `cargo xtask repeat` is
> expected to keep passing on cinematic scenes, and it is the gate that proves
> it. **If it cannot, the solver is wrong.**

Everything here is downstream of that sentence.

## Decision

### 1. The domain, and what the scene authors

`extent` is required and `fill` (default 0.5) is the whole initial condition.
The grid is **64 cells along the longest axis, always** — so the cell size is
derived, `max_extent / 64`, and is never authored. `loom_scene` refuses an
extent whose cell falls outside `[0.05, 0.5]` m, naming the metres in the
message: below it a 64³ grid covers under three metres and the water is a
puddle in the corner of the shot; above it a pontoon can be smaller than one
cell and sit entirely inside a cell the solver calls air.

**The domain is anchored to the node** (ADR 0053 §5). `FluidDomain` has a
centre and an extent and there is deliberately nowhere in it to put an eye.

Eight particles per filled cell, on a fixed 2×2×2 lattice jittered by a hash of
the particle's own ordinal. No `thread_rng`, no seed state: seeding is a pure
function of the index.

### 2. APIC, not FLIP — ADR 0053 §5

Quadratic B-spline weights. `C_p = (4/h²) Σ wᵢ uᵢ (xᵢ − x_p)ᵀ`, and the `4/h²`
is why the weights had to be quadratic: it is the inverse of that spline's
second moment, and a linear weight makes the same line wrong without making it
look wrong.

What this buys is the deletion of the PIC/FLIP ratio — a knob with no correct
value whose failure mode is null-space energy appearing as jitter, then
interior voids, then instability, and which is close to undebuggable in a
system nobody can read back cheaply.

### 3. No float atomics, and **the trap that follows from that rule rather than being stated by it**

ADR 0053 §5 forbids float atomics on the solver path, because float addition is
not associative and a nondeterministic reduction order feeds the next tick's
pressure solve. So every particle→grid transfer here is a **gather**: a face
node reads the particles near it, over a counting-sorted bucket list.

**That is not sufficient, and the reason is the whole of this section.**

> An `atomicAdd`-scatter counting sort gives each particle a bucket rank equal
> to its **arrival** order, which is nondeterministic — and the gather then
> sums floats in that order, silently reintroducing the exact nondeterminism
> the float-atomic ban exists to kill. The rank must be a deterministic
> function of particle index: compute per-bucket offsets by exclusive scan
> (integer, fixed tree), then each particle's rank among same-bucket particles
> by particle-index order — a stable counting sort with scan-derived ranks,
> **never arrival order**.

There is no atomic anywhere in that failure. The buffer holds the same *set* of
particles per bucket every run; only their order differs, and `a + b + c` is
not `c + b + a` in floats. It is the likeliest way `repeat` fails, and it is
invisible in every picture.

**What shipped**, and where it differs from the sentence above: the scatter
*does* use an integer `InterlockedAdd` cursor, and then `fluidSortCellMain`
re-sorts each bucket **by particle index** with a serial insertion sort, one
thread per cell. The outcome is identical — rank is a function of particle
index and of nothing else — and it is thirty lines rather than a block radix
sort. Buckets hold about eight particles, so it is ~32 comparisons in the
common case.

**The ceiling on that sort is not a performance knob, and setting it wrong
reproduced the exact failure this section is about.** `FLUID_SORT_MAX` began at
128 — sixteen times rest density, and surely unreachable. `slosh.loom` at
`--sim 150`, rendered in three fresh processes, gave **three different hashes,
7.2% of pixels apart with a worst channel of 153**. At 1024 the same three
renders are **byte-identical**. So a cell in that scene really does hold more
than 128 particles, the tail past the cap kept arrival order, and the gather
summed its floats in that order.

That is worth stating plainly: the trap was written down, the mechanism to
avoid it was implemented, and it still fired — through a ceiling that looked
like a safety valve and was actually the hole. Anything that leaves *any*
particle's rank to arrival order is the same defect.

> **The ceiling no longer exists — Addendum 2.** Everything above about why a
> tail in arrival order is fatal stands. What does not stand is the belief that
> some number is large enough: 128, 1,024 and 4,096 were each chosen as
> unreachable and each reached, the last by `slosh --sim 600`. The whole bucket
> is sorted now, and what made that affordable is in Addendum 2.

The two integer atomics that remain — the histogram and that cursor — are
licensed because integer addition **is** associative: the count and the set are
order-independent, and the one thing arrival order would have decided is taken
back by the sort.

### 4. Multigrid, not PCG — ADR 0053 §5

A MAC grid in storage buffers. Pressure is solved with a **fixed 3 V-cycles**,
4 levels (64→8), damped Jacobi at ω = 2/3, **4 pre and 4 post smooths**, twelve
sweeps at the coarsest level, full-weighting restriction, trilinear
prolongation. The smooth counts are measured, not chosen — see failure 2 below.

- **No residual test and no early exit, ever.** The dispatch sequence must be a
  fixed function of the tick alone, which is also ADR 0053 §6's reworded
  catch-up clause: `--sim N` is N identical step sequences, the same regardless
  of frames drawn, wall clock or camera.
- Every operator is a fixed local stencil. A PCG dot product is a global float
  reduction with no fixed summation tree, and there is no way to write one that
  is not order-dependent.
- The operator is `A = -h²∇²` in scaled form, so restriction carries a factor
  of `4/8`: four for doubling `h`, eight for averaging the children.
- Free surface by marker cells — air is a Dirichlet zero and still counts in
  the diagonal; solid is Neumann and does not.
- Coarse markers are **restricted, not re-baked**: a coarse cell is solid only
  when all eight children are, and fluid when **a majority** of them are.
  "Fluid when any is" was tried first and puts the coarse Dirichlet surface a
  level above the fine one, so the correction coming back down is a pressure
  the fine surface never asked for.

### 5. Substeps and the CFL guard

**Two substeps per tick, fixed, never adaptive**, at `dt = 1/120 s`. Velocities
are clamped to `0.9 · h / dt_sub` — 10.8 m/s at h = 0.1 m — which is the CFL
guard, and it is what makes two substeps enough: a particle may not cross more
than nine tenths of a cell in one substep, or the stencil it lands in has no
relation to the one it left.

### 6. The readback is synchronous, inside the fixed step, and reads probes

ADR 0053 §3. One `vkQueueSubmit` and one fence wait per tick, so tick N reads
the result of exactly N dispatches. Asynchronous or frame-delayed is forbidden:
it would lose the tick ordering, which is the sequencing the tier keeps.

**Probes, not the grid.** For each pontoon, one `float4` in and two out:
`{surface_y, velocity xyz}` and `{wetness}`. A few hundred bytes a tick, not
megabytes. The probe kernel is one thread per probe and its loops are serial on
purpose — there is nothing to parallelise in eight columns of sixty-four cells,
and a serial loop is a summation order the buffer decides.

**What comes back is a surface height, not a fraction, and that is the one
subtlety of the readback.** The body is rasterised into the solid mask so that
it pushes the fluid, which means the cells it occupies hold no particles — so a
column *through* a pontoon reports the water level at the bottom of the hull
and a floating crate reads as dry. The level a body floats in is the level of
the water *beside* it, which is what a waterline is, so the probe scans eight
columns on a ring at 1.5 radii and averages. A column's surface is its highest
cell holding at least **half** rest density: one particle is a fleck of spray,
and taking the top of the highest speck drifts upward over a run and reads as a
tank slowly gaining volume.

The CPU then turns that height into a displaced volume with the *same*
`submerged_volume` spherical cap the deterministic tier uses, and calls the
*same* `loom_water::buoyancy::solve` — the surface arrives in the field the
wavelet pool already writes and the velocity in the field the river already
writes. **One force law, two sources of surface**, and no second coefficient to
disagree with the first. **The body pushes back through the rasterised coverage** —
a face touching a solid takes that solid's own velocity, so the coupling is
two-way with no second force term to disagree with the first.

### 7. `ash` does not cross the boundary, and the solver owns a device

ADR 0053 §5, and `scripts/check-deps.sh` enforces it. The crossing is
`loom_render::FluidSolver` with `step(&mut self, inputs: &FluidInputs) ->
&FluidStepOutput`, both of which are plain `f32`/`u32` data.

**The solver creates its own headless compute device** — the same
`Device::new` the render path uses, no surface and no swapchain — rather than
borrowing the renderer's. `loom sim` has no renderer at all, and the
alternative is threading a Vulkan handle through `loom_cli`, which is the thing
the dependency rule exists to prevent. It costs one extra logical device and
about 60 MB.

**Every barrier belongs to the render graph** (never-do #4). The solver records
~180 compute passes per tick into a `RenderGraph` and submits them as **its own
submit**, separate from any frame — the precedent the TLAS rebuild sets. Every
buffer is named through `VK_EXT_debug_utils`.

### 8. The particles are drawn through the one particle renderer

ADR 0047's rule. `fluidInstanceMain` writes `ParticleInstance`s, which
`scene.slang`'s `particleVertexMain` already draws — same billboard, same
blend, same fog — using slice 2's `shutter` so a fast droplet is a smear rather
than a bead. **There is no second particle renderer and no fluid draw path.**

Every `stride`-th particle is drawn, `stride = ceil(N / 65,536)`: a function of
the domain alone, never of the camera, so two renders of the same tick draw the
same droplets. The surface mesh is the next slice; this one is the sloshing
read of reference image 65 without one.

## What this costs, measured

At 64×32×64 cells and 524,288 particles on a 4090 capped to 300 W:

| | ms/tick |
| --- | --- |
| whole step, CPU rasterisation included | **6.93** |
| of which the fence wait | 5.87 |

And on `slosh.loom`'s own domain — 64x16x32 cells, 131,072 particles — the
whole step is **3.1 ms/tick** with 2.3 ms of it the device round trip, so a
600-tick catch-up costs 1.9 s of wall clock. The device sync is therefore
roughly three quarters of the cost and is not the part worth attacking.

The **budget in the brief was 3 ms and it is not met**, and the reason is
structural rather than a missing optimisation: the gather P2G reads about 288
particles per face node, and it is 3.1 ms of the 4.9 ms substep. Two things
were tried and are in the code:

- **Reordering the particles into bucket order** before the gather, so it is
  contiguous rather than an indirection into a 42 MB buffer. Worth nothing
  measurable on its own here (6.95 → 6.95 ms), kept because it is what makes
  the merge below cheap.
- **Merging the three face components into one dispatch per cell**, reading
  each bucket once instead of three times: the union of the three stencils is
  54 buckets against 108 for three passes. Measured 4.2 → 3.1 ms for P2G.

**The named next step is shared-memory tiling**: a workgroup taking a 2×2×2
tile of cells, loading its 4×4×4 buckets into groupshared once. That is ~30 KB
of shared memory for eight particles a cell and it is a real piece of work; it
is not done, and it is where the remaining factor of three is.

Two V-cycles or four make **no measurable difference** (13.90 ms at three,
13.91 at one, both at two substeps) — the pressure solve is not the cost, which
is worth knowing before anyone optimises it.

## The two failures found by measurement, and one still open

**1. The push block's `uint4` aligns to sixteen, and a naive Rust mirror put it
at eight.** Slang emits `OpMemberDecorate %FluidPush 1 Offset 16`; `{u64,
[u32;4]}` in Rust is offset 8. Every dispatch then read its level, axis and
parity from past the end of a 24-byte push range, which the driver serves as
zero rather than faulting. The symptom was a tank of water that held its shape
perfectly and never moved a millimetre — `axis` was always 0, so `if (axis ==
1) v += gravity * dt` never fired, and a fluid with no gravity is a lattice.
Two visual checks and a settling test all passed on it. `spirv-dis | grep
OpMemberDecorate` is the check and a layout test now pins it.

**2. Two pre and two post smooths do not converge, and an under-converged
projection does not jitter — it compresses.** A settled tank held its surface
for forty ticks and then collapsed: 0.00 m at tick 40, −0.04 at 80, −0.65 at
119, accelerating, until the water was packed five times over at the bottom.
Sixty flat Jacobi sweeps with no multigrid at all were worse (−0.45 by tick
40), which is what identified it as convergence rather than discretisation: an
incompressible projection only removes the divergent part of the velocity
field, and what it leaves behind at the free surface has nothing pushing back.
**4 pre + 4 post with twelve sweeps at the coarsest level holds the surface at
exactly 0.0000 through 120 ticks** — and costs *less* than the broken version,
because a collapsed tank puts forty particles in a cell and the P2G gather pays
for every one.

**3. Still open: `slosh.loom` is reproducible at tick 150 and is not at 300.**
Three fresh processes agree byte for byte at `--sim 150` — the tick the
`GOLDEN` row is taken at — and disagree at `--sim 300` and `--sim 600`. The
step cost at 600 rises to **59.9 ms/tick from 3.1**, which is the signature of
the same compression as failure 2: cells accumulate far past rest density, the
gather slows in proportion, and buckets pass even the 1024 sort ceiling. So the
remaining nondeterminism is a *symptom* of a remaining volume instability under
sustained solid coupling, not an independent defect, and the fix is more
convergence or a density term in the right-hand side rather than a bigger sort.
**Until that is closed, the tier is honest only over the first few seconds of a
scene**, and that is what the `GOLDEN` row measures.

## Consequences

- A cinematic scene costs about 6 ms of wall clock per simulated tick, so
  `--sim 600` is seconds rather than milliseconds. That is paid by scenes that
  ask for the tier and by nothing else.
- `repeat` byte-identity is at genuine risk for the first time (ADR 0053 §4)
  and §3 above is where it will fail if it does.
- The tier does not leak: only bodies whose pontoons are inside the domain are
  touched, and a cinematic `WaterBody` applies **no** analytic buoyancy to
  anything, so no body is pushed by both.

---

# Addendum — the free surface, and where the reproducibility actually broke

- **Date:** 2026-08-18
- **Status:** **proposed** — needs human approval with the rest of this ADR.

## The surface is marched on the CPU, and that is forced rather than chosen

The plan was marching cubes as a compute pass: a counting pass, an exclusive
scan for the output offsets, an emit pass, and an indirect draw, all so that no
triangle's position depends on the order threads happened to append.

**It cannot go there, and the reason is in §"Why this owns a device" of
`fluid.rs`.** The solver holds a Vulkan device of its own — `loom_cli` has no
renderer and `loom sim` has no window — so a vertex buffer written on the
solver's device is not something the renderer's device can draw from. The
surface crosses the process boundary as plain `f32` whatever is built.

Once it must cross anyway, the GPU version buys nothing. A CPU loop over cells
in index order emits triangles in index order with **no atomic and no reduction
anywhere**, which is a stronger guarantee than the scan was going to give, in a
tenth of the code. `crates/loom_render/src/fluid_surface.rs` is 350 lines
including its tests.

It is **marching tetrahedra**, not cubes: six tets about the cube's main
diagonal need no case table at all where marching cubes needs 256 hand-written
rows. Winding is not decided either — the water pipeline culls nothing and the
normal comes from the density gradient, so a back-facing triangle shades
identically.

Two things about the field it marches:

- **A trilinear splat, not `cellCount`.** A count is a step function of the cell
  a particle is in, so an isosurface on it is a staircase at the cell size —
  10 cm on `slosh.loom`, which reads as Minecraft water. The splat is fixed-point
  integer atomics (ADR 0053 §5); a float `InterlockedAdd` here would make the
  surface depend on scheduler order.
- **The normal is differentiated from a copy smoothed six passes further than
  the one the surface's position comes from.** Shared, the far half of a tank
  reads as crushed tinfoil: the fraction field is a count of eight, its shot
  noise is ±12%, and a gradient differentiates exactly that. Smoothed together
  instead, the water visibly pulls in from the walls.

## Shading: there is no second water look

The mesh is drawn inside the existing water block by a new vertex entry point
and **the existing `waterFragmentMain`**, so cinematic water gets two-leg Snell
refraction (ADR 0048), per-channel Beer–Lambert, roughness-aware Fresnel, one
traced reflection ray with the analytic sky as its miss (ADR 0019) and 4x MSAA.
Water pass on `slosh.loom` at 1920x1080: **0.193 ms for 22,512 triangles**. The
march is 7–10 ms of CPU, once per frame *drawn* rather than once per tick.

**The fluid mesh does not enter the TLAS, and that is deferred loudly.** The
TLAS holds meshes only; entering it means a per-frame BLAS refit costed at
0.3–0.8 ms and unmeasured. The consequence is that the fluid does not appear in
*other* surfaces' traced reflections — the same accepted gap as the nappe (ADR
0054 §6). Revisit trigger: a scene where the fluid must visibly appear in
another surface's reflection on camera.

## Whitewater: the readback, into the field that already advects

Two CPU routes and no new machinery. A wetted probe deposits
`saturate(|v_water − v_body| / 2)` into `loom_water::foam`; a probe whose
wetness crosses zero is a submersion the *solver* resolved and deposits at full
strength and fires slice 3's SPLASH event. `waterFragmentMain` already floors
its coverage on `loom_foam_at` and the cinematic vertex path reaches it, so the
drawing side needed nothing. The honest ceiling is `FOAM_CELL` = 0.5 m, chosen
for open water: on `slosh.loom`'s tank, six cells across, the foam reads as a
patch rather than as lace.

`float_cinematic` returned before the foam field was ever stepped, so a
cinematic scene carried an allocated field that nothing wrote and nothing read.

## `FLUID_SORT_MAX` was too low for the second time — and failure 1 and failure 3 are one bug

§3 above records `slosh.loom` giving three different PNGs past ~200 ticks and
attributes it to the sort ceiling; §"failure 3" records the volume instability
separately. **They are the same defect seen twice.**

`ribbon.loom` — a cascade landing in a shallow pool — packs a cell to well over
a thousand particles during the transient. Past `FLUID_SORT_MAX` the bucket's
tail keeps its arrival order, the P2G gather sums those particles' floats in
that order, and the whole float-atomic ban is defeated with no atomic in sight.
Three fresh processes gave three pictures **37% of pixels apart**. At 4096:

| scene | tick | three fresh processes |
| --- | --- | --- |
| `ribbon` | 180 | byte-identical |
| `ribbon` | 400 | byte-identical |
| `plough_cinematic` | 110 | byte-identical |
| `slosh` | 150 | byte-identical |
| `slosh` | 600 | ~~byte-identical~~ **FALSE WHEN WRITTEN — see Addendum 2** |

> **That last row was wrong, and the review panel measured it wrong.** Nine
> renders across two judges on an idle GPU gave nine distinct hashes at
> `--sim 600`, the marched surface itself differing. It is corrected rather
> than deleted because the mistake is the instructive part: it was written
> from a single run that happened to agree, and one agreeing run is not a
> reproducibility measurement. Addendum 2 is the fix and the re-measurement.

The cost is the O(k²) insertion sort: `ribbon` at 180 is 52 ms a tick against
`slosh`'s 3, and `plough_cinematic` at 300 is 277 ms a tick, which is 83 seconds
to render one still. **That is the volume instability presenting as time.**

Two undeclared buffer reads were found while chasing it, and both are real
(never-do #4). `fluidDivergenceMain` and `fluidProbeMain` both read `cellCount`
without the graph being told; the probe one is the readback pass, so an
undeclared dependency there does not stay on the GPU — it goes through buoyancy
into rapier and comes back next tick as the solid mask. Every device-local
buffer is also zeroed in the first submit now: fresh device memory is undefined
and the coarse multigrid levels are only ever partially written.

## A density correction was built, measured, and rejected on the pictures

Steering the target divergence by how over-full a cell is (Ando et al.) is the
textbook fix for failure 3, and it works on the number it targets: `ribbon`'s
peak occupancy fell from 850 particles to 46 and its step from 37 ms to 2.4.

It is not shipped, because all three formulations broke a scene:

| form | what happened |
| --- | --- |
| one-sided from rest density | `slosh` became shattered foam filling the frame — eight particles a cell is a *mean* with σ = 2.8, so half the cells are over-full at any instant and a term that can only push apart has no restoring force |
| symmetric | the free surface collapsed: a surface cell genuinely holds two or three particles and pulling it toward eight is the original compression under a new name (peak 1,207, 157 ms a tick) |
| one-sided past a 2× deadband | still threw water out of the tank |

**The compression is real and the fix is not this.** The next thing to try is a
proper free-surface boundary condition (a ghost-fluid pressure at the air
interface rather than a hard Dirichlet zero), which is what makes the projection
conserve volume in the first place, and a block radix sort with scan-derived
ranks so that `FLUID_SORT_MAX` stops being a cost cliff.

## The two scenes

`ribbon.loom` (image 61) and `plough_cinematic.loom` (image 64) are both
`GOLDEN`, both with close authored cameras. `plough.loom` stays exactly as it
is; the pair is the tier comparison.

`ribbon`'s inflow is a **closed loop**: a `Cascade` authors the discharge, and
`per_tick` particles are recycled to the lip each tick chosen round-robin by
ordinal — the free-list-free arithmetic of ADR 0047, because a drain needs a
free list, which needs a compaction, which needs an atomic append. The particle
count never changes, so volume is exactly conserved.

---

# Addendum 2 — the ceiling was the bug, and a better sort is what let it go

- **Date:** 2026-08-18
- **Status:** **proposed** — needs human approval with the rest of this ADR.
- **Answers:** review defect 3 (`docs/design/WATER-REBUILD-REVIEW.md`), which
  found the first addendum's table row false by direct measurement.

## What the panel measured, and it was right

`slosh.loom --sim 600`, nine renders across two judges on a verified-idle GPU:
**nine distinct hashes**, adjacent pairs 5–10% of pixels apart, worst channel
94–154, the divergence bounded exactly by the water surface — and the marched
surface itself differing, 20,496 against 20,530 against 20,626 triangles. Cost
~350 ms/tick, against the 8.4 the slice reported.

Reproduced here first, before anything was touched, on the same hardware:
three fresh processes at the gate arguments gave `301fc02c`, `15e3721a`,
`1fc14e09`.

The first addendum's table said that run was byte-identical. It was written
from a run at `FLUID_SORT_MAX = 4096` that happened to agree, and **one
agreeing run is not a reproducibility measurement** — which is the whole reason
`cargo xtask repeat` renders three.

## The cause is the one §3 named, and the ceiling is not fixable by raising it

§3 of this ADR is right about the mechanism and it named its own hole:
`fluidSortCellMain` re-sorted only `min(occupancy, FLUID_SORT_MAX)` of a
bucket, `fluidP2GMain` gathers over the **full** occupancy, and so the tail past
the ceiling kept the arrival order the scatter's `InterlockedAdd` handed out and
had its floats summed in that order. No atomic is visible at the point of
failure, which is what makes it hard to find.

The number went 128 → 1,024 → 4,096, and a scene reached each one. That is not
three unlucky guesses; it is a design in which correctness depends on a
capacity constant, and such a constant goes stale the first time a scene is
authored that nobody had in mind. **The ceiling is now gone**: the sort takes
the cell's real occupancy and there is no clamp.

**What made it a ceiling was the sort, not the concept.** A plain insertion
sort is O(k²), so 4,096 was already 8M comparisons in one thread and raising it
further was a hang risk — which is exactly why the previous slices raised it in
small steps instead of deleting it. Shell's sort with Knuth's 3h+1 gaps is
O(k^1.5), is one line longer, needs no scratch buffer, and **produces the same
array**: a comparison sort on distinct keys has one answer, so for every bucket
that was under the old ceiling the output is bit-for-bit what it was. That is
the property that makes this a safe change to the gate rows, and it is
confirmed below rather than argued.

The gap sequence is fixed and derived from `count` alone, so the sequence of
comparisons is still a function of the buffer and of nothing else — the
requirement §3 states.

## Re-measured, three fresh processes each, this machine

| scene | tick | three fresh processes | ms/tick before | after |
| --- | --- | --- | --- | --- |
| `ribbon` | 180 | byte-identical, **same hash as before** | 50.40 | 14.77 |
| `ribbon` | 400 | byte-identical | — | 17.78 |
| `plough_cinematic` | 110 | byte-identical, **same hash as before** | 3.68 | 3.45 |
| `plough_cinematic` | 300 | byte-identical | 277 | 38.94 |
| `slosh` | 150 | byte-identical, **same hash as before** | 3.28 | 3.21 |
| `slosh` | 600 | **byte-identical** — the failing run | ~350 | 34.17 |

"Same hash as before" is the three `GOLDEN` rows, compared against renders from
a binary built at the parent commit: `1cda7461…`, `28b73934…`, `452251eb…`.
**No reference moves.**

Wall clock for a single still: `slosh --sim 600` 211 s → **21.5 s**;
`plough_cinematic --sim 300` 83 s → **12.6 s**.

## What this does *not* fix, stated plainly

**The volume compression (failure 3 of this ADR) is still open and is still the
reason the numbers above are 34 ms and not 3.** A settled tank should not be
packing cells past rest density at all, and the cost at tick 600 is the gather
paying for every particle in an over-full cell. The first addendum called the
compression and the nondeterminism "the same defect seen twice"; they are
better described as **one cause with two symptoms, and only one of the two is
fixed here**. The tier is reproducible past tick 600 now; it is not yet
*correct* past tick 600, and a scene that runs long enough will still look
wrong before it looks nondeterministic.

The named next step is unchanged and is upstream of both: a ghost-fluid
free-surface pressure boundary rather than a hard Dirichlet zero at the air
interface. What is no longer part of it is "a block radix sort with
scan-derived ranks so that `FLUID_SORT_MAX` stops being a cost cliff" — there
is no `FLUID_SORT_MAX`, and a radix sort would now buy performance rather than
correctness. It should be judged on that alone.

**A single cell holding every particle in the domain is still the worst case
for one thread**, and at 131,072 particles that is roughly 47M compare-exchanges
in one lane. It is bounded, it is not a hang, and it is a simulation that has
already failed for other reasons — but it is the price of having no ceiling,
and it is the honest one to pay: a stall is visible and a wrong picture is not.

---

## Addendum 3 — the per-tick costs, re-measured at HEAD (review defect 10)

- **Date:** 2026-08-18.
- **Answers:** the last bullet of defect 10 in `docs/design/WATER-REBUILD-REVIEW.md`
  — *"re-measure and re-record the per-tick costs that were reported from a
  different machine state than they reproduce on."*

This addendum records numbers only. It changes no code and no decision.

### What the earlier tables said, and what they say now

The body of this ADR reports `ribbon@180` at **52 ms/tick** and the panel
measured **107**. Both were taken before the bucket sort was fixed and neither
is current. Measured here on this machine, release, at the gate arguments and
`GOLDEN_SIZE`, from the CLI's own `cinematic water:` line:

| scene | ticks | ms/tick | of which the fence | wall |
| --- | --- | --- | --- | --- |
| `ribbon` | 180 | **14.78** | 13.91 | 2.66 s |
| `slosh` | 150 | **3.15** | 2.37 | 0.47 s |

The fence wait is **94%** of `ribbon`'s cost and 75% of `slosh`'s, which is the
same split §"What this costs" reports and is still the thing not worth
attacking.

### The host-load sensitivity does not reproduce at the scale reported

The panel found the same `slosh --sim 150` at **13.5 ms/tick standalone and
222.6 while clippy loaded the CPU** — a 16× swing — and called the fence wait
fragile under host load. Re-run here with **24 busy spinners on 24 cores**, the
heaviest load this box can present:

| scene | idle | 24 spinners | ratio |
| --- | --- | --- | --- |
| `ribbon@180` | 14.78 | 15.54 | **1.05×** |
| `slosh@150` | 3.15 | 4.43 | **1.41×** |

So the effect is real and it is small: the fence wait is a spin-then-block and
a descheduled host thread pays for it, but the cost is tens of percent, not
sixteen times. The panel's measurement was taken at `FLUID_SORT_MAX = 4096`,
where a single tick's sort could be 8M comparisons in one lane; the 16× is
better read as the O(k²) sort interacting with load than as fence fragility.
**Nothing here is a reason to change the readback**, which ADR 0053 §3 requires
to be synchronous and inside the fixed step.

### The `Renderer::new` `ERROR_UNKNOWN` flake — attributed and bounded

Slice 6 reported an intermittent `Renderer::new` failure under concurrent test
binaries; three judges never saw it fire. It reproduces here, and what it is
**not** matters more than what it is:

- **Not out of memory.** Peak VRAM across the whole stress was 9.3 GB of 24.6,
  sampled at 4 Hz. The error is `ERROR_UNKNOWN`, never `ERROR_OUT_OF_*`.
- **Not one test, and not one call site.** It has fired from three different
  `Renderer::new` calls in three different tests.
- **Not per-process.** In one round **two separate processes failed in the same
  wall-clock window on the same two tests**. That is the shape of a driver-level
  transient across concurrent `vkCreateDevice`, not a race inside the crate.

Measured rate, `target/debug/deps/loom_render-*` run directly:

| shape | concurrent devices | process runs | failed |
| --- | --- | --- | --- |
| whole suite, 8 processes × 4 test threads | up to 32 | 96 | **3** |
| one test, 12 processes | 12 | 288 | **1** |

So it needs many simultaneous device creations and it is roughly 3% per process
at 32 and a third of a percent at 12. `cargo test --workspace` runs far fewer
than 32 at once, which is why it is rare there and why three judges' runs and
this pass's runs were all clean. **`cargo xtask image`, `repeat` and `validate`
spawn `loom render` one at a time and create one device at a time**, so the
gates are not exposed to it.

**Deliberately not fixed.** A retry in `Renderer::new` would hide a real
device-creation failure, which is the one class of error this project most
needs to see; and the condition that triggers it is a stress harness, not
anything the engine does. Recorded so the next person who sees it knows it has
been chased. If it ever fires in a gate, that is a different bug and this
paragraph is the negative control.

---

# Addendum 4 — the presentation path was on the wrong side of the bus, and failure 3 is mostly at the wall

Two findings, from chasing `loom run assets/test/plough_cinematic.loom` falling
to one or two frames a second when the hull lands. They are unrelated to each
other; the first is fixed and the second is not.

## The three presentation stages, split — and the timer that hid them

`cinematic surface: 27706 triangles, marched in 19.2 ms` timed `fluid_draw`,
which is a density readback, the CPU march and a spray readback. **The word
`marched` named the smallest of the three.** Split and measured quiet, release,
`plough_cinematic`, per frame drawn:

| | tick 95 (pre-impact) | tick 115 (post-impact) |
| --- | --- | --- |
| before, one number | 13.7 ms | 23.1 ms |
| density | 0.5 ms | 0.5 ms |
| march | 4.2 ms | 8.2 ms |
| spray | 1.3 ms | 1.2 ms |

**`spray` does not scale with the impact.** It is a fixed 65,536-thread dispatch
and it cannot; a reading that showed it doubling was taken against a busy GPU.
Two measurements of a fence wait taken under different GPU load are not
comparable, which is the same shape of error as comparing two AA numbers across
a change in lighting.

## What was actually costing 8 ms a stage: `GpuToCpu` is not free to *write*

`density` and `instances` were allocated `GpuToCpu` and `solid` `CpuToGpu`.
Host-visible memory is addressable from a shader, which makes it look free and
is the trap: every access is a PCIe transaction, not a cached one.
`fluidDensitySplatMain` ran half a million `InterlockedAdd`s on host memory and
`fluidInstanceMain` wrote 3 MB as 196,608 scattered 16-byte stores — 8.04 ms and
8.34 ms, for two dispatches and one, against 34 ms for the whole 175-dispatch
step. All three are device-local now with a host-visible twin and a
`vkCmdCopyBuffer` between them; the same bytes as a DMA are 0.5-1.0 ms.

`probes` and `consts` stay host-visible deliberately: kilobytes touched once a
tick, where the copy would cost more than the access.

**`fluid_zero` declared `ComputeReadWrite` and records `vkCmdFillBuffer`.** The
graph therefore emitted the next barrier with a source mask that did not cover
what happened — latent until a second transfer wrote one of those buffers, then
a plain `SYNC-HAZARD-WRITE-AFTER-WRITE`. Fixed in the same commit.

## The device split survives this, and the GPU march is still not worth building

§"The surface is marched on the CPU" stands, and the numbers above sharpen it
rather than overturning it. Moving the march onto the device would buy 4-8 ms of
a frame whose *step* is 30-48 ms a tick after the impact. It would be optimising
the second-smallest term while the largest sits next door as a known-open
failure, and it would put a scan or an atomic on the one path in this tier whose
reproducibility is currently free.

## Failure 3, located: the compression is mostly at the domain boundary

Peak cell occupancy on `plough_cinematic`, as a multiple of rest density, with
where it sits:

| tick | peak | cells over 4x | of those, on the boundary |
| --- | --- | --- | --- |
| 60 | 1.18 | 0 | 0 |
| 120 | 60.3 | 982 | 270 |
| 200 | 160.7 | 797 | 486 |
| 400 | 415.8 | 759 | 536 |

The peak at tick 400 is at cell `(53, 0, 31)` of `[64, 16, 32]` — the floor, at
the far wall. **Two thirds of the over-dense cells are on the boundary layer and
the fraction grows monotonically**, which is a different diagnosis from "the
projection does not conserve volume": `fluidG2PMain` clamps a particle's
position into `[origin + h/4, origin + (dims - 1/4)h]` while `escapeSolid` pushes
it out of the solid boundary cells, and a particle caught between the two is in
a cell the marker never labels `FLUID`, so no pressure is ever solved to move it
out. It accumulates and never leaves.

That is what presents as time: `fluidSortCellMain` is one thread per cell and
`fluidP2GMain` gathers a cell's whole bucket, so a cell holding 3,300 particles
costs a single thread thousands of comparisons while its group idles. Marginal
cost per tick, measured at HEAD:

    ticks   0-60   3.5 ms      120-180   28.0 ms      240-300   65.7 ms
           60-90   3.0         180-240   34.5         300-400   48.0
           90-120  4.3

And it is what presents as the picture: at tick 115 the whole tank erupts into
foam and at tick 200 it has not settled — the bed is visible across most of the
pool because the water has collapsed into over-dense sheets against the walls.

**A viewer cannot survive that whatever the presentation costs.** `Play::advance`
clamps catch-up to `dt.min(0.25)`, which is fifteen ticks; at 40 ms a tick that
is a 600 ms frame, and since a tick then costs more than the 16.7 ms it
represents the accumulator can never drain. Measured: 611 ms a frame, which is
the 1-2 fps that was reported. The clamp is in *seconds* and its own comment
says it exists to stop exactly this spiral — expressing it as a tick count would
turn the collapse into slow motion. That is a change to the fixed-step contract
for every scene and it is left for the human to rule on.

---

# Addendum 5 — failure 3 was one kernel disagreeing about the boundary condition

**Addendum 4's mechanism for failure 3 is retracted.** It read the aftermath as
the cause. The pile-up against the walls was real and its measurement was
sound; the explanation was not, and the fix is three deleted lines in
`fluidProlongMain`.

## The bug

Four kernels read `marker`. Three of them agree that an air cell is a Dirichlet
zero which still counts in the diagonal — `fluidJacobiMain` and
`fluidRestrictMain` do `diag += 1.0` and contribute nothing, `fluidProjectMain`
uses `pa = 0.0`. `fluidProlongMain` instead accumulated the surviving corners'
weight and divided by it, so the coarse correction was *renormalised* rather
than masked. The trilinear weights are a partition of unity and the smallest
nonzero corner weight is 1/64, so `1/wsum` reached 64x: the prolongation
extrapolated the coarse pressure across the boundary condition the other three
enforce, and the V-cycle stopped being a contraction.

Relative residual after a cycle, `||r|| / ||rhs||`, on `plough_cinematic`:

| | tick 0 | tick 399 |
| --- | --- | --- |
| with `/ wsum` | 0.355 | **1.31** — worse than the zero guess |
| without | 0.154 | 0.18 |

So the projection never removed the divergence at the impact. Particles piled up
because nothing was solved to move them, and both `fluidSortCellMain` (one
thread per cell) and `fluidP2GMain` (a whole bucket gathered) scale with the
worst bucket. **The frame rate was the symptom; the wrong picture was the bug.**

## What is retracted

1. **The `escapeSolid` / `fluidG2PMain`-clamp mechanism.** No particle was ever
   stuck in a solid cell: `psolid = 0` at every tick of a 400-tick run, before
   the fix as well as after. The two never fought.
2. **The boundary reading.** 536 of 759 over-dense cells were on the boundary
   because water driven by a divergent pressure field piles against whatever
   stops it, and the walls are what stop it. It is where the failure lands, not
   where it comes from. It is now 2 cells of 17,191 occupied.
3. **Failure 2's prescription.** More smoothing helped because it partly
   compensated a cycle that was not converging. `CYCLES`, `SMOOTHS`,
   `COARSE_SMOOTHS` and `LEVELS` are unchanged in this commit *deliberately* —
   they were tuned against a divergent solver and re-tuning them is a separate,
   single-variable experiment that moves the pixels a second time. Do it after
   the references are blessed, not with them.
4. **The tick-count clamp on `Play::advance`.** Not needed and not shipped. At
   3.0–3.6 ms a tick with 4x headroom in a 16.7 ms budget the accumulator drains
   on its own, and the spiral is structurally unreachable. The fixed-step
   contract is untouched. The question addendum 4 left for the human is closed
   by not needing an answer.

The density correction stays rejected — but for its stated reason (it looked
worse), not for anything about the divergence, and the comment claiming
`fluidDivergenceMain` performs one has been corrected: that kernel reads the
three velocity components and the marker, and nothing else.

## The rule this leaves

**Every kernel that reads `marker` must agree that an air cell is a Dirichlet
zero counting in the diagonal.** Four read it and one disagreed. That is a
whole-solver invariant which no single kernel can be read to check, and it is
the reason a per-kernel review missed it three times.

## Measured, on a quiet machine (load 1.8, no compute clients, GPU 39% desktop)

    plough_cinematic, GPU ms/tick    before   after
      ticks   0-120                    3.7      3.4
      ticks 120-180                   28.8      2.9
      ticks 180-240                   34.5      2.9
      ticks 240-300                   66.5      3.0
      ticks 300-400                   46.6      3.2
      wall clock, whole run           33.03     3.58
      peak occupancy, tick 399         415x      3x
      occupied cells (16,384 seeded)  3,162    17,191
      over-dense cells on boundary      530        2
      fluid_p2g / fluid_sort, ms   19.9/25.3  1.0/0.07

`ribbon` 16.54 -> 5.49 ms/tick at 180 ticks, `slosh` 3.59 -> 3.33 at 150. A
1200-tick soak on `plough_cinematic` is flat at 3.0–3.3 ms/tick and ends with
97.7% of the seeded volume still in the domain, peak 3x, residual ratio 0.13.

Byte identity, three fresh processes each, by hand: `slosh --sim 150`
`cb7e1884`, `ribbon --sim 180` `a395724d`, `plough_cinematic --sim 110`
`385b62ab`.

## `ribbon` still accumulates, and it is the scene that bounds any future cap

A continuous inflow is a genuine sustained pile-up at its plunge point, and the
fix does not remove it:

    ribbon, GPU ms/tick   0-49  3.14   150-199  5.78   300-349  11.29
                        50-99  5.13   200-249  5.53   350-399  14.57
                       100-149 5.45   250-299  7.18

Peak occupancy climbs 16 -> 202 -> 522 -> 1036 over 400 ticks, and the residual
ratio spikes above 1 transiently (0.20 at tick 350, 1.72 at 399) without
trending. It crosses the 16.7 ms budget somewhere around tick 450. **This is
open**, and it is the reason the P2G bucket cap below is recorded rather than
built: a cap sized on `plough`'s post-fix peak of 3x would bind on `ribbon`
every tick and move its pixels.

## Deferred: the P2G bucket cap `FLUID_P2G_MAX`

A hard bound `K` on the particles a cell's P2G gather reads makes the worst tick
a bounded multiple of the average — the gather's cost is `p2gmax <= 54·K`
exactly, since the stencil touches 54 cells. It is not built, because:

- Its motivation was a 33 ms tick that is now 3.6 ms.
- It must be pixel-neutral or it is a deliberate re-bless, and `ribbon` reaching
  1036 particles in a cell means `K` would have to be around 640+ to avoid
  binding — at which point it bounds nothing interesting.

**Triggers to build it:** any cinematic scene measured over ~12 ms/tick
sustained on the convergent solver (`ribbon` past ~tick 350 already qualifies),
or a peak occupancy in the thousands. **And if it is built:** the sort bound and
the gather bound must be one shared expression. `FLUID_SORT_MAX` was too low
twice, and both times the failure was a bucket's tail read past what the sort
had ordered.

## Instrumentation

`LOOM_FLUID_DEBUG=1` prints peak occupancy in units of rest density, the peak
cell, the occupied-cell count, and the over-dense counts including the boundary
subset. It sits in `FluidSolver::density`, which all three readback paths go
through. Fault-injected to prove it is falsifiable: restoring the `/ wsum` takes
`plough_cinematic` from `peak=3.0x over4b=0` to `peak=192x over4b=576` and 23
ms/tick. **This is what makes the triggers above checkable** — none of the five
green checks could see a solver at 415x rest density, and all five were passing
while it was.

## Not done here

`CYCLES`/`SMOOTHS` re-tuning; a level set; reseeding; a separating boundary
condition; the sort rework; the `ribbon` accumulation above. Each was evaluated
against a solver whose projection was diverging, which means each was evaluated
against the wrong problem.

---

# Addendum 6 — the density-correction rejection was about velocity space, and failure 3 is closed

**Filed alongside ADR 0058, which is the fix.**

"A density correction was built, measured, and rejected on the pictures" reads,
as written, like a rejection of ever letting the solver look at particle
density. It is not, and ADR 0058 needs the narrower reading, so it is stated
here rather than left to be inferred.

Every one of the three forms in that table steers the **target divergence** —
the right-hand side the pressure solve is asked to satisfy — by how over-full a
cell is. That is a *velocity-space* correction: it reaches particle positions
only through the projection and then through the advection, and it puts energy
into a tank that was not moving. Which is exactly what the table records. The
one-sided form had no restoring force because half the cells are over-full at
any instant when eight is a mean with σ = 2.8; the symmetric form pulled surface
cells toward eight, which is the original compression under a new name.

**ADR 0058 corrects positions directly and symmetrically, and never touches a
velocity.** A settled tank whose particles already sit further apart than the
separation radius is a fixed point of it: the push is not small, it is exactly
zero. That is the property whose absence killed all three forms above, and it is
the property a velocity-space correction cannot have.

So the rejection stands, scoped to what it measured. What it does not license is
the sentence that follows it — *"the compression is real and the fix is not
this"* — being read as "the fix is a ghost-fluid boundary condition and nothing
else". A ghost-fluid pressure is still the right thing for the *free surface*;
it is not what closes failure 3, because failure 3 is a particle-density
integral and the projection has no particle density in it at any boundary
condition.

## Failure 3 is closed

`plough_cinematic` and `slosh` both hold peak density between 1.3x and 1.6x
rest, with zero cells past four times rest, for ten thousand ticks. The per-tick
cost is flat over the same run — 3.6 ms at tick 400, 3.21 ms at tick 10,000 —
where at HEAD it doubled. The 45-second explosion the human photographed does
not happen.

`ribbon`'s accumulation, which Addendum 5 left open as the scene that bounds any
future cap, is covered by the same pass and is byte-reproducible across three
fresh processes.

What remains is a **16% one-time settling** of a still tank below its authored
fill, which converges rather than ratcheting. ADR 0058 §7 owns it.

## The instrumentation grew, because peak density is blind to a uniform ratchet

`LOOM_FLUID_DEBUG=1` now also prints `wet` — cells at or above the isovalue the
surface is drawn at — plus total `mass` and the height of the wet centre of
mass. It is not decoration: with the separation pass at its first radius the
tank sat at 3.4x peak from tick 400 to tick 10,000, `over4` was zero the whole
way, and the water under it halved. The picture showed it and no counter did.
