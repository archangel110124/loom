// BOTH KNEES. Side off `position[0]`, like every joint here.
//
// **HALF-WAVE RECTIFIED, NOT FULL-WAVE.** `abs(k*sin(w))` has period pi — two
// knee bends per stride, one of them in the middle of the stance phase — so
// the leg is bent 25 degrees at exactly the moment it is carrying weight. On a
// wooden deck with footstep audio that reads as skating.
//
// **THE STANCE TERM IS NOW ZERO BY DEFAULT, AND THAT IS THE THIRD LINE OF THE
// ANTI-SKATE FIX.**  It used to be 12 degrees, on this argument: the hip
// swings +-30 deg, so hip-to-ground is sqrt(h^2 + d^2) — longest at heel
// strike and toe-off, shortest under the body — and the knee has to give in
// mid-stance or the leg is too long.  The argument is right and the joint is
// the wrong one.  0.308 m of shin swung through 12 degrees carries the ANKLE
// 64 mm forward across half a stance, and a sole that moves is not planted:
//
//     walk_knee_down 12.0   skate 18.2%   sole still for 17.8% of ticks
//     walk_knee_down  6.0   skate 10.5%   ...            36.7%
//     walk_knee_down  0.0   skate  5.0%   ...            86.1%
//
// The same vertical slack is bought for nothing by `walk_bob` in
// `deckhand_hips.rhai`, which lifts the whole pelvis instead of shortening one
// leg, and which cannot move a foot sideways because it does not rotate
// anything.  Two mechanisms were solving one problem and only one of them
// charged for it.  **Turn `walk_knee_down` back up if a visibly softer knee is
// worth the slip** — 6.0 is the halfway house and the numbers above are what
// it costs.
//
// The swing term's phase is PINNED rather than fitted. A fitted pair scored
// 17 mm on the foot-plant metric by putting 48 degrees of flexion at heel
// strike — a perfectly planted foot and an obvious crouch. A plant metric
// cannot tell a walk from a squat. (Nor, until this round, a walk from a
// skate: that metric measured the sole's VERTICAL error only, which is how
// 46% horizontal slip survived underneath a number driven to 16 mm.)
//
// Negative because a knee flexes BACKWARD. `compose` maps (0,-L,0) under a
// rot-X of t to (0, -L cos t, -L sin t), so z = -L sin t and +Z is behind: a
// positive angle swings a segment forward. Both terms are `max(0, .)`, so the
// sum can never go the other way and the knee can never hyperextend.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let lift = if "walk_knee" in state { state.walk_knee } else { 60.4 };
let down = if "walk_knee_down" in state { state.walk_knee_down } else { 0.0 };
let soft = if "walk_knee_soft" in state { state.walk_knee_soft } else { 3.7 };
let w = if position[0] < 0.0 { g } else { g + 3.14159265 };
let c = cos(w);

rotation = [-b * (lift * c.max(0.0) + down * (-c).max(0.0) + soft), 0.0, 0.0];
