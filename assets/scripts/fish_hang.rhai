// **The fish, which the demo has never once shown.**
//
// Rendered from `Rig/Player/Eye` at the tick the HUD reads `LANDED   it is in
// your hands`, the frame is two rods, a bulwark and open sea — exactly the
// frame of the tick before, and of the twenty minutes before that. The payoff
// of the whole loop was a string. `IN THE CRATE   that is a trip` is the same
// again with a different string.
//
// So the catch is a thing that hangs in the cockpit from the moment it comes
// over the rail until the moment it goes in the box. Same seam as
// `float_bob.rhai` and `rod_bend.rhai`: a node script writing its own
// `position`, `rotation` and `scale` off the rules script's `state`, one tick
// stale, with no memory and therefore no state of its own.
//
// **`state.infish` is the condition, not `phase`.** `infish` is `has_count(hold,
// "fish")` — how many are in your hands right now — so the fish appears when it
// is landed and disappears on the tick it is stowed in the box, which is the
// two moments the player needs told. A phase test would show it for
// `OVER_TICKS` and then take it away while he was still carrying it.
//
// `gleamsprat_chrome.obj` is 0.10 m nose to tail along -Z, so scale 6.5 is a
// 65 cm fish, and it is turned nose-down because that is how a fish hangs. At
// 1.9 m from the eye it is a fifth of the frame height: this is a demo's one
// payoff and it is not being drawn coy.
//
// **AND IT IS IN HIS HANDS NOW, WHICH IT WAS NOT.**
//
// The first version wrote a fixed boat-local `(-7.40, 1.85, -3.60)`. Measured
// at tick 1910 of the landing tape: `Catch` at world (-0.800, 1.878, -14.056)
// and `Player` at (-0.746, 1.607, -12.305) — he walks to the fish box and it
// stays exactly where it was, hanging over open water with the caption reading
// `FISH IN HAND`. Rendered from his own eye it is a pale teardrop in an orange
// grip, floating a metre and three quarters in front of him, attached to
// nothing: the payoff of twenty-two seconds of play reads as a bug.
//
// `deeper_rules.rhai` now solves her yaw from two of her own nodes and
// publishes `state.plx/ply/plz` — the player, on her deck, in her frame — so
// this is a fixed offset from *him* instead of from her. He carries it.
//
// **The offset is toward her port beam rather than toward his own nose**, and
// that is a limit rather than a choice: `fx`/`fz` are a world heading and the
// rotation that would turn them into hers is solved one script away, in the
// rules file, which cannot write a node's transform. At the heading this demo
// teaches — the only one it reaches without a mouse, and `events.heading` is 1
// across the whole tape — port *is* dead ahead of him, because the helm mat
// faces her beam. Turn on the spot and the fish swings to your side, which is
// where a fish in one hand hangs anyway.
//
// `ponytail:` no line from the rod tip to its jaw. Needs a second node whose
// length is a scale — worth it when the rod is in his hands, not in a holder.

let infish = if "infish" in state { state.infish } else { 0.0 };
let t = tick.to_float();
// Her frame, from the rules script, one tick stale like everything else here.
// The fallbacks are the old fixed spot, so a scene that runs this script
// without those rules still draws a fish somewhere sane.
let plx = if "plx" in state { state.plx } else { -7.40 };
let ply = if "ply" in state { state.ply } else { 0.95 };
let plz = if "plz" in state { state.plz } else { -2.70 };

if infish < 0.5 {
    // Parked at the rod butt, small. Same reasoning as the float's stow: not
    // zero scale (degenerate geometry) and not below decks (one wrong number
    // and there is a fish inside the hull).
    position = [-7.62, 1.56, -2.98];
    rotation = [0.0, 0.0, 0.0];
    scale = [0.09, 0.09, 0.09];
} else {
    // Swinging, because a fish on a trace does. One slow sine on the swing and
    // a slower one on the turn, so it never reads as a pendulum on a beat.
    let swing = (t * 0.0330).sin();
    let turn = (t * 0.0190).sin();
    // **Held out to one side and below the eye line, not dead centre.**
    // `Rig/Player/Eye` is +0.75 on the capsule's centre, so `ply + 0.20` is
    // 0.55 m below his eye; 1.55 m out and 0.35 m to starboard puts it in the
    // lower-left third of the frame rather than across the horizon. Rendered
    // at 1.25 m dead ahead first, and at that distance and height a 0.65 m
    // fish is 55% of the frame and the sea is behind it — the payoff of the
    // demo should be the biggest thing in the shot, not the only thing.
    // He is 0.35 m in radius, so this never clips him, and it rides his local
    // y, so it goes up the boarding treads and over the bait box with him
    // instead of through them.
    position = [plx + 0.35 + swing * 0.10, ply + 0.20 + swing * 0.03, plz - 1.55];
    // **Broadside to the eye, and hung by the tail.** Degrees, and both
    // numbers were chosen by rendering it. `rot_x` 90 turns the mesh's
    // nose-tail axis (its local -Z) vertical; `rot_y` -90 then turns its thin
    // axis toward the eye so the frame gets the 0.65 x 0.38 m silhouette
    // rather than the 0.12 m edge. The mirrored pair (-90, 90) was rendered
    // too: it hangs head-up, which is the conventional way to hold a fish and
    // reads worse here, because the node's origin is at the nose so the flip
    // drops the body behind the bulwark.
    rotation = [90.0 + swing * 6.0, -90.0 + turn * 16.0, 0.0];
    scale = [6.5, 6.5, 6.5];
}
