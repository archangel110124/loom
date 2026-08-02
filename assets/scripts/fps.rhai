// A first-person movement model.
//
// The engine sweeps the capsule and reports what it hit. Everything about how
// this character *feels* is in this file: how fast it walks, how sharply it
// starts and stops, how much control it keeps in the air, how high it jumps,
// and how forgiving the jump is at the edge of a ledge. Change a number here
// and the character changes with no Rust rebuilt.
//
// What it is told:      tick, dt, position, velocity, grounded,
//                       move_x, move_z, forward, right, jump, sprint
// What it answers with: velocity
// What it may keep:     memory, a map that survives to the next tick
//
// `forward` and `right` are flat — the look direction projected onto the
// ground — so looking at the sky does not make W walk into it.

// ---------------------------------------------------------------------------
// Tuning.
// ---------------------------------------------------------------------------

let WALK = 5.0;          // metres per second
let SPRINT = 8.5;
let GROUND_ACCEL = 60.0; // how hard it reaches that speed, and stops
let AIR_ACCEL = 12.0;    // much weaker: you commit to a jump
let GRAVITY = 24.0;      // heavier than 9.81, which reads as floaty in a game
let JUMP_SPEED = 8.0;    // about a 1.3 m hop at the gravity above
let COYOTE_TICKS = 6;    // jump grace after walking off a ledge, ~0.1 s
let BUFFER_TICKS = 8;    // jump pressed just before landing still counts

// ---------------------------------------------------------------------------
// Carried between ticks. Both of these are why `memory` exists: neither is
// derivable from a single instant, and both are the difference between a
// character that feels responsive and one that feels like it ignores you.
// ---------------------------------------------------------------------------

let airborne_for = if "airborne_for" in memory { memory.airborne_for } else { 999 };
let jump_buffered = if "jump_buffered" in memory { memory.jump_buffered } else { 999 };

if grounded { airborne_for = 0; } else { airborne_for += 1; }
if jump { jump_buffered = 0; } else { jump_buffered += 1; }

// ---------------------------------------------------------------------------
// Where the player is asking to go, in world space.
// ---------------------------------------------------------------------------

let wish_x = forward[0] * move_z + right[0] * move_x;
let wish_z = forward[2] * move_z + right[2] * move_x;

// Normalise, so pressing W and D is not 1.41x faster than pressing W. This is
// the oldest bug in first-person movement and it is always this line.
let wish_len = (wish_x * wish_x + wish_z * wish_z).sqrt();
if wish_len > 1.0 {
    wish_x /= wish_len;
    wish_z /= wish_len;
}

let speed = if sprint { SPRINT } else { WALK };
let target_x = wish_x * speed;
let target_z = wish_z * speed;

// ---------------------------------------------------------------------------
// Accelerate toward it. The same rule decelerates, so releasing the keys
// coasts to a stop rather than stopping dead — and in the air it barely
// applies, which is what makes a jump a commitment.
// ---------------------------------------------------------------------------

let accel = (if grounded { GROUND_ACCEL } else { AIR_ACCEL }) * dt;

let vx = velocity[0];
let vz = velocity[2];
let dx = target_x - vx;
let dz = target_z - vz;
let gap = (dx * dx + dz * dz).sqrt();

if gap <= accel || gap < 1e-6 {
    vx = target_x;
    vz = target_z;
} else {
    vx += dx / gap * accel;
    vz += dz / gap * accel;
}

// ---------------------------------------------------------------------------
// Vertical.
// ---------------------------------------------------------------------------

let vy = velocity[1] - GRAVITY * dt;

// Coyote time AND jump buffering. Together they cover both halves of "I
// pressed jump and nothing happened": pressing slightly after leaving the
// ground, and pressing slightly before landing on it.
if airborne_for <= COYOTE_TICKS && jump_buffered <= BUFFER_TICKS {
    vy = JUMP_SPEED;
    // Spend both, or the grace period fires again next tick and the character
    // climbs the air.
    airborne_for = COYOTE_TICKS + 1;
    jump_buffered = BUFFER_TICKS + 1;
}

// ---------------------------------------------------------------------------
// The weapon.
//
// A grenade launcher without the grenade: it detonates where the view ray
// lands, this tick. The host casts that ray — a script has no access to the
// physics world, and giving agent-authored code a live borrow of the
// simulation is not a trade worth making — and hands back `aim_point`.
//
// The cooldown is why `memory` exists here too: without it, holding the button
// would fire on every tick the binding reports, which is sixty explosions a
// second.
// ---------------------------------------------------------------------------

let BLAST_RADIUS = 5.0;
let BLAST_IMPULSE = 700.0;
let RELOAD_TICKS = 45;

let last_shot = if "last_shot" in memory { memory.last_shot } else { -9999 };

// `aim_hit` and not merely `fire`: detonating at the end of the ray's range
// when it struck nothing puts an explosion in empty sky, which reads as the
// weapon being broken rather than as a miss.
if fire && aim_hit && tick - last_shot >= RELOAD_TICKS {
    detonate = aim_point;
    detonate_radius = BLAST_RADIUS;
    detonate_impulse = BLAST_IMPULSE;
    last_shot = tick;
}

memory.last_shot = last_shot;

// ---------------------------------------------------------------------------
// Answer, and remember.
// ---------------------------------------------------------------------------

memory.airborne_for = airborne_for;
memory.jump_buffered = jump_buffered;

velocity = [vx, vy, vz];
