// A walk around a boat, as a list of places to be.
//
// `patrol.rhai` walks back and forth along one axis and turns round when it
// feels blocked. That is the right model for a corridor and useless on a deck,
// where the route is a loop with a shape: aft cockpit, up one side, round the
// bow, down the other, and up the companionway to the bridge. So this one
// steers toward waypoints in order.
//
// **It steers in world XZ and the walk scene authors no wind for that reason.**
// The engine hands a script its *world* position; the boat's own frame is not
// something a script can see, and giving it one would mean handing sandboxed
// code a live borrow of the transform hierarchy. On flat water this hull sits
// within a few millimetres of where it started, so world and boat frame are the
// same thing to a waypoint. The assertions in the scene do not make that
// assumption — they read `local_*`, which is exact.
//
// What it is told:      tick, dt, position, velocity, grounded
// What it answers with: velocity

let SPEED = 2.4;          // metres per second — a walk, not a run
let ACCELERATION = 30.0;  // how hard it reaches that speed
let GRAVITY = 22.0;       // heavier than 9.81, same reason as patrol.rhai
let ARRIVED = 0.55;       // horizontal distance that counts as "there"

// x, z pairs. Chosen so the capsule never has to pass somewhere it does not
// fit, and two of them were moved after watching it get stuck.
//
// The companionway's underside clears only 0.96 m over the port side deck aft,
// so the route cannot simply continue aft along that side — it comes back to
// the centreline instead. And it must reach the ramp at its **foot**: stepping
// onto the side of a 42 degree plank is a 0.28 m rise onto a slope and the
// capsule refused it, while the same plank at x = -9.05 is 0.10 m off the deck
// and is walked onto without noticing. The transom stops the capsule at
// x = -9.15, so -9.05 is as far aft as it can stand — five centimetres of
// margin, and that is the whole reason the plank's foot is where it is.
//
// z = 1.30 rather than 1.10 on the way aft: the bait box reaches |z| = 0.78 and
// a 0.35 m radius at 1.10 clips its corner by three centimetres.
//
// **The bridge deck is walked round its perimeter, not across it.** The hardtop
// over it comes down to 4.80 above a deck at 3.08 — 1.72 m against a 1.90 m
// capsule — so the middle of that deck is somewhere a person does not fit, and
// the collider says so. The route runs the port side strip at z = -2.10, which
// is 0.20 m clear of the hardtop inboard and 0.15 m clear of the rail outboard,
// and comes out ahead of it. Aimed at the middle instead, the walker stood with
// its head a quarter of a metre inside the roof and looked fine.
let ROUTE = [
    [-8.60,  1.20],   // aft cockpit, starboard of the bait box
    [-6.60,  2.10],   // starboard side deck, aft
    [-1.00,  2.10],   // starboard side deck, amidships
    [ 3.00,  2.10],   // starboard side deck, forward
    [ 5.20,  0.00],   // foredeck
    [ 7.30,  0.00],   // the stem
    [ 5.00,  0.00],   // back off the bow
    [ 3.00, -2.10],   // port side deck, forward
    [-1.00, -2.10],   // port side deck, amidships
    [-6.40, -2.10],   // port side deck, aft — stops short of the ramp
    [-6.90,  1.30],   // round the forward end of the ramp
    [-9.00,  1.30],   // aft, outboard of the bait box
    [-9.05, -2.00],   // the foot of the companionway, hard against the transom
    [-6.30, -2.00],   // up it, onto the bridge deck
    [-4.00, -2.10],   // bridge deck, port side, outboard of the helm
    [ 0.00, -2.10],   // bridge deck, port side, outboard of the hardtop
    [ 3.75, -2.00],   // forward along that strip, past the hardtop's corner
    [ 3.75,  0.00],   // and inboard, ahead of it, at the forward rail
];

let leg = if "leg" in memory { memory.leg } else { 0 };

// The last waypoint is held, not looped. A patrol that starts over would make
// every assertion depend on which lap the tick count landed in.
if leg >= ROUTE.len() { leg = ROUTE.len() - 1; }
let goal = ROUTE[leg];

let dx = goal[0] - position[0];
let dz = goal[1] - position[2];
let distance = (dx * dx + dz * dz).sqrt();

if distance < ARRIVED && leg < ROUTE.len() - 1 {
    leg += 1;
    goal = ROUTE[leg];
    dx = goal[0] - position[0];
    dz = goal[1] - position[2];
    distance = (dx * dx + dz * dz).sqrt();
}

// Accelerate toward the target rather than snapping to it, so the same rule is
// also the brake: a target of zero decelerates at exactly this rate. On a deck
// that is the whole trick — "zero" is zero in the deck's frame (ADR 0060), so
// asking to stop is asking to stand on the boat.
let tx = 0.0;
let tz = 0.0;
if distance > 0.001 {
    tx = dx / distance * SPEED;
    tz = dz / distance * SPEED;
}

let step = ACCELERATION * dt;
let vx = velocity[0];
let vz = velocity[2];

let ex = tx - vx;
if ex.abs() <= step { vx = tx; } else if ex > 0.0 { vx += step; } else { vx -= step; }

let ez = tz - vz;
if ez.abs() <= step { vz = tz; } else if ez > 0.0 { vz += step; } else { vz -= step; }

let vy = velocity[1] - GRAVITY * dt;

memory.leg = leg;
velocity = [vx, vy, vz];
