// **The float, and it is the only thing in the frame that says you are
// fishing.**
//
// The measured complaint about this demo is that the player's own view never
// changes while he fishes. Rendered from `Rig/Player/Eye` at the moment the
// HUD reads `LINE OUT, BAITED`, eight headings 45 degrees apart, the frame is
// two rods standing in their holders and open sea — the identical frame it was
// before he cast, and the identical frame it will be while a fish is running.
// Everything the fishing loop does, it does in text.
//
// A float fixes that with one node. It sits out on the water where he is
// already looking, it **dunks on the take**, and it is dragged under and swung
// while the fish runs. That is the picture the caption has been carrying alone.
//
// ---------------------------------------------------------------------------
// Why a node script and not a component
// ---------------------------------------------------------------------------
//
// `rod_bend.rhai` established the seam: a node script may write its own
// `position`, `rotation` and `scale`, and reads the rules script's `state` one
// tick stale. That is enough for all of this, and it means the float is
// authored content — nothing in Rust knows it exists. Runtime spawning is
// forbidden (and would be wrong here anyway): the float is always in the
// scene, and casting is where it goes, not whether it exists.
//
// **There is no memory in a node script**, so every number below is a pure
// function of `tick` and `state`. That is not a limitation to work around; it
// is what makes the float free of the sim hash and identical in three
// processes.
//
// ---------------------------------------------------------------------------
// The phases, from `deeper_rules.rhai`
// ---------------------------------------------------------------------------
//
//   0  nothing out            stowed at the rod
//   1  line out, waiting      on the water, bobbing
//   3  the take               DUNKED — this is the 600 ms window the HUD
//                             shouts `BITE! CLICK NOW` in, and until now the
//                             shout was the entire signal
//   4  the fight              under, and swinging with the stress
//   5  landed / 6  lost       stowed again
//
// ---------------------------------------------------------------------------
// Where
// ---------------------------------------------------------------------------
//
// Boat-local, because the node hangs off `Rig/Boat` — you fish from wherever
// she is, and a float parented to the static rig would stay at the berth when
// she left. The hull is a dynamic body, so a renderable under it bakes no
// static collider (`play.rs`); this is a mark on the water and not a thing to
// walk into.
//
// The rod's butt is at boat-local (-7.60, 1.62, -2.90) leaning 45 degrees
// outboard, tip about (-7.60, 3.25, -4.30). The helm eye is at (-7.30, 2.29,
// -1.775) looking down -Z. So -Z is out to sea past the rod, and a float at
// z = -8.6 is 6.8 m dead ahead and 19 degrees below the centre of frame: in
// shot from the helm, from the cockpit, and from the port side deck.
//
// `ponytail:` boat-local y = 0 is the waterline only while she is upright. She
// rolls, and at a 6.1 m lever arm five degrees of heel is half a metre of
// float that is briefly in the air or under. A float that read the surface
// would need `water@x,z.height` inside a node script, which does not exist.
// Worth building when the sea gets rougher than a harbour.

let phase = if "phase" in state { state.phase } else { 0.0 };
let stress = if "stress" in state { state.stress } else { 0.0 };
let t = tick.to_float();

if phase < 0.5 || phase > 4.5 {
    // **Stowed is small and at the rod, not scaled to zero.** A zero scale is
    // degenerate geometry, and parking it out of sight under the deck means
    // one wrong number puts a bright orange ball inside the hull. Clipped to
    // the blank it reads as the lure waiting on the rod, which is where a lure
    // is between casts.
    position = [-7.60, 1.50, -3.05];
    scale = [0.045, 0.045, 0.045];
} else {
    // Two slow sines, deliberately not harmonics of each other, so the bob
    // never repeats on a beat the eye can catch.
    let swing = (t * 0.0170).sin();
    let bob = (t * 0.0431).sin();

    // The take: under it goes, hard, for the whole 600 ms the player has to
    // answer in. `phase == 3` is only ever entered once per fish.
    let taken = if phase > 2.5 && phase < 3.5 { 1.0 } else { 0.0 };
    // The fight: the harder the blank is loaded, the deeper and wider it goes.
    let load = if phase > 3.5 { stress / 100.0 } else { 0.0 };

    position = [
        -7.60 + swing * (0.25 + load * 2.20),
        // **-0.08, not 0, and the difference is a third of the ball.** The
        // node's origin is the sphere's centre and its radius is 0.26, so a
        // centre on the waterline floats the whole ball on top of the sea like
        // a beach toy. Rendered both: at -0.08 about 0.18 m shows, which is a
        // float, and the take then takes it completely under rather than
        // merely lower.
        -0.08 + bob * 0.055 - taken * 0.34 - load * 0.30,
        -8.60 + bob * 0.10 + load * 2.10,
    ];
    scale = [0.26, 0.26, 0.26];
}
