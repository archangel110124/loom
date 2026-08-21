// **THE GAIT CLOCK — and it can only live in a `GameRules` script.**
//
// A node `Script` has NO memory (`ScriptHost::tick` pushes tick, position,
// rotation, scale and a read-only copy of `state`; it takes back three vectors
// and nothing else), so thirteen joints cannot each integrate their own phase.
// A `GameRules` script's `state` is the only place in the engine that survives
// a tick, so the phase is accumulated here once and every joint reads it.
//
// It also owns the one thing a joint cannot see: **how fast he is walking.**
// A node script's `position` is its LOCAL offset, which for `Hips` is the
// constant 0.800; only a rules script gets `positions`, keyed by node path.
// So the speed is a first difference of the player's world position, and the
// two previous samples are state.
//
// ===========================================================================
// >>> DECKHAND KNOBS — CHANGE THE WALK HERE, AND NOWHERE ELSE
// ===========================================================================
//
// Every amplitude and every rate in the whole character is one of the numbers
// below.  They are published into `state` each tick and the thirteen joint
// scripts read them from there, so **editing one line here changes the walk
// with no rebuild and no restart beyond a scene reload.**  Each joint keeps
// the same number as its own fallback, so the puppet still poses in a scene
// with no rules script — but a scene that HAS one is tuned only from here.
//
// **The block between the two markers is duplicated verbatim in
// `deeper_rules.rhai`**, because the engine allows one `GameRules` per scene
// and the demo's is already taken.  There is no import in this rhai — modules
// are switched off (`loom_script`: `set_max_modules(0)`) so a script cannot
// pull in anything the host registered elsewhere.  A test
// (`the_two_copies_of_the_deckhand_knobs_agree`) reads both files and fails if
// the two blocks stop matching, which is the only thing standing between a
// tuning session and half a character.
//
// >>> DECKHAND KNOBS
state.walk_stride = 4.36;   // rad of gait per metre walked -- see below
state.walk_leg = 30.0;      // deg  how far the thigh swings, fore and aft
state.walk_knee = 60.4;     // deg  knee bend at the top of the swing
state.walk_knee_down = 0.0; // deg  knee bend while the foot is DOWN -- see below
state.walk_knee_soft = 3.7; // deg  knee never quite straight, at any phase
state.walk_foot = 14.0;     // deg  ankle roll, heel strike to toe-off
state.walk_arm = 23.0;      // deg  shoulder swing
state.walk_elbow = 21.0;    // deg  elbow bend at the top of the swing
state.walk_bob = 0.0400;    // m    how far the pelvis hangs, twice a stride
state.walk_roll = 5.0;      // deg  pelvis roll into the standing side
state.walk_twist = 9.0;     // deg  spine counter-yaw against the pelvis
state.walk_lean = 3.0;      // deg  spine pitch, twice a stride
state.walk_stoop = 3.2;     // deg  how far forward he stands, always
state.walk_head = 6.0;      // deg  neck yaw, lagging the shoulders
state.idle_breath = 4.2;    // deg  standing sway -- spine, shoulders, neck
state.idle_shift = 1.6;     // deg  standing weight shift, hip to hip
state.walk_at = 0.40;       // m/s  speed at which the walk is fully on
// <<< DECKHAND KNOBS
//
// **`walk_stride` is the one number that decides whether the feet skate**, and
// it was 2.30 for four rounds while every note in the rig said skating was
// impossible.  Distance-driven phase makes the skate SPEED-INDEPENDENT; it
// does not make it zero.  Zero needs the sole to travel backwards under him at
// exactly the rate he travels forwards, and that pins the constant to the
// leg's own reach: half a cycle covers `pi / stride` metres of ground, the
// hip's +-30 deg on a 0.720 m hip-to-ankle chain reaches 0.720 m, so
// `pi / 0.720 = 4.36`.  At 2.30 the clock demanded a 1.37 m step from a leg
// that reaches 0.72 m and the missing 0.65 m left as slip.  Measured, on the
// slower of the two feet:
//
//     stride 2.30, sine hip, stance knee 12   skate 46.3%   planted  1.1%
//     stride 4.36, sine hip, stance knee 12   skate 43.2%   planted  9.4%
//     stride 4.36, shaped hip, stance knee 12 skate 18.2%   planted 17.8%
//     stride 4.36, shaped hip, stance knee 0  skate  5.0%   planted 86.1%
//
// **The constant alone buys almost nothing** — the other two lines are in
// `deckhand_hip.rhai` and `deckhand_knee.rhai` and all three are one fix.
// 4.36 is also the physically right number: at 1.38 m/s it is 1.9 steps a
// second, which is an adult walking pace.  0.51 strides/s, which the old
// comment called "a normal walk", is a 1.37 m stride and a stroll on the moon.
//
// **`walk_knee_down` is a real trade and 0 is a choice.** A knee bent during
// stance swings the shin forward under a planted sole -- 0.308 m of shin
// through 12 deg is 64 mm of travel the foot cannot afford -- so every degree
// here is skate.  The vertical clearance it used to buy is the pelvis bob's
// job (`walk_bob`), which costs nothing.  6.0 is the halfway house: 10.5%
// skate for a knee that visibly gives under load.
//
// What it publishes, every tick, on top of the knobs:
//     state.gait    phase in radians, 0..2*PI
//     state.speed   planar speed, m/s
//     state.blend   0 = idle, 1 = full walk
//     state.grip    0 = arms swing, 1 = both hands on the rod
//
// `grip` rather than `hold` because `deeper_rules.rhai` already uses "hold" for
// the fish hold -- four slots and a boat -- and two meanings of one word in one
// `state` map is a bug waiting for a reader.
//
// **Speed is smoothed, and that is not polish.** A `CharacterController`'s
// position is resolved against the collision world every tick, so a single
// tick's difference jitters by several percent while walking in a straight
// line and steps to zero for one tick on any contact. Unsmoothed, `blend`
// flickers and the whole body twitches between the idle and the walk pose. A
// 12-tick time constant is 0.2 s, below which a player cannot see the lag.
//
// This file is the standalone rig's rules script.

let seen = "px" in state;
let px = if seen { state.px } else { 0.0 };
let pz = if seen { state.pz } else { 0.0 };
let sm = if "speed" in state { state.speed } else { 0.0 };
let g = if "gait" in state { state.gait } else { 0.0 };

let node = "Root/Walker";
if node in positions {
    let p = positions[node];
    // **Gated on there BEING a previous sample, not on the tick number**, and
    // that is the difference between a walk and a bug. The first rules tick is
    // 1, not 0, so a `tick < 1` guard never fires: the first difference was
    // taken against an unset 0.0, read -3.0 m of travel in one tick as 180 m/s,
    // and the 12-tick smoother took eighty ticks to forget it. He spawned
    // sprinting on the spot.
    let raw = if seen {
        let dx = p[0] - px;
        let dz = p[2] - pz;
        (dx * dx + dz * dz).sqrt() / dt
    } else { 0.0 };
    state.px = p[0];
    state.pz = p[2];
    sm = sm + (raw - sm) * 0.083;   // 12-tick time constant
}

state.speed = sm;
state.blend = if sm / state.walk_at > 1.0 { 1.0 } else { sm / state.walk_at };
state.gait = (g + sm * state.walk_stride * dt) % 6.283185307;
state.grip = 0.0;
