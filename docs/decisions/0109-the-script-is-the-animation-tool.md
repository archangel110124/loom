# 0109 — The script is the animation tool

Status: accepted · 2026-09-09

## Context

Five review passes agreed on what was left: **no in-editor scripting, no
animation tooling, no game-UI designer**. Each was called structurally large.

Two of the three turned out to be much less absent than the survey said — which
is the third time this session a "missing feature" has been a capability the
engine already had. So this ADR is as much about what was *not* built.

## What was actually missing: editing a script

`GameRules.path` and `Script.path` name `.rhai` files. Nothing in the editor
opened one. To change how a game plays — or how a character walks — you left the
window.

**A `Script` tab.** It shows the script the selection names, with a filterable
picker over every `.rhai` in the project for when nothing is selected. Selecting
a node opens its script: click the deckhand's left shoulder and
`deckhand_shoulder.rhai` is on screen, which is the file that bends it.

**A save compiles before it writes.** `ScriptHost::compile` is the parser the
runner uses, so a syntax error is caught with its line while the human is
looking at the code, rather than at the next Play as a console warning and a
paused simulation — by which time the broken file is on disk and the *scene*
looks like the thing at fault. Nothing reaches disk that will not parse.

**A running game keeps the script it started with.** Reloading mid-run would
change the rules under a simulation whose whole value is that its history
reproduces (ADR 0045). Saving during Play says so and leaves the run alone.

The selection only pulls a new script in while the buffer matches the file. The
Hierarchy is one stray click from anywhere, and taking an unsaved edit away
because the selection moved would lose work.

## What was not missing: an animation editor

ADR 0072 made a character **separate rigid parts animated by `rhai` writing
absolute rotations on the fixed step** — not because skinning was hard, but
because every secondary ray in this engine intersects the acceleration structure
at authored transforms, so a skinned character in a mirror stands still while
his body walks.

There is therefore no clip, no curve and no track to edit. The walk cycle *is*
`deckhand_gait.rhai`, and its thirteen joints are thirteen scripts. Building a
timeline over that would have been building a UI for a data model this engine
does not have.

What an animator here actually needs was two things, and one already existed:

- **Step the simulation a tick at a time and watch the pose.** `⏸ Pause` and
  `⏭ Step` have been in the transport since the editor had one.
- **See the code that moves the joint you clicked.** That is the Script tab, and
  it is why scripting and animation are one feature rather than two.

## What was not missing: authoring the HUD

`Hud` is a line of text in screen space with an anchor, an offset, a size, a
colour and two visibility flags. Add Component offers it, the Inspector edits
every field — the anchor as a dropdown, the colour through the sRGB-correct
swatch — and `run.rs` composes HUD elements **in edit mode as well as in Play**,
so a line appears in the viewport as it is typed. Photographed: `OXYGEN 84%` in
amber, in the stopped editor, seconds after the component was added.

That is the authoring loop a HUD designer exists to provide, and it was already
there.

**What is genuinely absent is rich UI**: buttons, panels, layout, anything that
takes input. `hud.rs` draws the title screen, the pause menu and the inventory
grid from Rust, and none of that is authorable from a scene file. That is a real
gap and it is a large one — it needs a widget model in the scene format before
it needs an editor — so it stays on the list rather than being quietly counted
as done.

## Consequences

The tab list was "fixed at eleven, once"; this is the thirteenth, and it is
safe for the reason ADR 0106 established: layouts persist as titles and
`from_json` backfills any tab the file lacks. The rule that survives is that a
tab needs a body, and this one has one.

`walk_for_scenes` became `walk_for(dir, extension, ..)` so the picker can find
`.rhai` the same way the Project panel finds `.loom`.
