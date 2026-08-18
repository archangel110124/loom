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
