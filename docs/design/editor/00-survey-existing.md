# Survey — the editor as it exists today

Written before any rework decision, so that the rework can be judged against what is
actually there rather than against a memory of it. Every claim below is a line in the
tree at `62f9ebe`; nothing here is inferred from the milestone notes.

**The headline: there is no "editor" in this codebase.** There is a `loom run` window
(`crates/loom_cli/src/run.rs`) that draws a scene and, when `--edit` was passed, also
draws six egui panels over it and opens a `loom_scene::Session`. The distinction is not
pedantic — it is the single fact that shapes the rework. `--edit` is a boolean on one
`App` struct (`run.rs:159-160`), the panels are functions taking `&PanelState` and
returning `Vec<UiAction>` (`panels.rs:128-143`), and the 3D image underneath is the whole
window rather than a rect the layout assigns. Everything that has to change, changes
because of that last sentence.

**What is worth protecting is not in the UI layer at all.** The transaction path, the
gesture coalescing, the version-token conflict handling and the schema-driven inspector
are the load-bearing parts, and three of the four live below `panels.rs` — in
`loom_scene::Session` and `loom_reflect::TypeRegistry`. A ground-up UI rewrite can throw
away every line of `panels.rs` and lose none of it, provided it re-enters through the
same four call sites named in §3.

---

## 1. Where the editor physically lives

| Concern | File | Lines | Verdict |
| --- | --- | --- | --- |
| Window, event loop, camera, play, watch, all editor actions | `crates/loom_cli/src/run.rs` | 2313 | **REWRITE** (split) |
| Panels, inspector generation, gizmo painting, agent overlay | `crates/loom_cli/src/panels.rs` | 898 | **REWRITE** |
| Gizmo maths: projection, handles, hit-test, drag | `crates/loom_cli/src/gizmo.rs` | 280 | **KEEP AS-IS** (extend) |
| Scene → renderable derivation, diffing | `crates/loom_cli/src/scene_view.rs` | 390 | **KEEP AS-IS** |
| Game HUD overlay | `crates/loom_cli/src/hud.rs` | 496 | **KEEP AS-IS** |
| Material/texture resolution | `crates/loom_cli/src/materials.rs` | 429 | **KEEP AS-IS** |
| Console log store | `crates/loom_cli/src/log.rs` | 112 | **KEEP AS-IS** |
| egui ↔ Vulkan wiring | `crates/loom_render/src/ui.rs` | 174 | **KEEP AS-IS** (one change, §7) |
| UI pass placement in the frame | `crates/loom_render/src/viewer.rs` | 1590-1619 | **KEEP AS-IS** (one caveat, §7) |
| Transactions, undo, coalescing, save/lock | `crates/loom_scene/src/edit.rs` | 817 | **KEEP AS-IS — do not touch** |
| Schema registry the inspector reads | `crates/loom_reflect/src/lib.rs` | 22-76 | **KEEP AS-IS** |

`run.rs` is the problem file. It is the winit `ApplicationHandler`, the fly camera, the
file watcher, the play-mode driver, the gizmo drag state machine, the GPU upload
scheduler for grass/terrain/rain, the weather clock, the CPU profiler, and the
implementation of all twenty `UiAction` variants. Nothing in it is *wrong*; it is simply
six responsibilities in one 2313-line `impl`, and a Unity-like layout with painting modes
and a project hub cannot be threaded through it without it becoming four thousand lines.

**There is one binary and no feature flags** (`crates/loom_cli/Cargo.toml:8-10`; no
`[features]` section exists in any crate manifest). `loom run --edit` and `loom render`
are the same executable, egui and winit are unconditional dependencies of `loom_render`,
and the ship target of "executable + assets folder, editor stripped from the runtime
build" has **no existing mechanism whatsoever**. That is new work, not a refactor.

---

## 2. Entry point and process shape

`loom run <scene.loom> [--edit] [--frames n] [--play]` — declared in `main.rs:176`
(the flag table that makes an unknown flag a failure rather than a no-op), documented at
`main.rs:131-135`, dispatched at `main.rs:294-306`, and implemented by
`run::open_scene` (`run.rs:2291-2312`).

`open_scene` reads the file, builds a `SceneView` from the text, takes a `VersionToken`
of what it read, and opens a `loom_scene::Session` **only if `editable`**
(`run.rs:2306-2309`). Read-only mode therefore has no undo stack, no save, and no write
path at all — `transact_as` returns early when `self.session` is `None`
(`run.rs:1720-1722`). That is a good property and the rework should keep it: a viewer
that cannot write cannot race the agent.

`--frames n` closes after n frames and prints the mean/worst CPU cost (`run.rs:1201-1210`,
`report_cpu` at `run.rs:1536-1545`). `--play` starts the simulation on the first frame
that has a viewer (`run.rs:935-938`). Both exist so `cargo xtask validate` can drive the
*whole* window lifecycle — create, draw, tear down — under the validation layers
(`xtask/src/main.rs:1024` and `:1077`). **Any new editor must keep both flags working, or
the windowed half of green check 2 goes dark.**

`LOOM_WINDOW_AT=x,y` positions the window (`run.rs:776-781`) for the same reason: the
gate opens five windows and a multi-head desk scatters them.

Verdict: **KEEP the flags and the headless-drivability. REWRITE the dispatch** — a hub
that lists projects cannot be a positional scene path.

---

## 3. How an editor action becomes a SceneOp

This is the part the rework must not lose, and it is smaller than it looks.

**Every mutation in the editor funnels through two functions.** `transact` (`run.rs:1707`)
and `transact_as` (`run.rs:1716-1756`) are the only places `loom_scene::Session::apply` is
called from the UI. They build a `loom_scene::Transaction { label, ops, dry_run: false,
expect_version: None }` and hand it to `Session::apply` or `Session::apply_coalescing`.
`Session::commit` (`edit.rs:299-311`) then overwrites `expect_version` with the session's
own token, so **the editor cannot skip the staleness check even by mistake** — the field
it passes is ignored.

The mapping from UI intent to ops is `App::act` (`run.rs:1251-1314`), a match over the
twenty `UiAction` variants (`panels.rs:24-56`). Concretely:

| Action | Ops issued | Where |
| --- | --- | --- |
| Gizmo drag / inspector transform edit | `SetTransform` (via `transform_op`, `run.rs:2141-2153`) | `run.rs:1950-1993`, `run.rs:1762-1772` |
| Inspector field edit | `SetField { node, "Type.field", value }` | `run.rs:1774-1779` |
| Add Component | one `SetField` per schema default | `run.rs:1598-1628` |
| Remove Component | `RemoveComponent` | `run.rs:1268-1274` |
| Add child | `SpawnNode { parent, name, mesh: Some("box") }` | `run.rs:1786-1807` |
| Duplicate | `SpawnNode` + `SetTransform` + one `SetField` per component field | `run.rs:1814-1867` |
| Delete | `RemoveNode` per node, **deepest first** | `run.rs:1874-1896` |
| Rename | `RenameNode`, then `reselect` follows the path | `run.rs:1631-1647` |
| Reparent (hierarchy drag) | `ReparentNode` | `run.rs:1650-1673` |
| Assign mesh (asset click) | `SetField "MeshRenderer.mesh" = {asset}` per selected node | `run.rs:1899-1910` |
| Nudge (IJKLUO) | one `SetTransform` per node, **one transaction** | `run.rs:2113-2137` |

Three details worth carrying forward verbatim:

**Duplicate is built out of existing ops rather than a `DuplicateNode` op**
(`run.rs:1811-1813`). Spawn, then replay the original's transform and every component
field. It is one transaction, so it is one Ctrl+Z, and the op vocabulary stayed small.
The new editor should resist adding ops for its own convenience for the same reason.

**Delete sorts children-first** (`run.rs:1884`) because removing a parent that still has
children is correctly refused by the op layer. The editor does the sort so the human does
not have to — that is the shape of every "editor convenience" this codebase permits: it
orders and labels, it does not add semantics.

**Reparent warns about non-uniform parent scale before issuing the op**
(`run.rs:1655-1662`) rather than preventing the drop. Cycles and name collisions are
refused by the op layer, not duplicated as rules in the UI (`panels.rs:398-401`).

**Two ops exist and the editor never issues them: `RevertOverrides` and `UnpackPrefab`**
(`ops.rs:85-97`). Prefabs are reachable only from `loom prefab` on the command line
(`prefab_cmd.rs`). Given S4 landed prefabs in full and the inspector is where "revert this
field" belongs, this is the largest single gap between the op vocabulary and the UI.

Verdict: **KEEP the funnel exactly.** `transact`/`transact_as` should survive the rewrite
as-is, moved into whatever type replaces `App`. The `UiAction` enum itself is **REWRITE** —
it is a flat list that will not survive brushes, painting modes, prefab operations and a
hub, and it should become per-tool intent types feeding the same two functions.

---

## 4. Gesture coalescing

**A gizmo drag fires a transaction per frame and collapses into one undo step, and the
mechanism is four lines.** `Session::apply_coalescing` (`edit.rs:282-297`) commits the
transaction normally and then, if the gesture key matches the previous call's, pops the
undo snapshot and the history label that this frame just pushed. The snapshot from the
frame that *began* the gesture is what remains, so one Ctrl+Z rewinds the whole movement.
`Session::apply`, `undo` and `redo` all clear `self.gesture` (`edit.rs:263, 315, 326`), so
an agent write landing mid-drag ends the run and cannot be swallowed into the human's undo
entry.

The keys are constructed in the editor:

- gizmo: `format!("gizmo:{node}:{axis}:{gesture_epoch}")` — `run.rs:1992`
- inspector scrub: `format!("field:{node}:{field}:{gesture_epoch}")` — `run.rs:1781`

`gesture_epoch` is bumped on **every mouse-button release** (`run.rs:898`), which is what
makes letting go and re-grabbing the same handle a second undo step rather than a
continuation of the first. That is a small idea doing a lot of work and it is easy to lose.

**Only those two things coalesce.** Nudge, duplicate, delete, rename, reparent, add/remove
component and asset assignment all go through plain `transact`. A rework that adds brush
strokes, splat painting or vertex-colour painting **must add a gesture key per stroke** —
one stroke, one undo — and the natural key is `paint:{node}:{layer}:{epoch}`. There is no
other machinery to build; `apply_coalescing` already does it.

The one thing coalescing cannot express today: a *stroke* is a stream of ops against the
same target, and `apply_coalescing` collapses history but still re-applies and re-writes
the full scene text per frame (`commit` → `apply` → `SceneView::build_cached`,
`run.rs:1736`). At scene scale that is fine — measured behaviour is a gizmo drag at
interactive rates — but a texture-paint stroke writing an op list per frame is a different
volume and deserves a measurement before the design is fixed.

Verdict: **KEEP AS-IS.** Do not build a second coalescing mechanism in the UI.

---

## 5. The inspector, generated from the type registry

`panels.rs:454-539` draws the inspector; `inspect_component` (`panels.rs:777-898`) is the
generation. It takes the component's JSON value and the schema from
`TypeRegistry::describe` (`loom_reflect/src/lib.rs:46`), and picks a widget per field:

- **number with `minimum` and `maximum`** → `egui::Slider` bounded by the schema, so a
  slider cannot be dragged out of what the validator would accept (`panels.rs:822-827`)
- **number otherwise** → `DragValue` at speed 0.1
- **bool** → checkbox
- **array of numbers** → one `DragValue` per element, honouring `items.minimum/maximum`
  (`panels.rs:846-877`)
- **string** → **read-only label** (`panels.rs:878-880`)
- **anything else** (nested objects, `AssetRef`, voxel op lists) → read-only, wrapped,
  summarised past 160 characters (`summarise`, `panels.rs:110-121`)

The field's doc comment became the schema `description` at M1 and becomes the tooltip here
(`panels.rs:798-803`). Transform is drawn separately and first (`inspect_transform`,
`panels.rs:741-773`) because it is node-key sugar rather than a component table; its edits
are re-sugared back into `SetTransform` by `set_field` (`run.rs:1762-1772`).

**Add Component is the same registry, filtered** (`panels.rs:256-299`): every registered
type the node does not already carry, minus `Name` and `Transform`. Defaults come from the
schema, not from a table in the editor (`run.rs:1598-1628`) — a second list of what a
component starts as would be a second answer.

The rename field is worth a line because it was broken once: the text buffer lives in
egui's per-id temp store keyed on the node path (`panels.rs:484-502`), because a buffer
rebuilt from the node's name each frame overwrites every keystroke before the next repaint.
It commits on Enter + `lost_focus`, not per character.

**What the generated inspector cannot do**, all of which the rework has to answer:

1. **Strings are not editable.** `Script.path`, `Hud.text`, material texture aliases and
   every other string field can be read and not typed. This is the single most limiting
   gap in the current editor.
2. **No enum widget** — schema `enum` variants render as an uneditable string.
3. **No asset picker.** Nested objects like `{ asset = "box" }` are read-only; the only way
   to change a mesh is to click a button in the Assets panel, which applies to the whole
   selection (`panels.rs:562-569`).
4. **No colour picker.** Colours are `[f32;3]`, so they are three drag values.
5. **No prefab override affordance** — no "this field is overridden" marker, no revert.
6. **Multi-selection shows a count and nothing else** (`panels.rs:462-466`).
7. **No per-component collapsing, reordering, search, or copy/paste of component values.**

Verdict: **KEEP the generation strategy — it is the reason a new component type costs
nothing.** **REWRITE the widget table**: it needs string editing, enums, asset references,
colours, override state and multi-edit, and those are widget-kind decisions driven off the
same schema. The registry API (`describe`, `type_names`, `validate`) needs no change.

---

## 6. Gizmos and picking

`gizmo.rs` is 280 lines, five of them public functions, and it is the cleanest module in
the editor. **Picking and the gizmo share one projection** — literally the same `View`
type, not two equivalent implementations (`gizmo.rs:8-10`), with a test asserting
`project` and `ray` are inverses (`gizmo.rs:211-224`).

- `View::new` builds the camera basis; `project` returns `None` for anything closer than
  0.01 along forward, because a point on the near plane draws a handle across the whole
  screen (`gizmo.rs:70-86`).
- `handles` (`gizmo.rs:123-151`) projects the node centre and the three unit axes, gives
  each handle a fixed **90 px** length so it stays grabbable at any distance
  (`HANDLE_PIXELS`, `gizmo.rs:36`), and **drops an axis whose on-screen length is under a
  pixel** — dragging a zero-length handle multiplies the mouse delta by infinity.
- `grab` (`gizmo.rs:156-165`) is nearest-segment-within-**10 px** (`GRAB_PIXELS`).
- `drag_distance` (`gizmo.rs:183-191`) projects the mouse delta onto the handle direction
  and scales by world-units-per-pixel at the node's depth.

The drag itself is a small state machine in `run.rs`. `Drag` (`run.rs:138-149`) freezes the
handle and the node's transform **as they were at the press**, and every frame computes an
*absolute* value from that start rather than accumulating deltas — a dropped frame then
costs nothing (`run.rs:143-148`). `press_in_viewport` (`run.rs:1914-1932`) tries `grab`
first and falls through to picking. `drag_gizmo` (`run.rs:1935-1994`) converts the world
delta into the node's parent space via `SceneView::parent_inverse`
(`scene_view.rs:231-244`, tested at `scene_view.rs:352-384`) — without that, a gizmo under
a rotated parent moves the node in the wrong direction.

Handles are recomputed every frame (`run.rs:1015-1025`) and only when a `Session` exists,
Play is not running, and **exactly one node is selected** (`focused()`, `run.rs:469-473`).
They are painted by egui into the background layer (`panels.rs:701-739`) so a handle never
draws on top of the inspector it is behind.

Picking is `pick_at_cursor` (`run.rs:2002-2030`): one ray against every node's world AABB
(`ray_box`, `run.rs:2156-2178`), nearest wins, Ctrl extends, empty space clears. The
ponytail comment at `run.rs:1998-2001` names the ceiling honestly: pixel-perfect picking
needs an ID buffer and a readback; this is thirty lines and right for a blockout editor.

**What the gizmos cannot do:** no plane handles, no screen-space translate, no rotation
arc-ball (rotate is 45° per world-unit of projected drag, `run.rs:35` and `:1975`), scale
is *additive* rather than multiplicative so zero is not a trap (`run.rs:1985`), no
snapping or grid, no numeric readout during the drag, no local/world space toggle, no
pivot mode, no gizmo for a multi-node selection, and no gizmo at all for anything that is
not a node transform (lights, colliders, emitter shapes, camera frusta all have no
manipulator).

Verdict: **KEEP `gizmo.rs` AS-IS and build on it** — `View` is the shared projection every
new tool will need (brush cursor projection, decal placement, terrain sculpt raycasts).
**REWRITE the drag state machine** in `run.rs`: it hardcodes three modes and one node, and
a tool system needs press/drag/release as a trait-free enum of active tools. Picking is
**KEEP AS-IS** until a tool needs surface-accurate hits — UV painting and decals will need
a real ray-vs-triangle hit with a UV and a normal, which the AABB test cannot give. That
is the one place a GPU ID/depth readback or a CPU BVH becomes justified, and it should be
argued on that need, not on picking accuracy in general.

---

## 7. egui inside the Vulkan frame

`crates/loom_render/src/ui.rs` is the whole wiring and it is 174 lines.

`Ui::new` (`ui.rs:34-99`) creates **its own `gpu-allocator` instance** rather than sharing
the viewer's behind an `Arc<Mutex<>>` (`ui.rs:40-54`) — both are suballocators over
`vkAllocateMemory`, so a second one costs a handful of blocks against threading interior
mutability through every renderer resource. It builds `egui_winit::State` and
`egui_ash_renderer::Renderer` in `RenderMode::DynamicRendering` with the swapchain's colour
format and **no depth or stencil attachment** (`ui.rs:70-81`), `in_flight_frames: 1`, depth
test and write off, `srgb_framebuffer: false`.

`Ui::draw` (`ui.rs:117-151`) is the per-frame sequence: `take_egui_input` →
`Context::run_ui(input, build)` → `handle_platform_output` → `tessellate` →
`set_textures(queue, pool, …)` for the font atlas delta → `cmd_draw` → `free_textures`.
The closure receives a root `egui::Ui` (egui 0.35 shape), which is why panels attach to a
`&mut egui::Ui` rather than a `Context` (`panels.rs:128`).

**Where the UI sits in the frame.** `Viewer::draw_with_ui` (`viewer.rs:936`) builds a
render graph whose pass order is:

```
forward  →  [water resolve, if split]  →  rain  →  tonemap  →  ui  →  [cmaa2_edges, cmaa2]  →  present
  1245                    1399           1502      1573       1590            1624             1654
```

The `ui` pass (`viewer.rs:1590-1619`) declares one use — `(post_id, Access::ColorWrite)` —
and calls `begin_overlay_rendering` with a **`None` resolve target**: the UI draws into the
already-resolved, already-tonemapped LDR image **at one sample**
(`viewer.rs:1599-1602`). MSAA is 4x on the forward pass only (`viewer.rs:313`,
`MSAA_SAMPLES`); multisampling a text overlay would cost fill rate for a difference nobody
can see. Rain is likewise after the resolve and before the UI, which is why streaks do not
fall across the panels (`ui.rs:73-78`).

**One caveat the rework should decide about deliberately: CMAA2 runs *after* the UI pass.**
It is opt-in (`LOOM_CMAA2`, `viewer.rs:75-81`) and therefore off by default, but when it is
on it filters egui's text along with the scene. Nobody appears to have looked at that.

Event routing back the other way is `Ui::on_window_event` (`ui.rs:105-111`) returning
egui's `consumed`, checked in `run.rs:830-857`. Two rules there are hard-won and must
survive: **a consumed event must not also reach the viewport** or clicking a panel flies the
camera; and **Tab is special** — egui claims it unconditionally as its focus key, so
`select_next` never fired, and `wants_keyboard` was the wrong fix because it is true
whenever *any* widget has focus. The fix un-consumes Tab unless a **text field**
specifically has focus (`Ui::wants_text_input`, `ui.rs:164-173`; used at `run.rs:852`).
Viewport mouse presses are gated on `Ui::wants_pointer` (`run.rs:889`).
`Ui::wants_keyboard` (`ui.rs:159-162`) is now dead code.

**Teardown order is written out rather than left to field order** (`run.rs:294-335`), and
the comment explains both bugs it encodes: the viewer must drop before the `Ui` (egui's
pipeline and descriptor pool are still referenced by the viewer's command pool —
VUID-vkDestroyPipeline-pipeline-00765), and the `Device` must drop before the `Instance`
(a tuple drops `.0` first, which segfaulted the NVIDIA driver at process exit). The window
is released last because the surface refers to it. `shutdown` (`run.rs:1560-1566`) repeats
the same order for the early-exit path, because X can destroy the window before the event
saying so arrives and egui asks the window for its size every frame.

Verdict: **KEEP `ui.rs` AS-IS.** The one likely change is `in_flight_frames: 1` if the
rework ever pipelines frames. The graph position is **KEEP AS-IS** with one addition: a
docked viewport (§9) needs the scene rendered into an *image the UI samples*, which moves
the scene from "under the UI" to "a texture inside it" — that is the single largest
renderer-side consequence of a Unity-like layout, and it is a real change to this pass
order, not a UI detail.

---

## 8. The live file view and the divergence banner

**The window polls the scene file four times a second** (`WATCH_INTERVAL`, `run.rs:50`;
`poll_file`, `run.rs:635-678`). The ponytail comment at `run.rs:631-634` records the
decision: re-read the file rather than take an inotify dependency, because a scene is
kilobytes; switch to `notify` at megabytes or when watching an asset tree matters. **The
rework will make that upgrade necessary** — a project browser watching a whole asset
folder is exactly the case the comment names.

The comparison is `VersionToken::of(disk)` against `self.disk_seen` — the version *we last
read or wrote* (`run.rs:178`, `:646-649`) — not against our in-memory text, because that
would flag every unsaved edit as somebody else's write.

Three outcomes:

1. **No session, or clean:** reload and re-derive via `show_external` (`run.rs:476-499`),
   which snapshots the previous nodes, rebuilds, and diffs.
2. **Unsaved edits and the file moved:** `self.conflict = Some(disk)` and a warning
   (`run.rs:651-659`). **Both versions are kept intact and nothing is merged** — never-do
   #15. The banner (`panels.rs:347-371`) offers exactly two buttons, each labelled with what
   is *lost*: "Reload from disk" (`accept_disk`, `run.rs:681-698`, discards unsaved edits)
   and "Keep mine" (`keep_mine`, `run.rs:702-717`, which must also call
   `Session::accept_disk_version` or the next Ctrl+S is refused and the human is locked out
   of saving the version they just chose — the escape hatch documented at `edit.rs:366-385`).
3. **A transaction rejected as `stale_version`:** reload, never force
   (`run.rs:1743-1753`). `save` hitting `SaveRejected::Stale` raises the same banner rather
   than overwriting (`run.rs:1584-1588`).

Underneath, `Session::save` (`edit.rs:339-364`) takes an exclusive **lock on a sidecar
file** — not the scene, because `write_atomically` renames a new file over the target and a
lock on the old inode protects nothing (`lock_scene`, `edit.rs:113-136`) — re-reads, checks
the token, and writes via `write_atomically` (`edit.rs:37-71`: write beside, `sync_all`,
rename). `apply_to_file` (`edit.rs:90-111`) holds the same lock across read-apply-write for
the agent path. **This is the most carefully-reasoned code in the editor stack and none of
it is UI.**

**Then it says which nodes changed.** `SceneView::changes_from` (`scene_view.rs:184-221`)
diffs node-by-node into Added / Removed / Moved / Edited / MovedAndEdited.
`App::agent_marks` (`run.rs:422-466`) projects each changed node's AABB corners to screen
space and fades over `CHANGE_FADE = 6.0` seconds (`run.rs:419`); `agent_overlay`
(`panels.rs:663-695`) draws a labelled box in a hue distinct from the axis colours.
Deliberately not a modal, not a list to acknowledge — the human is already looking at the
viewport (`panels.rs:661-662`).

One behaviour to carry forward consciously: **the watcher is asleep while Play runs**
(`run.rs:972-974`), because reloading the authored scene under a running simulation would
be wrong; `stop_play` re-arms it immediately (`run.rs:1428-1431`).

Verdict: **KEEP the whole conflict model AS-IS.** **REWRITE the polling** into a project-
level watcher when the hub lands. The change-mark overlay is **KEEP** in behaviour and
**REWRITE** in presentation — a docked viewport changes the coordinate mapping (§9).

---

## 9. The panels themselves

`panels::draw` (`panels.rs:128-143`) adds, in order: toolbar (top), conflict banner (top,
conditional), hierarchy (left), inspector (right), console (bottom), assets (bottom), then
paints the gizmo and agent overlays. **Order matters and is documented**: first added is
outermost, and anything filling the centre must come last (`panels.rs:125-127`).

**The 3D scene is not in a panel. It is the whole window, and the panels are drawn over
it.** `gizmo_overlay` states the consequence plainly: "the viewport is the whole window;
panels are drawn over it. So window pixels map to egui points by the one scale factor, with
no offset" (`panels.rs:706-710`). Picking, gizmo projection and the change marks all assume
`gizmo::View::new(&camera, extent.0, extent.1)` where extent is the **swapchain** extent
(`run.rs:1007-1009`). A camera pointed at a scene therefore frames it behind the inspector
as well as in the visible strip. **This is the structural change the Unity-like layout
demands**, and it touches the renderer (§7), the projection, the picking ray and the
overlay coordinates together.

**Toolbar** (`panels.rs:145-249`): Move/Rotate/Scale selectable labels, Focus, Duplicate,
Delete, transport, Undo/Redo/Save, a play-state or read-only or "● unsaved" indicator, and
right-aligned `fps · nodes · draws`. Every button is `add_enabled(editing, …)` where
`editing = editable && playing.is_none()`.

**Hierarchy** (`panels.rs:373-452`): a flat list of `view.paths` indented by slash count.
Each row is both a `dnd_drag_source` and a drop target, which is what makes reparenting a
drag; a `▪`/`·` marker says whether the node draws anything; Ctrl-click extends; a context
menu offers Add child / Duplicate / Delete. **No collapse, no filter, no search, no sibling
reordering, no icons, no multi-drag, no scroll-to-selection.** Keyboard navigation is Tab /
Backquote cycling the flat list (`run.rs:2094-2110`).

**Assets** (`panels.rs:543-574`): not a project browser. It lists the aliases *this scene
resolved* (`MeshLibrary::names`, `main.rs:1226`); `voxel:` entries are disabled with a
tooltip saying they are baked from the node's op list; clicking any other assigns it to the
whole selection. **No filesystem, no import, no thumbnails, no textures, no materials, no
drag-into-scene, no folders.**

**Console** (`panels.rs:579-651`): two columns — engine messages from the global log store
(`log.rs`, a `Mutex<Vec<Entry>>` with repeat collapsing at `log.rs:41-60` and a 500-entry
cap at `log.rs:33`) and the session's transaction labels. The transaction column is the
human's window onto what the agent did, and it is why transactions carry labels at all.

**HUD** (`hud.rs`): the *game's* overlay, not the editor's. `elements` reads `Hud`
components from the world and substitutes `{name}` from `GameState`; `draw`
(`hud.rs:136-161`) paints into `available_rect_before_wrap()` — which is why panels must be
added first, or the score lands on top of the hierarchy (`run.rs:1179-1182`).

**No layout persistence of any kind.** `egui::Context::default()` (`ui.rs:56`) has no
storage backend, so resized panels reset every launch. **No theming** — egui defaults, with
three axis colours hardcoded at `panels.rs:95-99`.

Verdict: **REWRITE all of it.** The panel *content* is the specification of what the new
panels must still show; the layout, the docking model, the styling and the flat-list
hierarchy are all replaced.

---

## 10. Play mode

Not strictly editor UI, but it shares the window and the rework inherits it.

`start_play` (`run.rs:1321-1354`) builds a fresh `World` from the scene and hands it to
`crate::play::Play`. **The file is never written, now or at Stop** — Unity's oldest
usability wound is edits made during play silently vanishing, and nothing here is at risk
because nothing was written (`run.rs:1316-1320`). Pointer capture is Locked-then-Confined
(`capture_pointer`, `run.rs:1361-1378`) and only when the scene has both a
`CharacterController` and a `Camera`. Escape releases the pointer rather than closing the
window while captured (`run.rs:954-960`).

While playing: handles are suppressed (`run.rs:1015`), editing keys are ignored
(`run.rs:2042-2044` — the toolbar already refused, the keys did not, and the edits were
invisible because the viewport was drawing the simulated world), the watcher sleeps, and
particles advance only on ticks that actually ran (`run.rs:979-988`, `:1053-1064`).
`stop_play` (`run.rs:1403-1432`) drops the world, silences audio, resets the detonation and
splash counters, and logs the tick count and state hash.

Verdict: **KEEP the semantics AS-IS** — particularly "Play never writes the file" and
"editing is inert during Play". **REWRITE the presentation**: a Unity-like layout wants a
Game view distinct from the Scene view, which the current single-window model cannot
express.

---

## 11. Camera and input

`FlyCamera` (`run.rs:53-126`) is position + yaw/pitch + FOV. It opens at the scene's
authored `Camera` when there is one and frames the whole scene otherwise
(`run.rs:352-354`), and `F` reframes the selection or the scene (`run.rs:962-967`,
`focus_bounds` at `run.rs:744-760`). Look is **right-mouse-held** — deliberately not a
captured pointer, and deliberately not the left button, because a button that both orbits
and drags the thing you clicked is how you fling a wall across the map
(`assets/input/default.toml`, `look` binding).

All keys go through `loom_input::ActionMap` loaded from `assets/input/default.toml` with
the compiled-in copy as fallback (`load_bindings`, `run.rs:2242-2251`), in three contexts:
`fly`, `edit`, `play` (`run.rs:129-133`). Editing bindings are digits for gizmo modes
(W/E/R are already fly movement), IJKLUO for nudge, Ctrl+Z/Y/S/D, Delete, Tab/Backquote.
Keys are read **once per redraw**, not per event (`run.rs:868-878`, `:940-950`), because
`Pressed` is latched for the frame and evaluating per event fired one Delete several times.

**No orbit, no pan, no ortho, no view presets, no bookmarks, no in-UI speed control, and no
keybinding editor.**

Verdict: **KEEP the ActionMap plumbing and the three contexts.** **REWRITE the camera** —
a Unity-like editor needs orbit/pan/zoom around a pivot alongside fly, and the bindings
file needs a UI.

---

## 12. Scene derivation and GPU upload scheduling

`SceneView` (`scene_view.rs:50-140`) is everything derived from scene text: parsed `Scene`,
resolved `World`, `MeshLibrary`, `MaterialLibrary`, draw-call `objects`, per-node AABB
`picks`, scene `bounds`, `paths`, `assets`, `mesh_key`, and a cached `scattered` list. It
exists so the derivation can be **re-run** — that is what makes the window a live view of
the file rather than a screenshot (`scene_view.rs:1-9`).

It is rebuilt on **every transaction**, including every frame of a gizmo drag (`resync`,
`run.rs:622-627` → `show`, `run.rs:507-539`). Four things keep that affordable:

- `mesh_key` (`main.rs:1206-1224`) — equal keys mean the GPU buffers are still valid, so a
  transform edit costs no re-upload while a re-baked voxel volume does (`run.rs:516-523`).
- `VoxelCache` reused across rebuilds (`run.rs:157-158`) so a drag does not re-bake volumes.
- `grass_key` / `terrain_key` string keys (`upload_grass`, `run.rs:547-573`;
  `upload_terrain`, `run.rs:583-619`) — the terrain bake also produces the rain collision
  field, which is why carving a roof in the editor lets rain through on the next frame with
  no reload.
- `scattered` cached for the `SceneView`'s whole lifetime (`scene_view.rs:70-85`) —
  re-placing it per frame measured **103 ms on `forest.loom`**, which is 9 fps.

**Invalid text leaves the last good view on screen and says so** (`run.rs:504-514`) —
blanking the viewport because a file was caught mid-save would be worse than useless.

Verdict: **KEEP AS-IS.** This is the model layer and the rework should treat it as one.
The rewrite's job is to stop re-deriving the *whole* view on every stroke of a paint
gesture, and the seam for that already exists in the keyed uploads.

---

## 13. What is simply not there

Verified absent by search, not assumed. Each of these is new construction:

**Project and file management.** No hub, no project concept — the unit is a scene path
argument (`run.rs:2291`). No New Scene, no Save As, no recent list, no templates, no
first-person/third-person samples, no base scene a new project opens to.

**Creation.** `Add child` always spawns a `MeshRenderer` on `"box"` (`run.rs:1803`), while
`loom_asset::primitives::build` already offers box, plane, sphere, cylinder and capsule
(`main.rs:477`). There is no create menu, no placement-at-cursor, no create-as-sibling.

**All four painting systems.** Nothing in the editor writes a splat weight, a texture
texel, a vertex colour or a decal. No brush, no cursor projection, no layer stack, no
texture write-back path. `Material` is authored numerically through the inspector only.

**Terrain sculpting.** Voxel ops are CLI-only — `loom place --op` and `loom explode`. The
editor bakes and uploads voxel volumes and can select their AABB, and that is all.

**Prefabs.** `RevertOverrides` and `UnpackPrefab` exist as ops and `loom prefab` exists as a
command; the editor exposes neither, and the inspector cannot show that a field is an
override (§3, §5).

**Scripts.** No editing, no listing, no error surface beyond console lines.

**Everything else with no UI:** navigation/A* visualisation, the event log, `GameRules`
win/lose state, audio and acoustics, wind/water/rain/cloud authoring (all inspector-numeric
at best), lights and cameras have no manipulators, colliders have no visualisation, and
`cargo xtask flythrough` / `shimmer` are not reachable from the window.

**Editor infrastructure:** no docking, no layout persistence, no theme, no search, no
copy/paste, no per-node visibility or lock, no snapping, no measurement tools, no profiler
panel beyond the fps label and `LOOM_GPU_TIMING`, and no end-user documentation.

---

## 14. Things the rewrite must not quietly lose

A checklist, because each of these was a bug once and the comment explaining it lives in
the file being deleted:

1. **One write path** — `transact`/`transact_as` only, `expect_version` overwritten by the
   session (`run.rs:1707-1756`, `edit.rs:299-311`).
2. **Gesture epoch bumped on mouse release** (`run.rs:898`), or re-grabbing a handle
   continues the previous undo entry.
3. **`dirty` set only when something actually moved** (`run.rs:1289-1303`) — a no-op Ctrl+Z
   latched it true, the viewport stopped following the file, and "Keep mine" then wrote back
   the text as it was when the editor opened.
4. **Keys read once per redraw**, not per event (`run.rs:940-950`).
5. **Tab un-consumed unless a text field has focus** (`run.rs:842-857`).
6. **Editing keys inert during Play** (`run.rs:2042-2044`).
7. **Teardown: viewer → ui → (device → instance) → window** (`run.rs:294-335`).
8. **Rename buffer survives the frame** in egui's temp store (`panels.rs:477-487`).
9. **Panels added before the HUD**, or the HUD anchors to the whole window
   (`run.rs:1179-1182`).
10. **Delete sorts deepest-first**; **Duplicate is one transaction** (`run.rs:1874-1896`,
    `:1814-1867`).
11. **Log repeats collapse** (`log.rs:41-60`) — the view re-derives per frame, so one
    missing asset wrote the same line hundreds of times a second.
12. **`--frames` and `--play` keep working**, or the windowed validation gate goes dark.
13. **Play never writes the file.**
14. **Invalid scene text keeps the last good view** (`run.rs:504-514`).

---

## 15. Dependency-rule note

`loom_render` deliberately re-exports both `egui` and `ash`/`ash_window`
(`loom_render/src/lib.rs:63-69`) so the CLI can build panels and a `VkSurfaceKHR` without
its own dependency on either — the rule is "nothing outside `loom_render*` imports ash",
and this satisfies it by letter and by intent. **But surface creation itself lives in the
CLI** (`build_viewer`, `run.rs:2180-2234`, using `ash_window::create_surface` and
`ash::khr::surface::Instance`). If the rework introduces a `loom_editor` crate, that
function is the natural thing to push down into `loom_render` rather than carry across.
