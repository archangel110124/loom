# ADR 0045 — The VFX determinism line

- **Date:** 2026-08-17
- **Status:** **accepted**
- **Numbering:** 0023–0042 are reserved by `docs/design/editor/PLAN.md` for the
  editor rework, which is on hold rather than cancelled. The next free number
  above every consumed one is 0045; VFX ADRs continue upward from here so the
  two tracks cannot collide when the editor resumes.
- **Decision touched:** none of CLAUDE.md's locked decisions moves. This ADR
  *states* where the existing determinism property's edge is, so that the fire,
  smoke and water work has a rule to build against instead of rediscovering one
  per milestone. It refuses one recipe from
  `docs/design/VFX-IMPLEMENTATION-REPORT.md` §4.5 milestone 4 by name.
- **Applies to:** everything in the VFX overhaul — water shading, whitecaps,
  ripples, marched smoke, spray, the GPU particle pool, and any FFT tier.

## Context — three properties, and only one of them is at risk

CLAUDE.md's third property is that the runtime is deterministic *so that the
agent's assertions are trustworthy*. The determinism that is actually gated is
narrow and worth naming precisely, because every VFX proposal so far has been
argued against a vaguer version of it:

- **`cargo test`'s determinism hash is `self.physics.state_hash()`**
  (`crates/loom_cli/src/play.rs` → `crates/loom_physics/src/lib.rs:1067`):
  rigid-body bit patterns in handle order. Nothing else is in it.
- **`cargo xtask image` is the gate on everything visual.** Grass, rain,
  particles and water surface pixels are checked there and nowhere else.
- **`loom sim --assert` and `rhai`** read quantities the CPU computed — wave
  height, wind speed, rain rate — and are the reason those quantities have CPU
  implementations at all.

So there are two different guarantees with two different enforcement
mechanisms, and the mistake available to every milestone below is to reason
about one while changing the other.

## Decision

**The sim hash stays physics-only and does not grow.** Adding water, fire or
smoke state to `state_hash` would convert every visual tune into a hash re-pin
across the repo, which is the churn `MANIFEST.txt` and the per-path golden
scenes exist to keep readable. The four clauses below are what replaces
growing it.

### 1. The force-and-assertion rule

> Anything that produces a force on a `rapier3d` body, or that is readable by
> `loom sim --assert` or by `rhai`, is computed **on the CPU** as a
> deterministic function of (scene, tick).

Two shapes satisfy it, and both are admissible:

- **Closed form at a point.** `loom_water::sample_water` is the model: it can
  be asked for the surface at any `(x, z, t)` with no history, which is exactly
  why buoyancy can sample a pontoon anywhere and why an assertion can name a
  coordinate the renderer never drew.
- **State advanced inside the fixed step**, under never-do #7 (no `HashMap`
  iteration, no `thread_rng`) and never-do #8 (no wall clock). `rapier3d`
  itself is this. A future CPU ripple grid would be this.

The rule is deliberately *not* "must be closed-form": stepped CPU state is
exactly as reproducible as the physics engine it feeds, and forbidding it would
rule out interactive ripples for no gain. It is deliberately *not* "may not
produce a force" either: an assertion that reads a GPU-only quantity is as
broken as a force that does, and trustworthy assertions are the engine's
premise.

### 2. GPU floats never cross that line

**No readback, ever.** Two files already state this as a structural rule and
the shipped code obeys both:

- `crates/loom_water/src/lib.rs:11-18` — "Never GPU readback… readback timing
  is not reproducible, so `loom sim --assert` would go flaky, and the latency
  makes buoyancy lag the surface visibly."
- `assets/shaders/rain_sim.slang` — "Nothing here is read back, ever."

**The report's milestone-4 recipe — "buoyancy sampling that reads back the
height field" — is refused.** It is not a missing feature: buoyancy is already
built the correct way, on CPU pontoons sampling `sample_water` from inside the
fixed step (`crates/loom_cli/src/play.rs`, `loom_water::buoyancy::solve`).
Adding the readback would be a regression that also costs a device→host sync
per tick.

Enforced structurally rather than by convention: the existing "nothing outside
`loom_render*` imports `ash`" rule already means a crate that can produce a
force cannot see a GPU buffer, and `scripts/check-deps.sh` is where any new
VFX-facing crate gets its stanza. A GPU-only quantity reaching a force should
be a build failure, not a code-review catch.

### 3. GPU-stateful rendering effects are admissible, under ADR 0017's
conditions, now made a gate

Rain is the precedent and it is a good one. The conditions it meets, which any
future GPU-stateful path (a foam accumulator, a pool of particles, an FFT tier)
must also meet:

- **Seeding is a pure function of (scene, index).** No clock, no allocator
  leftovers.
- **Catch-up to `--sim N` is one dispatch**, so a headless still is always
  seed-then-advance-N and never depends on how many frames were drawn.
- **No atomic on any seed path.** Atomic resolution order picks the slot, the
  slot picks the seed, and the seed is in the golden image. Atomics are
  permitted only for counts that nothing in the same dispatch reads — the
  `rainSplashArgsMain` pattern, where the cursor is consumed by a *later* pass
  with a barrier between.
- **Byte-identity across processes, proven rather than asserted.**

That last condition had no instrument. ADR 0017 claimed byte-identity
"verified across three processes"; it was verified once, by hand, on one
RTX 4090, and it rests in part on additive blending being order-independent —
which float addition is not. **`cargo xtask repeat` is that instrument** and it
lands before the next stateful path does: three fresh processes per stateful
scene, byte-compared.

### 4. The FFT ocean, if it is ever built, has exactly one admissible shape

Neither rejected outright nor scheduled. A Tessendorf tier is admissible only
as a **force-free detail tier**:

- Windowed from the *same* Pierson–Moskowitz spectrum the Gerstner set is drawn
  from, above an ω_split, so that `m0_low + m0_high = m0` is a CPU-checkable
  test and the "boat floats on water it is not drawn on" divergence is bounded
  to chop a hull integrates away.
- **Stateless in `t`**: `h̃(k,t)` evaluated from `h̃₀`, never accumulated.
- Gaussian draws by Box–Muller on the frozen `loom_field::noise` hash, never
  from a crate (ADR 0006 — a crate bump must never change the sim hash).
- **Buoyancy keeps reading the Gerstner tier through `sample_water`,
  unchanged.**

Building it needs its own ADR *and* flythrough evidence at authored cameras
that the Gerstner sea visibly tiles — a quality argument made on the
instrument, the same way the grass compute pass was deferred on GPU timestamps.

### The trap neither design flagged: a sim grid must not follow the camera

`docs/design/VFX-IMPLEMENTATION-REPORT.md` §2.2a prescribes "centre the grid on
the camera; when it scrolls, reproject by an integer-cell shift". That is
correct for a *rendering* ripple texture and **forbidden for any grid that
produces a force**: it would make the force on a floating body depend on where
the viewer is standing, so `loom render --sim N` and `loom run` would compute
different physics and the determinism hash would go viewer-dependent — the
exact failure this engine exists to prevent.

> **A force-producing CPU sim grid anchors its domain to sim state** — the
> water body, or the tracked bodies — **never to the camera.**

## Consequences

- Milestones that only draw (refraction, whitecaps, traced water reflection,
  marched smoke, spray, the GPU particle pool) are gated by `cargo xtask image`
  and, when stateful, by `cargo xtask repeat`. They need no hash re-pin and
  they may not be read by an assertion.
- Milestones that feed forces (ripples) are CPU, in the fixed step, and get
  their own ADR because they put new state on the force path.
- `WaterSample::fold` stays the queryable whitecap quantity precisely because
  it is closed-form; a foam *accumulator*, if one is ever added, is new state
  and rendering-only, and takes the `repeat` gate.
- **GPU particles are documented as unreadable by `--assert`.** That is not a
  gap to close later; it is clause 1 applied. A particle a script must reason
  about is a CPU particle, which is what `loom_particles` already is.

## What this does not settle

Whether any of the deferred items are worth building. This ADR says what shape
each is allowed to take *if* it is built, so that the shape is not relitigated
per milestone. The scheduling argument is in the work order, on evidence.
