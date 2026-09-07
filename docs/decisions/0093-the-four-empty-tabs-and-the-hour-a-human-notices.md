# ADR 0093 — The four empty tabs, and the hour a human notices

- **Date:** 2026-09-07 (overnight)
- **Status:** **accepted** and built.
- **Scope:** the first pass at "everything a human needs to actually work in
  this editor", scoped in `docs/design/EDITOR-SCOPE.md`.
- **Adds:** no crate, no dependency, no new architecture. Six features, one
  bug fix, and two panels' worth of things that were already computed and
  never shown.

## 1. What was actually missing

The editor was further along than "start working on the editor" implies:
docking, hierarchy with multi-select and drag-reparent, a typed inspector with
prefab overrides and revert, gizmos, play/pause/step, undo/redo, save, disk
conflict resolution, a console, and an asset list. 3270 lines.

**Four of its eleven tabs rendered the string `"not built yet"`** — Problems,
History, Prefabs, Agent — and four interactions a human reaches for in the
first hour did not exist at all.

The tabs were deliberate holes: the eleven variants were fixed once because
adding one invalidates every saved layout, so each tab existed before its panel.
This ADR fills them.

## 2. What was built

**Grid snapping.** Move, rotate and scale were free-dragging. `gizmo::Snap`
holds three increments and a toggle; Ctrl **inverts** it rather than setting it,
so somebody working on the grid can step off for one drag.

*The result is snapped, not the movement.* Snapping the drag delta gives
increments from wherever the node happened to be — a node at x = 0.37 steps to
0.87 and never reaches a round number. Snapping the result is what makes a grid
a grid. Applied to the **local** transform, the number the file stores and the
inspector shows, because snapping in world space puts a node under a turned
parent on values that look arbitrary everywhere they are displayed.

**Hierarchy search.** `deeper_demo` has 260 nodes and there was no filter box.
Matching is case-insensitive over the whole path, not the leaf: a scene with
eleven nodes called `Body` is exactly when somebody reaches for a filter.

**A node clipboard.** Copy and paste, carrying whole subtrees, prefab instances
and per-instance overrides. The clipboard holds **the nodes, not their paths** —
one holding paths would paste nothing after the original was deleted, which is
when somebody reaches for cut-and-paste.

**The Problems panel**, reading the same three sources `loom validate` does — an
asset alias resolving to nothing, a voxel op list that will not parse, a prefab
override pointing at a child that no longer exists. All three were already
computed and reachable only from a terminal. Rows select their node.

**The History panel.** `Transactions` is a log; this is a *position*. It shows
what is behind the cursor and what is waiting in front of it, and clicking a row
walks there — undo and redo without counting steps.

**The Prefabs panel.** What the scene declares, who instances each, and a button
to add one. Read from the *unresolved* file, because `prefab_load::for_reading`
replaces an instance with the subtree it stood for and the resolved scene has no
prefabs left in it.

**The Agent panel** — the one worth most, and §3.

**A Create menu**, in the toolbar and on every hierarchy row. Making a cube —
the most common operation in a blockout editor — used to be three steps: add an
empty child, add a `MeshRenderer`, point it at an alias. It is now one, and one
transaction, so it is one Ctrl+Z rather than three.

**Frame cost in the status bar.** `cpu` and `draw` milliseconds beside the fps,
smoothed the same way. These are the two numbers `--frames` prints on the way
out, and until now the only way to see which half of a frame to fix was to close
the editor and read a terminal.

**Opening another scene**, from a list of the scenes beside the open one in the
Project panel. Until now the scene came from the command line and switching
meant restarting, which makes an editor a viewer. **Refused while there are
unsaved edits** — the alternative is a confirmation dialog, and the alternative
to that is losing somebody's work to a misclick in a file list.

One directory, not a recursive walk: a project's scenes live together, and a
walk under `assets/` would list 137 files of which 130 are test fixtures. Cached
on scene change rather than listed per frame — a `read_dir` at 144 Hz to draw a
row of buttons is a filesystem call per frame for a list that changes when
somebody adds a file.

**Prefab edit mode, which turned out to be free.** A prefab *is* a scene —
`assets/prefabs/deckhand.loom` validates as one — so editing a prefab is opening
it, and the Prefabs panel grew an Open button. Its declared `path` is a hint for
finding the file (§3), resolved against the current scene's directory, which is
what makes the button work regardless of where the editor was started.

**Select all, and deselect.** Escape empties the selection rather than resetting
it to the first node: "nothing is selected" is a state a human asks for, because
it is how you stop the gizmo drawing over the thing you are looking at.

## 3. The agent panel is the human's half

Unity, Unreal and Godot have bolted assistants on: a chat box that emits code
you paste. This project is arranged the other way round. An agent already drives
**the same transactions the UI does**, through `loom_agent`'s MCP tools over CLI
commands that already work and are already tested, and every panel here is a
pure function returning `UiAction`s that the caller turns into the same
`Transaction`s. There is one edit path and the agent is on it.

What was missing was nowhere for a human to *see* it. The viewport marks fade
after six seconds — right for an overlay, useless as a record: an agent that
edited eleven nodes while the human read the inspector left nothing behind.

So the panel keeps the log the marks throw away: what changed, how long ago, one
click to the node. That is the smallest thing that makes an agent a collaborator
rather than a process that moves your scene when you are not looking.

## 4. The bug this turned up

`Session::undo` popped a label from `history`; `Session::redo` never pushed one
back. **A scene that had been undone and redone described itself with one row
missing from the log, permanently, and more with every round trip.** Nothing
noticed because the only consumer was an append-only log panel where a missing
row looks like a row that was never added.

Building a panel that shows the *cursor* is what made it visible. Undone labels
now live on their own stack, which the History panel reads, and a fresh edit
discards that branch along with its labels — a log that kept them would offer to
redo something that no longer exists.

## 5. How it is checked

`every_tab_draws_with_content` draws all eleven tabs, one at a time so a failure
names the tab, over egui's headless test context with populated state. Four of
them had never been exercised by anything. Fault-checked: a panic injected into
the Problems panel fails it.

Plus unit tests for snapping (including that a zero or NaN step is not
snapping), the filter predicate, and the undo/redo label round trip.

## 6. What is still missing

`docs/design/EDITOR-SCOPE.md` §2 has the full ranked list. The notable ones:

- **A stats readout.** `run.rs` already prints `cpu N ms/frame`; nothing
  surfaces it in the UI.
- **Viewport view modes** — wireframe, unlit, normals, overdraw. The renderer's
  `LOOM_ABLATE_*` switches are close to this already.
- **Material editing** beyond inspector fields, terrain sculpting, particle
  authoring, multi-scene editing.
