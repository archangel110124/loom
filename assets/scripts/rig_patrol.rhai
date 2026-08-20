// **The Rig's reference pilot.** It walks the hub end to end so that a headless
// run can assert the hub is walkable, because `loom sim` never calls
// `set_input` and headless input is zeros forever — the same reason
// `deeper_rules.rhai` carries five reference policies.
//
// It is a new file rather than `patrol.rhai` for one measured reason.
// `patrol.rhai` hops at 7.5 m/s against a gravity of 22, and
// `deeper_player.rhai` hops at 7.5 against 24. That is 1.28 m against 1.17 m,
// and the difference is the whole question the rail has to answer: a pilot that
// jumps *higher than the player can* condemns a rail the player could never get
// over, and one that jumps lower clears a rail that will not hold him. **The
// pilot must jump exactly as the player jumps**, so the two constants below are
// copied from `deeper_player.rhai` and have to move with it.
//
// (There is no way to say that once. A `Script` has a `path` and no `params`,
// so a shared model cannot be handed a different policy — the fight benches
// solve it by keeping every policy inside the one rules script, and a movement
// script has no equivalent because it cannot see the scene. `ponytail:` if a
// second hub-shaped pilot ever appears, that is the moment to ask for
// `Script.params` rather than to copy this a third time.)
//
// What it is told:      tick, dt, position, velocity, grounded
// What it answers with: velocity
// What it may keep:     memory

let SPEED = 4.2;         // deeper_player.rhai's WALK
let ACCEL = 60.0;        // its GROUND_ACCEL
let GRAVITY = 24.0;      // its GRAVITY  \  these two are the jump, and they
let JUMP_SPEED = 7.5;    // its JUMP_SPEED /  are what the rail is measured against
let JUMP_EVERY = 90;     // hop about every 1.5 s — often enough that a lap
                         // tries the rail from both directions
let BLOCKED_TICKS = 10;  // slowed for one tick is not a wall

let heading = if "heading" in memory { memory.heading } else { 1.0 };
let asked_for = if "asked_for" in memory { memory.asked_for } else { 0.0 };
let blocked_for = if "blocked_for" in memory { memory.blocked_for } else { 0 };
let airborne_for = if "airborne_for" in memory { memory.airborne_for } else { 999 };

if grounded { airborne_for = 0; } else { airborne_for += 1; }

// No raycast: if the last tick asked to move and the velocity that came back is
// a fraction of it, something absorbed the rest. It has to persist, because
// climbing anything costs speed for a few ticks without being a wall.
if grounded && asked_for != 0.0 && abs(velocity[0]) < abs(asked_for) * 0.3 {
    blocked_for += 1;
} else {
    blocked_for = 0;
}
if blocked_for >= BLOCKED_TICKS {
    heading = -heading;
    blocked_for = 0;
}

let target = heading * SPEED;
let step = ACCEL * dt;
let vx = velocity[0];
let delta = target - vx;
if abs(delta) <= step { vx = target; } else if delta > 0.0 { vx += step; } else { vx -= step; }

let vy = velocity[1] - GRAVITY * dt;

// **The player's own two water lines, copied for the same reason the jump
// constants are.** `deeper_player.rhai` floats a capsule that goes over the
// side and stops accumulating gravity while grounded; a pilot without either
// falls forever and cannot certify the guarantee the rig actually makes, which
// is not "you never get out" but "you always get back". These three lines and
// the four above are one set, and they move together.
if grounded { vy = velocity[1]; }
if !grounded && position[1] < 0.25 { vy = (-0.70 - position[1]) * 3.0; }

// Grounded only, and no coyote grace: a pilot that can jump off a ledge it has
// already left is measuring the grace period rather than the deck.
if grounded && airborne_for == 0 && tick % JUMP_EVERY == 0 && tick > 0 {
    vy = JUMP_SPEED;
    airborne_for = 1;
}

memory.heading = heading;
memory.asked_for = target;
memory.blocked_for = blocked_for;
memory.airborne_for = airborne_for;

// **Z is held at zero, deliberately.** The supplies stand in a line along X at
// z ~= +2.2 and the spawn lane is z = +0.90; a pilot that wandered in Z would
// make "did it pick the bait up" depend on where it happened to be standing.
velocity = [vx, vy, 0.0];
