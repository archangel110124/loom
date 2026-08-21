// BOTH SHOULDERS, and it writes TWO channels.
//
// `rotation[0]` is the swing, in the sagittal plane. Each arm is pi from the
// leg on its own side, which is what contralateral means and is the second of
// the three phase relationships the cycle rests on.
//
// `rotation[2]` is the SPLAY, and it is zero for the whole walk. Under `Y-X-Z`
// a rot-Z applies BEFORE the swing, so it tucks the elbow inboard and carries
// the hand toward the centre line without tilting the swing plane. Two arms
// swinging in the sagittal plane alone put the hands 0.52 m apart, which is
// shoulder width and not a grip on a rod; +-22 degrees brings them to 0.23 m.
//
// **`state.grip` REPLACES the walk term, it does not add to it.** A hand that
// swings while it is holding a rod is worse than no animation at all.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let grip = if "grip" in state { state.grip } else { 0.0 };
let left = position[0] < 0.0;

if grip >= 1.0 {
    // Solved as forward kinematics in the engine's own convention, not summed
    // by eye: 32 at the shoulder and 78 at the elbow put the mittens at
    // y 1.238, z -0.483 — between the hips and the sternum, 40 degrees below
    // the eye, with both elbows a quarter of a metre BELOW the shoulders.
    rotation = [32.0, 0.0, if left { 22.0 } else { -22.0 }];
} else {
    let w = if left { g } else { g + 3.14159265 };
    // A slow breath at 0.22 Hz, cross-fading in as the walk fades out, so a
    // standing deckhand is never a photograph.
    let idle = 2.0 * sin(tick.to_float() * 0.0230 + 0.4);
    rotation = [b * 23.0 * sin(w + 3.14159265) + (1.0 - b) * idle, 0.0, 0.0];
}
