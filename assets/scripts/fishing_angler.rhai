// The angler at the rod. Six lines, and they are the only path a human's
// hands have into the fight.
//
// A node `Script` is the one thing in the engine that sees input; `GameRules`
// sees `positions` and `events` and no keys at all. So this reads the two
// bindings the fight spends and forwards them as an event, which the rules
// script picks up on the **same tick** — the motion pass runs before the rules
// pass, so nothing is a frame stale.
//
// Two verbs, deliberately. `move_z` forward is reel, `fire` is the cast, the
// hook-set and the rhythm window. `move_x`, `sprint` and `jump` are left
// unspent so that counter-steer, drag and cut-the-line can land later without
// an engine change and without moving what the player already learned.
emit.push(#{
    kind: "angler",
    reel: if move_z > 0.5 { 1.0 } else { 0.0 },
    press: if fire { 1.0 } else { 0.0 },
});

// The angler stands at the rail and fishes. Standing still is the movement
// model here, and it is a movement model — a script that left `velocity`
// untouched would coast, because an unwritten velocity means "keep going".
velocity = [0.0, 0.0, 0.0];
