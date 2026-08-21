// A character that walks in a straight line at a walking pace, so a headless
// run has something for the gait clock to measure. `loom sim` never calls
// `set_input`, so headless input is zeros forever and a player script stands
// still — the same reason `rig_patrol.rhai` exists.
//
// **1.4 m/s and no hop.** `patrol.rhai` runs at 4.5 and `rig_patrol.rhai` at
// 4.2, and both jump. This is the pace the amplitudes in `deckhand_gait.rhai`
// were fitted at, and at `walk_stride` 4.36 it is 1.9 steps a second.
//
// What it is told:      tick, dt, position, velocity, grounded
// What it answers with: velocity
// What it may keep:     nothing
//
// **He stands for the first three seconds and then walks, so ONE scene proves
// both halves.** The first half-second of that is the spawn drop — every
// character scene in this repository starts 0.1 m above the deck to prove the
// floor is there.
//
// **THE RAMP IS ON `tick`, NOT ON `velocity`, AND THAT IS A BUG FIX.**  It used
// to lerp `velocity[0]` toward the target at 0.15 a tick, and `play.rs` feeds
// back the velocity that SURVIVED the move — so every one-tick contact with
// deck geometry zeroed the walk and cost fifteen ticks of re-ramp.  Over ticks
// 300-800 that held his mean speed at 1.223 m/s against a commanded 1.4 and
// stalled him outright on 12 ticks of 500.  A ramp on the tick number has no
// feedback path into the collision result: 1.381 m/s and 6 stalls, on the same
// scene, changing nothing else.  The stalls that remain are the controller's
// and are the same with the body's 27 meshes deleted.
let t = (tick.to_float() - 180.0) / 30.0;
let want = if t < 0.0 { 0.0 } else if t > 1.0 { 1.4 } else { 1.4 * t };

velocity = [
    want,
    velocity[1] - 24.0 * dt,
    0.0,
];
