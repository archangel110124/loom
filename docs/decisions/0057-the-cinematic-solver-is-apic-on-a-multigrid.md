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

The ceiling is honest and named: `FLUID_SORT_MAX = 128`. Every particle is
still stored and still transferred; only the *ordering* past 128 in one cell is
left in arrival order. A cell holding 128 particles is sixteen times rest
density, which means the solve has already failed.

The two integer atomics that remain — the histogram and that cursor — are
licensed because integer addition **is** associative: the count and the set are
order-independent, and the one thing arrival order would have decided is taken
back by the sort.

### 4. Multigrid, not PCG — ADR 0053 §5

A MAC grid in storage buffers. Pressure is solved with a **fixed 3 V-cycles**,
4 levels (64→8), damped Jacobi at ω = 2/3, 2 pre and 2 post smooths, four
sweeps at the coarsest level, full-weighting restriction, trilinear
prolongation.

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
  when all eight children are, and fluid when any is.

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

**Probes, not the grid.** For each pontoon inside the domain, one `float4` in
and two out: `{fluid_fraction, velocity xyz}` and `{pressure}`. A few hundred
bytes a tick, not megabytes. The probe kernel is one thread per probe and its
sample loop is serial on purpose — there is nothing to parallelise in 216
samples, and a serial loop is a summation order the buffer decides.

Forces reuse the existing pontoon shape: `ρ g V_sphere · fraction` up, plus
drag against `(u_fluid − v_body)`, through the same `apply_force_torque` path
`buoyancy.rs` uses. **The body pushes back through the rasterised coverage** —
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
| whole step, CPU rasterisation included | **9.73** |
| of which the fence wait | 8.76 |

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

## Consequences

- A cinematic scene costs about 6 ms of wall clock per simulated tick, so
  `--sim 600` is seconds rather than milliseconds. That is paid by scenes that
  ask for the tier and by nothing else.
- `repeat` byte-identity is at genuine risk for the first time (ADR 0053 §4)
  and §3 above is where it will fail if it does.
- The tier does not leak: only bodies whose pontoons are inside the domain are
  touched, and a cinematic `WaterBody` applies **no** analytic buoyancy to
  anything, so no body is pushed by both.
