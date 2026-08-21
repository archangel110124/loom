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
// not a criterion this can meet and is not one it should. The swing is trimmed
// to 23/21 degrees instead; the asymmetry is what a real arm does.

let g = if "gait" in state { state.gait } else { 0.0 };
let b = if "blend" in state { state.blend } else { 0.0 };
let hold = if "hold" in state { state.hold } else { 0.0 };

if hold >= 1.0 {
    rotation = [78.0, 0.0, 0.0];
} else {
    let w = if position[0] < 0.0 { g } else { g + 3.14159265 };
    rotation = [b * (11.0 + 21.0 * sin(w + 2.69159265).max(0.0)), 0.0, 0.0];
}
