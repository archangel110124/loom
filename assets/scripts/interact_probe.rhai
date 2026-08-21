// **The interact channel's witness.** Nothing here is a game mechanic.
//
// `interact` is the sixth digital channel (E). The engine does nothing with
// it — a script is the only thing that can — so the only way to see that a
// press arrived is to have a script say so. This one says so twice:
//
//   used       once per tick the channel is true
//   used_edge  once per rising edge of it
//
// The pair is the point. `loom sim --hold` writes `Runner::input` straight and
// holds it for the rest of the run, so `used` counts the ticks and `used_edge`
// counts the presses; `loom run` goes through `Play::set_input`, which takes
// the edge itself, so there a held key can only ever produce one of each.
//
// A real consumer should edge-detect like this, exactly as `deeper_player.rhai`
// does for `fire`, rather than trusting whoever is driving.
//
// What it reads:  interact, memory
// What it answers with: events on `emit`
// What it may keep: memory.held

// **And the seventh channel, `bag`, on the same bench.** It is the same
// machinery — a `--hold` name, a `Motion` field, a `scope.push` — so a second
// scene would be a second copy of this file with one word changed. What it is
// worth proving separately is that the two do not alias: `bagged` counts Tab
// and `used` counts E, and a run that holds one must leave the other at zero.
//
//   loom sim assets/test/interact_probe.loom --ticks 120 \
//     --hold "30:bag=1; 31:" \
//     --assert "events.bagged == 1" --assert "events.used == 0"

let held = if "held" in memory { memory.held } else { false };
let bag_held = if "bag_held" in memory { memory.bag_held } else { false };

if interact {
    emit.push(#{ kind: "used" });
    if !held {
        emit.push(#{ kind: "used_edge" });
    }
}

if bag {
    emit.push(#{ kind: "bagged" });
    if !bag_held {
        emit.push(#{ kind: "bagged_edge" });
    }
}

memory.held = interact;
memory.bag_held = bag;
