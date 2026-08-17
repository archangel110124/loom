# ADR 0047 — The GPU particle pool, and how it spawns without an atomic

- **Date:** 2026-08-17
- **Status:** **accepted**
- **Numbering:** above 0044, for the reason ADR 0045 gives — 0023–0042 are
  reserved by the editor plan, which is on hold rather than cancelled. 0046 is
  reserved for interactive ripples, which are not built.
- **Governed by:** ADR 0045. This is that document's clause 3 exercised for the
  first time, and it could not land until `cargo xtask repeat` existed.
- **Decision touched:** none of CLAUDE.md's locked decisions. It extends the
  render graph's *use* to a second buffer chain, not its shape. It does **not**
  supersede `loom_particles`: the CPU path stays the default and stays the only
  one a script or an assertion can read.
- **Answers:** `NIAGARA-AND-FIRE-RESEARCH.md` §4.6, which blocked GPU-resident
  particles on determinism rather than on cost, and named the only spawn scheme
  that survives. This is that scheme.

## Context — the blocker was spawning, and it was correctly identified

The research pass wrote the objection precisely enough to build against:

> A real particle system's spawn needs an atomic counter; the atomic's ordering
> picks the slot; the slot picks the seed; the seed picks position, roll and
> phase — all visible in the golden image. The only spawn scheme that survives
> is "slot = pure function of (index, tick)", i.e. a fixed pre-seeded pool,
> which *is* rain, which already exists.

Rain (ADR 0017) never spawns: 131,072 drops forever, thread `i` owns drop `i`,
a landed drop recycled in place. Every textbook GPU particle system reaches for
a free list and `InterlockedAdd`, and every one of them is closed to a project
whose fourth green check is a pixel diff.

The cost case was also correctly dissolved: `smoke.loom` runs ~725 particles at
about 0.02 ms a tick, and `MAX_PARTICLES` is 65,536, so nothing in the repo
needed this. **What was missing was a scene that did**, and the trigger the
research named — a population the CPU cannot afford — is measured below.

## Decision — slot ownership is arithmetic, so there is nothing to allocate

> Slot `i` holds the largest birth ordinal `n < birthsBy(tick)` with
> `n ≡ i (mod N)`.

That is `((births − 1 − i) / N) · N + i`, integer arithmetic, evaluated per
thread per tick. A tick where the arithmetic hands a slot a *different* ordinal
is that slot's birth instant; a tick where it hands back the same one is an
ordinary step. There is no free list, no dead list, no alive list, **no atomic
anywhere on the seed path**, and no indirect dispatch.

Three things follow, and each is a rule rather than an implementation detail.

### `birthsBy` is the allocator, so it is written twice and compared

`loom_particles::births_by` and `gpuBirthsBy` are one expression in two
languages. It is a closed form, `burst + floor(rate · emittingSeconds)`, gated
by delay and duration — deliberately **not** `System`'s `owed` accumulator,
because:

- the GPU cannot carry a residual across a dispatch boundary without making the
  answer depend on how the ticks were grouped, which is exactly the property
  "advance `--sim N` in one dispatch" must not have; and
- `floor(rate · n · dt)` is a multiply and a floor, both correctly rounded in
  IEEE-754, so it is bit-identical on both sides given the same inputs. An
  accumulated sum is not: float addition is not associative.

`dt` is **passed to the shader** rather than declared in it, so it is the same
`f32` on both sides. `1.0 / 60.0` folded by `rustc` and by `slangc` is a one-ulp
risk sitting on the comparison that decides how many particles exist.

The twin test compares the closed form against the CPU accumulator at *every*
tick out to ten thousand. Swept to 200,000 the first disagreement is one
particle at tick 26,363 — 7.3 minutes of simulation, an order of magnitude past
the longest `--sim` in the repo — so ten thousand is inside the exactly-equal
region on purpose, and the test is a real gate rather than a tolerance.

### `N` is a correctness parameter, refused at load

Ordinals `n` and `n + N` share a slot and are born `N / rate` seconds apart. A
particle lives at most `lifetime · (1 + lifetime_jitter)`. A pool smaller than
`burst + rate · maxLifetime` therefore overwrites live particles, which reads
as the plume blinking — a visual artifact with nothing in the frame to blame it
on.

`loom_particles::pool_size` computes it and `loom_scene` refuses anything over
`GPU_POOL_MAX` **with the required number in the error**. That number is
authored in `loom_scene` rather than `loom_render`, because it is an authoring
constraint before it is a buffer size and `loom_scene` may depend on nothing
that could tell it; a test in `gpu_particles.rs` reads the file and compares,
which is the trick `rain.rs` already plays on the shader header.

### Additive only, and one pool per scene

Both are load-time refusals, not comments — the S4 lesson, applied before it
can bite:

| Refusal | The symptom it replaces |
| --- | --- |
| `gpu = true` needs `additive = true` | there is no GPU sort and none is planned, so a blended pool draws in slot order and reads as a scramble |
| pool over `GPU_POOL_MAX` | a live particle overwritten by its successor: the plume blinks |
| a second `gpu = true` emitter | one of them silently does not draw, which is the failure no gate here can see |
| `gpu = true` beside a `RigidBody` | a catch-up dispatch has ONE origin, so every particle born during `--sim N` is born where the node *ended*: a trail laid along the wrong path |

The last one is only detectable for the emitter's own node. A parent that moves
is the author's to avoid, and the component doc says so.

## What was built

- `assets/shaders/gpu_particles.slang` — one compute entry point.
- `crates/loom_render/src/gpu_particles.rs` — two device-local buffers (state,
  instances), one host-visible 144-byte parameter block, one pipeline with **no
  descriptor set at all**; everything it reads is a device address.
- `loom_particles::births_by` / `pool_size`, and the twin test.
- `ParticleEmitter.gpu`, defaulting to **false**, plus the four refusals.
- `assets/test/emberfall.loom`, in `SCENES` and `GOLDEN`.
- `cargo xtask repeat`, which is ADR 0045 clause 3's instrument and had to
  exist first.

### No second particle renderer, and that is the load-bearing choice

The dispatch writes `ParticleInstance`s — the record `scene.slang`'s
`particleVertexMain` already reads. So a GPU plume is billboarded, rotated,
blended, depth-tested and fogged by exactly the code a CPU plume is, and the
only difference at the draw is which pointer the push block carries. A second
renderer would be a second place for the two to look different, and the whole
value of a `gpu = true` switch is that it changes the *scale* and nothing else.

A dead slot writes a degenerate quad rather than being culled. Six vertices at
one point rasterise nothing, which is cheaper than any mechanism for telling a
shared vertex shader that a slot is empty.

**There is no indirect draw**, and it is not an omission. Rain has one because
its splash count is a GPU fact the CPU cannot know; the pool's slot count is a
CPU fact the CPU computed. An indirect draw arrives with a *cull*, via a
one-thread args pass like `rainSplashArgsMain`, and not before.

### The barrier is the graph's, buffers included

Compute writes the instance buffer; the vertex shader reads it in the same
command buffer. Without the dependency the draw shows last frame's particles,
which looks almost right — never-do #4's whole reason for covering buffers.
`pass_with((pool, ComputeReadWrite), (instances, ComputeReadWrite))` then
`(instances, VertexRead)` on the forward pass **and** on the water block, since
the particle draws move there when the pass splits. Read-after-read emits
nothing, so declaring it twice is free.

## Measurements

All on the RTX 4090 at 300 W, `emberfall` at 1920x1080, `LOOM_GPU_TIMING=1`.
16,200 slots; measured live population 12,083, which is `rate × lifetime` as
the sizing rule predicts.

```
gpu_particles, steady frame (1 tick)     0.008 ms
gpu_particles, catch-up (600 ticks)      0.677 ms   ≈ 1.13 µs/tick
forward pass (16k additive quads)        0.430 ms
```

**The whole `--sim 600` render, wall clock, release, same scene both ways:**

```
gpu = true    0.64 s
gpu = false   5.70 s
```

That five seconds is the CPU stepping 600 ticks of up to 12,000 particles
inside the fixed timestep. It is the trigger, measured rather than asserted: at
`smoke.loom`'s ~725 particles the CPU path is 0.02 ms a tick and this ADR would
be optimising something already free.

**The forward pass, not the simulation, is now the cost.** 0.43 ms of it is
16,200 additive quads' overdraw at 1080p. Anything that wants more particles
than this wants a cull before it wants a faster dispatch.

## Determinism

- **Nothing is read back, ever.** A GPU particle is invisible to
  `loom sim --assert` and to `rhai`, by construction (ADR 0045 clause 1). The
  component doc says so; it is not a gap to close later.
- **The sim hash is untouched.** GPU particles are not ECS entities, and
  `state_hash` is rigid bodies only.
- **Byte-identity across three fresh processes**, and it is now a gate rather
  than a claim: `cargo xtask repeat` renders every `GOLDEN` scene three times
  and compares the PNGs byte for byte. Derived from `GOLDEN` rather than from a
  hand-kept list of "the stateful ones", because a hand-kept list of scenes has
  gone stale three times in this project.
- **`gpu` defaults to false**, so all eight blessed `ParticleEmitter`
  references are untouched. Verified: the image gate reported 31 matches and
  one missing reference, and `MANIFEST.txt` gained exactly one line.

## What this deliberately is not

**It is not the same pixels as the CPU emitter, and it does not claim to be.**
The force chain is `System::advance` transcribed — swirl, gravity, wind
coupling, damping, Euler — but the random half cannot be: the CPU draws from a
serial seeded `Rng`, and the GPU hashes the birth ordinal. The swirl uses
`loom_value_noise` because that is the noise with a Slang half; the CPU's is
`loom_terrain::noise::value`. So `gpu = true` and `gpu = false` on one scene are
the same *effect* and two different plumes. `birthsBy` is the part that must
agree and is the part that is tested.

**The wind is unsheltered.** The CPU path samples `loom_field::wind::Wind`,
which applies S3's voxel-SDF occlusion; there is no GPU form of that march.
Grass takes the same unsheltered field for the same reason.

**It is one pool.** Not a Niagara. There is no module stack, no parameter map,
no simulation stages, no data interfaces — the research pass rejected all four
for want of a named authoring failure, and nothing here changes that. This is
one emitter's population moved to the device, gated on a measured trigger.

## What would fire the next decision

- **A scene needing two GPU emitters.** The refusal is honest but it is a
  refusal; a second pool is a `Vec` of buffers and a dispatch each, and the
  reason not to build it today is that nothing has asked.
- **A cull.** The forward pass is now the expensive half. A distance falloff
  like grass's, feeding an indirect draw from a one-thread args pass, is the
  shape — and it should be judged on `cargo xtask shimmer` **at the scene's
  authored camera**, because the last time a density falloff was measured
  against auto-framed bounds it won by deleting the subject.
- **An emitter on a moving node.** Wanted the moment something wants a rocket
  trail. The fix is a small ring of recent origins uploaded per frame, indexed
  by the tick a particle was born on; it is not hard, and it is not needed.
