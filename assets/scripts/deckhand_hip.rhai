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
// the knee and ankle sharing the hip's phase, are the three relationships the
// whole cycle rests on.
//
// **THE SWING IS NOT A SINE, AND THAT IS THE HALF OF THE ANTI-SKATE FIX THAT
// IS NOT A CONSTANT.**  A planted sole has to travel backwards under him at
// exactly his own speed -- so the hip's angle has to fall LINEARLY through
// stance, not sinusoidally.  A sine is fastest in the middle of stance and
// stationary at each end, so at best it matches his speed at one instant per
// step and slips either side of it.  Measured, on the slower of the two feet
// at `walk_stride` 4.36:
//
//     sine        skate 43.2%   sole horizontally still for  9.4% of ticks
//     shaped      skate 18.2%   ...                         17.8%
//     shaped, and with the knee straight through stance:
//                 skate  5.0%   ...                         86.1%
//
// `f` is +1 at heel strike, ramps straight down to -1 at toe-off, and comes
// back on a cosine through the swing -- so the leg returns smoothly and only
// the two moments where the foot changes job carry a corner, which is where a
// walk has one anyway.  `u` is the phase measured FROM heel strike:
// `sin(w)` peaks at `w = pi/2`, so `u = w - pi/2` (+2pi to keep `%` positive).
//
// The exact law is `asin(reach * f / leg)` rather than `swing * f`, because
// what has to be linear is the sole's OFFSET and not the thigh's angle. It was
// built, measured at 18.0% against the shaped version's 18.2%, and deleted:
// two hundredths of a percent does not buy an `asin` and a clamp.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let a = if "walk_leg" in state { state.walk_leg } else { 30.0 };
let w = if position[0] < 0.0 { g } else { g + 3.14159265 };

let u = (w + 4.71238898) % 6.28318531;
let f = if u < 3.14159265 { 1.0 - u / 1.57079633 } else { -cos(u - 3.14159265) };

rotation = [b * a * f, 0.0, 0.0];
