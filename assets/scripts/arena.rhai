// The rules of Proving Ground.
//
// Hunters walk out of the dark. Blow them off the platform before they reach
// you. Every idea here is a rule and none of it is in the engine: that a
// hunter in the pit is dead, that clearing the field wins, that running out of
// time loses, that being hit costs health, that a kill is worth a hundred.
//
// The engine's half is what a script cannot do — casting to see who a blast
// caught and how exposed they were, routing the hunters around the pillars,
// and measuring what the generator sounds like from where you are standing.
//
// No waves, because a scene cannot spawn nodes yet. Three hunters, authored
// in the file, and the game is over when they are or when you are. Said
// plainly rather than faked with a counter that respawns nothing.

let HUNTERS = ["Arena/Hunter1", "Arena/Hunter2", "Arena/Hunter3"];
let PLAYER = "Arena/Player";
let PIT = -4.0;             // below this, gone
let TIME_LIMIT = 3600;      // one minute at sixty ticks
let FULL_HEALTH = 100.0;
let DAMAGE_SCALE = 150.0;   // exposure of 1.0 is instantly fatal
let POINTS_EACH = 100;
let SURVIVAL_BONUS = 5;     // per second left on the clock

let health = if "health" in state { state.health } else { FULL_HEALTH };
let score = if "score" in state { state.score } else { 0 };
let killed = if "killed" in state { state.killed } else { 0 };

// ---------------------------------------------------------------------------
// What happened this tick.
//
// A blast and a claw arrive the same way: the engine raises one for the
// explosion it measured, `enemy.rhai` raises one for a hit it landed, and both
// carry exposure. Health is turned from that here and only here.
// ---------------------------------------------------------------------------

for e in events {
    if e.kind == "damage" && e.node == PLAYER {
        health -= e.exposure * DAMAGE_SCALE;
    }
}

// ---------------------------------------------------------------------------
// Who is left. A hunter below the pit line is dead — a rule, not a physical
// fact. The engine would happily simulate it falling for the rest of the run.
// ---------------------------------------------------------------------------

let standing = 0;
for name in HUNTERS {
    if name in positions && positions[name][1] > PIT {
        standing += 1;
    }
}

let gone = HUNTERS.len() - standing;
if gone > killed {
    // Only the newly dead score, or the total climbs every tick a body spends
    // below the floor.
    score += (gone - killed) * POINTS_EACH;
    killed = gone;
    emit.push(#{ kind: "kill", killed: killed });
}

// ---------------------------------------------------------------------------
// The outcome. Death first: dying on the tick you clear the field is a death,
// because the claw landed.
// ---------------------------------------------------------------------------

if health <= 0.0 {
    status = "lost";
    message = "they got you — " + killed + " of " + HUNTERS.len() + " down";
} else if standing == 0 {
    let left = (TIME_LIMIT - tick) / 60;
    score += left * SURVIVAL_BONUS;
    status = "won";
    message = "arena cleared with " + left + "s to spare — " + score + " points";
} else if tick >= TIME_LIMIT {
    status = "lost";
    message = "out of time — " + standing + " still standing";
}

state.health = if health < 0.0 { 0.0 } else { health };
state.score = score;
state.killed = killed;
