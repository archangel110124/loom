# ADR 0060 — A character stands in the frame of what it stands on

- **Date:** 2026-08-19
- **Status:** **accepted**
- **Extends:** ADR 0045, whose clause 2 this stays inside (the whole mechanism
  is CPU rapier state, a deterministic function of scene and tick); and ADR
  0053 §5, whose leak this closes two more ways.
- **Applies to:** `Physics::move_character`, `Physics::add_character`,
  `CharacterMove::velocity`, the `BoxCollider` arm of `Sim::new`, and the
  `local_x` / `local_y` / `local_z` axes of `loom sim --assert`.

## The request

> "how is the player gonna be able to move around on the ship? … we also need
> to make sure that the player is not gonna be slipping and sliding around on
> the ship due to the fact that it's two objects on top of each other that are
> waving around due to the water physics."

## Three decisions, and only the first is interesting

### 1. `Motion.velocity` becomes ground-frame

`move_character` now casts one short ray past the capsule's foot while the
character is grounded, and if it lands on a collider with a rigid-body parent it
takes `velocity_at_point` **at the hit point** — `linvel + ω×r`, the same call
buoyancy damping has trusted per pontoon since the water work. That vector is
added to the requested motion before the sweep and subtracted from the reported
one after it.

The consequence is a **semantic change to a documented contract**, which is why
this is an ADR and not a commit message. `CharacterMove::velocity` used to be
"the velocity that survived collision", in world space. It is now that velocity
*in the frame of whatever the character is standing on*. On static scenery the
two are identical, because `add_static_box` inserts parentless colliders and
`collider.parent()` is `None` for every one of them — so the carry is
structurally zero, not numerically small, for every scene that existed before
this. The five pinned character-scene hashes are unchanged and that is the
proof.

What it buys: `fps.rhai` is correct **unmodified**. A movement model that
decelerates toward zero was, on a heaving deck, decelerating toward *the sea's*
zero and sliding overboard; decelerating toward zero in the deck's frame is
standing on the deck. No script in this repository knows this happened.

**Why the addition is gated on last tick's `grounded` and not on the probe
hitting something.** A jump adds the carry on the tick it leaves — so the player
departs with the deck's motion, which is what a jump off a moving boat does —
and then the sweep leaves the ground, so nothing is subtracted, so the impart
happens exactly once. Ungated, the second tick of a jump would add the carry
again while the probe still reached the deck, and hand out a free horizontal
kick of up to ~1.25 m/s at the rail. One condition, no stored state, no
launch-shaped edge case.

**Why the carry is folded into `requested` and never written to the position.**
`check_and_fix_penetrations` is an empty stub in vendored rapier 0.34 and
`move_shape` ignores a shape it already penetrates, so a position write of a few
centimetres a tick can start the next sweep inside the bulwark. Folding it in
gets collide-and-slide on the boat's motion for free.

Rapier's own moving-platform path is not used: it filters `rb.is_kinematic()`
and the hull is dynamic, so it is dead code here.

### 2. A character no longer shoves dynamic bodies

The capsule's collider gets `solver_groups(InteractionGroups::none())`. A
kinematic body is infinite-mass to the solver, so an 80 kg player standing on a
44-tonne hull won every argument with it: measured **+33.4° of roll at wind 12
and a full inversion at wind 40**, from a rider standing still. Solver groups
remove contact *forces* only — queries, blasts, shots, navigation and the
controller's own sweep all still see the capsule.

The behaviour lost is that walking into a crate used to nudge it. Nothing in the
repository depended on it (`proving_ground`'s hash is unchanged), and shoving is
better as an explicit script impulse than as a side effect of infinite mass.

### 3. Colliders compound onto a dynamic ancestor

A `BoxCollider` on a child of a dynamic node now becomes a collider **on that
body**, posed in the body's frame, instead of being silently ignored. It is
inserted with `ColliderMassProps::Mass(0.0)`, which rapier resolves to
`MassProperties::default()` — exact zero mass, zero inertia, no centre-of-mass
shift. That is load-bearing rather than tidy: `jib_vi_painted` authors
`mass = 43776` and every stability number in its header (rights from 75°, GZ
0.70 m at 21°, GM 2.7 m) was measured with the hull box's inertia alone.

**Two things about proving that turned out to be false, and both were going to
be quoted as gates.**

*The printed `state_hash` is not a physics hash.* `loom sim` prints
`World::state_hash`, which eats every entity's index, generation, **name** and
global matrix. Adding twenty-three nodes to a scene moves it whatever they
weigh — and an empty node with no components at all moves it too, which is how
this was found. "The three boat hashes are byte-identical with the deck
attached" is not a statement that can be true, and a gate written that way
fails on every asset addition while saying nothing about mass. The physics-only
`Physics::state_hash` exists one layer down and is not what the CLI reports.

*The hull's trajectory is not bit-identical either, and it is still not a
leak.* Read off the body: `inv_mass` is unchanged exactly, `local_com` is
unchanged exactly. What moves is `inv_principal_inertia` and
`principal_inertia_local_frame` — attaching colliders re-runs the symmetric
eigendecomposition, and with more terms in the sum it lands on a different,
equally valid ordering of the *same* axes, permuted with a 90° frame
compensating. Same tensor, about one ULP apart in representation. Over 1800
ticks on a 44-tonne hull that comes out as **2.6e-8 m** of position difference.
Thirty nanometres, against a header that quotes centimetres.

So the tripwire is the test, not a hash: mass and centre of mass compared
**exactly**, the world-space inverse inertia compared to a relative 1e-5, and
the hull's own rigid transform — position plus three markers parented at
(5,0,0), (0,5,0), (0,0,5), which is the full 4x4 — compared with and without
the deck at ticks 300, 600 and 1800. A real leak moves `inv_mass` by orders of
magnitude and cannot hide under either threshold.

The arm is deliberately narrow — an **explicitly authored** `BoxCollider` under
a dynamic ancestor, never `is_renderable` — so it is a provable no-op on every
scene that existed before it. Relaxing that would also change what the cinematic
fluid solver bakes as obstacles, and that is a separate decision.

**The rain bake skips them in the same commit.** It freezes every `BoxCollider`
in the scene at its load pose; without the skip, a boat's deck plates become
rain-blocking slabs sitting in the sea wherever the hull happened to start.
Recorded consequence: **rain falls through a boat's deck.**

## What this closes in ADR 0053 §5

The cinematic tier's leak had one door and now has three, because a rider is a
new way for GPU floats to reach something that reads like a simulation fact:

- `loom_scene` refuses `simulation = "cinematic"` beside a `CharacterController`
  without `acknowledge_nondeterminism`, exactly as it already did beside
  `GameRules`.
- `loom sim --assert` refuses a node assertion on a floating body **or any
  descendant of one** in a cinematic scene, alongside the existing refusal of
  `water@…`.

Both are the same sentence: a number that came off this GPU must not be read as
a claim about the simulation.

## `local_x` / `local_y` / `local_z`

`--assert` had x, y and z, all global. A character on a boat is at a world
position that heaves, rolls and pitches, so global `y` cannot say which deck it
is on and global `z` cannot say whether it is still inboard. The local axes read
the node's own transform — which `drive_characters` and `write_back` already
write in the parent's frame every tick — so a claim about walking a deck is
made in the deck's frame. No new state and no second inversion.

## Rejected

- **A stored `base` field plus an explicit impart on departure.** State, and a
  one-tick base flap at the rail fires a spurious impulse, in exchange for about
  a millimetre a tick of accuracy against a five-centimetre gate.
- **A kinematic twin of the hull.** Rapier's platform path sums over contact
  manifolds in broad-phase order, which is the non-associative reduction ADR
  0053 §5 forbids, and it is 58% effective on pure translation anyway.
- **Mesh-derived colliders (`voxelized_mesh`, VHACD).** They make the sim hash a
  function of the art file, and the art file changed while this was being
  designed.
- **Making the hull kinematic.** Destroys buoyancy.
- **Raising `step_height` so the drawn ladder is climbable.** It clears the
  rungs and not the 0.08 m tread, and it walks up walls everywhere else.
