// The rules of a game. Not the engine's — this file's.
//
// It runs once per tick, after everything has moved, and it is the only thing
// that can decide the game is over. The engine contributes the loop and the
// word "over"; every other idea here is authored, and none of it is in Rust:
//
//   - that there are targets at all, and which nodes they are
//   - that knocking one off the platform destroys it
//   - that destroying all of them wins
//   - that taking too long loses
//   - that there is a score, and what it counts
//
// What it is told:      tick, dt, positions, events
// What it answers with: status ("playing" | "won" | "lost"), message
// What it may keep:     state, which outlives every character in the scene
//
// `positions` is every node by path. `events` is everything that happened on
// this tick — explosions, damage, and anything a script chose to raise. Each
// carries a `kind`, a `node` it happened to, a place, and its own numbers.

// ---------------------------------------------------------------------------
// The rules, as numbers.
// ---------------------------------------------------------------------------

let TARGETS = ["Range/Target", "Range/TargetLeft"];
let PLAYER = "Range/Player";
let FULL_HEALTH = 100.0;
let BLAST_DAMAGE = 150.0;  // at the centre; the engine reports the falloff.
                           // Point blank is about 0.8 exposure, so a shot at
                           // your own feet is lethal and one a few metres off
                           // is survivable.
let PIT = -1.5;         // below this, a target has left the platform
let TIME_LIMIT = 600;   // ticks — ten seconds at sixty a tick
let POINTS_EACH = 100;

// ---------------------------------------------------------------------------
// State. It belongs to the game, not to any node: the score has to survive a
// target being destroyed, which is exactly when a per-node home would lose it.
// ---------------------------------------------------------------------------

let destroyed = if "destroyed" in state { state.destroyed } else { 0 };
let score = if "score" in state { state.score } else { 0 };
let shots = if "shots" in state { state.shots } else { 0 };
let health = if "health" in state { state.health } else { FULL_HEALTH };

// **Health is a rule, not an engine concept.** The engine reports that a
// character was caught in a blast and how exposed it was; turning that into a
// number, and deciding what happens when the number runs out, is this file's
// business. A game with shields instead of health changes here and nowhere
// else.
for e in events {
    if e.kind == "blast" {
        shots += 1;
    }
    if e.kind == "damage" && e.node == PLAYER {
        health -= e.exposure * BLAST_DAMAGE;
    }
}

// ---------------------------------------------------------------------------
// Count what is still standing.
//
// A target is destroyed when it falls past the pit line. That is a rule, not a
// physical fact — the engine has no concept of a target being destroyed, and
// would happily simulate it falling forever.
// ---------------------------------------------------------------------------

let standing = 0;
for name in TARGETS {
    if name in positions && positions[name][1] > PIT {
        standing += 1;
    }
}

let gone = TARGETS.len() - standing;
if gone > destroyed {
    // Only the newly destroyed ones score, or the score climbs every tick a
    // target spends off the edge.
    score += (gone - destroyed) * POINTS_EACH;
    destroyed = gone;
}

// ---------------------------------------------------------------------------
// The outcome.
//
// Checked in this order deliberately: clearing the last target on the tick the
// clock runs out is a win. A game that can be lost by winning too late reads
// as broken, whatever the stopwatch says.
// ---------------------------------------------------------------------------

if health <= 0.0 {
    status = "lost";
    message = "you blew yourself up";
} else if destroyed >= TARGETS.len() {
    status = "won";
    message = "all targets cleared in " + shots + " shots";
} else if tick >= TIME_LIMIT {
    status = "lost";
    message = "out of time — " + destroyed + " of " + TARGETS.len() + " cleared";
}

state.health = if health < 0.0 { 0.0 } else { health };
state.destroyed = destroyed;
state.score = score;
state.shots = shots;
