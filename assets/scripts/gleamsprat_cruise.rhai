// Where the gleamsprat IS. Its Deform is only what shape it is.
//
// **This is the other half of ADR 0062 and it is the half that is in a hash.**
// `World::state_hash` eats every node's `GlobalTransform`, rigid body or not, so
// the numbers below are compared between debug and release builds by
// `cargo xtask validate`. The wave in the vertex shader is in no hash at all,
// because vertices are not entities.
//
// A pure function of `tick`. Node scripts get no `memory`, and leaning on that
// rather than integrating is what keeps `loom render --sim N` a single jump to
// tick N instead of a replay, and keeps the state reproducible from the file.

let t = tick.to_float() / 60.0;
let beat = 6.28318 * 3.0 * t;   // the Deform's own frequency, and it must match

// **0.24 m/s at 3.0 beats/s on a 0.161171 m body is 0.50 body lengths per
// beat, and speed and frequency are one decision rather than two.** Beat faster
// without moving faster and the animal reads as swimming against a current;
// move faster without beating faster and it glides like a thrown object.
position[0] = -0.30 + 0.24 * t;

// The recoil a rigid pod is allowed: it rises and falls with the fringe's beat
// and yaws slightly into it. Small on purpose — the pod is a rigid shell and a
// pod that swings is the artifact, not the animal. This is a Script rather than
// a second shader term precisely because it is *place*, not *shape*.
position[1] = 0.13 + 0.008 * sin(beat);
rotation[1] = 90.0 + 4.0 * sin(beat);
