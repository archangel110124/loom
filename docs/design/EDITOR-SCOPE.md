# The editor, scoped against Unity, Unreal and Godot

> ## Superseded in part — read this first
>
> **§1 and §2 describe the editor as it stood on 2026-09-07 and are now wrong in
> specifics.** They were a worklist, the worklist got done, and nothing said so —
> which by this project's own first rule ("never state a checkable fact you
> haven't checked") makes them the exact failure they were written to avoid. A
> critic reading them on 2026-09-09 came away with four false beliefs.
>
> Corrected, with where the work landed:
>
> | This doc says | Actually |
> |---|---|
> | Problems, History, Prefabs and Agent render `"not built yet"` | All four are built — ADRs 0093, 0100, 0101 |
> | No `snap` anywhere in `loom_editor` | `Snap` is in the toolbar and in `gizmo.rs` — ADR 0099 |
> | The Hierarchy has no filter box | It has one — ADR 0093 |
> | Nothing surfaces the profiler numbers in the UI | A Profiler tab, with a frame graph — ADR 0106 |
>
> Since then the editor has also gained: a project-wide scene browser and a
> no-scene launch (0104), asset import by drag-and-drop (0105), component
> copy/paste (0107), agent proposals with Apply/Discard (0103), nested-object
> editing and the Add Component null fix (0108).
>
> **§3 onward — the comparison against Unity, Unreal and Godot, and what is still
> genuinely absent — stands.** The gaps it names (no animation tooling, no
> in-editor scripting, `.obj`/`.png` import only, no game-UI designer, no terrain
> brush, no build UI) are all still real.
>
> The ADRs in `docs/decisions/0088`–`0108` are the current record. This file is
> kept for its comparison and its reasoning, not for its status table.

**Written 2026-09-07, overnight.** Every "has" below was verified by reading the
code, not by remembering it. Twice in this project a survey has claimed
something was missing when it existed under a different name — "no rebinding"
when `ActionMap` was already wired, "no game UI layer" when `hud.rs` was 1882
lines — so the method here is: name the feature, name the file, or say it is
absent.

## 1. Where the editor actually is

`crates/loom_editor` is 3270 lines over five modules, plus its driver in
`loom_cli/src/run.rs`. It is architecturally unusual in one respect that matters
for everything below:

> **Every panel is a pure function of borrowed state.** A panel reads what it is
> given and returns `UiAction`s; the caller turns those into
> `loom_scene::ops::Transaction`s and applies them through *the same code path
> the agent uses*. The editor has no undo stack of its own — never-do #16 — and
> lives in a crate that structurally cannot reach the apply path.

That is worth more than it sounds. It means every feature added below is
automatically undoable, automatically scriptable, automatically agent-drivable,
and automatically identical whether a human or an agent did it. Most editors
grow a second, weaker path for the UI and spend years reconciling it.

### Built and working

| Area | Where |
|---|---|
| Docking, 11 tabs, saved layouts | `dock.rs` |
| Hierarchy: select, multi-select, reparent, add, delete, duplicate, rename | `panels.rs`, `UiAction` |
| Inspector: typed fields, arrays via `SpliceArray`, add/remove component | `panels.rs` |
| Prefab overrides, and reverting one field or a whole instance | `UiAction::RevertOverride` |
| Gizmos: move, rotate, scale, with screen projection and picking | `gizmo.rs` |
| Asset list, assign a mesh to a `MeshRenderer` | `panels::assets` |
| Play / Pause / Step-one-tick / Stop, in editor | `UiAction` |
| Undo / redo / save, disk-conflict resolution | `UiAction`, transactions |
| Console with levels | `console.rs` |
| Transactions view | `panels::transactions` |
| Theme tokens | `theme.rs` |
| Agent marks drawn over the viewport | `panels::AgentMark` |
| MCP server: `initialize`, `tools/list`, `tools/call`, `ping`; 8 catalog tools | `loom_agent` |

### Declared but empty

Four tabs render the string `"not built yet"`:

- **Problems** — validation and errors as a live list.
- **History** — the undo stack, visible and navigable.
- **Prefabs** — the prefab library.
- **Agent** — where a human works *with* the agent.

The tabs exist deliberately: the eleven variants were fixed once because adding
one invalidates every saved layout. So the holes are known and reserved, not
oversights.

## 2. What the three big editors have that this does not

Ranked by *whether a human can do a day's work without it*, not by how
impressive it is.

### Tier 1 — a human notices within an hour

1. **Grid and snapping.** No `snap` anywhere in `loom_editor`. Move, rotate and
   scale are all free-dragging. Every DCC has increment snap, and without it
   laying out a deck or a dock by hand is guesswork.
2. **Hierarchy search.** `deeper_demo` has **260 nodes**. There is no filter
   box. Finding `Rig/Boat/Helm` means scrolling.
3. **A node clipboard.** `Duplicate` exists; copy and paste do not, so a node
   cannot be moved between parents or scenes by hand.
4. **The Problems panel.** `loom validate` already produces structured errors
   with `field`, `constraint` and `hint`. None of that reaches the editor, so a
   scene is validated in a terminal.

### Tier 2 — a human notices within a week

5. **The History panel.** Undo works; seeing *what* it will undo does not.
6. **The Prefabs panel**, and prefab edit mode — opening a prefab as its own
   document rather than editing one instance's overrides.
7. **A stats/profiler readout.** The numbers exist — `run.rs` prints
   `cpu N ms/frame` — but nothing surfaces them in the UI.
8. **Viewport view modes.** Wireframe, unlit, normals, overdraw. The renderer
   has ablation switches already (`LOOM_ABLATE_*`) that are close to this.

### Tier 3 — real, but not what stops work today

9. Material editing beyond inspector fields; a graph is a separate product.
10. Animation timeline. Deliberately thin: ADR 0072 has no skeletal animation,
    and characters are node scripts, so the "timeline" here is a script.
11. Terrain/voxel sculpting tools. `loom_voxel` bakes from recipes; there is no
    brush.
12. Particle authoring beyond component fields.
13. Multi-scene editing, and a level-streaming concept.

### Deliberately absent, and staying that way

- **A second undo stack for the UI.** Never-do #16.
- **A UI-only edit path.** The reason every feature above is cheap.
- **Skinning tools.** ADR 0072, upheld on measurement in ADR 0092.

## 3. Agent tools, which the other three do not really have

Unity, Unreal and Godot have bolted assistants on: a chat box that emits code
you paste. This project is arranged the other way round — the agent drives the
*same transactions the UI does*, and `loom_agent` is a thin MCP adapter over
CLI commands that already work and are already tested.

What is missing is not agent capability. It is the **human's half of the
conversation**: a panel where a human can see what the agent proposed, what it
changed, and accept or reject it before it lands. That is what the Agent tab is
reserved for, and it is the single highest-value thing in this document,
because it is the one thing no other engine's editor is arranged to do well.

## 4. Order of work

Tonight, in this order, because each is independently useful:

1. Gizmo snapping (Tier 1.1)
2. Hierarchy search (Tier 1.2)
3. Node clipboard (Tier 1.3)
4. Problems panel (Tier 1.4)
5. History panel (Tier 2.5)
6. Agent panel (§3)

Everything else is listed above so the next person picking this up does not have
to re-derive the map.
