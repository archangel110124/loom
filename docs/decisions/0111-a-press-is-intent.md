# 0111 — A press is intent

Status: accepted · 2026-09-09

## Context

ADR 0110 gave the HUD bars and panels and stopped deliberately at the edge of
input: *"a button is not another kind — it needs a press to reach the simulation
on the fixed tick, the way key input already does, or a run stops reproducing."*

This is that decision.

Without it the title menu, the pause menu and the inventory stay in Rust, and a
game built on this engine gets the three `hud.rs` happens to draw.

## The decision

**A press is intent, and it travels the way every other intent does.**

`loom_net::Intent` is what one player asked for on one tick — *"everything here
is a thing a human did with their hands"* — and it is a **fixed-width 45-byte
frame**, because a variable one is a length prefix, an allocation and a parser on
the hot path of a lockstep tick. A button press is a thing a human did with their
hands. So it goes in the frame, and the frame stays fixed width: one more byte,
`ui`, and 46.

**The byte is an index, and the script sees a path.** A name cannot ride a fixed
frame. So the wire carries the **1-based position of the button among the
scene's `Hud` button elements, in world order**, with zero for none — and the
runner turns that back into the button's node path before any script sees it.
That is well defined precisely because lockstep peers run the same scene: both
resolve the same index to the same path.

**A press arrives as an ordinary event.** `Event { kind: "ui", node: "Menu/Start" }`,
in the `events` array a rules script already reads. No new scope variable, no new
vocabulary, and no id to author — **the node's own path is the button's
identity**, so there is nothing to keep in step between the scene and the script.

**The edge is taken in the runner, not by callers.** `Intent.ui` is a *level*:
which button is held. Left as a level, a finger resting on START pressed it sixty
times a second — measured, a fifteen-tick hold raised sixteen events. Taking the
edge in `Runner::tick` means the window, `loom sim --hold ui=1` and a remote peer
all get one press from one press, rather than three callers each remembering to.

**A button claims its own rectangle and nothing else.** `hud.rs` paints rather
than lays out for exactly this reason: a transparent `CentralPanel` once fixed
the HUD's anchoring and broke the trigger, because a panel is an interactive
region and egui reported every click in the viewport as consumed — the crosshair
was visible and the gun did nothing. `Ui::interact` over one rect claims that
rect. Tested at three points: on the button, below it, beside it.

**`--hold ui=<n>`** presses the nth button headlessly, so a menu is not the one
thing in this engine no scripted run can exercise.

## What this cost, and what it did not

No new component: `HudKind::Button` joins `text`, `bar` and `panel`, and a button
is `text` in a box you can click. No new script API. One byte on the wire. The
determinism hash is untouched — a press changes the simulation only by being an
input, which is what an input is.

## The bug the tests did not catch

Four unit tests built `Element` directly, so none of them touched the
string-to-kind mapping in `elements` — and `"button"` was missing from it.
Buttons drew as plain coloured text with no box; `--hold ui=1` still raised the
event, because `button_path` reads the component itself; every test passed.

A screenshot found it. There is now a test that walks each authored kind from a
scene through `elements`, fault-injected by removing the arm again.

The lesson is the one this project keeps relearning: a test that builds the
intermediate value skips the step that produces it. The screenshot is not a
nicety here — it is the only instrument that sees the whole path.

## Still open

The title and pause menus are still Rust. They can now be scenes, and turning
them into scenes is content work rather than engine work — but `title_menu`
returns a `TitleChoice` the engine acts on, and porting it means deciding how a
scene says "quit". That is a smaller decision than this one and it is not this
one.
