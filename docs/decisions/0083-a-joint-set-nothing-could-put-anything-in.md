# ADR 0083 — A joint set nothing could put anything in

- **Date:** 2026-09-06
- **Status:** **accepted** — first of the nine subsystems the human scoped as
  "what is missing for a full-blown game engine". Built to the general-engine
  bar they chose: a registered component, a physics API, validation, a unit
  test, a scene, and two gate rows.
- **Decision touched:** none of CLAUDE.md's locked table. `rapier3d` is already
  the physics engine and voxel colliders are already the terrain; this adds
  nothing to that row, it uses a part of rapier the crate already carried.

## 1. The gap, and that it was a gap rather than a choice

`crates/loom_physics/src/lib.rs` has constructed an `ImpulseJointSet` and
stepped it since the crate was written. **Nothing in the workspace could put
anything in it.** There was no `Joint` component, no builder call, no path from
a `.loom` file to a constraint.

So every hinge in this project is a script writing a transform each tick. That
is not the same object:

- a scripted door does not **resist** — it cannot be leaned on;
- it carries no **momentum** — it cannot swing shut behind you;
- it cannot be **pushed by what hits it** — the boat that strikes a gate passes
  through the gate's authority and the gate follows its script regardless.

For a game whose whole physical vocabulary is a boat hitting things, that is a
real absence rather than a stylistic one.

## 2. Decision

A `Joint` component, on the node that moves, naming the node it attaches to.

    [node.components.Joint]
    kind = "revolute"          # fixed | revolute | spherical | prismatic
    connected = "Rig/Hull"     # empty anchors to the world
    anchor = [2.0, 0.0, 0.0]   # in this body's local frame
    other_anchor = [2.0, 0.0, 0.0]
    axis = [0.0, 0.0, 1.0]     # revolute/prismatic only, normalised on use
    limits = [-1.57, 1.57]     # radians or metres; absent means free

**It hangs on the moving part, not the anchor.** A cabin door carries the joint
and names the hull. That keeps the door's whole definition — mesh, collider,
body, constraint — in one place, and deleting the door deletes its hinge rather
than leaving a dangling reference in the hull.

**An empty `connected` anchors to the world**, built as a fixed body with no
collider (`Physics::add_world_anchor`). A swinging sign on an unsimulated wall
should not require authoring an invisible box to hang from.

**`kind` is a typed enum, not a string**, following `WaterKind`: serde and the
schema reject a misspelling at load, where a string would validate cleanly and
hinge about nothing.

**The axis is normalised rather than refused.** `[0, 2, 0]` is the same hinge as
`[0, 1, 0]`; refusing it is pedantry. A zero axis falls back to +Y, because a
hinge about nothing is not a constraint the solver can express and a silent
no-op is the failure this engine's registry exists to prevent.

## 3. Why free fall is the test

`assets/test/joint_pendulum.loom` releases a bob level with a hinge 2 m to its
+X. Held, it can never leave that radius: y stays inside [3, 5]. Unheld it is in
free fall, and ten seconds of that is `5 - 0.5 * 9.81 * 100 = -485`.

Measured, with the component deleted from the scene: **-485.706**. With it:
**4.458 at tick 120, 4.755 at tick 600.**

That is the point of choosing a pendulum over a door. There is no tolerance to
argue about, no calibrated threshold to re-pin when the hull's mass changes
again, and no way for a broken joint to resemble a working one. The gate's
second row — y below 4.9 at tick 120 — is what a *frozen* body fails: a static
bob satisfies the radius for ever and never swings.

The same claim is pinned again at the physics level, without a scene, in
`a_revolute_joint_holds_a_bob_against_gravity`. Two levels because the scene row
proves the *authoring path* and the unit test proves the *solver path*, and a
regression in either would otherwise be reported as the other.

## 4. What it does not do yet

- **No motors.** rapier's builders expose `motor_velocity` and
  `motor_position`; a powered hinge is what a winch or a hatch actuator needs.
  Not built because nothing has asked for one, and a motor with no consumer is
  the inert field this project has been burned by.
- **No rope or spring joints**, though rapier has both. A rope is expressible
  today as a chain of spherical joints; whether that is good enough is a
  question for whoever authors the first one.
- **Multibody joints are untouched.** `ImpulseJointSet` is the maximal-coordinate
  set and the one already stepped. `MultibodyJointSet` is a different solver with
  different stability, and adopting it is its own decision.
- **The load-time refusal is a warning, not an error.** A joint whose
  `connected` names a node with no dynamic body warns and is skipped. Promoting
  that to a validation failure belongs with the sanity checks in
  `loom_physics::sanity`, and wants its own pass over what else there is
  currently only a warning for.
