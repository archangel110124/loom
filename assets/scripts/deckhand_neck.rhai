// THE NECK — and the head LAGS the shoulders, which is the one line that does
// most of the work in the front view.
//
// The spine counter-yaws against the pelvis; if the head simply rode it, the
// whole upper body would still be one rigid piece and the legs would read as a
// machine carrying a mannequin. 0.55 rad of lag is about a tenth of a stride.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };

// 0.13 Hz, against the spine's 0.22 — two idle channels that never repeat
// together.
let idle = 1.5 * sin(tick.to_float() * 0.0137);

rotation = [0.0, b * 6.0 * sin(g + 2.59159265) + (1.0 - b) * idle, 0.0];
