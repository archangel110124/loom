// ONE FILE, BOTH HIPS. `HipL` and `HipR` share this script and tell themselves
// apart by `position[0]` — the node's own LOCAL offset, which nothing writes
// and which therefore stays put for the whole run. The left hip is authored at
// x -0.118 and the right at +0.118, so `position[0] < 0.0` is the side, and
// there is no per-node script and no rest-pose table to keep in step.
//
// **It writes an ABSOLUTE angle, never an increment**, and that is forced.
// `play.rs` reads the node's LIVE transform, ticks the script and writes the
// result straight back, so `rotation[0] + d` integrates forever: at 10 deg a
// tick it is a full turn in nine. The channel is therefore authored at zero in
// `deckhand.loom` and this file owns it outright.
//
// Legs are pi apart. That, each arm being pi from the leg on its own side, and
// the knee and ankle lagging the hip, are the three phase relationships the
// whole cycle rests on.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let w = if position[0] < 0.0 { g } else { g + 3.14159265 };

rotation = [b * 30.0 * sin(w), 0.0, 0.0];
