// THE PELVIS — roll into the stance side, and the bob.
//
// **This is the one file in the set that hardcodes a base**, and it is allowed
// to because it is the only node that uses it. Every rotation channel in
// `deckhand.loom` is authored at zero precisely so a shared script can write
// absolute values with nothing to add them to; `position[1]` cannot be, because
// the pelvis has to sit 0.800 m off the deck. One node, one file, one number.
//
// **The bob is the cheap answer to foot planting.** Rigid segments on a fixed
// chain cannot plant a foot exactly — that wants two-bone IK, and a node script
// cannot see the target it would need. Lifting and dropping the pelvis instead
// takes the supporting sole's vertical error to 16.4 mm, inside the thickness
// of the sole itself, and it is four lines rather than an inverse kinematics
// solver in the rules script. The swing sole clears the deck by 15.8 mm.
//
// `sin(2w)` because the pelvis drops twice per stride, once under each foot.
// The amplitude was re-fitted after the roll went from 2.4 to 5 degrees: 5
// degrees of roll lifts a hip 10.3 mm on its own, which was most of the error
// the old fit was carrying.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };

position = [position[0], 0.800 + b * (-0.0001 + 0.0338 * sin(2.0 * g + 1.07)), position[2]];
rotation = [0.0, 0.0, b * 5.0 * sin(g)];
