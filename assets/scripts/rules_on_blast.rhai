// Rules that end the game on the first explosion.
//
// A test fixture, and a sharp one: `detonations` holds what went off on *this*
// tick. Rules run last, after the characters have acted, so a shot fired this
// tick is visible this tick. Run them earlier in the tick and this list is
// always empty — the game would never end, and nothing else in the suite
// would notice.

if detonations.len() > 0 {
    status = "won";
    message = "first blast at " + detonations[0].at[0];
}
