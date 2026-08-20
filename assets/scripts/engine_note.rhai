// **The engine note, which is a distance because distance is the only knob.**
//
// `AudioSource` has `volume` and `range` and both are authored constants; a
// node script may write `position`, `rotation` and `scale` and nothing else;
// and `sound.rs` starts a `Voice` only for an `autoplay` source, so there is no
// way to begin or end one from a rule. What is left is where the sound *is* —
// and `range` is documented linear, so moving a source is a fader with the
// engine's ray-traced occlusion still attached to it.
//
// So this node dives. Moored she sits 12 m under the keel, through the hull and
// most of a range of 14, which is a whisper; under way it rises to just below
// the cockpit sole. **Straight down, never sideways**: the hull stays between
// it and the helmsman at every setting, so what changes is loudness and not
// which side of him it is on. A source that swung round her would pan across
// the listener every time he opened the throttle.
//
// **Moored is silent, and that is right rather than a shortcut.** A boat at her
// berth with nobody at the wheel has her engine off. The demo's first six
// seconds are sea only; the note comes up as she gathers way, and dies as she
// coasts, because `state.knots` is her measured deck speed rather than the
// throttle setting — so it lags the key, which is what a boat does.
//
// `state.knots` is written by `deeper_rules.rhai` from the hull's own position
// one tick to the next; it is smoothed there, so nothing more is needed here.

let knots = if "knots" in state { state.knots } else { 0.0 };
// Astern is as loud as ahead.
let k = if knots < 0.0 { -knots } else { knots };

// **6.0 kn, not the 7 kn wall.** ADR 0063 measured this hull settling at 3.586
// m/s — a shade under seven knots — so a full-scale at 7 would never quite be
// reached and the note would sit forever just short of open. Anything at or
// above six is full.
let f = if k >= 6.0 { 1.0 } else { k / 6.0 };

// -12.0 m to +0.55 m in boat-local y. The top end is under the cockpit sole
// (0.64) and the bottom is 12 m down against a 14 m range, which is 14% of the
// linear falloff before the hull is even counted.
position = [-7.80, -12.0 + 12.55 * f, 0.0];
