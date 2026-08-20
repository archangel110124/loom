// **The demo's rules, and the four slots the human asked for.**
//
// ---------------------------------------------------------------------------
// WHERE THE INVENTORY LIVES, AND WHY IT LIVES HERE
// ---------------------------------------------------------------------------
//
// There is no inventory component in this engine and this file is the decision
// not to add one. The hold is two keys in `GameRules`' `state`, and the three
// candidates were weighed like this:
//
//   * **A movement script's `memory`.** It is the only place that sees a key
//     press, so it looked like the obvious home. It is invisible to
//     `loom sim --assert` and invisible to the `Hud`, which draws the rules
//     script's `message` and nothing else. An inventory nothing can read is an
//     inventory nobody can test and the player cannot see. Rejected.
//   * **The event log.** `events.pickup >= 1` is already assertable and the log
//     is already deterministic, so it is a fine record of *what happened*. It
//     is a poor *container*: reconstructing "what am I holding now" means
//     folding the whole log every tick, and there is no way to take something
//     out again. It stays as the audit trail, not the store.
//   * **`GameRules` state.** Assertable (`state.carried >= 2`), drawable
//     (`{message}` is a rules-script string), survives the whole session, and
//     is already how `fishing_fight.rhai` carries a fight. Chosen.
//
// The cost, stated plainly: `state` is **not** in `state_hash`, which is
// physics-only. So `cargo test` and `cargo xtask repeat` are blind to the hold
// and `loom sim --assert` is its only detector — the same bargain the fight
// makes, for the same reason, written down in the same place.
//
// ---------------------------------------------------------------------------
// WHAT FILLS THEM
// ---------------------------------------------------------------------------
//
// Four supplies, taken by standing next to them. **Deliberately not pickups of
// unique objects**: nothing in this engine can remove a node at runtime, so an
// object that vanished into your pocket would still be sitting on the deck
// afterwards — simulation self-consistent, picture a lie. A barrel you take a
// handful of bait from is still a barrel. The slots therefore hold *supplies*,
// and the fish a later round lands never existed as a node, so it will not need
// to disappear either.
//
//   loom sim assets/test/rig_walk.loom --ticks 900 \
//     --assert "state.carried >= 3" --assert "events.pickup >= 3"

let RANGE = 1.7;   // metres, horizontal. A pace off the crate.
let SLOTS = 4;     // "three to four items they can carry", and four is the
                   // number the rig authors, so a fifth is unreachable rather
                   // than refused. The guard below is still there, because the
                   // round that adds a fifth supply must find a full hold.

// The supplies, as node path and the word the HUD prints. The paths are the
// contract with the scene: rename a crate and it stops being takeable, which
// is a fault a `--assert` catches on the next run.
let SUPPLY = [
    ["Rig/BaitBarrel", "bait"],
    ["Rig/LineSpool", "line"],
    ["Rig/Lantern", "lantern"],
    ["Rig/Thermos", "thermos"],
];

let PLAYER = "Rig/Player";

let hold = if "hold" in state { state.hold } else { [] };

// Nothing to do before the player exists in the position map.
if PLAYER in positions {
    let at = positions[PLAYER];

    for entry in SUPPLY {
        let path = entry[0];
        let name = entry[1];

        // Already carrying it. `hold` is the truth; a per-item flag beside it
        // would be a second opinion about one fact.
        let have = false;
        for held in hold {
            if held == name { have = true; }
        }
        if !have && hold.len() < SLOTS && path in positions {
            let it = positions[path];
            let dx = at[0] - it[0];
            let dz = at[2] - it[2];
            // **Horizontal only.** The crates sit on the deck and the player's
            // origin is the capsule's centre, 0.9 m up — a spherical range
            // would spend most of itself on a height difference that is the
            // same at every crate and means nothing.
            if dx * dx + dz * dz <= RANGE * RANGE {
                hold.push(name);
                emit.push(#{ kind: "pickup", item: name });
            }
        }
    }
}

state.hold = hold;
state.carried = hold.len();

// The HUD line. One string, assembled here, because `Hud` draws `{message}` and
// has no list widget — and a list widget cannot change what the demo teaches,
// so it must not be allowed to delay it.
let line = "";
for held in hold {
    if line != "" { line += " - "; }
    line += held;
}
if line == "" { line = "empty"; }

message = "HOLD " + hold.len() + "/" + SLOTS + "   " + line;
