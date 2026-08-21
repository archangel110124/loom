// THE PELVIS — roll into the stance side, the bob, and the standing weight
// shift.
//
// **This is the one file in the set that hardcodes a base**, and it is allowed
// to because it is the only node that uses it. Every rotation channel in
// `deckhand.loom` is authored at zero precisely so a shared script can write
// absolute values with nothing to add them to; `position[1]` cannot be, because
// the pelvis has to sit 0.800 m off the deck. One node, one file, one number.
//
// **The bob is the cheap answer to foot planting, and since the knee's stance
// bend went to zero it is the ONLY answer.** Rigid segments on a fixed chain
// cannot plant a foot exactly — that wants two-bone IK, and a node script
// cannot see the target it would need. Lifting and dropping the pelvis instead
// takes the supporting sole's vertical error to 16.7 mm, inside the thickness
// of the sole, and it is two lines rather than an inverse kinematics solver.
// Crucially it rotates nothing, so unlike a bent stance knee it cannot move a
// planted sole sideways — see `deckhand_knee.rhai` for what that cost.
//
// **IT HANGS BELOW THE STANDING HEIGHT; IT DOES NOT SWING ABOUT IT.** That is
// the `- 1.0`, and it is forced by which way the geometry goes. A stance leg
// is LONGEST when it is vertical, so the ankle is lowest at mid-stance and the
// pelvis has to be highest there — and it is already as high as it can be,
// because standing IS the straight-leg case. Swinging symmetrically about
// 0.800 instead lifts the sole 30-53 mm clear of the deck at mid-stance at
// every amplitude and phase tried, which is a man walking on air. Measured, on
// the sole's ride through a stance (`floats` above the deck / `sinks` into it):
//
//     symmetric, 0.0340 rad 2.00   ride 20.5 mm   floats 35.7   sinks -15.2
//     hanging,   0.0280 rad 2.00   ride 26.4 mm   floats 21.8   sinks   4.6
//     hanging,   0.0340 rad 2.00   ride 20.5 mm   floats 10.1   sinks  10.4
//     hanging,   0.0400 rad 2.00   ride 16.7 mm   floats -0.8   sinks  17.6
//     hanging,   0.0478 rad 2.00   ride 28.1 mm   floats -0.8   sinks  29.0
//
// 0.0400 is the row where the sole touches the deck at mid-stance and rides
// inside its own 40 mm of boot the rest of the time. So he walks 40 mm lower
// than he stands, which is what a real walk does.
//
// `sin(2g)` because the pelvis drops twice per stride, once under each foot.
//
// **THE IDLE IS A WEIGHT SHIFT, NOT A BREATH.** A standing man rests on one
// hip, then the other, on a cycle far slower than his breathing; two rounds of
// 1.2-degree spine sway measured 1.7 grey levels of mean pixel difference over
// two and a half seconds, which is a photograph. The shift is on the same roll
// channel the walk uses, at 0.055 Hz, deliberately incommensurate with the
// spine's 0.22 and the neck's 0.13 so the three never come back into phase.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let bob = if "walk_bob" in state { state.walk_bob } else { 0.04 };
let roll = if "walk_roll" in state { state.walk_roll } else { 5.0 };
let shift = if "idle_shift" in state { state.idle_shift } else { 1.6 };

let rest = shift * sin(tick.to_float() * 0.00576);

position = [position[0],
            0.800 + b * bob * (sin(2.0 * g + 2.0) - 1.0),
            position[2]];
rotation = [0.0, 0.0, b * roll * sin(g) + (1.0 - b) * rest];
