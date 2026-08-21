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
// **`state.fishsize` is the condition, not `phase` and no longer `infish`.**
// It is the cell area of the fish the player can *see* — on the rod tip or in
// his hands — and it is zero for a fish that is in the creel, because a fish
// that is in the creel is in a bag. `infish` was the test until the creel
// existed and it meant "in your hands"; it now means "packed", and leaving this
// on it drew a fish hanging in the cockpit that the player had put away.
//
// `gleamsprat_chrome.obj` is 0.10 m nose to tail along -Z, so scale 6.5 is a
// 65 cm fish, and it is turned nose-down because that is how a fish hangs. At
// 1.9 m from the eye it is a fifth of the frame height: this is a demo's one
// payoff and it is not being drawn coy.
//
// **And she is now as big as her footprint.** Four sizes go in the creel — 1,
// 2, 4 and 6 cells — and a conger that occupies six cells and renders at a
// sprat's 65 cm is the grid lying about the world. `1 + 0.28 * (area - 1)`
// takes 6.5 to 8.3, 12.0 and 15.6, so the conger is a metre and a half. Linear
// in *area*, not in length, which is wrong physically and right on screen: at
// a true cube root she would be 1.18 m and read as a slightly large sprat.
// **The in-hand pose backs off with her** — a 1.56 m fish at the sprat's 1.15 m
// fills the frame — one tenth of a metre per extra cell, so the conger is held
// at 1.65 m.
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

let fishsize = if "fishsize" in state { state.fishsize } else { 0.0 };
// 6.5 is the sprat, which is what every number in this file was chosen at.
let grow = 1.0 + 0.28 * (if fishsize > 1.0 { fishsize - 1.0 } else { 0.0 });
let fatness = 6.5 * grow;
let showing = if fishsize > 0.5 { 1.0 } else { 0.0 };
// **On the line, which is a state the fish did not used to have.** A landing
// put it straight into a slot; now it swings off the rod tip until the player
// presses E. `deeper_rules.rhai`'s E block is what moves it, and `online` and
// `infish` are mutually exclusive by construction there.
let online = if "online" in state { state.online } else { 0.0 };
let t = tick.to_float();
// Her frame, from the rules script, one tick stale like everything else here.
// The fallbacks are the old fixed spot, so a scene that runs this script
// without those rules still draws a fish somewhere sane.
let plx = if "plx" in state { state.plx } else { -7.40 };
let ply = if "ply" in state { state.ply } else { 0.95 };
let plz = if "plz" in state { state.plz } else { -2.70 };

if online > 0.5 {
    // **HANGING OFF THE TIP.** The same arithmetic `rod_hold.rhai` does for the
    // butt, carried 2.0 m up the blank: at 60 degrees of pitch that is 0.87
    // forward and 0.50 up per metre. Duplicated rather than shared because a
    // node script has no way to read another node's transform — `state` is
    // numbers from the rules pass and nothing else — so the alternative is
    // three more published numbers for a two-line sum. If the rod's pitch or
    // the hand offsets move, this moves with them.
    let bfx = if "bfx" in state { state.bfx } else { 0.0 };
    let bfz = if "bfz" in state { state.bfz } else { -1.0 };
    let rx = -bfz;
    let rz = bfx;
    // Swinging on the trace, which is what a fish on a line does. Smaller
    // amplitude than the in-hand swing: it is hanging from two metres of blank
    // rather than from a fist.
    let swing = (t * 0.0480).sin();
    position = [
        plx + bfx * (0.55 + 1.74) + rx * 0.30 + swing * 0.06,
        ply + 0.30 + 1.00,
        plz + bfz * (0.55 + 1.74) + rz * 0.30,
    ];
    // Hung by the nose, broadside to the eye — the same two numbers the in-hand
    // pose uses and for the same reason, which is that the mesh's origin is at
    // its nose and its long axis is local -Z.
    rotation = [90.0 + swing * 9.0, -90.0, 0.0];
    scale = [fatness, fatness, fatness];
} else if showing < 0.5 {
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
    // **In HIS frame now, not off her port beam.** The block above this file's
    // code states the limit the old pose was written under — a rules script
    // sees no rotations, so `fx`/`fz` could not be turned into hers and the
    // fish hung 1.55 m toward her port side whichever way he was looking. That
    // is exactly the "floating off the side of the ship" the player reported:
    // walk to the fish box, which means turning, and the payoff swings out over
    // open water. `state.bfx`/`bfz` is the rotation, published this round for
    // the rod, and this is the second thing it buys.
    //
    // 1.15 m ahead, 0.42 m to **port** — the hand the rod is not in — and 0.53
    // below the eye. Rendered at 0.85 m first: a 0.65 m fish at that range is
    // 2*atan(0.325/0.85) = 41 degrees of a 75 degree FOV, which is 85% of the
    // frame height and the sea disappears behind it. 1.15 m puts it at 31
    // degrees, or 41% — the biggest thing in the shot without being the only
    // thing, which is the same judgement the old pose recorded and the same
    // number it landed on.
    let bfx = if "bfx" in state { state.bfx } else { 0.0 };
    let bfz = if "bfz" in state { state.bfz } else { -1.0 };
    let rx = -bfz;
    let rz = bfx;
    // Held further out the bigger she is — see the header. 1.15 m for a sprat,
    // 1.65 for a conger.
    let out = 1.15 + 0.10 * (if fishsize > 1.0 { fishsize - 1.0 } else { 0.0 });
    position = [
        plx + bfx * out - rx * 0.42 + swing * 0.08,
        ply + 0.22 + swing * 0.03,
        plz + bfz * out - rz * 0.42,
    ];
    // **Broadside to the eye, and hung by the tail.** Degrees, and both
    // numbers were chosen by rendering it. `rot_x` 90 turns the mesh's
    // nose-tail axis (its local -Z) vertical; `rot_y` -90 then turns its thin
    // axis toward the eye so the frame gets the 0.65 x 0.38 m silhouette
    // rather than the 0.12 m edge. The mirrored pair (-90, 90) was rendered
    // too: it hangs head-up, which is the conventional way to hold a fish and
    // reads worse here, because the node's origin is at the nose so the flip
    // drops the body behind the bulwark.
    rotation = [90.0 + swing * 6.0, -90.0 + turn * 16.0, 0.0];
    scale = [fatness, fatness, fatness];
}
