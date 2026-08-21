// BOTH KNEES. Side off `position[0]`, like every joint here.
//
// **HALF-WAVE RECTIFIED, NOT FULL-WAVE, AND IN TWO TERMS.** `abs(k*sin(w))`
// has period pi — two knee bends per stride, one of them in the middle of the
// stance phase — so the leg is bent 25 degrees at exactly the moment it is
// carrying weight. Measured on the first version of this walk: the supporting
// sole rode between -47.6 mm and +69.5 mm and never touched the deck at any
// phase. On a wooden deck with footstep audio that reads as skating.
//
// The two terms are geometry, not taste. The hip swings +-30 degrees, so the
// distance from hip to ground is sqrt(h^2 + d^2): longest at heel strike and
// toe-off, shortest under the body. The knee therefore has to be STRAIGHTEST
// AT THE EXTREMES and bent in mid-stance, which one half-wave cannot produce
// at any amplitude. `max(0, cos w)` is exactly the swing window and
// `max(0, -cos w)` exactly the stance window.
//
// The phases are PINNED rather than fitted. A fitted pair scored 17 mm on the
// foot-plant metric by putting 48 degrees of flexion at heel strike — a
// perfectly planted foot and an obvious crouch. A plant metric cannot tell a
// walk from a squat.
//
// Negative because a knee flexes BACKWARD. `compose` maps (0,-L,0) under a
// rot-X of t to (0, -L cos t, -L sin t), so z = -L sin t and +Z is behind: a
// positive angle swings a segment forward. Both terms are `max(0, .)`, so the
// sum can never go the other way and the knee can never hyperextend.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let w = if position[0] < 0.0 { g } else { g + 3.14159265 };
let c = cos(w);

rotation = [-b * (60.4 * c.max(0.0) + 12.0 * (-c).max(0.0) + 3.7), 0.0, 0.0];
