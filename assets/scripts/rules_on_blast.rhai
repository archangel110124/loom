// Rules that end the game on the first explosion.
//
// A test fixture, and a sharp one: `events` holds what happened on *this*
// tick. Rules run last, after the characters have acted, so a shot fired this
// tick is visible this tick. Run them earlier in the tick and the list is
// always empty — the game would never end, and nothing else in the suite
// would notice.

for e in events {
    if e.kind == "blast" {
        status = "won";
        message = "first blast at " + e.at[0];
    }
}
