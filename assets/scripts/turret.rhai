// A turret: stands still and fires once, on a tick it chooses.
//
// The same seam a player's weapon uses, with nobody at the keyboard. That is
// the point of it — a movement script is not "the thing the human drives", it
// is the movement and behaviour model for a character, and most characters in
// a finished game have no human behind them.
//
// It is also what makes the trigger checkable. `loom sim` has no input, so a
// weapon bound to a mouse button can only be verified by a human clicking;
// one bound to a tick count is verified by an assertion.

let FIRE_AT = 30;

// Gravity, and nothing else. A turret does not walk.
let vy = velocity[1] - 24.0 * dt;
if grounded && vy < 0.0 { vy = 0.0; }

// One shot, at a tick, at whatever the ray found. `aim_hit` still matters:
// a turret pointed at open sky should not detonate the sky.
if tick == FIRE_AT && aim_hit {
    detonate = aim_point;
    detonate_radius = 5.0;
    detonate_impulse = 700.0;
}

velocity = [0.0, vy, 0.0];
