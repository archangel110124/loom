# ADR 0062 — Place is a script on the CPU; shape is a wave on the GPU

- **Date:** 2026-08-19
- **Status:** **proposed** — needs human approval before merge.
- **Constrains:** nothing. It *amends* ADR 0045 with one fact the ADR does not
  state, and records where an animation system sits relative to that line.
- **Applies to:** the `Deform` component and anything that animates geometry
  after it.

## The amendment: there are two hashes, and only one of them is gated

ADR 0045 opens by naming `cargo test`'s determinism hash as
`self.physics.state_hash()` — rigid-body bit patterns in handle order, nothing
else. **That is correct as written and it is not stale.**
`crates/loom_cli/src/play.rs:1844` is literally:

```rust
pub fn state_hash(&self) -> u64 { self.physics.state_hash() }
```

But it is not the only hash, and it is not the one the green gate actually
compares. `loom sim` *prints* `World::state_hash()`
(`crates/loom_cli/src/main.rs:3331`), which is a different function
(`crates/loom_ecs/src/lib.rs:786-808`): per entity, in creation order, it eats
the index, the generation, the name bytes and **all sixteen floats of
`GlobalTransform`**. `xtask`'s `determinism_holds` shells out to
`loom sim --ticks 300` under both build profiles and string-scans that JSON
for `"state_hash"`, so **the printed world hash is the one `cargo xtask
validate` gates, and it contains every node's transform.**

The practical consequence, which is why this is written down rather than left
to be rediscovered: **moving a node is in a gated hash even when the node has
no rigid body.** A script that writes `position[0]` on a plain node changes a
number that debug and release must agree about. Deforming a node's *vertices*
in a shader changes no hash at all, because vertices are not entities.

Nobody may write "ADR 0045's opening sentence is stale" in a commit message or
a design document. It is not. This is a different, additional fact.

## The decision

An animated creature is split in two, along that line:

| | where it runs | in `World::state_hash`? |
| --- | --- | --- |
| **Place** — where the animal is | a `rhai` `Script` on the CPU, fixed step | **yes** |
| **Shape** — what shape it is | a wave in the vertex shader | **no** |

`Deform` is the shape half: one travelling sinusoid down a body's long axis,
evaluated in `vertexMain` beside the existing `windBend`, from two `float4`
lanes on `ObjectData`. It is **rendering only** and holds the same ADR 0045
exemption grass, rain and `windBend` already hold — **"not an entity", not
"not physics"**:

- **Clause 1** — it produces no force (it never reaches `rapier3d`; a collider
  on the same node keeps the rest shape, which is a stated consequence rather
  than a defect), it is not readable by `loom sim --assert` (whose vocabulary
  addresses *nodes* and returns the node's global matrix, which the deform does
  not touch), and it is not readable by `rhai` (`NodeState` carries position,
  rotation and scale and nothing else).
- **Clause 2** — there is no readback and none is added. No crate outside
  `loom_render*` gains an `ash` import.
- **Clause 3** — does not apply. The deform is **stateless**: a pure function
  of (packed vertex, `ObjectData`, `weather.z`). No device buffer accumulates
  across frames, no atomic, no seeding, no allocator order. `cargo xtask
  repeat` therefore passes trivially, and that is not a coincidence to be
  grateful for — it is what "pure function of (scene, tick)" means.
- **ADR 0053's cinematic tier is not reached for.** Reaching for it here would
  be the exact error that ADR's §1 names: *do not reach for the tier where
  presentation would do.*

## What this rules out, and the trigger that reopens it

**The wave does not join `loom_field::all()`.** Three reasons, each checked:
`windBend` is already hand-written Slang that displaces vertices, so the
precedent is in the shipped source; there is no CPU consumer of a vertex
offset; and membership buys no test, because `field_agree.rs` hardcodes
`loom_field::wind()` and `loom_field::clouds()` and does **not** loop `all()`
— a new field would go green in that test having never been evaluated.

*Trigger to revisit:* a socket, a hitbox or a `loom sim --assert` needs the
deformed tail's position. At that point the wave moves into `loom_field`,
`field_agree.rs` gains a hand-written entry point, and it gets its own ADR.

**No acceleration-structure work.** Every ray — reflection, shadow, RTAO — sees
the rest pose, because the TLAS holds a separate never-touched vertex copy.
Bounded rather than argued: at `gleamsprat.loom`'s authored settings the chrome
shell displaces by **0.23 mm** on a 161 mm animal.

*Trigger:* a deformed mesh's reflection or shadow becomes the subject of a
shot, or a scene displaces a specular surface by more than about 2 mm.

**No `kind` enum, no layer stack, no clip, no keyframe track, no skeleton.**
One animation exists in this repository. A `kind` field with one variant is
never-do #12 wearing a hat; the generality lives in the numbers instead — a
long `wavelength` is a sway, a short one with a small `amplitude` is a shiver,
and the same formula covers an eel, a tentacle, a flag and kelp.

*Trigger for skinning specifically:* the `.obj` precedent covers *text a human
can read a vertex out of*; it does not stretch to an opaque binary weight
array. If a character outgrows a formula, the next rung is **morph targets** —
another `.obj` with the same vertex order, which `import_obj` already parses —
not skinning.

## The asymmetry, stated rather than discovered

`weather.z` is `--sim N / 60` headless and a free-running clamped wall clock in
`loom run`. So in the viewer a `Deform` beats on wall time — the same deal
grass and every tree already have, for the documented reason that foliage
frozen until you press Play is wrong in an editor.

It means a **rate** error is invisible to the human's window *and* to any
single still. The only thing that can catch one is a second golden reference
far from the first: 1% of 3 Hz is 1.3° of phase at tick 7 and 24° at tick 133.
That is why `gleamsprat` is gated at two ticks 126 apart and not at two ticks
ten apart.
