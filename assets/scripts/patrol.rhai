// A movement model for a walking character.
//
// The engine does not have one of these. It owns collision — sweeping the
// capsule, sliding it along walls, stepping it up ledges, deciding what counts
// as ground — and then asks this file, every tick, how fast the character
// wants to be going. Everything below is a choice, and every one of them can
// be different in the next file without touching a line of Rust.
//
// What it is told:      tick, dt, position, velocity, grounded
// What it answers with: velocity
// What it may keep:     memory, a map that survives to the next tick
//
// `velocity` arriving is not the velocity that was asked for last tick — it is
// the one that SURVIVED. Walk into a wall and the component into the wall is
// gone; stand on the floor and the accumulated fall is gone. That feedback is
// what lets this script notice it is blocked without casting a single ray.

// ---------------------------------------------------------------------------
// Tuning. The whole personality of the character is these seven numbers.
// ---------------------------------------------------------------------------

let SPEED = 4.5;        // metres per second on flat ground
let ACCELERATION = 45.0; // how hard it reaches that speed
let AIR_CONTROL = 0.3;   // fraction of that authority while airborne
let GRAVITY = 22.0;      // heavier than 9.81: real gravity feels floaty
let JUMP_SPEED = 7.5;    // roughly a 1.3 m hop at the gravity above
let COYOTE_TICKS = 6;    // grace period for jumping after walking off an edge
let JUMP_EVERY = 100;    // this character hops periodically, to show it can
let BLOCKED_TICKS = 10;  // how long to stay stuck before believing it is a wall

// ---------------------------------------------------------------------------
// State carried between ticks. None of this is derivable from one instant,
// which is the entire reason `memory` exists.
// ---------------------------------------------------------------------------

let heading = if "heading" in memory { memory.heading } else { 1.0 };
let airborne_for = if "airborne_for" in memory { memory.airborne_for } else { 0 };
let asked_for = if "asked_for" in memory { memory.asked_for } else { 0.0 };
let blocked_for = if "blocked_for" in memory { memory.blocked_for } else { 0 };

// Coyote time. Jumping is allowed for a few ticks after the ground goes away,
// because a human presses jump slightly after walking off a ledge and a
// controller that refuses feels unresponsive rather than strict.
if grounded {
    airborne_for = 0;
} else {
    airborne_for += 1;
}

// ---------------------------------------------------------------------------
// Turn around when something is in the way.
//
// No raycast: if the last tick asked to move and the velocity that came back
// is a fraction of it, something absorbed the rest.
//
// **Being slowed for one tick is not a wall.** Climbing a step costs speed for
// a few ticks while the capsule is lifted over the lip, and the first version
// of this read that as an obstacle and turned around at every staircase. So it
// has to be stuck for a while before it believes it.
// ---------------------------------------------------------------------------

if grounded && asked_for != 0.0 && abs(velocity[0]) < abs(asked_for) * 0.3 {
    blocked_for += 1;
} else {
    blocked_for = 0;
}

if blocked_for >= BLOCKED_TICKS {
    heading = -heading;
    blocked_for = 0;
}

// ---------------------------------------------------------------------------
// Horizontal: accelerate toward the target rather than snapping to it. The
// same rule slows the character down, so this is friction as well — a target
// of zero decelerates at exactly the same rate.
// ---------------------------------------------------------------------------

let target = heading * SPEED;
let authority = if grounded { ACCELERATION } else { ACCELERATION * AIR_CONTROL };
let step = authority * dt;

let vx = velocity[0];
let delta = target - vx;
if abs(delta) <= step {
    vx = target;
} else if delta > 0.0 {
    vx += step;
} else {
    vx -= step;
}

// ---------------------------------------------------------------------------
// Vertical: gravity always, a jump when it is allowed and wanted.
// ---------------------------------------------------------------------------

let vy = velocity[1] - GRAVITY * dt;

if airborne_for <= COYOTE_TICKS && tick % JUMP_EVERY == 0 && tick > 0 {
    vy = JUMP_SPEED;
    // Spend the coyote grace, or the next few ticks jump again mid-air.
    airborne_for = COYOTE_TICKS + 1;
}

// ---------------------------------------------------------------------------
// Answer, and remember.
// ---------------------------------------------------------------------------

memory.heading = heading;
memory.airborne_for = airborne_for;
memory.asked_for = target;
memory.blocked_for = blocked_for;

velocity = [vx, vy, 0.0];
