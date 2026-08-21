// THE NECK — and the head LAGS the shoulders, which is the one line that does
// most of the work in the front view.
//
// The spine counter-yaws against the pelvis; if the head simply rode it, the
// whole upper body would still be one rigid piece and the legs would read as a
// machine carrying a mannequin. 0.55 rad of lag is about a tenth of a stride.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let a = if "walk_head" in state { state.walk_head } else { 6.0 };
let breath = if "idle_breath" in state { state.idle_breath } else { 4.2 };

// 0.13 Hz, against the spine's 0.22 and the pelvis's 0.055 — three idle
// channels that never repeat together.
let idle = breath * 0.36 * sin(tick.to_float() * 0.0137);

rotation = [0.0, b * a * sin(g + 2.59159265) + (1.0 - b) * idle, 0.0];
