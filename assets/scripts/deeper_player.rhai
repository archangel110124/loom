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
//                       interact, stand_node, stand_local
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
// When this was written the engine had five digital channels; `jump` was jump
// and `fire` belonged to fishing, so position was the only budget left — and it
// is also the only control scheme a player who was told nothing can find on
// their own. **There is a sixth channel now, `interact` on E**, and this file
// reads it — but only to forward it as a `use` event. The helm is still a mat:
// moving it onto E is a deliberate change to make, not a gap to close by
// accident, and the fish is the thing that needed a key.
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
// of her berth, against a closed rail. **She now carries her own ramp** —
// `Rig/Boat/SternLadder`, which is this paragraph's old last sentence answered
// ("the rig has a slipway for the player and nothing for the boat") — so a
// swimmer gets back onto her. Getting the *hull* back off a quay is still what
// the limiter below is for; a boarding ramp does not steer.
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
// Walking into something is the same shape and the same reason: a wish to move
// and no travel is something to say out loud. `STUCK_SPEED` used to live here —
// one metre a second along the wish, from one tick's delta — and it is gone,
// because a rate cannot tell a wedge from a ripple. See `STUCK_NET`, at the
// measurement itself.
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

// ---------------------------------------------------------------------------
// **THE CREEL, AND WHAT IT DOES TO HIS BODY.**
// ---------------------------------------------------------------------------
//
// **Only a movement script sees input**, so the grid's every gesture starts
// here and crosses to `deeper_rules.rhai` as an event, exactly the way `use`
// and `angler` do. The flag itself lives in this file's `memory`, because
// nothing else in the engine has to know a bag is open.
//
// **His legs stop and his head does not, and that is the whole design.** The
// pointer stays captured, so mouse-look keeps steering the camera while his
// hands are in the bag — sweep the horizon, watch the rod tip, see another
// player go over the side, with the grid up. Releasing the pointer instead is
// what the pause menu does and it would zero all seven channels *and* stop
// `DeviceEvent::MouseMotion`: unable to walk and unable to look, which is
// strictly worse than the pause it is avoiding.
//
// **`Play::paused` is never touched.** The boat drifts, the fish pulls, and
// `SLACK_LIMIT` is still counting — five seconds of not reeling throws the
// hook, so rummaging mid-fight costs you the fish. That is the no-pause
// consequence, it was already built, and it is why nothing here has to force
// the creel shut on a bite.
//
// **Refused at the wheel**, with a word, because W/A/S/D are the throttle and
// the rudder there and one key cannot be two things at one moment. This is the
// design's worst input collision and `at_helm` — computed three lines up —
// removes it diegetically: your hands are on the wheel.
let bag_open = if "bag_open" in memory { memory.bag_open } else { false };
let bag_held = if "bag_held" in memory { memory.bag_held } else { false };
// Edge-detected here even though `Play::set_input` already does it, for the
// reason `fire` and `interact` both carry: `--hold bag=1` writes `Runner::input`
// straight and holds it for the rest of the run, which would strobe the grid.
let bag_edge = bag && !bag_held;
memory.bag_held = bag;

if bag_edge {
    if bag_open {
        bag_open = false;
        emit.push(#{ kind: "creelshut" });
    } else if at_helm {
        emit.push(#{ kind: "creelbusy" });
    } else {
        bag_open = true;
        emit.push(#{ kind: "creelopen" });
    }
}
// **The mat can arrive under him**, which his legs cannot do while the grid is
// up: she is a rigid body and the deck moves. One line, and it keeps "hands on
// the wheel" true rather than only checked at the moment of pressing.
if bag_open && at_helm {
    bag_open = false;
    emit.push(#{ kind: "creelshut" });
}
memory.bag_open = bag_open;

// **The cursor, on W/A/S/D, with a key repeat this file has to build itself.**
// `move_x`/`move_z` are analogue axes and nothing in this engine has ever
// edge-detected one — `interact` got its latch in Rust precisely because level
// input is unusable for a discrete gesture. So: an edge at |axis| > 0.5, then
// nothing for `CURSOR_DELAY`, then one cell every `CURSOR_REPEAT`.
//
// **These two numbers are guessed and they are the ones that most need a human
// at a window.** `--assert` can confirm the cursor reached x = 1; it cannot
// tell whether it got there when you asked. They are alone up here so tuning
// them is an edit and not a rebuild.
let CURSOR_DELAY = 20;    // a third of a second before the first repeat
let CURSOR_REPEAT = 8;    // then one cell every eighth of a second

// **How long SHIFT must be HELD to put what is in your hand over the side.**
// One second. Held rather than tapped because it is the only irreversible act
// in this game, and SPACE — the obvious "drop" key — is the helm release on the
// mat, where a fat finger is a boat adrift.
let DITCH_TICKS = 60;

if bag_open {
    // +Z on the grid is *down*, and `move_z` is forward, so W is -1.
    let ax = if move_x > 0.5 { 1 } else if move_x < -0.5 { -1 } else { 0 };
    let az = if move_z > 0.5 { -1 } else if move_z < -0.5 { 1 } else { 0 };
    // One integer, so "did the direction change" is one comparison. 4 is the
    // centre — the same trick `station` uses one screen down.
    let packed = (ax + 1) * 3 + (az + 1);
    let was = if "cur_was" in memory { memory.cur_was } else { 4 };
    let held_for = if "cur_held" in memory { memory.cur_held } else { 0 };
    let dx = 0;
    let dy = 0;
    if packed != was {
        held_for = 0;
        dx = ax;
        dy = az;
    } else if ax != 0 || az != 0 {
        held_for += 1;
        if held_for >= CURSOR_DELAY && (held_for - CURSOR_DELAY) % CURSOR_REPEAT == 0 {
            dx = ax;
            dy = az;
        }
    }
    memory.cur_was = packed;
    memory.cur_held = held_for;
    if dx != 0 || dy != 0 {
        emit.push(#{ kind: "cursor", dx: dx.to_float(), dy: dy.to_float() });
    }
} else {
    memory.cur_was = 4;
    memory.cur_held = 0;
}

// **The ditch, and its progress bar.** `== DITCH_TICKS` fires exactly once, so
// holding SHIFT longer does not throw a second thing over the side and the
// counter is reset by letting go. The progress is emitted in **quarters, on
// change** — four events per ditch rather than sixty — the same discipline
// `station` and `heading` follow, and for the same reason.
let ditch_held = if "ditch_held" in memory { memory.ditch_held } else { 0 };
if bag_open && sprint { ditch_held += 1; } else { ditch_held = 0; }
if ditch_held == DITCH_TICKS { emit.push(#{ kind: "ditch" }); }
memory.ditch_held = ditch_held;
let dq = if ditch_held > 0 { ditch_held * 4 / DITCH_TICKS } else { 0 };
if dq != (if "dq" in memory { memory.dq } else { -1 }) {
    emit.push(#{ kind: "ditching", q: dq.to_float() });
    memory.dq = dq;
}

// How fast the deck under his feet is actually travelling, from his own world
// position one tick to the next. Written every tick whether or not he is at
// the wheel, so the delta never spans a gap; read only when he is, which is
// the only time it is his boat's speed rather than his legs'.
let deck_speed = 0.0;
if "helm_px" in memory {
    let dpx = position[0] - memory.helm_px;
    let dpz = position[2] - memory.helm_pz;
    deck_speed = (dpx * dpx + dpz * dpz).sqrt() / dt;
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
    //
    // **Clamped, because full wheel throws the helmsman off the wheel — and a
    // keyboard has no other setting.** `Input::axis` returns exactly -1, 0 or
    // +1, so every A or D a player can press was the full 1.3 MN.m. Measured
    // on this scene, `--hold "0:move_z=1; 400:move_z=1,move_x=<w>"`, watching
    // `state.at_helm`:
    //
    //   w 0.2   helm 1 at every sample out to tick 3200, 5.8 kn throughout
    //   w 0.3   helm 1 at every sample out to tick 3200, 5.8 kn throughout
    //   w 0.4   helm 0 by tick  600, knots 0.06 and then 0.00 for ever
    //   w 1.0   helm 0 by tick  480, and he never gets back on
    //
    // At 1.0 he leaves the mat two seconds after the first press and the boat
    // stops dead with the caption "stand on the mat to steer" — the mat he
    // believed he was standing on. Traced: he does not fall in, he *slides*,
    // from boat-local x = -7.31 to +2.21 in about 120 ticks, the length of the
    // aft deck onto the foredeck. Under way with the wheel over she heels and
    // trims, and ADR 0060's carry is `linvel + omega x r` at the foot contact
    // with **no friction on a tilted plate** — the ADR says so itself and
    // measures 780 mm of drift at wind 12 on the one sloped station it has. A
    // hard turn is a much bigger tilt than wind 12.
    //
    // So the clamp is a **yaw-rate limit expressed as the only knob the helm
    // has**, and 0.3 is the measured wall with a margin, not a feel choice.
    //
    // `ponytail:` the ceiling is a turning circle of roughly 84 m — big for a
    // 19 m hull, and the reason the demo's return leg is astern rather than a
    // turn. Lifting it needs friction on a moving slope in `move_character`,
    // which re-pins five character scenes and is ADR work, not a demo round.
    let WHEEL_MAX = 0.30;
    let wheel = if move_x > WHEEL_MAX {
        WHEEL_MAX
    } else if move_x < -WHEEL_MAX {
        -WHEEL_MAX
    } else {
        move_x
    };
    helm_torque = [0.0, -wheel * WHEEL, 0.0];

    // **WHAT THE TWO CONTROLS ARE SET TO, because the frame will not say.**
    //
    // The boat's drawn wheel and throttle lever are prefab nodes, and they are
    // at boat-local (2.38, 4.24, -0.52) and (2.45, 4.18, -1.00): up on the
    // flybridge, ten metres forward of this mat and three and a half metres
    // above it. The helmsman cannot see either one. So "show it in the world"
    // has nothing to show here — measured off the meshes' own vertex bounds —
    // and the confirmation that a press registered has to be text.
    //
    // Both are three-valued (`Input::axis` is exactly -1, 0 or +1, and the
    // wheel is then clamped), so this changes only when the player changes it:
    // the same on-change discipline as `station` and `heading`.
    let drive = if stalled > STALL_TICKS { 0 } else if move_z > 0.0 { 1 } else if move_z < 0.0 { -1 } else { 0 };
    let rudder = if wheel > 0.01 { 1 } else if wheel < -0.01 { -1 } else { 0 };
    let set = drive * 4 + rudder;
    if set != (if "helmset" in memory { memory.helmset } else { 99 }) {
        emit.push(#{ kind: "helmset", drive: drive.to_float(), rudder: rudder.to_float() });
        memory.helmset = set;
    }
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

// **`bag_open` first**, because the creel takes the movement keys: the same
// W/A/S/D that walk him are the cursor, and a man who walks while packing walks
// off the boat. His head is still free — see the creel block above.
let speed = if bag_open { 0.0 }
            else if at_helm { 0.0 }
            else if swimming { SWIM }
            else if sprint { SPRINT }
            else { WALK };
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
// **`aboard || bag_open`**, because with the grid up LMB turns the item in
// your hand and the creel opens ashore as well as afloat. The edge is the same
// edge either way, so a click cannot both turn a fish and cast a line.
let fire_now = (aboard || bag_open) && fire;
let fire_edge = fire_now && !(if "fire_held" in memory { memory.fire_held } else { false });
memory.fire_held = fire_now;
if bag_open {
    // Turning what is in your hand. No `angler` while the grid is up: SHIFT is
    // the ditch and LMB is the turn, and a reel or a cast fired from inside the
    // inventory is one key doing two things.
    if fire_edge { emit.push(#{ kind: "turn" }); }
} else if aboard && (sprint || fire_edge) {
    emit.push(#{
        kind: "angler",
        reel: if sprint { 1.0 } else { 0.0 },
        press: if fire_edge { 1.0 } else { 0.0 },
    });
}

// ---------------------------------------------------------------------------
// **E, AND IT IS THE ONLY THING IN THIS FILE THAT WORKS ASHORE AS WELL.**
// ---------------------------------------------------------------------------
//
// The sixth digital channel, crossing the same gap as `angler` and for the same
// reason: `GameRules` sees no input, so a key that means "use the thing I am
// standing at" has to enter the game here and leave as an event.
//
// **Not gated on `aboard`, which every other verb in this file is.** Casting is
// aboard-only because a rod cast from the wharf is nonsense; the crate that
// closes the trip is *on the wharf*, so gating this the same way would make the
// demo's last beat unpressable. Where the press means something is the rules
// script's question, and it answers it with a distance — see `within`.
//
// **Edge-detected here even though `Play::set_input` already does it**, and the
// reason is the same one `fire` has: `--hold interact=1` writes `Runner::input`
// straight and holds it for the rest of the run, which would deliver a fish to
// the crate sixty times a second. `interact_probe.rhai` is the bench that
// separates the two drivers.
let use_held = if "use_held" in memory { memory.use_held } else { false };
if interact && !use_held {
    emit.push(#{ kind: "use" });
}
memory.use_held = interact;

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
// FORWARD TO THE BOAT". `swimming` is already computed above; `stuck` is below,
// because a character pressed against geometry has a wish and no travel.
//
// **It used to be ashore only, and that was measured wrong rather than argued
// wrong.** The note here read "aboard, walking into the deckhouse is normal and
// the boat captions have more useful things to say". The boat captions do not:
// held against `col_engine_box` with a fish in his hands, a player made no
// progress for four hundred and fifty ticks while `state.stuck` read 0.0 and
// the HUD read `ABOARD   CLICK to cast`. The one place in this demo where being
// blocked is *confusing* rather than obvious was the one place this could not
// fire. Traced with the round-8 tape plus `1900:move_z=-1`: z went -12.304 to
// -11.605 in the first fifty ticks and did not move again.
//
// **NET TRAVEL FROM A MARK, NOT A PER-TICK RATE, and this is the half that had
// to be rebuilt rather than ungated.** The first version of this measured
// metres per second along the wish from one tick's delta. Wedged in a corner
// the contact solver pushes the capsule out a centimetre and he walks straight
// back in, and at 60 Hz a 1.7 cm ripple *is* one metre per second — so the
// counter reset every few ticks and the genuine four-hundred-tick wedge never
// reported. A mark he has to get a metre away from cannot be fooled by a
// ripple, because a ripple has no mean.
//
// **Along the wish, because that is what catches a slide.** Pressed diagonally
// into her topsides he keeps a metre and a half a second of sideways travel, so
// plain distance from the mark reads him as walking normally while the screen
// is a wall of blue hull. Projected onto the direction he asked for, it is zero.
//
// **In the deck's frame when he is on the deck**, and that is not a refinement:
// a world-space mark on a hull making five knots is left behind at five knots
// whether he moves his feet or not. `stand_local` is his position on the body
// under him (ADR 0060), which is the only frame in which "am I getting across
// this deck" has an answer. Ashore `stand_node` is empty — the rig is static
// and reports nothing — so world position is that frame and the two cases share
// one piece of code.
//
// **Still never at the helm**: there `speed = 0.0` by design, so a wish with no
// travel is what holding the wheel *is*.
let STUCK_NET = 1.0;     // metres along the wish. A walk covers this in 15
                         // ticks at 4.2 m/s, so a man who is walking re-marks
                         // three times before the counter is a third full.

let px = if on_boat { stand_local[0] } else { position[0] };
let pz = if on_boat { stand_local[2] } else { position[2] };
// **`!bag_open`, or the creel reads as being wedged.** `trying` is "he has a
// wish and is making no progress", and with the grid up he has a wish (the
// cursor is on the same axes) and by design no progress at all.
let trying = !at_helm && !bag_open && !swimming && grounded && wish_len > 0.01;

// **A tick this measurement cannot read, and skipping it is the whole reason
// the second version of this still did not fire.** `on_boat` is the raw
// per-tick ground cast, and the note two hundred lines above says in so many
// words that it flickers: every frame of a scramble, and every heave that puts
// the deck out from under the cast, reads as *not on the boat*. Latched
// `aboard` and raw `on_boat` therefore disagree a few ticks in every second —
// and each disagreement was throwing away `stand_local` for a world position,
// re-marking, and resetting the counter to zero. Three hundred ticks pressed
// against the bait box still read `stuck = 0`.
//
// So a tick where he is aboard and the cast has lost the deck is **held**: not
// counted, not reset, nothing written. The counter is a stopwatch and this is
// a pause, which is the correct thing to do with an unreadable sample.
//
// **`grounded` is the same story and had to be measured to find.** With only
// `on_boat` held, the longest run at the wedge was 60-90 ticks against the 450
// the player actually spends there — bisected by moving the threshold, which is
// how a counter with no readout gets debugged. `grounded` is `move_character`'s
// contact and `on_boat` is a separate shape cast (the note above says they
// disagree for a tick at a time), and on a hull that heaves *both* drop out.
// Ashore `!grounded` means airborne under his own legs and must reset; aboard
// it means the deck fell away for a tick, so it is held with the rest.
let blind = aboard && (!on_boat || !grounded);
if !blind {
    // The mark is dropped afresh whenever the frame changes under him, because
    // a mark in the rig's frame compared against a position in the boat's is a
    // twelve-metre jump and reads as a sprint.
    let framed = if "stuck_frame" in memory { memory.stuck_frame == on_boat } else { false };
    let ashore_stuck = if "ashore_stuck" in memory { memory.ashore_stuck } else { 0 };
    let mark_x = if "mark_x" in memory { memory.mark_x } else { px };
    let mark_z = if "mark_z" in memory { memory.mark_z } else { pz };
    if trying && framed {
        // Recomputed rather than reusing `wish_len`: that was measured *before*
        // the normalisation above, so it is the divisor only for a wish shorter
        // than one. This is the length of the vector actually being projected
        // onto.
        let wl = (wish_x * wish_x + wish_z * wish_z).sqrt();
        let gone = ((px - mark_x) * wish_x + (pz - mark_z) * wish_z) / wl;
        if gone > STUCK_NET {
            mark_x = px;
            mark_z = pz;
            ashore_stuck = 0;
        } else {
            ashore_stuck += 1;
        }
    } else {
        mark_x = px;
        mark_z = pz;
        ashore_stuck = 0;
    }
    memory.ashore_stuck = ashore_stuck;
    memory.mark_x = mark_x;
    memory.mark_z = mark_z;
    memory.stuck_frame = on_boat;
}
let ashore_stuck = if "ashore_stuck" in memory { memory.ashore_stuck } else { 0 };

// **Aboard it waits longer, and the number is a measurement.** Boarding at the
// taught aim, the capsule stands at (-4.30, -9.30) from tick 230 to 310 while
// its y creeps 1.604 to 1.750 — it is climbing the boarding treads with the
// deck heaving under it, and it makes no horizontal ground at all for eighty
// ticks. At the ashore threshold that printed `BLOCKED  go round it` across
// ticks 275-315 of the one gesture this demo teaches, where the correct
// instruction is *keep holding W*: a caption lying in the opposite direction
// from the one being fixed. Gating on "is he rising" was tried first and does
// not separate them — the first forty of those eighty ticks are flat.
//
// **Swept, both cases, at eight thresholds**, because with the blind ticks held
// the boarding run got longer too and the first guess of 150 no longer split
// them:
//
//     ticks    boarding hits    wedge
//        60          4            1
//       120          3            1
//       150          1            1
//       200          0            1
//       400          0            1
//
// The genuine wedge is unbounded — it never clears — so any threshold above the
// boarding run works and none below it does. **240**, four seconds, is 200 with
// the margin the sweep's own resolution asks for. What it costs is four seconds
// of a caption that is still correct (`the YELLOW FISH BOX is 3 m dead behind
// you`) before a second one arrives to say why he is not getting there.
let ABOARD_STUCK_TICKS = 240;

let stuck = ashore_stuck > (if aboard { ABOARD_STUCK_TICKS } else { STUCK_TICKS });

// ---------------------------------------------------------------------------
// **WHICH WAY HE IS LOOKING, which is the one fact `GameRules` cannot get.**
// ---------------------------------------------------------------------------
//
// A rules script sees `positions` and nothing else, so every direction it could
// name was a *world* direction: "the fish box is to starboard" is true about the
// boat and useless to a man standing in his own frame with his back to it, and
// `way_out`'s compass words have the same defect one deck down — there is no
// compass on this screen. `forward` is already here, flat and normalised, so the
// fix is to hand it over and let the caption say "behind you".
//
// **Quantised to quarters and emitted on change**, the same discipline as
// `station` above and for the same reason: a mouse-look turn is a per-tick
// event source and a caption is not worth a log flood. A quarter of a unit
// component is about thirteen degrees near the axes, which is four times finer
// than the ahead/left/right/behind words it feeds.
let qfx = (forward[0] * 4.0).round() / 4.0;
let qfz = (forward[2] * 4.0).round() / 4.0;
if qfx != (if "qfx" in memory { memory.qfx } else { 99.0 })
    || qfz != (if "qfz" in memory { memory.qfz } else { 99.0 }) {
    emit.push(#{ kind: "heading", fx: qfx, fz: qfz });
    memory.qfx = qfx;
    memory.qfz = qfz;
}

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
