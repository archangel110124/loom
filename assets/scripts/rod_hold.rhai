// **THE ROD IS IN HIS HANDS, AND IT WAS IN A HOLDER ON THE BULWARK.**
//
// `deeper_demo.loom` authored the rod at boat local (-7.60, 1.62, -2.90) —
// eight and a half metres forward of the cockpit, standing in a rack. The whole
// fishing loop was therefore played at a thing across the deck: rendered from
// `Rig/Player/Eye` at the tick the HUD reads `LINE OUT, BAITED`, the blank is a
// dark line among three identical rods and the two beats that matter, the take
// and the run, are text. The demo already let you cast from anywhere aboard
// (`deeper_player.rhai` gates the cast on `aboard`, not on a station) and
// nothing in the frame said so.
//
// So the rod rides the player. Same seam as `float_bob.rhai`, `rod_bend.rhai`
// and `fish_hang.rhai`: a node script writing its own transform off the rules
// script's `state`, one tick stale, with no memory and no state of its own.
//
// **It lives in the prefab, not on the instance, and that is forced.**
// `loom_scene::prefab::expand` keeps an instance node's `transform` and its
// `[node.overrides]` and **discards its `components` entirely** — so
// `[node.components.Script]` on `Rig/Boat/Rod` is silently dropped, and an
// override cannot add a component the prefab lacks (`orphaned_override`). The
// only place a script can be attached to a prefab's root is inside the prefab,
// which is where `rod_bend.rhai` already lives for the same reason.
//
// **A scene with no hub is untouched.** `state.plx` is written only inside
// `deeper_rules.rhai`'s `if hub`, so `fishing.loom` and `assets/test/
// fishing_rod.loom` read nothing here, this file assigns nothing, and the host
// keeps the transform the file authored. That is the same bargain
// `rod_bend.rhai` makes with `stress`, and it is what lets one prefab serve a
// rack and a hand.
//
// What it reads:  tick, state.plx/ply/plz, state.bfx/bfz
// What it writes: position, rotation
// What it may keep: nothing.

if "plx" in state {
    let plx = state.plx;
    let ply = state.ply;
    let plz = state.plz;
    // His flat forward **in her frame**, which is the number this round added
    // to `deeper_rules.rhai`. `fish_hang.rhai`'s header states why it did not
    // have it: a rules script sees world positions and no rotations, and the
    // 2-D rotation that takes a world heading into hers is solved there. The
    // fish took the deal and hangs off his port beam; a rod cannot, because a
    // rod pointing ninety degrees away from where you are looking reads as a
    // bug rather than as a hand.
    let bfx = if "bfx" in state { state.bfx } else { 0.0 };
    let bfz = if "bfz" in state { state.bfz } else { -1.0 };
    // Starboard of him is `cross(forward, up)` with up = +Y — the same
    // convention `toward` uses in the rules script.
    let rx = -bfz;
    let rz = bfx;

    // **The butt, and it is a view-model offset rather than an anatomical one.**
    //
    // Anatomy was written first and rendered: `ply` is the capsule's *centre*,
    // 0.90 m above his feet, the eye is +0.75 on top of that, so a grip held at
    // chest height is 0.73 m below the eye and about 0.2 m in front of it —
    // which is 76 degrees below the view axis against a half-FOV of 37.5. The
    // grip was **entirely off the bottom of the frame** and what the player saw
    // was a bare blank rising out of the sea with no hand on it. Every
    // first-person weapon ever drawn cheats the same way and for the same
    // reason.
    //
    // 0.55 m ahead and 0.45 m below the eye is 39 degrees down — the reel just
    // inside the bottom edge at this FOV — and 0.30 m to starboard puts it in
    // the right-hand third rather than across the middle. He is 0.35 m in
    // radius, so the blank never passes through him, and it rides his own y, so
    // it goes up the boarding treads with him instead of through them.
    // **0.50 / 0.40 / 0.18, and it was 0.55 / 0.30 / 0.30.** The old numbers
    // were a first-person composition and nothing else, from before there was
    // a body: measured against the mittens they held the butt 36 cm off his
    // hands, 30 cm of it sideways, so the mirror — the one feature that exists
    // to look at him — showed a man miming. The grip pose moved to meet this
    // (`deckhand_shoulder.rhai` carries the sweep) and this moved to meet the
    // grip; they are 88 mm apart now, which is one mitten.
    //
    // The frame it was tuned for survives. The eye is +0.82, so the butt was
    // 0.52 below it and 0.55 ahead — 43.4 degrees down against a half-FOV of
    // 37.5 — and is now 0.42 below and 0.50 ahead, 40.0 degrees down. Higher
    // in the frame, not lower. What genuinely moves is the lateral third:
    // 0.30 m of starboard at 0.55 m is 28.6 degrees right of centre and
    // 0.18 m at 0.50 m is 19.8, so the reel sits nearer the middle than it did.
    // That is the price of the rod being in his hands and it is worth it.
    position = [
        plx + bfx * 0.50 + rx * 0.18,
        ply + 0.40,
        plz + bfz * 0.50 + rz * 0.18,
    ];

    // **Sixty degrees of pitch, and the yaw is one `atan`.**
    //
    // `loom_ecs::compose` is intrinsic Y-X-Z, so the blank's own axis — local
    // +Y, which every one of the twenty-four chain nodes runs up — comes out at
    // `(sin(ry)*sin(rx), cos(rx), cos(ry)*sin(rx))`. Setting `ry` so that
    // `sin(ry) = -bfx` and `cos(ry) = -bfz` makes the horizontal half of that
    // point exactly where he is looking, and `ry = atan2(-bfx, -bfz)` is that
    // in one call. (rhai's `atan(x, y)` is `x.atan2(y)`.)
    //
    // **-60, not the -45 the rack was authored at.** At -45 the blank is half
    // forward and half up and the tip finishes 1.7 m above his feet and 1.4 m
    // ahead — a rod carried like a pike. -60 is 0.87 forward to 0.50 up, so
    // roughly 2.0 m of blank puts the tip 1.7 m ahead and 1.0 m up, which is a
    // metre above his hands and a quarter above the eye. Both were rendered;
    // the renders are named in the round's report.
    //
    // **The bend is not here.** `rod_bend.rhai` writes the twenty-four joints
    // off `state.stress` and this writes only the root, so a rod being loaded
    // by a fish still curves out of the hand that is holding it.
    rotation = [-60.0, atan(-bfx, -bfz).to_degrees(), 0.0];
}
