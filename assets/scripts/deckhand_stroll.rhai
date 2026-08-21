// A character that walks in a straight line at a walking pace, so a headless
// run has something for the gait clock to measure. `loom sim` never calls
// `set_input`, so headless input is zeros forever and a player script stands
// still — the same reason `rig_patrol.rhai` exists.
//
// **1.4 m/s and no hop.** `patrol.rhai` runs at 4.5 and `rig_patrol.rhai` at
// 4.2, and both jump: at 4.2 m/s the gait clock turns 1.54 strides a second,
// which is a sprint, and a contact sheet of a sprint cannot answer whether the
// walk reads. This is the pace the amplitudes in `deckhand_spec.py` were
// fitted at.
//
// What it is told:      tick, dt, position, velocity, grounded
// What it answers with: velocity
// What it may keep:     nothing

// **He stands for the first three seconds and then walks, so ONE scene proves
// both halves.** The first half-second of that is the spawn drop — every
// character scene in this repository starts 0.1 m above the deck to prove the
// floor is there — and the rest is the idle, which is 1.2 degrees of spine
// sway against a 4.5-second breath and cannot be seen in a frame taken 20
// ticks after another one.
let want = if tick < 180 { 0.0 } else { 1.4 };

velocity = [
    velocity[0] + (want - velocity[0]) * 0.15,
    velocity[1] - 24.0 * dt,
    0.0,
];
