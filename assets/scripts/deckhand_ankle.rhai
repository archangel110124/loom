// BOTH ANKLES. Heel strike to toe-off roll, lagging the hip.
//
// **This is the joint that proves the side rule matters.** It is the only one
// of the six leg and arm joints that is NOT rectified, so when the knee, elbow
// and ankle pivots were all authored at x = 0.0 and `position[0] < 0.0` was
// false on both sides, the knees and elbows survived by luck —
// `abs(sin(p)) == abs(sin(p + pi))` — and both feet rolled in unison while the
// legs were half a cycle apart. The pivots now carry a millimetre of offset
// purely so this test has an answer.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let w = if position[0] < 0.0 { g } else { g + 3.14159265 };

rotation = [b * 14.0 * sin(w), 0.0, 0.0];
