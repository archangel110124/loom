// **THE GAIT CLOCK — and it can only live in a `GameRules` script.**
//
// A node `Script` has NO memory (`ScriptHost::tick` pushes tick, position,
// rotation, scale and a read-only copy of `state`; it takes back three vectors
// and nothing else), so fourteen joints cannot each integrate their own phase.
// A `GameRules` script's `state` is the only place in the engine that survives
// a tick, so the phase is accumulated here once and every joint reads it.
//
// It also owns the one thing a joint cannot see: **how fast he is walking.**
// A node script's `position` is its LOCAL offset, which for `Hips` is the
// constant 0.800; only a rules script gets `positions`, keyed by node path.
// So the speed is a first difference of the player's world position, and the
// two previous samples are state.
//
// What it publishes, every tick:
//     state.gait    phase in radians, 0..2*PI
//     state.speed   planar speed, m/s
//     state.blend   0 = idle, 1 = full walk
//     state.hold    0 = arms swing, 1 = both hands on the rod
//
// **Speed is smoothed, and that is not polish.** A `CharacterController`'s
// position is resolved against the collision world every tick, so a single
// tick's difference jitters by several percent while walking in a straight
// line and steps to zero for one tick on any contact. Unsmoothed, `blend`
// flickers and the whole body twitches between the idle and the walk pose. A
// 12-tick time constant is 0.2 s, below which a player cannot see the lag.
//
// **2.30 rad per metre is the stride constant.** At 1.4 m/s it gives 0.51
// strides a second, which is a normal walk. Phase advances with DISTANCE, not
// with time, so the feet cannot skate: halve the speed and the cycle halves
// with it.
//
// This file is the standalone rig's rules script. `deeper_demo.loom` has its
// own rules and carries the same six lines inside them, because the engine
// allows one `GameRules` per scene and `deeper_rules.rhai` is already it.

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
state.blend = if sm / 0.40 > 1.0 { 1.0 } else { sm / 0.40 };
state.gait = (g + sm * 2.30 * dt) % 6.283185307;
state.hold = 0.0;
