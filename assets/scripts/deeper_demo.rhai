// **Moved. The demo's rules are `deeper_rules.rhai`.**
//
// A scene has one `GameRules` and `play.rs` refuses a second by name
// (`duplicate_game_rules`). The demo needed the hold, the helm caption *and*
// the fight, and the fight is two hundred lines of tuned constants that the
// five `fight_*.loom` benches pin by number. Copying them into a second rules
// script is green on both sides right up until the two drift apart, so the
// fight's own file grew a hub block instead — entirely behind
// `"Rig/Player" in positions`, so `fishing.loom` and the benches are unchanged.
//
// Everything that was here is in `deeper_rules.rhai` below the line marked
// `THE HUB`, including the reasoning about where an inventory lives.
//
// **This file is dead and should be deleted.** It is left as a stub rather
// than removed because deleting files is not something this round was
// authorised to do on its own. Nothing references it: `grep -rn deeper_demo.rhai`
// returns this file and nothing else.
