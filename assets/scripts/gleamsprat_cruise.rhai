// Where the gleamsprat IS. Its Deform is only what shape it is.
//
// **This is the other half of ADR 0062 and it is the half that is in a hash.**
// `World::state_hash` eats every node's `GlobalTransform`, rigid body or not, so
// the numbers below are compared between debug and release builds by
// `cargo xtask validate`. The wave in the vertex shader is in no hash at all,
// because vertices are not entities.
//
// **A pure function of `tick`, and every line below is one.** Node scripts get
// no `memory`, so there is nothing to integrate into; leaning on that rather
// than fighting it is what makes the state reproducible from the file alone and
// what makes any tick answerable without replaying the ticks before it.
//
// (`Sim::step` does loop once per tick — `--sim N` is a replay, not a jump, and
// an earlier version of this comment said otherwise. The purity is worth having
// anyway, and it is what lets a reader check tick 65 with a calculator.)

let t = tick.to_float() / 60.0;
let beat = 6.28318 * 3.0 * t;   // the Deform's own frequency, and it must match

// **0.24 m/s at 3.0 beats/s on a 0.161171 m body is 0.50 body lengths per
// beat, and speed and frequency are one decision rather than two.** Beat faster
// without moving faster and the animal reads as swimming against a current;
// move faster without beating faster and it glides like a thrown object.
//
// **Plus the surge, which is the single cue that separates a swimmer from a
// pushed object.** A constant-velocity fish is a fish on a rail: the fringe
// waves, the body translates, and nothing connects the two. A real swimmer
// gains on the stroke and coasts between, so the displacement carries a small
// oscillation and the *velocity* is what the eye actually reads:
//
//   x(t) = -0.30 + 0.24 t - 0.002 cos(2 beat)
//   v(t) = 0.24 + 0.075 sin(2 beat)      -> 0.165 .. 0.315 m/s
//
// **Twice the beat, because the fringe pushes on both halves of its cycle** —
// the thrust peaks at each extreme of the stroke, not once per round trip.
// Never negative: a swimmer that stops dead twice a second is a different and
// much worse artifact than one that does not surge at all. The mean is
// untouched, so "0.50 body lengths per beat" above is still exactly true and
// the golden row at tick 60 moves by 2 mm.
position[0] = -0.30 + 0.24 * t - 0.002 * cos(2.0 * beat);

// **The weave, which is the cue that says an animal is choosing where to go.**
// A creature travelling in a dead-straight line at a constant heading is the
// screensaver reading, however good the wave on its body is: the fringe moves,
// the body translates, and nothing about the path is a decision. One lazy S
// over the run fixes it, and it costs a line.
//
// 0.22 Hz is slow against the 3 Hz beat on purpose — it is a course, not a
// wiggle, and 133 ticks is a little over half a cycle, so the animal is turning
// throughout and never repeats inside the shot.
let weave = 6.28318 * 0.22 * t;
position[2] = -0.20 + 0.04 * sin(weave);

// **And the heading follows the path rather than being authored beside it.**
// The nose must point along the velocity or the animal crabs sideways, which is
// a worse tell than not weaving at all. `rotation[1] = 90` points the nose at
// +X, so the heading is 90 degrees less the angle the path makes with +X:
//
//   vx = 0.24 + 0.075 sin(2 beat)          the surge, differentiated
//   vz = 0.04 * 2pi * 0.22 * cos(weave)    the weave, differentiated
//      = 0.0553 cos(weave)
//
// **Both are hand-differentiated, so changing a line above means changing one
// here.** That is the cost of a closed form with no `memory` to hold a previous
// position in, and it is cheaper than the alternative: a heading integrated
// from finite differences would make the animal's facing depend on the tick
// rate, which is exactly what a fixed step exists to prevent.
//
// +/-13 degrees is the swing that falls out. `atan` rather than `atan2`,
// because `vx` is 0.165 at its slowest and never reaches zero — the same fact
// the surge's own comment turns on. The last term is the pod's recoil into its
// own stroke, much smaller because a rigid shell that swings is the artifact
// rather than the animal.
let vx = 0.24 + 0.075 * sin(2.0 * beat);
let vz = 0.0553 * cos(weave);
rotation[1] = 90.0 - atan(vz / vx) * 57.29578 + 4.0 * sin(beat);

// The recoil a rigid pod is allowed vertically: it rises and falls with the
// fringe's beat. This is a Script rather than a second shader term precisely
// because it is *place*, not *shape*.
position[1] = 0.13 + 0.008 * sin(beat);
