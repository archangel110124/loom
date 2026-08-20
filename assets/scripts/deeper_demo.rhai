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

// **The one prompt in the game, and it is a place rather than a state.**
//
// The helm has no key to learn — you stand on the mat and W/S/A/D are the
// boat — but a stranger at the wheel could not tell they were driving:
// standing there at the opening yaw the whole frame is a rod holder and open
// sea, because the mat faces her beam and the bow is ninety degrees to the
// right. The layout answer is to berth her bow-out, which moves every pinned
// drive number in the gate and belongs to the round that re-berths her. A line
// of text is what a demo does in the meantime, and it also carries the one
// thing nothing else can teach: **Space lets go of the wheel.**
//
// It is keyed on the drawn mat's *world* position out of `positions`, so it
// follows her when she turns and when she pitches, and there is no second copy
// of the helm's bounds to drift out of step with `deeper_player.rhai`'s.
//
// **Worded as a station and not as a readout**, deliberately: `GameRules` sees
// no input, so it cannot know whether the player has pressed Space, and a line
// that claimed "at the wheel" after they had let go would be lying. "THE HELM"
// is true either way. Emitting the release as an event to close that gap would
// mean an event every tick, which is a log flood for a caption.
let MAT = "Rig/Boat/HelmMat";
let MAT_RANGE = 1.35;   // horizontal. The mat is 2.6 x 1.75 m and this is a
                        // little inside its half-diagonal, so it fires where
                        // `deeper_player.rhai`'s box does and not before.
let HELM_LINE = "THE HELM   W ahead  S astern  A/D wheel  SPACE hands off";

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

// Standing on the mat replaces the line rather than adding to it. `Hud` is one
// string and one row; two claims sharing it would be a scrolling caption, and
// the hold is the one thing you can check by pressing nothing at all.
let at_the_helm = false;
if PLAYER in positions && MAT in positions {
    let at = positions[PLAYER];
    let mat = positions[MAT];
    let mdx = at[0] - mat[0];
    let mdz = at[2] - mat[2];
    let mdy = at[1] - mat[1];
    // The vertical band is what stops it firing on a swimmer under her hull or
    // on someone up on the flybridge: standing on the mat puts the capsule's
    // centre 0.90 above it.
    at_the_helm = mdx * mdx + mdz * mdz <= MAT_RANGE * MAT_RANGE
        && mdy > 0.35
        && mdy < 1.55;
}
if at_the_helm { message = HELM_LINE; }

// **Assertable, because the caption is not.** `--assert` reads numbers, and
// `state.at_helm` is the only way a gate can say "the player reached the
// wheel" rather than "the player is somewhere inside a position band a
// drifting boat could also satisfy".
state.at_helm = if at_the_helm { 1 } else { 0 };
