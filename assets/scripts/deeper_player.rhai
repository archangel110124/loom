// **The player of DEEPER.** A movement model and nothing else.
//
// This is `fps.rhai` with the grenade launcher taken out, and taking it out is
// the whole reason the file exists: `fps.rhai` detonates a 5 m blast wherever
// the view ray lands when `fire` is pressed, and in a scene whose centrepiece
// is a forty-three tonne dynamic rigid body that means left-clicking throws the
// boat across the harbour. `fire` is also the binding fishing needs — the cast,
// the hook-set, the rhythm window (`fishing_angler.rhai`) — so it has to arrive
// here unspent.
//
// What it is told:      tick, dt, position, velocity, grounded,
//                       move_x, move_z, forward, right, jump, sprint, fire
// What it answers with: velocity, and events on `emit`
// What it may keep:     memory, a map that survives to the next tick
//
// **A node `Script` is the only thing in this engine that sees a key press.**
// `GameRules` sees positions and events and no input at all. So anything the
// player *presses* has to enter the game here and leave as an event, which the
// rules script reads on the same tick — the motion pass runs before the rules
// pass, so nothing is a frame stale. Nothing presses anything yet; the hold is
// filled by walking up to a supply, which needs no binding.
//
// **`velocity` decelerates toward zero in the frame of what the character is
// standing on** (ADR 0060), which is why this file is correct on the rig's
// static deck and on the boat's heaving one without knowing which it is on.

// ---------------------------------------------------------------------------
// Tuning. The whole feel of the character is these seven numbers.
// ---------------------------------------------------------------------------

let WALK = 4.2;          // metres per second. Slower than the arena's 5.0:
                         // the rig is 24 m across and a hub is somewhere you
                         // stand about in, not somewhere you clear.
let SPRINT = 7.0;
let GROUND_ACCEL = 60.0; // how hard it reaches that speed, and stops
let AIR_ACCEL = 12.0;    // much weaker: you commit to a jump
let GRAVITY = 24.0;      // heavier than 9.81, which reads as floaty in a game
let JUMP_SPEED = 7.5;    // about a 1.2 m hop at the gravity above — enough to
                         // clear the boat's bulwark from the wharf, not enough
                         // to hop the rail and swim
let COYOTE_TICKS = 6;    // jump grace after walking off a ledge, ~0.1 s
let BUFFER_TICKS = 8;    // jump pressed just before landing still counts

// ---------------------------------------------------------------------------
// Carried between ticks. Neither is derivable from a single instant, and both
// are the difference between a character that feels responsive and one that
// feels like it ignores you.
// ---------------------------------------------------------------------------

let airborne_for = if "airborne_for" in memory { memory.airborne_for } else { 999 };
let jump_buffered = if "jump_buffered" in memory { memory.jump_buffered } else { 999 };

if grounded { airborne_for = 0; } else { airborne_for += 1; }
if jump { jump_buffered = 0; } else { jump_buffered += 1; }

// ---------------------------------------------------------------------------
// Where the player is asking to go, in world space. `forward` and `right` are
// flat, so looking at the sky does not make W walk into it.
// ---------------------------------------------------------------------------

let wish_x = forward[0] * move_z + right[0] * move_x;
let wish_z = forward[2] * move_z + right[2] * move_x;

// Normalise, so pressing W and D is not 1.41x faster than pressing W.
let wish_len = (wish_x * wish_x + wish_z * wish_z).sqrt();
if wish_len > 1.0 {
    wish_x /= wish_len;
    wish_z /= wish_len;
}

let speed = if sprint { SPRINT } else { WALK };
let target_x = wish_x * speed;
let target_z = wish_z * speed;

// Accelerate toward it. The same rule decelerates, so releasing the keys
// coasts to a stop rather than stopping dead.
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
// Vertical. Coyote time AND jump buffering: together they cover both halves of
// "I pressed jump and nothing happened".
// ---------------------------------------------------------------------------

let vy = velocity[1] - GRAVITY * dt;

if airborne_for <= COYOTE_TICKS && jump_buffered <= BUFFER_TICKS {
    vy = JUMP_SPEED;
    // Spend both, or the grace period fires again next tick and the character
    // climbs the air.
    airborne_for = COYOTE_TICKS + 1;
    jump_buffered = BUFFER_TICKS + 1;
}

memory.airborne_for = airborne_for;
memory.jump_buffered = jump_buffered;

velocity = [vx, vy, vz];
