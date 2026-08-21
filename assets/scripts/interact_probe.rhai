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

let held = if "held" in memory { memory.held } else { false };

if interact {
    emit.push(#{ kind: "used" });
    if !held {
        emit.push(#{ kind: "used_edge" });
    }
}

memory.held = interact;
