# ADR 0053 — The cinematic tier, and the end of universal determinism

- **Date:** 2026-08-17
- **Status:** **accepted** — the human approved this explicitly, in these words:
  *"they should be allowed to produce forces on rigid bodies. make it so. I want
  actual niagra VFX in loom. Whatever needs to be torn out, do so."*
- **Decisions touched:** this is the first ADR in the project's history to
  **amend a locked decision rather than state its edge**. It amends ADR 0045
  clauses 1 and 2 — the force-and-assertion rule, and the no-readback rule —
  by scoping them, and it changes CLAUDE.md's third founding property from a
  global invariant to a per-body one.
- **Supersedes in part:** ADR 0045 (clauses 1 and 2 gain a scope), ADR 0046
  (the CPU ripple grid is replaced, see §7).

## Context — the constraint was real, and what it was buying was narrower than it looked

CLAUDE.md's three properties are: everything authored is diffable text; the
agent can see and test its own work; the runtime is deterministic so those
assertions are trustworthy. The third exists to serve the second.

**The premise is being deliberately changed.** The project's stance moves from
*the LLM agent is the main author* to *the agent is a heavily-assisted
collaborator*. Under the old stance the agent's ability to assert on any
quantity was load-bearing, and determinism was the price of it. Under the new
stance it is not, and the human has decided the visual ceiling is worth more.

Two things were established before this was decided, and both are recorded here
so that nobody re-derives them:

**The determinism rule was serving two masters, and only one of them was the
agent.** `proving_ground.loom` has a deterministic event log, damage built on
it, and `GameRules` with win/lose conditions. Replays, save-and-reload and any
future networked play rest on the same property that `loom sim --assert` does.
The human was shown this distinction explicitly and chose to allow forces
anyway. That is a decision about what Loom is for, and it is theirs. §4 is
where the cost is written down rather than softened.

**Loosening buys less than it appears to.** ADR 0045 clause 1 already admits
*state advanced inside the fixed step*. A CPU grid that pushes a rigid body was
therefore always legal — the bow wave, the wake, advected foam, crowns,
waterfalls and drips in the panel's presentation core need **no** rule change.
What this ADR unlocks is specifically a **GPU** solver: water that parts
volumetrically, a dry hollow behind a hull, connected thrown sheets. Reference
images 61 and 65. It is a real gain and it is not a large one measured in
features; it is large measured in *look*, which is the thing being bought.

## Decision

**A `WaterBody` may opt into a `cinematic` simulation tier. Inside that tier the
fluid may be GPU-stateful, may be read back, and may produce forces on
`rapier3d` bodies. Outside it, ADR 0045 stands unchanged.**

### 1. The opt-in is per effect, in the scene text, and it defaults off

**The tier is a property of any cinematic effect, not of water.** Water is merely
its first user, because water is what was being rebuilt when the human approved
it. The same treatment is wanted for fire, smoke, rain and sparks, and the
mechanism is therefore **general from the start** rather than retro-fitted:

```toml
[node.components.WaterBody]
kind = "pool"
simulation = "cinematic"   # default "deterministic"

[node.components.ParticleEmitter]   # fire, smoke, sparks
simulation = "cinematic"

[node.components.Weather]           # rain
simulation = "cinematic"
```

`simulation` is **one shared type in `loom_scene`**, parsed and defaulted in one
place, not a field re-declared per component. A component-local copy is how a
second, subtly different meaning gets born, and this project has paid for that
pattern before.

**Most VFX do not need clause 1 relaxed, and it is worth being exact about
this.** Rain (ADR 0017), the GPU particle pool (ADR 0047) and marched smoke
(ADR 0020/0050) are *already* presentation-only, already outside the sim hash,
and already unreadable by `--assert`. Nothing about them is blocked by ADR 0045
today. What the tier actually buys them is narrower and should not be
overstated:

- **Forces.** A fire's updraft lifting debris, sparks that bounce off geometry
  and come to rest, rain that accumulates and drains, smoke that a passing body
  displaces. These are the genuinely new capability.
- **Readback**, and with it the ability for those effects to influence anything
  the CPU can see.
- **A grid gas solver**, which this project previously rejected *on evidence*.
  That rejection was made under ADR 0045's constraints; those constraints have
  moved, so the rejection is vacated for effects inside the tier and must be
  re-argued on its own merits rather than assumed either way.

What the tier does **not** buy is the visual ambition itself. Niagara-class
fire, smoke and sparks are mostly a *rendering and authoring* problem, and a
large fraction of that work needs no clause relaxed at all. Do not reach for the
tier where presentation would do; §4's costs are real and are paid per scene.

Default off is not politeness. Every scene in the repository today, every
blessed reference, every pinned hash and `proving_ground` itself must be
bit-identical after this lands, and that is an acceptance criterion, not a hope.

### 2. What the tier is allowed to do

- Hold state in device-local buffers advanced by compute dispatches.
- **Be read back**, synchronously, inside the fixed step (§3).
- Apply forces and torques to `rapier3d` bodies from that readback.
- Be excluded from `loom sim --assert` and from `rhai`. Assertions against a
  cinematic body's surface **fail loudly with a message naming this ADR**; they
  do not silently return a stale or approximate number. A quiet wrong answer is
  worse than a refusal, and this is where one would otherwise be born.

### 3. Determinism becomes machine-local, and that is the precise loss

This is the sentence the whole ADR turns on, and it is narrower than "water is
nondeterministic":

> A GPU dispatch is a deterministic function of its inputs **on the same device,
> driver and dispatch order**. It is not reproducible across hardware.

So:

- **Same machine, repeated runs: still reproducible.** `cargo xtask repeat` is
  expected to keep passing on cinematic scenes, and it is the gate that proves
  it. If it cannot, the solver is wrong — float atomics in a nondeterministic
  reduction order are the likely cause, which is why §5 forbids them.
- **Across machines: gone.** A hash pinned on this box means nothing on another.
  Any cinematic scene is therefore barred from the pinned-hash tests, and
  `cargo xtask validate`'s debug/release agreement check excludes them.
- **The readback is synchronous and inside the fixed step**, so *tick ordering*
  stays exact: tick N always reads the result of exactly N dispatches. What is
  lost is portability of the floats, not the sequencing. An asynchronous or
  frame-delayed readback would lose the sequencing too and is **forbidden**.

### 4. What is given up, written plainly

- **Cross-machine reproducibility for any scene using the tier.** A replay, a
  save file or a networked session involving cinematic water is not guaranteed
  to agree between two computers.
- **`--assert` and script access to that water.** By construction, refused loudly.
- **A device→host sync per tick** for bodies in the tier. This is the cost ADR
  0045 refused on performance grounds, now accepted for scenes that ask for it.
  It must be measured and reported, not assumed tolerable.
- **`repeat` byte-identity is at genuine risk for the first time.** Every other
  GPU-stateful effect in this engine rolls its recurrence up in registers per
  thread. A pressure projection is globally coupled and cannot.

### 5. Constraints that survive, and are not negotiable

These are what keep the loss to §4 rather than letting it spread:

- **No float atomics anywhere on the solver path.** Float addition is not
  associative, and a nondeterministic reduction order feeds the next tick's
  pressure solve. Integer fixed-point atomics, or gather-based transfer over a
  counting-sorted bucket list.
- **Multigrid, not PCG.** Every multigrid operator is a fixed local stencil; a
  PCG dot product is a float reduction with no fixed summation tree. This is a
  determinism requirement first and a performance one second.
- **APIC, not FLIP.** It deletes the PIC/FLIP blend ratio, a knob with no correct
  value whose failure mode — accumulating null-space energy, appearing as
  jitter, then interior voids, then instability — is the hardest thing to debug
  in a system nobody can read back cheaply.
- **The domain anchors to sim state, never to the camera.** ADR 0045's trap
  clause is untouched and applies with more force here, not less: a
  camera-following domain would make physics depend on where the viewer stands.
- **A deterministic body may never be forced by a cinematic one.** The tier does
  not leak. A rigid body touched by cinematic water is itself outside the
  reproducibility guarantee, and the scene loader refuses a scene that puts a
  `GameRules`-bearing body in cinematic water without an explicit acknowledgement
  field. Silently demoting a game's determinism is the failure this clause exists
  to prevent.
- **Nothing outside `loom_render*` imports `ash`.** The readback crosses the
  boundary as plain `f32` through a narrow, named interface, so the dependency
  rule that makes clause 2 structural stays intact for everything else.

### 6. The stale clause, fixed on its own merits

ADR 0045 §3 says *"Catch-up to `--sim N` is one dispatch."* That was already
inconsistent with shipped code: ADR 0046's CPU ripple grid is stepped state on
the force path and is nobody's one dispatch. Reworded to state the intent it
always had:

> Catch-up to `--sim N` is a fixed function of N alone — the same sequence of
> steps regardless of frames drawn, wall clock, or camera.

This would have been worth doing whether or not the cinematic tier existed.

### 7. What gets torn out

The human's instruction was *"whatever needs to be torn out, do so."* The list,
each item deleted in the slice that replaces it and never before:

- `crates/loom_water/src/ripples.rs` in full, the `Ripples` component,
  `COURANT_LIMIT`, `MAX_RIPPLE_CELLS` and the edge sponge. Only `pool.loom` and
  `wake.loom` author the table; migration is two files.
- `spray::column`, `COLUMN_*` and `loom_cli::particles::column_visual` — the
  48-quad splash all three judges independently rendered and called a grey
  mushroom.
- `RIPPLE_CALM`, `RIPPLE_FOAM_SLOPE` and `RIPPLE_FOAM_MAX` as standing shader
  terms; their mechanisms relocate where they are earned.
- `WATER_FOAM_STRETCH` along the global wind — direction becomes the local
  breaking eigenvector.

## Consequences

The engine now has two water tiers with different guarantees, and the tier is
visible in the scene text rather than inferred. That is the cost of the look,
and it is paid per scene by the author who wants it.

**The honest risk** is that `simulation = "cinematic"` spreads by default
because it looks better, and the reproducibility guarantee erodes scene by scene
until it is nominal. The counter is that it is one grep, and that the pinned
hashes, `--assert` scenes and `proving_ground` all continue to fail loudly if
they are moved into the tier. Whether that holds is worth re-reading in six
months.

**This ADR is reversible in one direction only.** Scenes authored against
volumetric water cannot be restored to a height field without being rebuilt.
