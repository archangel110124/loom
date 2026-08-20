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
//                       move_x, move_z, forward, right, jump, sprint, fire,
//                       stand_node, stand_local
// What it answers with: velocity, helm_force, helm_torque, events on `emit`
// What it may keep:     memory, a map that survives to the next tick
//
// **A node `Script` is the only thing in this engine that sees a key press.**
// `GameRules` sees positions and events and no input at all. So anything the
// player *presses* has to enter the game here and leave as an event, which the
// rules script reads on the same tick — the motion pass runs before the rules
// pass, so nothing is a frame stale. The hold still needs no binding — it is
// filled by walking up to a supply — and `fire` is still unspent, for fishing.
//
// **`velocity` decelerates toward zero in the frame of what the character is
// standing on** (ADR 0060), which is why this file is correct on the rig's
// static deck and on the boat's heaving one without knowing which it is on.
//
// ---------------------------------------------------------------------------
// THE TWO THINGS THIS FILE DOES BESIDES WALK
// ---------------------------------------------------------------------------
//
// **1. It drives the boat.** `stand_node` names the rigid body under the
// character's feet and `stand_local` says where on it — the same frame
// `--assert Node.local_y` reads, and the only frame a station on something
// that moves is fixed in. Standing inside the helm bounds hands W/S/A/D to
// `helm_force` and `helm_torque`, which the host applies to that body through
// the same `propel` step an authored `Propulsion` goes through.
//
// **The helm is a place, so there is no key to learn and no key to spend.**
// The engine has five digital channels; `jump` is jump and `fire` belongs to
// fishing. Position was the only budget left, and it is also the only control
// scheme a player who was told nothing can find on their own.
//
// **2. It swims.** Nothing in this engine catches a character in water: fall
// off the rig and you fall forever, and round 1 shipped a rail that leaked at
// walking speed. Below the surface this floats the capsule instead, and the
// slipway at the rig's east end is what it swims to. Together those are the
// answer to "a demo must not contain an unrecoverable state" — the railing is
// not, and a taller railing could not be, because the height a wall would need
// is the jump apex plus the step height, which is above the player's own eye.

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
// In the water. Three numbers, and the first one is the only one that matters.
// ---------------------------------------------------------------------------

let SWIM_LINE = 0.25;    // capsule centre below this, and not standing on
                         // anything, means in the sea. The rig's own deck is
                         // at 1.40 and the boat's at 0.64, so nothing walkable
                         // in this scene is anywhere near it.
let FLOAT_Y = -0.70;     // where a swimmer settles. The eye rides 0.75 above
                         // the capsule centre, so this puts it at +0.05 —
                         // eye-level with the surface, which is what swimming
                         // looks like. Feet at -1.60, which is the depth the
                         // slipway had to reach past.
let FLOAT_RATE = 3.0;    // how hard buoyancy pulls toward it, per second.
                         // Higher bobs; lower lets a fall from the cap bar
                         // punch three metres under before it recovers.
let SWIM = 1.9;          // metres per second. Under half the walk, because a
                         // swimmer who moves at walking pace makes the slipway
                         // pointless and the sea safe.

// ---------------------------------------------------------------------------
// The helm, as a box in the boat's own frame.
//
// The aft cockpit, hard against the deckhouse's after face — `col_house` ends
// at local x = -6.20 and `col_engine_box` (the bait box) starts at -8.40, so
// this 1.0 x 3.0 m patch is the clear deck between them. `HelmMat` in
// `deeper_demo.loom` is drawn at exactly these bounds; the two are one
// decision and neither is meaningful without the other.
// ---------------------------------------------------------------------------

let HELM_NODE = "Rig/Boat";
let HELM_X_MIN = -9.50;
let HELM_X_MAX = -6.10;
let HELM_Z_MIN = -2.65;
let HELM_Z_MAX = -0.90;

// **Thrust, inside ADR 0063's wall and well inside it.** That ADR swept this
// hull: 1.6 MN settles at 3.586 m/s (seven knots) and holds trim; 2.0 MN
// reaches 3.74 and the trim walks; 2.63 MN submarines to y = -14.34. Thrust is
// in the body frame and `Buoyancy` damps all three axes, so past the wall the
// bow digs in and the vector acquires a downward component that digs it in
// further — pushing harder makes the boat go *down*, not faster.
//
// 1.1 MN is deliberately short of the ceiling. A harbour is not a passage, the
// rig is 24 m across, and a boat that answers the wheel at three knots is a
// better demo than one that porpoises at seven. Measured, in this scene: see
// the round's report.
let AHEAD = 1100000.0;
let ASTERN = 500000.0;   // less than half. A screw astern is inefficient in
                         // life and a stern that digs in is the same failure
                         // as a bow that does, so this end of the range gets
                         // the wider margin.
// **The bollard-pull limiter, and it is the only thing standing between the
// demo and a lost boat.**
//
// 1.1 MN is under ADR 0063's wall *in open water*. Against an obstacle it is
// not: the bow cannot move, the contact pitches her, body-frame thrust follows
// the bow down, and the two feed each other. Traced with her bow on the rig's
// west end, `--hold move_z=1`, before this existed:
//
//   tick  120   boat y  -0.001   player y  1.681
//   tick  300   boat y  -0.333   player y  1.194
//   tick  900   boat y  -0.691   player y  0.899
//   tick 2400   boat y  -1.463   player y  0.111
//   tick 3000   player y -0.696                     <- swimming
//
// **The rider is not sinking through the deck** — his separation from the
// hull's origin stays inside 7 cm the whole way down (1.601, 1.527, 1.590,
// 1.573) and the aft cockpit even rises as the bow digs in. He rides a boat
// that submarines, which is one bug and not two.
//
// It is worth fixing because it is not recoverable in the way the sea is. She
// pops back up when you let go (y = 0.031 at tick 3600) — twenty metres west
// of her berth, against a closed rail, with nothing in this scene that lets a
// swimmer climb aboard. The rig has a slipway for the player and nothing for
// the boat.
//
// So: **if she is commanded ahead and is not making way, the throttle goes to
// zero.** A skipper pressed on a quay does not hold full ahead, and neither
// does this. It is measured from the rider's own world position, because at
// the helm his legs are dead (`speed = 0.0`) and his motion *is* hers — no new
// engine reading, no readback, and the one place in the script that already
// knows the answer.
//
// **Ahead only.** Clamping astern as well deadlocks the escape: your speed is
// zero when you first press S to back off, so the counter would still be
// running and the throttle would stay shut. Astern is 500 kN, less than half,
// and it is the way out of this rather than a second way into it.
let STALL_SPEED = 0.35;  // m/s. She passes this within a handful of ticks from
                         // rest at 25 m/s/s of unopposed acceleration, so a
                         // standing start never trips it.
// Ashore, the same shape and the same reason: a wish to move and no travel is
// something to say out loud — but measured **along the direction he asked
// for**, because the failure this exists for is a diagonal press into her
// topsides, where he keeps 1.6 m/s of sideways slide and makes no progress at
// all toward the boat.
let STUCK_SPEED = 1.0;   // m/s along the wish, against a 4.2 m/s walk.
                         // **1.6 was tried and is worse**: at that threshold a
                         // shallow slide sits on the boundary, the flag
                         // flickers, and `--assert` reads whatever the last
                         // tick happened to be. A latch that is going to be
                         // asserted on has to be stable, not sensitive.
let STUCK_TICKS = 45;
let STALL_TICKS = 45;    // three quarters of a second below that speed with
                         // the throttle open. Long enough that a wave or a
                         // hard turn cannot close the throttle; short enough
                         // that the bow has not yet buried itself.
let WHEEL = 1300000.0;   // newton-metres of yaw. The hull's yaw inertia is
                         // about 1.5e6 kg m^2 and the pontoons damp the
                         // rotation, so this is a terminal rate, not an
                         // acceleration — measured in the report.

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

// ---------------------------------------------------------------------------
// At the helm? Then W/S/A/D are the throttle and the wheel, and your legs are
// not going anywhere: you are holding on to something.
// ---------------------------------------------------------------------------

// **Latched across short airborne gaps, and that is not polish.** `stand_node`
// is a raw per-tick ground test, so every frame of a scramble over the bait box
// reads as *not on the boat*. Traced on the taught walk before this existed —
// four-tick samples, `--hold move_z=1`:
//
//   t 316  aboard 0   t 320  aboard 0   t 324  aboard 1   t 328  aboard 0
//   t 336  aboard 1   t 344  aboard 0   t 356  aboard 1
//
// Six flips in forty ticks on a dead-straight walk, which the HUD printed as
// "WALK FORWARD TO THE BOAT" over a player standing on it — the biggest text
// on the screen strobing at second three of the demo. `events.station` was 10
// where it should be about 4. **And it dropped keypresses**: `aboard` gates
// the cast emit below, so every `fire` in one of those windows went nowhere.
//
// Landing on anything else clears it on the same tick, so stepping ashore is
// still instant — the latch only ever spans *air*. Going under clears it too,
// because a swimmer is not aboard however recently he was.
//
// `on_mat` is latched with it and off the same counter. `stand_local` is not
// meaningful while airborne, so re-testing the rectangle against a stale
// reading would drop the helm for a tick every time she heaved; the last
// on-deck answer is the truthful one to hold.
let ABOARD_LATCH = 30;   // half a second. Long enough for a hop with the deck
                         // rising under it, far too short to cover a fall.

let on_boat = stand_node == HELM_NODE;
let on_mat_now = on_boat
    && stand_local[0] > HELM_X_MIN
    && stand_local[0] < HELM_X_MAX
    && stand_local[2] > HELM_Z_MIN
    && stand_local[2] < HELM_Z_MAX;

let off_boat_for = if "off_boat_for" in memory { memory.off_boat_for } else { 999 };
let mat_was = if "mat_was" in memory { memory.mat_was } else { false };
// **`stand_node`, not `grounded`, and the difference is the whole bug.**
// `grounded` comes out of `move_character`'s contact and `stand_node` out of a
// separate shape cast (`play.rs`, `Sim::support`), and the two disagree for a
// tick at a time while a capsule scrambles: the contact says yes, the cast
// finds nothing. A latch cleared on `grounded` therefore cleared itself on
// exactly the ticks it existed to cover, and the strobe survived the first
// version of this code. An empty `stand_node` means *no dynamic body under
// me* — air, or the rig, which is static and reports nothing — so it is the
// answer that cannot flicker.
if on_boat {
    off_boat_for = 0;
    mat_was = on_mat_now;
} else if stand_node != "" {
    off_boat_for = 999;   // some other moving thing. Not this boat, not later.
} else {
    off_boat_for += 1;
}
memory.off_boat_for = off_boat_for;
memory.mat_was = mat_was;

let latched = off_boat_for <= ABOARD_LATCH && position[1] > SWIM_LINE;
let aboard = on_boat || latched;
let on_mat = if on_boat { on_mat_now } else { latched && mat_was };

// ---------------------------------------------------------------------------
// **Space is hands off the wheel**, and it is the only way off the mat that is
// not a way into the sea.
//
// Standing on the mat used to make `speed` zero unconditionally, so all four
// direction keys were the boat and your legs did not exist. The one gesture
// left for leaving was jump-plus-a-direction — and that direction was
// *simultaneously the wheel*. Traced on `rig_drive.loom` with
// `--hold "jump=1,move_x=1"`: airborne at tick 60, y = -0.66 by tick 400,
// y = -0.700 and 12 m astern by tick 600. **The only exit threw you overboard
// while spinning your boat.** Astern instead (`jump=1,move_z=-1`) merely
// stranded you against the bulwark.
//
// So the mat gets a release. `jump` on the mat does not jump — it lets go, and
// your legs come back on the same tick. Walk off and the latch clears, so
// stepping back on takes the helm again. No timer: leaving the box *is* the
// reset, which means the state cannot be stuck on and cannot expire under you.
//
// It costs nothing that was spent. `fire` is still untouched and belongs to
// fishing; `jump` still jumps everywhere in the scene except on this 1.3 x
// 0.9 m patch of deck, where holding on to something is the truthful thing for
// a person at a wheel to be doing anyway.
let hands_off = if "hands_off" in memory { memory.hands_off } else { false };
if !on_mat { hands_off = false; }
if on_mat && jump { hands_off = true; }
memory.hands_off = hands_off;

let at_helm = on_mat && !hands_off;

// How fast the deck under his feet is actually travelling, from his own world
// position one tick to the next. Written every tick whether or not he is at
// the wheel, so the delta never spans a gap; read only when he is, which is
// the only time it is his boat's speed rather than his legs'.
let deck_speed = 0.0;
// **How fast he is going *the way he asked to go*, which is a different
// question and the one that catches a slide.** Pressed diagonally into her
// topsides he keeps a metre and a half a second of sideways travel, so a plain
// speed reads him as walking normally while the screen is a wall of blue hull.
// This is his travel projected onto the direction he is asking for.
let along_wish = 99.0;
if "helm_px" in memory {
    let dpx = position[0] - memory.helm_px;
    let dpz = position[2] - memory.helm_pz;
    deck_speed = (dpx * dpx + dpz * dpz).sqrt() / dt;
    let wl = (wish_x * wish_x + wish_z * wish_z).sqrt();
    if wl > 0.01 {
        along_wish = (dpx * wish_x / wl + dpz * wish_z / wl) / dt;
    }
}
memory.helm_px = position[0];
memory.helm_pz = position[2];

let stalled = if "stalled" in memory { memory.stalled } else { 0 };
if at_helm && move_z > 0.0 && deck_speed < STALL_SPEED { stalled += 1; } else { stalled = 0; }
memory.stalled = stalled;

if at_helm {
    // The bow is the hull's own +X, so ahead is +X in the body frame and the
    // host rotates it. Nothing here knows or needs to know which way she is
    // pointing.
    let throttle = if move_z >= 0.0 { move_z * AHEAD } else { move_z * ASTERN };
    // Pressed against something and still pushing. See `STALL_TICKS`.
    if stalled > STALL_TICKS { throttle = 0.0; }
    helm_force = [throttle, 0.0, 0.0];
    // **Negative Y for a turn to starboard.** A rotation about +Y takes +X
    // toward -Z, and this hull's starboard side is +Z, so pressing D — the
    // wheel to the right — is a torque about -Y. Verified by driving her.
    helm_torque = [0.0, -move_x * WHEEL, 0.0];
} else if aboard {
    // **Aboard and not at the wheel means all stop, every tick.** The thrust
    // table is live: a body keeps the last force written to it, so a helm that
    // only ever wrote while someone was standing on the mat would leave the
    // boat running away the moment they stepped off it. Walking away from the
    // wheel is the second way to stop her, and it is the one a player finds by
    // accident.
    helm_force = [0.0, 0.0, 0.0];
    helm_torque = [0.0, 0.0, 0.0];
}

let swimming = !grounded && position[1] < SWIM_LINE;

let speed = if at_helm { 0.0 } else if swimming { SWIM } else if sprint { SPRINT } else { WALK };
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

// **Standing on something means asking for no downward motion, and on a deck
// under way that is the difference between riding and drowning.**
//
// `velocity[1]` comes back as zero minus the deck's own rise while grounded
// (ADR 0060), so `- GRAVITY * dt` makes every grounded tick request 0.4 m/s
// downward and rely on the contact to refuse it. A *static* floor refuses it
// every time. A floor that is also translating does not: rapier's `move_shape`
// ignores a shape it already penetrates, and at speed the sweep starts inside
// the deck often enough that the request is accepted about one tick in seven.
// The capsule ratchets down into the plate and eventually falls out of the
// bottom of the boat.
//
// Measured on `assets/test/rig_drive.loom`, at the helm, hull at 3.0 m/s —
// two characters, mirrored port and starboard on the same flat plate, one on
// this script and one on `idle.rhai`:
//
//   tick    5   both 1.536
//   tick  300   both 1.279      <- 2 cm of sink per metre the hull travels
//   tick  600   both 0.973
//   tick  900   over the side
//
// **Identical to four decimal places, so it is not the movement model** — it
// is every movement model in this repository, all of which write this same
// line. It has never shown before because no scene had ever driven a floating
// body with anybody standing on it (ADR 0063's own opening sentence).
//
// **The fix belongs in `loom_physics::move_character` and is not being put
// there in a demo round.** Clamping the grounded downward request in the
// engine would fix `idle`, `fps`, `patrol`, `boat_patrol`, `rig_patrol` and
// `enemy` in one place — and it would also change what happens on a slope
// steep enough to slide down, so it moves the determinism hash of every
// character scene in the repository. That is an ADR and a re-pinning pass,
// exactly like ADR 0063's seven-knot wall. This line is the local fix, and it
// is here rather than in the engine so it can be deleted in one edit when the
// real one lands.
if grounded { vy = velocity[1]; }

// **In the sea, buoyancy replaces gravity outright rather than opposing it.**
// A drag term against a 24 m/s/s pull is a tuning problem with two knobs and a
// terminal velocity that depends on how far you fell; a straight approach to
// `FLOAT_Y` is one knob and always ends in the same place. Nothing else in the
// engine is doing this for the character — `Buoyancy` is a rigid-body
// component and a `CharacterController` is not a rigid body.
if swimming {
    vy = (FLOAT_Y - position[1]) * FLOAT_RATE;
}

// **`!on_mat`, not `!at_helm`.** On the mat `jump` is the helm release and
// must not also be a hop: a hop off a deck moving at 3 m/s lands you a metre
// aft of where you left, which on this boat is the transom. Once released you
// are still standing in the box with your legs live, and pressing Space again
// re-latches a flag that is already set rather than launching you — so the one
// patch of deck where a jump is dangerous is the one patch that has none.
if !on_mat && !swimming && airborne_for <= COYOTE_TICKS && jump_buffered <= BUFFER_TICKS {
    vy = JUMP_SPEED;
    // Spend both, or the grace period fires again next tick and the character
    // climbs the air.
    airborne_for = COYOTE_TICKS + 1;
    jump_buffered = BUFFER_TICKS + 1;
}

memory.airborne_for = airborne_for;
memory.jump_buffered = jump_buffered;

velocity = [vx, vy, vz];

// ---------------------------------------------------------------------------
// **THE ROD, and the two channels that were being kept for it.**
// ---------------------------------------------------------------------------
//
// A node `Script` is the only thing in this engine that sees a key press, and
// the fight in `deeper_rules.rhai` is a `GameRules` script, which sees none. So
// the two verbs cross the gap as an event, read on the same tick — the motion
// pass runs before the rules pass, so nothing is a frame stale. This is the
// same seam `fishing_angler.rhai` opened for `fishing.loom`; only the bindings
// differ, and they differ for one reason.
//
// **Reel is SHIFT here and `move_z` there.** In `fishing.loom` the angler is a
// post: his movement model writes `velocity = [0,0,0]` and W is free. Here W is
// the throttle at the wheel and his legs everywhere else, so binding reel to it
// would mean either walking off the boat while playing a fish or a third stance
// with its own way in and its own way out. `sprint` was unspent, it is a hold
// rather than a tap, and "hold to reel" survives the change intact.
//
// The cost, stated: on the boat SHIFT is now reel **and** still sprint. A
// player sprinting round the cockpit with a fish on loads the line. That is a
// dozen ticks of stress on a 100-point meter and it is not worth a stance to
// close.
//
// **Fishing is aboard-only, and it is enforced here rather than in the rules.**
// No event means no reel and no press, so a rod cast from the wharf is not a
// refusal the fight has to carry — it is a thing that never reaches it.
//
// Emitted only on a tick where something is actually pressed, and `fire` only
// on its **edge**. The fight reads absence as "not reeling, not pressing",
// which is exactly right, and the log stays a log rather than a per-tick trace:
// `loom sim --hold fire=1` holds the button for the whole run and produced 708
// `angler` events before the edge went in, for one cast.
//
// The fight edge-detects the press itself (`edge = press && held == 0`) and
// still must: it also drives five reference pilots, which do not come through
// here at all.
// **The edge is measured aboard, not everywhere.** Walking up the boarding
// steps with the button already down should cast when you arrive, and an edge
// tracked ashore is spent before you get there — which is exactly what
// `--hold fire=1` does, and it silently produced a run with no cast in it.
let fire_now = aboard && fire;
let fire_edge = fire_now && !(if "fire_held" in memory { memory.fire_held } else { false });
memory.fire_held = fire_now;
if aboard && (sprint || fire_edge) {
    emit.push(#{
        kind: "angler",
        reel: if sprint { 1.0 } else { 0.0 },
        press: if fire_edge { 1.0 } else { 0.0 },
    });
}

// ---------------------------------------------------------------------------
// **WHERE HE IS STANDING, on change only.**
// ---------------------------------------------------------------------------
//
// Round 3 shipped a caption keyed on a rectangle in the rules script while the
// fact it claimed to state — whether the wheel is answering — lived here, under
// the same name. Pressing SPACE changed nothing on screen in either direction
// because the rules script could not see that it had been pressed.
//
// Three booleans, packed into one integer so the comparison is one comparison,
// and emitted only when the integer moves: a boarding is two events, not two
// thousand. `jump` is still not a fishing binding and `fire` is still not a
// helm one; nothing here spends a channel.
//
// **Two more facts, and both are captions the demo used to get wrong.** Three
// degrees right of the taught aim walks you into her topsides — a wall of blue
// hull filling the screen — and three degrees left puts you in the sea with
// nothing in frame at all, and in both cases the HUD went on reading "WALK
// FORWARD TO THE BOAT". `swimming` is already computed above; `ashore_stuck`
// is the same shape as the helm's own stall limiter, measured off his own
// world position, because a character pressed against geometry has a wish and
// no travel.
//
// **Ashore only.** Aboard, walking into the deckhouse is normal and the boat
// captions have more useful things to say.
let ashore_stuck = if "ashore_stuck" in memory { memory.ashore_stuck } else { 0 };
if !aboard && !swimming && grounded && wish_len > 0.01 && along_wish < STUCK_SPEED {
    ashore_stuck += 1;
} else {
    ashore_stuck = 0;
}
memory.ashore_stuck = ashore_stuck;
let stuck = ashore_stuck > STUCK_TICKS;

let station = (if aboard { 1 } else { 0 })
            + (if at_helm { 2 } else { 0 })
            + (if stalled > STALL_TICKS { 4 } else { 0 })
            + (if swimming { 8 } else { 0 })
            + (if stuck { 16 } else { 0 });
if station != (if "station" in memory { memory.station } else { -1 }) {
    emit.push(#{
        kind: "station",
        aboard: if aboard { 1.0 } else { 0.0 },
        helm: if at_helm { 1.0 } else { 0.0 },
        stalled: if stalled > STALL_TICKS { 1.0 } else { 0.0 },
        swim: if swimming { 1.0 } else { 0.0 },
        stuck: if stuck { 1.0 } else { 0.0 },
    });
    memory.station = station;
}
