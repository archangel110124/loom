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

// One shot, on the first tick at or after `FIRE_AT` where the ray actually
// found something.
//
// **Not `tick == FIRE_AT`.** Every character is handed the same look
// direction, so in the editor this turret aims wherever the human points the
// mouse. Turn away on that one exact tick and `aim_hit` is false — and with an
// equality test the turret then never fires again, ever. The game ran its full
// ten seconds and reported LOST with zero shots, which is a broken weapon
// rather than a missed shot.
//
// `aim_hit` still matters: a turret pointed at open sky should not detonate
// the sky. It waits for a target instead of giving up.
let fired = if "fired" in memory { memory.fired } else { false };
if !fired && tick >= FIRE_AT && aim_hit {
    detonate = aim_point;
    detonate_radius = 5.0;
    detonate_impulse = 700.0;
    fired = true;
}
memory.fired = fired;

velocity = [0.0, vy, 0.0];
