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
// **`state.grip` REPLACES the walk swing, it does not add to it.** A hand that
// swings while it is holding a rod is worse than no animation at all. What it
// does NOT replace is the body underneath: a gripped arm still rides a walking
// stride and still breathes standing still, at a fraction of the free swing.
// The demo holds `grip = 1` for its whole run, so without that fraction the
// most finished part of the upper body is never seen moving in the one game
// that ships this character.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let a = if "walk_arm" in state { state.walk_arm } else { 23.0 };
let breath = if "idle_breath" in state { state.idle_breath } else { 4.2 };
let grip = if "grip" in state { state.grip } else { 0.0 };
let left = position[0] < 0.0;
let w = if left { g } else { g + 3.14159265 };

// A slow breath at 0.22 Hz, cross-fading in as the walk fades out, so a
// standing deckhand is never a photograph.
let idle = breath * 0.48 * sin(tick.to_float() * 0.0230 + 0.4);

if grip >= 1.0 {
    // **Solved by measurement, against the rod's own butt.** The pose used to
    // be 32/78 with a symmetric +-22 splay, which puts the mittens 0.403 m in
    // front of the capsule centre, 0.390 above it and 0.013 to PORT — while
    // `rod_hold.rhai` held the butt 0.30 m to STARBOARD. The reflection was a
    // man miming, with his tackle floating 36 cm off his hands, and that is the
    // one shot the mirror exists for.
    //
    // Neither end could move alone. The rod's offset is a view-model
    // composition measured at the first-person camera (`rod_hold.rhai`), and
    // 0.30 m of starboard is not reachable by two arms without crossing them:
    //
    //     shoulder 32 splay  +0 elbow 78 -> fwd 0.403 up 0.390 stbd -0.013
    //     shoulder 32 splay +14 elbow 78 -> fwd 0.396 up 0.400 stbd +0.069
    //     shoulder 44 splay +14 elbow 66 -> fwd 0.433 up 0.444 stbd +0.081
    //     shoulder 44 splay +20 elbow 60 -> fwd 0.433 up 0.428 stbd +0.131
    //     shoulder 50 splay +18 elbow 58 -> fwd 0.444 up 0.465 stbd +0.117
    //
    // So both ends moved: this pose, and the rod to (0.50, 0.40, 0.18). 88 mm
    // apart, which is one mitten.
    //
    // **The splay is asymmetric now and that is the pose, not a bug.** A bias
    // of +20 on both arms is the right hand back on the butt and the left
    // forward across the body, which is how a rod is actually held.
    //
    // The stride rides on top at a sixth of the free swing: hands stay on the
    // rod, arms stop being a photograph.
    rotation = [
        44.0 + b * a * 0.17 * sin(w + 3.14159265) + (1.0 - b) * idle * 0.5,
        0.0,
        if left { 42.0 } else { -2.0 },
    ];
} else {
    rotation = [b * a * sin(w + 3.14159265) + (1.0 - b) * idle, 0.0, 0.0];
}
