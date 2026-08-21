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
// `walk_stoop` is a POSE and stays, blended with everything else so a standing
// deckhand is upright.
//
// **The idle sway is 4.2 degrees where it was 1.2, and the old number was
// below the floor.** Two independent reviews measured the shipped idle's MEAN
// pixel difference over 155 ticks at 1.7 and 3.1 grey levels out of 255 — a
// changed-pixel count in the double digits with nothing behind it, which is
// antialiasing on a pale mitten and not a man breathing. Changed-pixel counts
// cannot tell a breath from a still; the mean can, and it said still.
//
// Re-measured on one instrument, ticks 20 against 175 at 420x460, whole frame:
//
//     1.2 deg, no weight shift   differing 5.7%   mean 0.71   worst 239
//     4.2 deg, shift 1.6         differing 9.6%   mean 2.13   worst 245
//     8.4 deg, shift 3.2         differing 10.9%  mean 3.20   worst 245
//
// Three times the motion for the first step and one and a half for the second,
// and 8 degrees of spine sway on a standing man is a visible lean rather than a
// breath. `idle_breath` and `idle_shift` are knobs; these are the numbers to
// re-run if either moves.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let twist = if "walk_twist" in state { state.walk_twist } else { 9.0 };
let lean = if "walk_lean" in state { state.walk_lean } else { 3.0 };
let stoop = if "walk_stoop" in state { state.walk_stoop } else { 3.2 };
let breath = if "idle_breath" in state { state.idle_breath } else { 4.2 };

// Breathing at 0.22 Hz, deliberately incommensurate with the neck's 0.13 Hz
// and the pelvis's 0.055 Hz so no two ever come back into phase and the idle
// never looks like a loop.
let idle = breath * sin(tick.to_float() * 0.0230);

rotation = [
    b * (stoop + lean * sin(2.0 * g + 0.60)) + (1.0 - b) * (stoop + idle),
    b * twist * sin(g),
    0.0,
];
