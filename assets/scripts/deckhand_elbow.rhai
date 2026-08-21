// BOTH ELBOWS.
//
// **POSITIVE, and half-wave rectified — exactly like the knee and for exactly
// the same reason: a hinge bends one way, once per stride, on its own side.**
//
// The sign is the one that was wrong for two rounds. `compose` maps (0,-L,0)
// under a rot-X of t to (0, -L cos t, -L sin t), so z = -L sin t and +Z is
// behind: a POSITIVE angle swings a segment forward. A knee flexes backward,
// so it is negative; an elbow flexes forward, so it is positive. Written
// negative, the forward hand reached 0.11 m ahead of the shoulder against
// 0.46 m behind it — a 4.2x asymmetry, with the rear hand riding 187 mm HIGHER
// than the forward one. Two hands at different heights at the same instant is
// the signature of a backwards elbow.
//
// The second bug lived in the same expression: `abs(k*sin(w + pi))` equals
// `abs(k*sin(w))`, so both elbows bent in unison, twice a stride. Half-wave
// rectification fixes the sign, the frequency and the left/right split at
// once.
//
// **A forward-flexing elbow makes the hand reach further FORWARD than back,
// necessarily**, at every amplitude — so "front and back reach within 20%" is
// not a criterion this can meet and is not one it should. The asymmetry is
// what a real arm does.
//
// The gripped pose keeps a sixth of the swing, for the reason in
// `deckhand_shoulder.rhai`: the demo never clears `grip`.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let a = if "walk_elbow" in state { state.walk_elbow } else { 21.0 };
let grip = if "grip" in state { state.grip } else { 0.0 };
let w = if position[0] < 0.0 { g } else { g + 3.14159265 };
let bend = a * sin(w + 2.69159265).max(0.0);

if grip >= 1.0 {
    rotation = [60.0 + b * bend * 0.17, 0.0, 0.0];
} else {
    rotation = [b * (11.0 + bend), 0.0, 0.0];
}
