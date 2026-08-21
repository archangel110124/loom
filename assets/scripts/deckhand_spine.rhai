// THE SPINE — counter-yaw against the pelvis, and the forward pitch.
//
// **The lean is the channel that carries the SIDE view, and it was the one
// nailed to a constant.** A walk judged only from a side sheet has two of its
// four upper-body channels invisible: from +X, a Spine rot-Y and a Hips rot-Z
// both move the hat straight into the camera. Measured on the round-2 frames,
// the whole upper body — hat, collar, creel, torso — held ONE rigid attitude
// while the legs walked underneath it: the hat's centroid moved 2.1 px over
// eight frames. Raising the yaw from 4 to 9 degrees and the roll from 2.4 to 5
// bought 2.4 px of that. Making the lean oscillate bought 11.4.
//
// Twice per stride, at each heel strike, for the same reason the pelvis drops
// twice: that is when the body is catching itself.
//
// The 3.2 degrees of standing pitch is a POSE and stays, blended with
// everything else so a standing deckhand is upright.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let t = tick.to_float();

// Breathing at 0.22 Hz, deliberately incommensurate with the neck's 0.13 Hz so
// the two never come back into phase and the idle never looks like a loop.
let idle = 1.2 * sin(t * 0.0230);

rotation = [
    b * (3.2 + 3.0 * sin(2.0 * g + 0.60)) + (1.0 - b) * idle,
    b * 9.0 * sin(g),
    0.0,
];
