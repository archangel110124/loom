// An enemy: hunts the player, remembers where it last saw them, gives up.
//
// The engine tells it what it can perceive — whether the player is visible,
// where, how far, and the next step along a route it asked for. All of those
// are casts, and a script has no physics world. Everything it *does* with them
// is here, and none of it is in Rust:
//
//   - that seeing the player means chasing
//   - that losing sight means going to where they were, not giving up at once
//   - that giving up means going home
//   - that being close enough is an attack, and how often it can attack
//
// What it is told:      tick, dt, position, velocity, grounded,
//                       can_see_target, target_at, target_distance,
//                       path_next, path_found
// What it answers with: velocity, goal, emit
// What it may keep:     memory

let SPEED = 3.4;
let CHASE_SPEED = 4.6;
let ACCEL = 30.0;
let GRAVITY = 24.0;
let REACH = 1.9;          // metres — close enough to hit the player
let ATTACK_TICKS = 40;    // cooldown between swings
let ATTACK_DAMAGE = 18.0;
let GIVE_UP_TICKS = 240;  // four seconds hunting a place they are not

// ---------------------------------------------------------------------------
// Memory. An enemy with none is either omniscient or blind: it would know
// where the player is at all times, or forget the instant they step behind a
// crate. Both read as broken, and the difference between them is these
// three values.
// ---------------------------------------------------------------------------

let home = if "home" in memory { memory.home } else { position };
let last_seen = if "last_seen" in memory { memory.last_seen } else { [] };
let hunting_for = if "hunting_for" in memory { memory.hunting_for } else { 0 };
let last_attack = if "last_attack" in memory { memory.last_attack } else { -9999 };

// ---------------------------------------------------------------------------
// Decide where to be.
// ---------------------------------------------------------------------------

if can_see_target {
    last_seen = target_at;
    hunting_for = 0;
} else if last_seen.len() == 3 {
    hunting_for += 1;
    if hunting_for > GIVE_UP_TICKS {
        // Lost them. Go back to the post rather than standing in a corridor.
        last_seen = [];
    }
}

let destination = if last_seen.len() == 3 { last_seen } else { home };
goal = destination;

// ---------------------------------------------------------------------------
// Walk there.
//
// Toward the next waypoint the host handed back, not toward the destination:
// walking at the goal directly is what makes an enemy stand grinding against
// the wall between it and the player.
// ---------------------------------------------------------------------------

let step_x = 0.0;
let step_z = 0.0;

if path_found {
    step_x = path_next[0] - position[0];
    step_z = path_next[2] - position[2];
} else if can_see_target {
    // No route, but it can see them — so there is nothing in the way and a
    // straight line is right. This is the case a grid misses at the very
    // start, before the first route comes back.
    step_x = target_at[0] - position[0];
    step_z = target_at[2] - position[2];
}

let span = (step_x * step_x + step_z * step_z).sqrt();
let speed = if can_see_target { CHASE_SPEED } else { SPEED };

let want_x = 0.0;
let want_z = 0.0;
// Stop at arm's length rather than shoving the player across the room.
if span > 1e-3 && target_distance > REACH * 0.8 {
    want_x = step_x / span * speed;
    want_z = step_z / span * speed;
}

let vx = velocity[0];
let vz = velocity[2];
let rate = ACCEL * dt;
let dx = want_x - vx;
let dz = want_z - vz;
let gap = (dx * dx + dz * dz).sqrt();
if gap <= rate || gap < 1e-6 {
    vx = want_x;
    vz = want_z;
} else {
    vx += dx / gap * rate;
    vz += dz / gap * rate;
}

let vy = velocity[1] - GRAVITY * dt;

// ---------------------------------------------------------------------------
// Attack.
//
// Raised as an event rather than applied. The enemy says "I hit them"; the
// rules decide what that costs, exactly as they do for a blast. An enemy that
// subtracted health directly would be a second place health is defined.
// ---------------------------------------------------------------------------

if can_see_target && target_distance <= REACH && tick - last_attack >= ATTACK_TICKS {
    emit.push(#{ kind: "damage", node: "Range/Player", exposure: ATTACK_DAMAGE / 150.0 });
    last_attack = tick;
}

memory.home = home;
memory.last_seen = last_seen;
memory.hunting_for = hunting_for;
memory.last_attack = last_attack;

velocity = [vx, vy, vz];
