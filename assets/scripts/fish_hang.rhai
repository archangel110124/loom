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
// `ponytail:` it hangs in mid-air. A line from the rod tip to its jaw is the
// obvious next thing and needs a second node whose length is a scale — worth
// it when the rod is in the player's hands rather than in a holder.

let infish = if "infish" in state { state.infish } else { 0.0 };
let t = tick.to_float();

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
    position = [-7.40 + swing * 0.10, 1.85 + swing * 0.03, -3.60];
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
