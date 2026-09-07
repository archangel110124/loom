# ADR 0087 — A menu nobody can reach with a controller

- **Date:** 2026-09-06
- **Status:** **accepted** — fifth of the nine subsystems scoped as "what is
  missing for a full-blown game engine", at the general-engine bar.
- **Decision touched:** none. No new crate, dependency, pass or component. A
  struct, an enum of three booleans, and one new binding context.
- **Human decision this records:** a focus model driven by actions, rather than
  a settings screen or an in-game rebinding UI, which were the wider options.

## 1. The gap was not the one that was reported

The survey said "**no game UI layer**". That was wrong, and the correction is
worth recording because it is the third time in this run that a grep found an
absence that was really a naming mismatch.

`hud.rs` is 1882 lines. It has a title screen, a title menu, a pause menu, a
scrim, `{name}` interpolation against game state, anchors, and its own tests —
including one whose comment reads *"an untested menu is one nobody knows is
drawn"*.

**What it did not have was a way to reach any of it without a mouse.** Every
item was `ui.add_sized(..).clicked()`. No focus, no selection, no keyboard. The
gamepad added in ADR 0086 could walk a character around a scene and then not
press Start.

For a co-op game meant to be played on a couch, that is the hole.

## 2. Decision

`MenuFocus` holds a selected index and whether anything is selected. `MenuNav`
is three booleans — up, down, confirm — already resolved from the action map.

**`MenuNav` is a plain struct so `hud` never learns what a gamepad is.** The
binding layer has already turned a D-pad, the arrow keys and W/S into the same
three facts; resolving that in `run.rs`, where the bindings live, is what lets
these functions be tested without an input map at all. Five of the six new tests
never construct one.

**Nothing is selected until the player moves.** A menu that highlights its first
item on open fights the mouse: the pointer is elsewhere, and the highlight
claims to say what Enter does while the eye is on what the pointer is over. So
`engaged` starts false, the first press engages it, and hovering hands control
back — the two can never disagree about what confirm would do.

**The first press engages without moving.** Pressing down on a menu you just
opened lights the *first* item, not the second. Getting this wrong is the most
common way a controller menu feels broken, and it is one line and one test.

**It wraps.** A menu of two items with a stick in your hand is exactly where an
unwrapped list feels wrong.

**A `menu` context of its own**, because the same stick that walks a character
has to move a highlight when a menu is up, and one action name cannot mean two
things at one moment. Contexts are the tool `loom_input` already had for that.

**The stick is deliberately not bound to menu movement.** An analog source has
no press transition — it is a level — so a stick held past its dead zone would
step the highlight every frame it stayed there. The D-pad is buttons and
behaves. Every menu binding is `pressed` rather than `held` for the same reason:
one press is one step.

## 3. What is tested

Six, and the ones that matter are about *feel* rather than mechanism: that a
fresh menu selects nothing, that the first press does not skip, that up from
nothing lands on the last item, that the selection wraps both ways, that
hovering disengages, and that an empty menu is inert rather than a division by
zero — a menu built from a filtered list can be empty, and that is cheaper here
than in every caller.

## 4. Not built

- **No settings screen.** The wider option, not taken. A pause menu with only
  Resume and Quit is not what a shipped game has, and the human chose the
  navigation first — correctly, since a settings screen nobody can reach with a
  controller would have the same defect this fixes.
- **No in-game rebinding.** Rebinding is editing a text file, which is this
  engine's answer for everything authored; a shipped game usually wants a menu,
  and capturing "any input" needs a mode where the bindings are bypassed.
- **No mouse-wheel or page navigation**, and no held-to-repeat. Both are
  wanted the first time a menu is longer than a screen, and neither is wanted
  by a menu of two.
- **The editor's own panels are untouched.** egui already gives them keyboard
  focus; this is for the game's menus, which are hand-laid `Area`s.
