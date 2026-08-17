# Design — in-viewport authoring tools

*Editor rework, design phase. Read `00-survey-existing.md`, `00-survey-engine-surface.md` and
`00-survey-constraints.md` first; this document assumes all three and does not repeat them.
Everything cited as `file:line` was read in this worktree at `62f9ebe`. Everything I could not
settle by reading is in §16 and nowhere else.*

---

## 1. The one idea: a tool cannot produce anything but ops and view state

Never-do #16 says every editor action becomes `SceneOp`s. Prose cannot enforce that; a type can.
**Every tool in this design returns one value of one enum, and only two of its variants are not
scene ops.**

```rust
// loom_editor/src/tools/mod.rs
pub enum Outcome {
    /// The tool did nothing this event.
    None,
    /// A scene mutation. The only path to the file.
    Edit(Edit),
    /// Selection changed. Not authored state (§14).
    Select(Vec<String>),
    /// Viewport-only: isolate, hidden set, sculpt preview depth (§14).
    View(ViewChange),
}

pub struct Edit {
    /// Shown in the log panel and in git history. Written by the tool, per
    /// gesture, never generated from the op list.
    pub label: String,
    pub ops: Vec<SceneOp>,
    /// `Some` routes to `Session::apply_coalescing`; `None` to `Session::apply`.
    pub gesture: Option<String>,
}
```

`Edit` is handed to the surviving `transact`/`transact_as` funnel unchanged (`run.rs:1707-1756`) —
which is itself handed to `Session::apply` or `Session::apply_coalescing` (`edit.rs:262`, `:282`),
which overwrites `expect_version` with the session's own token (`edit.rs:299-301`) so a tool cannot
skip the staleness check even by writing the field.

**The value of this shape is what it makes impossible.** A tool has no `&mut SceneView`, no file
handle and no `&mut Session`. A tool that wanted to nudge a vertex, stash a mask, or write a
sidecar would have nowhere to put it. When a future feature genuinely cannot be expressed as ops
— texture painting is the known case — the compiler forces the exemption to be argued at the tool
boundary rather than discovered later in a diff.

**Tools are an enum, not a trait.** Never-do #12 bans a trait with one implementation, and the
weaker reason applies even at five: a `Tool` trait buys dynamic dispatch nobody needs and costs an
allocation and a vtable per tool switch, while an enum gives exhaustive matching on tool state and
lets `Select` and `Move` share the drag machinery by construction.

```rust
pub enum Tool {
    Select,
    Move, Rotate, Scale,   // three modes of one gizmo, not three tools
    Create(Primitive),
    Sculpt(Brush),
}

pub enum ToolEvent { Press(Vec2), Drag(Vec2), Release(Vec2), Cancel }

pub struct ToolCtx<'a> {
    pub view: &'a gizmo::View,        // built from the *viewport rect*, §6
    pub scene: &'a SceneView,
    pub selection: &'a [String],
    pub snap: &'a Snap,
    pub epoch: u64,                   // bumped on every mouse release (run.rs:898)
}

pub fn on_event(state: &mut ToolState, ctx: &ToolCtx<'_>, ev: ToolEvent) -> Outcome;
```

`ToolState` holds only what a gesture needs between press and release — the frozen start transform,
the grabbed handle, the stamps emitted so far. It is cleared by `Cancel` (Escape) and by any
external reload, per `LOOM-IMPLEMENTATION-ORDER.md:459-461`'s "never reload mid-gesture".

---

## 2. Where this lives

Assuming ADR F lands the `loom_editor` crate (survey §4.F; the brief already lists it at
`LOOM-BUILD-BRIEF.md:107-108`). If it does not, every path below reads `crates/loom_cli/src/editor/`
instead and nothing else in this design changes — the authoring layer touches no `ash`, so it
satisfies the dependency rule either way.

| File | What it is | New or moved |
| --- | --- | --- |
| `loom_editor/src/gizmo.rs` | `View`, `Handle`, `handles`, `grab`, `drag_distance` | **moved from `loom_cli/src/gizmo.rs`, extended** (§6) |
| `loom_editor/src/tools/mod.rs` | `Tool`, `ToolEvent`, `ToolCtx`, `Outcome`, `Edit`, dispatch | new |
| `loom_editor/src/tools/select.rs` | click, marquee, isolate | new |
| `loom_editor/src/tools/transform.rs` | the gizmo drag state machine | new (replaces `run.rs:1935-1994`) |
| `loom_editor/src/tools/create.rs` | primitive placement | new |
| `loom_editor/src/tools/sculpt.rs` | voxel brush, stamps, op-list panel | new |
| `loom_editor/src/cursor.rs` | the three-tier viewport raycast | new (§3) |
| `loom_editor/src/snap.rs` | grid, angle, surface, increment | new (§7) |
| `loom_editor/src/arrange.rs` | duplicate, group, align, distribute, array | new — mostly a wrapper over `loom_scene::place` |
| `loom_editor/src/prefabize.rs` | create prefab, instance, override editing | new (§11) |
| `loom_editor/src/script.rs` | script slot, buffer, external open | new (§12) |
| `loom_scene/src/ops.rs` | `SpliceArray`, `Declare`, `SpawnNode { prefab }` | **changed** (§13) |
| `loom_asset/src/primitives.rs` | `quad` | **changed** (§4) |

Nothing here imports `ash`; `egui` is reached through `loom_render`'s re-export exactly as
`panels.rs:17` does today.

---

## 3. The cursor: one raycast, three tiers, no new engine code

Every placement tool asks the same question — *what world point is under the mouse* — and the
answer has to work over meshes, over voxel terrain, and over nothing. **One function answers it,
so a cube dropped by the create tool and a sculpt stamp land in the same place.**

```rust
// loom_editor/src/cursor.rs
pub struct Hit { pub point: Vec3, pub normal: Vec3, pub node: Option<String>, pub tier: Tier }
pub enum Tier { Mesh, Voxel, Ground }

pub fn under_cursor(ctx: &ToolCtx<'_>, cursor: Vec2) -> Hit;
```

**Tier 1, meshes: the existing AABB test.** `View::ray` (`gizmo.rs:91-99`) against every entry of
`SceneView::picks` (`scene_view.rs:60`, a `BTreeMap<String, place::Bounds>`) with the slab test
already written at `run.rs:2156-2178`, nearest wins. This is what picking already does
(`pick_at_cursor`, `run.rs:2002-2030`) and reusing it means the object you click is the object you
drop onto — two separate implementations would eventually disagree by a pixel and nobody would find
out for a month.

**Tier 2, voxel terrain: sphere-march the SDF.** An AABB test against a `VoxelVolume` returns the
top of a 128 m box, which is worse than useless for placing anything on a hillside. `loom_voxel`
already exposes the field: `exposure::sample(volume, world) -> f32` (`exposure.rs:42`) is a signed
distance at a world point, so the ray march is the textbook loop — advance by the distance, stop
under a tolerance, cap the iterations. For the common downward case
`heightfield::surface_height(volume, offset, x, z)` (`heightfield.rs:85`) answers directly, and
`HeightField::has_ground` (`:192`) is the "there is no surface in this column" sentinel that must
fall through to tier 3 rather than returning a plausible-looking zero.

**Tier 3, nothing: the ground plane, then the focus plane.** Intersect the ray with `y = 0`. If the
camera looks up and misses that too, place at the camera's focus distance on the plane facing the
camera, so a create click always produces an object the user can see rather than silently doing
nothing.

**Rejected: `loom_physics::raycast` (`lib.rs:533`).** It queries the acceleration structure the
last `step` built, and its own doc comment says a collider added since then is invisible to a ray —
"Step first". Edit mode runs no physics, so using it means constructing and stepping a whole rapier
world to place a cube. The SDF and the AABBs are already in memory.

**Rejected: a GPU ID-buffer readback.** It is the right answer for pixel-accurate picking and will
become necessary for UV painting and decals, which need a triangle, a UV and an interpolated
normal that no AABB can produce. It costs a render-graph pass, a readback and a stall, and the
ponytail comment at `run.rs:1998-2001` already priced the alternative honestly for a blockout
editor. **Build it when a painting tool needs a UV, and argue it there** — not to make cube
placement 3 px better.

---

## 4. Creating primitives

**`quad` is the only missing mesh and it is twenty lines.** `primitives::NAMES` is
`["box", "plane", "sphere", "cylinder", "capsule"]` (`primitives.rs:10`), and `plane()`
(`primitives.rs:82-94`) is already a 4-vertex quad — but it lies in the XZ plane facing up, which
is a floor, not the camera-facing card people mean by "quad". So `quad` is `plane()` rotated into
XY facing `+Z`, with the same 0..1 UVs, added to `build` and to `NAMES` (whose type changes from
`[&str; 5]` to `[&str; 6]`).

**No `cube` alias.** The user asked for a cube; the format's name is `box`. A second name for one
mesh is a second answer and every error message would have to list both. The create menu is
labelled **Cube**, **Sphere**, **Capsule**, **Plane**, **Quad**, **Cylinder** and writes `box`,
`sphere`, `capsule`, `plane`, `quad`, `cylinder`. UI label and format name are allowed to differ;
the format is what has a stability guarantee.

**Primitives need no `[[asset]]` declaration.** `MeshLibrary` resolves a primitive name
procedurally before consulting the scene's aliases (`main.rs:459`, `:1150`), and `loom validate`
accepts a primitive name as a resolution (`main.rs:469-477`). So creating a cube is one
`SpawnNode { parent, name, mesh: Some("box") }` — the op already carries the mesh alias and writes
the `MeshRenderer` inline (`ops.rs:615-627`).

**Where it lands.** Click in the viewport with a primitive armed:

```
ops:  SpawnNode { parent: <selection's parent, or the root>, name: unique("Cube"), mesh: Some("box") }
      SetTransform { node, pos: Some(resting_position), .. }
label "Create Cube on Ground"
```

`resting_position` is the cursor hit (§3), snapped by §7, then lifted so the new object's **bottom**
sits on the surface — the same rule `place::resolve`'s `PlaceOn` enforces at `place.rs:189-196`
("Sit the mover's *bottom* on the surface's top. Using its centre is what puts a monitor
half-buried in a desk"). The lift comes from the primitive's own local bounds, computed once from
`primitives::build(name)` and cached in a `OnceLock<BTreeMap<&str, Bounds>>` — six meshes, built at
first use, never again.

Two ops, one transaction, one Ctrl+Z. **Not `PlaceOp::PlaceOn`,** because that needs a target
*node* and the cursor may have hit voxel terrain or nothing; the shared rule is the arithmetic, not
the op.

**Name collisions are the tool's job, not the op layer's.** `SpawnNode` correctly refuses a
duplicate sibling name (`ops.rs:598-605`); the create tool appends `_1`, `_2` before issuing, in
the same spirit as Delete sorting deepest-first (`run.rs:1884`) — the editor orders and labels, it
does not add semantics.

**Drag-to-size is deliberately not built.** Blender and Unity both let you drag out a box's extents
at creation; here that is a second way to set a scale that the gizmo already sets, on an object
that does not exist yet. Create at unit size, then scale. Revisit if blockout sessions actually
show the extra step hurting.

---

## 5. Selection

**Click, Ctrl-click and Escape keep working exactly as they do** (`run.rs:2002-2030`): one ray,
nearest AABB, Ctrl extends, empty space clears. It routes through `cursor::under_cursor` tier 1 so
picking and placement share the hit.

**Marquee is the projection running the other way and needs nothing new.** On a left-drag that did
not start on a gizmo handle, project each candidate node's eight world-AABB corners with
`View::project` (`gizmo.rs:70-86`), take their screen-space bounding rect, and select every node
whose rect intersects the marquee. Nodes behind the camera project to `None` and are excluded,
which is correct and free. Shift adds to the selection, Ctrl removes. Rejected: proper
frustum-vs-AABB clipping — it is more code for a case (a node straddling the near plane) that a
blockout editor hits rarely, and the failure mode is a missed selection, not a wrong edit.

**Hierarchy sync is one direction of truth.** The selection is a `Vec<String>` of node paths held
by the editor shell, and both the hierarchy panel and the viewport read it and write it through
`Outcome::Select`. Neither owns a copy. Selecting in the hierarchy scrolls the viewport nowhere;
selecting in the viewport **does** scroll the hierarchy to the row, because a tree of two hundred
nodes with the selected row off-screen is the single most common "where did it go" in every editor.

**Isolate and hide are view state and must never be written to the scene.** Unity writes visibility
into the scene file; here that would put a purely editorial concern into a file whose whole premise
is that it is the game, and it would bloat every scene the editor touches against
`docs/format/README.md` §4's "defaults are omitted". So `ViewChange::Isolate(Vec<String>)` and
`ViewChange::Hidden(BTreeSet<String>)` filter `SceneView::objects` at draw time, live in editor
state, and are **discarded on reload** along with everything else derived from a scene that moved
(`edit.rs:395-403`).

The cost of that choice is that a hidden object is invisible with no trace in the file, so **the
viewport shows a persistent bar — "3 objects hidden · Show all" — whenever the hidden set is
non-empty.** Without it, hide is a way to lose work permanently, and that is a worse bug than the
scene bloat it avoids.

---

## 6. Transform gizmos: nine changes, one dependency decision

**Keep `gizmo.rs` and extend it. Do not adopt `transform-gizmo`.** Phase 7's E3 names that crate
(`LOOM-IMPLEMENTATION-ORDER.md:434`) and the survey lists it under ADR E. I am rejecting it, and
the reason is specific rather than an aversion to dependencies: `gizmo.rs`'s load-bearing property
is that **picking and the gizmo project through the same `View`, not through two equivalent
implementations** (`gizmo.rs:8-10`, with a test asserting `project` and `ray` are inverses at
`:211-224`). An external gizmo brings its own camera math. The two would agree for months and then
drift by a handle-width under a rotated parent, which is precisely the bug class the shared
projection was written to prevent. The nine improvements below total a few hundred lines against
280 existing, all of it in the module that already has the tests.

**This is a partial answer to ADR E**: the gizmo question is settled here and needs no new
dependency. Docking and icons remain open and still need the ADR.

1. **Plane handles.** Three quads at the axis pairs. A drag intersects `View::ray` with the plane
   through the frozen start position, so the object tracks the cursor exactly instead of being
   geared to it. ~30 lines, and it is the handle people actually use for blockout.
2. **Screen-space translate.** A small square at the origin dragging on the plane facing the
   camera. Same intersection code as (1) with the plane normal set to `view.forward`.
3. **Local / world space.** `handles(view, origin)` becomes `handles(view, origin, basis: [Vec3; 3])`,
   passed world unit vectors or the node's own rotated axes. Everything downstream —
   `drag_distance`, `grab` — is already basis-agnostic because it works in screen space.
4. **Rotation becomes an arcball, and the current gearing goes.** Today rotate is 45° per world unit
   of projected drag (`run.rs:35`, `:1975`), which means the same wrist movement rotates differently
   depending on how far away the object is. Replace with the angle of the cursor about the projected
   origin: `atan2` at press, `atan2` now, delta is the rotation. Fewer lines than what it replaces
   and it is what every other editor does, so it needs no learning.
5. **Scale stays additive.** `run.rs:1985` chose additive over multiplicative so that zero is not a
   trap, and that is right. Add a uniform-scale centre handle.
6. **Numeric readout during the drag.** The absolute value, in metres or degrees, painted at the
   cursor. The drag machinery already computes absolute-from-frozen-start (`run.rs:143-148`), so the
   number is already in hand — it is a `painter.text` call and it is the difference between a gizmo
   you can trust and one you check in the inspector afterwards.
7. **Multi-selection gets a gizmo.** Today handles appear only when exactly one node is selected
   (`focused()`, `run.rs:469-473`). Draw them at the selection's pivot; a drag issues one
   `SetTransform` per selected node, in **one transaction, under one gesture key**. Rotation and
   scale about a shared pivot need each node's position transformed about that pivot and then back
   through `SceneView::parent_inverse` (`scene_view.rs:231-244`) — that function exists and is
   tested precisely because a gizmo under a rotated parent moves the node the wrong way without it.
8. **Pivot mode: median or individual origins.** One toggle. Median rotates the group; individual
   spins each in place. Two lines of arithmetic, and without it "rotate these six props a bit" is
   impossible.
9. **The viewport is a rect, not the window.** Every coordinate in `gizmo.rs` is currently window
   pixels because the scene fills the window (`panels.rs:706-710`). In a docked layout `View::new`
   takes the viewport rect's size and the cursor is made rect-relative before it reaches any tool.
   **This is a hard dependency on the render-to-texture viewport (ADR I) and the authoring layer
   cannot be finished before it**; until then every tool is off by the panel widths.

**Colliders, lights, emitters and cameras still have no manipulator, and that stays true here.**
A `BoxCollider` half-extent gizmo and a light-range sphere are worth building, but they are
*component* gizmos driven by the schema rather than transform gizmos, and they belong with the
inspector design, not this one. The seam they will use is the same `Outcome::Edit`.

---

## 7. Snapping — and an `f32` finding that decides the defaults

Snapping lives in one struct because translate, rotate, scale, create and sculpt all need it:

```rust
// loom_editor/src/snap.rs
pub struct Snap {
    pub grid: Option<f32>,      // metres, e.g. 0.25
    pub angle: Option<f32>,     // degrees, e.g. 15
    pub increment: Option<f32>, // scale steps, e.g. 0.25
    pub surface: bool,          // drop onto the cursor hit's surface
}
```

**Snap the absolute value, not the delta.** The drag machinery computes an absolute transform from
the frozen start every frame, so snapping is one `(v / step).round() * step` at the end of that
computation. Snapping the *delta* (Blender's default) moves an object by whole grid multiples from
wherever it already was, which for blockout means a wall that started at 0.07 stays at 0.07 forever.
Absolute snapping puts it on the grid, which is the only reason to have a grid.

**Surface snap reuses `place::resolve`.** With `Snap::surface` on and a mesh node under the cursor,
the drop is `PlaceOp::PlaceOn { node, surface, anchor: Center }` fed through
`place::resolve(&op, &Geometry { bounds_of: &|p| view.node_bounds(p).copied(), parent })`
(`place.rs:160`, `:137-142`) — tested code, and the same code path `loom place --op` uses, so the
mouse and the agent produce an identical diff, which is the M12 exit criterion
(`LOOM-BUILD-BRIEF.md:285`). Over voxel terrain there is no target node, so the lift is computed
from the tier-2 hit instead, with the same bottom-on-surface rule.

**The finding: `SetTransform` widens `f32` to `f64` and writes the noise.** `ops.rs:676-683` does
`array.push(f64::from(*component))`. `prefab::transform_toml` (`prefab.rs:180-190`) solved exactly
this for unpacking — `component.to_string().parse::<f64>()` emits the shortest decimal that
identifies the `f32` — and its doc comment explains why (`prefab.rs:166-172`): `1.4_f32` widened is
`1.399999976158142`, and "writing that back replaces the author's number with noise". The test that
pins it (`ops.rs:2208-2247`) covers `UnpackPrefab` only; `SetTransform` never got the fix.

The consequence for snapping is direct: a grid of 0.25 is an exact binary fraction and survives
widening, a grid of 0.1 does not, so **without the fix the snap steps have to be restricted to
powers of two** and a user who types 0.1 gets `0.10000000149011612` in their scene file.

**So fix it rather than design around it.** Three lines in `ops.rs`, reusing the trick from
`prefab.rs:186`, plus a test in the shape of `unpacking_writes_the_authored_numbers_not_widened_ones`
asserting the same for a snapped `SetTransform`. It is not an ADR — it is a bug in a function the
format spec already governs — but it must land **before** the snap UI, or the first grid setting
someone picks bakes the defect into every scene they touch.

---

## 8. Duplicate, group, align, array — mostly already written

**`loom_scene::place` is the biggest reuse in this document and the editor exposes none of it
today.** `PlaceOp` has `PlaceOn`, `AlignTo`, `FaceToward` and `GridOn` (`place.rs:102-131`),
`resolve` turns each into `SceneOp`s (`place.rs:160`), and the whole module is tested including
the "six desks in two rows" case (`place.rs:420-458`). The editor's align, distribute, drop-on and
array tools are a menu over `resolve`, a `Geometry` built from `SceneView::picks`, and a label.
`arrange.rs` is perhaps eighty lines.

| Menu item | Becomes | Label |
| --- | --- | --- |
| Drop on surface (End) | `PlaceOp::PlaceOn` | "Drop Crate onto Desk" |
| Align / distribute on X · Y · Z | `PlaceOp::AlignTo` | "Distribute 6 nodes on X, 2.0 m" |
| Face toward… | `PlaceOp::FaceToward` | "Face Turret toward Player" |
| Array on surface | `PlaceOp::GridOn` | "Array Desk 2x3 on Floor" |

**Duplicate keeps its existing shape and gains two fixes.** It is built out of `SpawnNode` +
`SetTransform` + one `SetField` per component field rather than a `DuplicateNode` op
(`run.rs:1811-1867`), one transaction, and that restraint should survive — the op vocabulary stays
small and the diff stays honest.

*Fix one: duplicating a prefab instance is currently wrong.* An instance carries no components of
its own (`scene.rs:48-56`), so replaying "every component field" of a resolved instance would write
the prefab's contents into a plain node and quietly break the link. With `SpawnNode { prefab }`
(§13) the branch is: instance → `SpawnNode { prefab: Some(alias) }` plus one `SetField` per entry
of `overrides`; plain node → today's behaviour.

*Fix two: Shift-drag duplicates for free.* Issue the duplicate transaction **using the drag's own
gesture key**. `apply_coalescing` pops the previous undo entry whenever the key matches
(`edit.rs:291-296`), so the duplicate and every frame of the drag that follows collapse into one
Ctrl+Z with no new machinery at all.

**Group needs compensating transforms or it teleports everything.** `SceneOp::ReparentNode`
(`ops.rs:898-943`) writes the node's `parent` key and rewrites descendant paths. **It does not touch
the transform** — which is correct, because the transform is local by definition
(`scene.rs:35-38`) — so reparenting under a node whose world transform differs moves the child in
the world. That is already a live gotcha in today's hierarchy drag; for Group it is fatal, because
the group node is placed at the selection's centroid and every member would jump by that centroid.

So Group is:

```
SpawnNode   { parent: <deepest common ancestor>, name: "Group", mesh: None }
SetTransform{ node: group, pos: Some(centroid) }
for each selected, outermost first:
  ReparentNode { node, parent: group }
  SetTransform { node, pos/rot/scale: local = parent_inverse(group) * world(node) }
```

One transaction, label `"Group 4 nodes"`. `SceneView::parent_inverse` (`scene_view.rs:231-244`)
supplies the matrix and is tested at `scene_view.rs:352-384`. Ungroup is the inverse plus a
`RemoveNode` on the emptied group, children reparented before the parent is removed.

**The same compensation should be offered on hierarchy drag-reparent**, as a "keep world position"
default with a modifier to opt out. The current code only warns about non-uniform parent scale
(`run.rs:1655-1662`) and lets the node move.

---

## 9. Converting a shape into another shape

**A primitive swap is one `SetField`, and the format makes it free.** `MeshRenderer.mesh` is an
`AssetRef { asset: String }` (`components.rs:49-61`), primitives resolve without a declaration
(`main.rs:459`), so "convert this cube to a plane" is:

```
SetField { node, field: "MeshRenderer.mesh", value: { "asset": "plane" } }
```

The transform, every other component, the node's children and its place in the tree are untouched,
which is the entire point of doing it as a conversion rather than delete-and-recreate.

**Two things the tool must handle rather than pretend away.**

*A collider that no longer fits.* A `BoxCollider` sized to a cube is wrong on a sphere and
catastrophically wrong on a plane (half-extents of 0.5 on a flat mesh is a 1 m thick floor). The
conversion offers, in the same transaction, a `SetField` on `BoxCollider.half_extents` derived from
the new mesh's local bounds — checked by default, because the alternative is a physics bug the user
will not connect to the mesh swap they made an hour earlier.

*A scale axis that stops meaning anything.* Converting a cube to a plane leaves `scale.y` governing
a mesh with no thickness. The tool does not silently normalise it — that would destroy a value the
user may want back — it says so in the confirmation and leaves the number alone.

**Mesh → voxel volume is not supported, and the reason is structural.** A `VoxelVolume` is an
ordered list of analytic CSG ops (`components.rs:358-480`) and there is no op kind meaning "the
shape of this mesh". Converting would require either serialising the resulting voxels (never-do #11)
or a new `mesh` op kind carrying an SDF built from triangles — real work in `loom_voxel`, a new
entry in the op-kind table, and a determinism question about the triangle-to-SDF conversion.
**Named, deferred, with a trigger:** build it when someone wants to carve terrain with an imported
shape, and give it an ADR then.

---

## 10. Voxel sculpting — the honest design

This is the hardest tool in the editor and the one where a comfortable lie is easiest. The lie
would be presenting sculpting as if it edits voxels. **It does not, it appends to a list, and the
UI has to say so, because everything that surprises a user about this tool follows from the list.**

### 10.1 What a stroke is

A `VoxelVolume` stores `ops: Vec<serde_json::Value>` — sphere, box, capsule, heightfield, terrain,
each with `mode` union/subtract/intersect, in the volume's own coordinate space
(`components.rs:358-480`). The brush emits the kinds that already exist:

- **A click is a sphere.** `{ kind: "sphere", center, radius, mode }`.
- **A drag is a run of capsules.** `{ kind: "capsule", a, b, radius, mode }` — one op per **stamp**,
  where stamps are spaced by `radius * 0.5` in *world units along the stroke*, not per frame. A 5 m
  drag with a 1 m brush is ten ops whether the user swept it in a second or ten. Per-frame stamping
  would put the op count at the mercy of the frame rate, which is the same defect class as reading
  the wall clock in simulation code.
- **The box brush** emits `{ kind: "box", center, half_extents, yaw_degrees, round }`, which is what
  carves doorways and cuts terraces.

Each frame of a stroke issues **only the stamps added since the last frame**, as a transaction
under a stable gesture key `sculpt:{node}:{epoch}`. `apply_coalescing` pops the previous frame's
undo entry (`edit.rs:282-297`), so the whole stroke is one Ctrl+Z and the work per frame is
proportional to the new stamps rather than to the stroke so far.

**Coordinates are the volume's, not the world.** Op space runs from the volume's near corner in +X
+Y +Z (`components.rs`, VoxelVolume doc), then the node's transform places it. The brush converts
the cursor hit through the volume node's inverse world matrix before writing the op, and this is
the single most likely place for a sculpt to land in the wrong spot — it deserves a unit test that
sculpts on a translated, rotated volume and asserts the resulting op's `center` puts material where
the cursor was.

### 10.2 Why `SetField` on the whole array is not acceptable, verified

The only route today is `SetField { field: "VoxelVolume.ops", value: <the entire array> }`, which is
what `loom validate` names as the field (`main.rs:369`) and what the CLI uses. Reading the writer
settles what it produces: `json_to_toml` turns a JSON array into a `toml_edit::Array` and a JSON
object into an `InlineTable` (`ops.rs:1039-1052`). So the first sculpt stroke **converts a readable
multi-line `[[node.components.VoxelVolume.ops]]` array-of-tables into one enormous single-line
inline array**, and every subsequent stroke rewrites that whole line.

The scene stays valid. The diff stops being reviewable, which is the entire reason the op list
exists instead of the voxels (never-do #11, `components.rs:340-348`). `git diff` as a verification
channel (`LOOM-BUILD-BRIEF.md:164`) goes dark for the one system whose authored form was designed
to be diffable.

### 10.3 The op the sculptor needs

**One new `SceneOp` covers append, delete, edit-in-place and reorder** (§13 has the full signature):

```rust
SpliceArray { node, field, index: Option<usize>, remove: usize, insert: Vec<Value> }
```

`index: None` appends. `remove: 1, insert: []` deletes. `remove: 1, insert: [new]` edits in place.
Two splices in one transaction reorder. It preserves the array-of-tables spelling when that is what
is on disk, so a stroke is *N added lines* in the diff rather than one changed line of 4,000
characters.

I considered three named ops (`AppendVoxelOps` — the survey's suggestion — plus a remove and an
edit) and rejected them: splice is one variant instead of three, it is a shape every programmer
already knows, and it generalises to the four other array-of-object fields the inspector cannot
edit today (`WaveSet.waves`, `Buoyancy.pontoons`, `Scatter.excludes`, and `GroundLayer`'s list).
Those four are existing callers, not imagined ones, which is what makes the general form the lazy
one rather than the speculative one.

### 10.4 The op-list panel, and how a user removes an earlier op

**Sculpt history is a visible list, not a hidden log.** When a `VoxelVolume` node is selected the
Sculpt panel shows one row per op: index, kind, mode, a one-line summary (`subtract sphere r=2.4 at
64, 18, 51`), and three affordances.

- **Delete** — `SpliceArray { index: i, remove: 1, insert: [] }`, one transaction, one Ctrl+Z.
- **Preview to here** — bakes `ops[..=i]` and shows it. **View state, not an op** (§14). This is what
  makes the list comprehensible: an ordered non-commutative CSG list is otherwise a wall of text.
- **Edit** — the op's own fields as an inspector row, each change a
  `SpliceArray { index: i, remove: 1, insert: [modified] }` under a gesture key so a dragged radius
  is one undo step.

**The thing the panel must say out loud is that the list is not commutative.** Deleting op 7 of 40
re-bakes everything after it, and a subtract that was carving a union which no longer exists will
change the shape in ways that look like corruption if the user was not told. `components.rs:346-348`
already states the rule for agents; the panel states it for humans, and the *Preview to here*
control is what makes the consequence visible before it is a surprise.

**Ctrl+Z and "delete op 7" are different verbs and the UI must not blur them.** Undo immediately
after a stroke removes that stroke's ops because it restores the previous scene text. Deleting op 7
an hour later is a new transaction, which itself undoes. Every editor that presents a sculpt
history as an undo stack teaches the wrong model here, because in this engine the list is the
authored artifact and the undo stack is a stack of whole scene files (`ops.rs:126-128`,
`edit.rs:314-323`).

### 10.5 The preview, and the divergence it risks

A live brush cannot re-bake the whole volume per frame — bake is linear in the op count
(`lib.rs:1100`). `Volume::edit(&op)` applies one op and returns the touched chunks
(`lib.rs:1227`), and `dirty_with_neighbours` widens that so the surface-nets remesh does not crack
at the seams (used exactly this way by `loom explode`, `main.rs:3475-3480`). So the stroke preview
applies each stamp incrementally, remeshes the dirty chunks, and re-uploads through the existing
`terrain_key` path (`run.rs:583-619`) — the same path whose bake already produces the rain
collision field, which is why carving a roof in the editor lets rain through on the next frame.

**The risk is that the preview and the truth are two implementations of one thing.** The preview is
`edit` applied stamp by stamp; the truth, on the next load, is `bake` over the whole op list. If
they ever disagree, sculpting looks right until you reopen the file — the exact failure S2 and
ADR 0006 exist to prevent, one crate over. **It needs a test in `loom_voxel`, not a comment:**
bake a list of N ops, and separately bake the first op and `edit` the remaining N-1, and assert the
fields are bit-identical. If that test cannot be made to pass, the preview must fall back to a full
re-bake on stroke *release* and accept the latency.

### 10.6 Smooth and flatten are not built, and the reason is not laziness

There is no `smooth` and no `flatten` op kind. A smoothing brush therefore has nothing to append —
its only implementation would write voxels directly, which is never-do #11 and would be invisible
to undo, to the diff and to the agent.

**Both are representable if they are wanted, and the mechanism is the same:** add an op kind to
`loom_voxel`'s `VoxelOp`, to `parse_ops`, and to the doc-comment schema on `VoxelVolume.ops` —
`smooth { center, radius, strength, iterations }` and `flatten { rect, height, falloff }` are both
pure functions of the field built so far, so they are deterministic and diffable like every other
op. That is a `loom_voxel` change with a bake-cost question attached, not an editor change.
**Deferred with a stated trigger:** build them when a terrain author asks for them twice, and take
the bake cost measurement first.

### 10.7 The op list only grows, and there is no honest compactor

Nothing coalesces two overlapping subtracts, so a long sculpting session grows the list without
bound and bake cost grows with it. I am not proposing a simplifier: general CSG simplification is a
research problem, and a wrong one silently changes the terrain.

What is cheap and honest: **the panel shows the op count and the last bake time, and warns past a
threshold.** And a future `loom voxel compact <scene>` CLI could drop provably-redundant ops — a
subtract sphere wholly inside an earlier subtract sphere contributes nothing, and that is a
containment test, not a solver — printing what it removed so a human can read the diff. CLI-first,
agent-verifiable, and buildable in an afternoon when the op counts justify it. **Not now.**

---

## 11. Prefabs in the editor

Prefabs are the largest gap between built engine behaviour and reachable UI (engine-surface survey
§"gap list", item 1), and the first thing to fix is not a tool at all.

**Fix the load path before building any prefab UI.** `SceneView::build` calls `Scene::parse`
directly (`scene_view.rs:110`) while every other reader goes through `prefab_load::for_reading`.
ADR 0008 names this "the single most likely way to regress S4": the instance arrives with no
components, draws nothing, and validates clean. `loom run --edit assets/test/prefab_room.loom` is
that bug today. Any authoring tool built on top of an unresolved scene is authoring against a lie.

**Instancing a prefab is a drag from the prefab browser into the viewport**, and it needs one thing
the op vocabulary lacks: `SpawnNode` cannot make an instance. Adding an optional field is additive
and needs no new variant:

```rust
SpawnNode { parent, name, mesh: Option<String>, prefab: Option<String> }
```

`mesh` and `prefab` are mutually exclusive — an instance carries no components of its own
(`scene.rs:48-56`) — and the op refuses both being set. The drop is
`SpawnNode { prefab: Some(alias) }` + `SetTransform` at the cursor hit, one transaction.

**Editing an override needs no branch in any tool**, and that is ADR 0008's design working as
intended: `SetField` on a prefab instance routes to `set_override` automatically
(`ops.rs:700-707`), so the gizmo, the inspector and the sculpt tool all write overrides without
knowing what kind of node they hold. The *display* of override state belongs to the inspector
design; the authoring side needs only to not defeat it.

**Reverting is `RevertOverrides`, which already exists and the editor has never issued**
(`ops.rs:85-89`). Per-field revert passes one key; "revert all" passes none. One op, one
transaction.

**Creating a prefab from a selection is two files and therefore two steps, and the button must say
so.** A `[[prefab]]` declaration is `key`, `id`, `path` (`scene.rs:69-79`) — the body always lives
in another file. So:

1. Write `prefabs/<name>.loom` containing the selected sub-tree. **A file write, outside undo**
   (§14), through `loom_scene::edit::write_atomically` (`edit.rs:37-71`).
2. One transaction on the scene: `Declare { kind: Prefab, key, id, path }` (§13), then `RemoveNode`
   over the old sub-tree deepest-first, then `SpawnNode { prefab: Some(key) }` and a `SetTransform`
   to where it was.

Ctrl+Z restores the scene and leaves the file — exactly the shape ADR 0008 chose for
`apply-overrides`, which "reports `undo_steps: 2` rather than implying one"
(ADR 0008:54-60). The dialog says: **"Creates prefabs/Crate.loom and replaces 3 nodes. Undo restores
the scene; the file stays."** A comfortable lie here is how a user loses a sub-tree.

**The CLI half is `loom prefab create --from <node> --out <file>`**, and it must exist before the
button does. Property 2 — the agent can do everything the human can — is not satisfied by an
editor-only verb, and `loom prefab` already hosts the other three operations.

**A prefab library is keyed by `id`, never by alias** (`prefab.rs:28-52`, and CLAUDE.md says it
twice). The browser lists ids and shows aliases as file-local labels. Two scenes may spell one
alias differently and a browser keyed on the word would show them as one prefab.

---

## 12. The scripting workflow

`Script { path }` and `GameRules { path }` are both a project-relative string
(`components.rs:1674-1685`), which the current inspector renders read-only (`panels.rs:878-880`).
Three affordances close the gap and none of them is a code editor.

**The script slot.** A field widget with the path, a picker over `assets/scripts/**.rhai`, a **New**
button and an **Open** button. Setting it is `SetField { field: "Script.path", value: "..." }` —
one op, and on a prefab instance an override, for free.

**New writes a file from a template and does not pretend to be undoable.** `assets/scripts/` gains
`_template_behaviour.rhai` and `_template_rules.rhai`; New copies one, then issues the `SetField` in
a separate step. The file write is outside undo (§14) and the button says "Creates
assets/scripts/patrol.rhai".

**Open launches the user's editor** — `xdg-open`, or `$EDITOR` in a terminal. **Rejected: building a
code editor in egui.** Syntax highlighting means a new dependency and therefore ADR E; without
highlighting an in-window editor is worse than the one the user already has configured; and the hot
reload loop is already tight because `ScriptWatcher::changed` compares mtimes and treats a
first-seen file as changed so load and reload share one path (`loom_script/src/lib.rs:882-907`).
Save in the external editor, see it in the viewport.

**A plain in-window buffer is offered anyway, and it has one trap worth stating.** egui's
`TextEdit::multiline` costs nothing, is already available, and covers "change 3 to 5 without
alt-tabbing". But **a script file is not scene text, so Ctrl+Z in that buffer must be egui's text
undo and must never reach `Session::undo`.** The same keystroke meaning two things depending on
focus is a real hazard: the rule is that while the script buffer has keyboard focus the editor's
transaction shortcuts are suppressed — the same shape as the existing Tab fix, which un-consumes Tab
unless a text field specifically has focus (`ui.rs:164-173`, used at `run.rs:852`).

The buffer polls its file's mtime like the scene watcher does and, on an external change with
unsaved buffer edits, **raises the same two-button banner and merges nothing** — "Reload from disk"
or "Keep mine". Never-do #15's reasoning is about the scene, but the failure it describes is about
destroying a human's edits, and that generalises.

**Errors go to the console with file and line, and clicking one selects the node that owns the
script.** Rhai reports position on both compile and runtime failure. Today a script error is a
console line with no route back to the thing that caused it, and on a scene with twenty scripted
enemies that is the difference between a two-second fix and a hunt.

**The sandbox limits are not authored and should stay that way.** `loom_script::Limits`
(`lib.rs:29`) is an engine safety property, not a level-design parameter; exposing it in the editor
would invite raising it to make a bad script work.

---

## 13. What the op vocabulary needs, and the ADR

**Nine ops become eleven, plus one field.** That is a 22% growth in the write vocabulary of the
whole engine and it should not be waved through; each item below has a caller that exists today.

```rust
/// Edit an ordered array-valued component field in place.
///
/// `index: None` appends. `remove` counts elements dropped at `index` before
/// `insert` goes in. Deleting is `remove: 1, insert: []`; editing an element
/// is `remove: 1, insert: [new]`; reordering is two splices in one transaction.
///
/// The array-of-tables spelling is preserved when that is what is on disk, so
/// a sculpt stroke is N added lines in the diff rather than one rewritten line
/// of four thousand characters.
SpliceArray {
    node: String,
    field: String,            // "VoxelVolume.ops", "WaterBody.waves"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    #[serde(default)]
    remove: usize,
    #[serde(default)]
    insert: Vec<Value>,
},

/// Upsert a file-level declaration: `[[asset]]` or `[[prefab]]`.
///
/// The op vocabulary can write nodes and fields but nothing at file scope, so
/// importing a mesh and creating a prefab both fall outside undo without this.
Declare {
    kind: DeclKind,           // Asset | Prefab
    key: String,              // the file-local alias
    id: String,
    path: String,
},
```

and `SpawnNode` gains `#[serde(default, skip_serializing_if = "Option::is_none")] prefab: Option<String>`,
mutually exclusive with `mesh`.

**Callers that exist today, not imagined ones.** `SpliceArray`: the sculpt brush and its op-list
panel (§10), plus four array-of-object fields the inspector cannot edit at all — `WaterBody.waves`,
`Buoyancy.pontoons`, `Scatter.excludes` and the ground-layer list (engine-surface survey, "Weather,
vegetation, water"). `Declare`: prefab creation (§11) and mesh import, which is the asset panel's
whole reason to exist. `SpawnNode { prefab }`: the prefab browser drag-drop and the duplicate fix
(§8).

**Rejected alternatives.** Three named ops instead of splice — more variants, same behaviour, and
no answer for editing an element in place. A dotted field path (`SetField "VoxelVolume.ops.3.radius"`)
— `SetField` splits the field name once and uses the remainder as a literal TOML key
(`ops.rs:690`, `:750-770`), so this writes a key named `ops.3.radius` and produces a scene that
means something else entirely. Doing prefab creation and mesh import as CLI-only commands — they
would be the only authoring actions with no undo at all, and `RevertOverrides` and `UnpackPrefab`
are already ops, so the asymmetry would be arbitrary.

### ADR draft — "The op vocabulary grows for ordered arrays and file-level declarations"

> **Status:** proposed. **Supersedes:** nothing. **Relates to:** ADR 0008 (prefabs), never-do #11,
> never-do #16.
>
> **Decision.** `SceneOp` gains `SpliceArray` and `Declare`, and `SpawnNode` gains an optional
> `prefab` alias. The vocabulary goes from nine operations to eleven.
>
> **Because** three authoring actions have no expression in the current nine, and each of them
> otherwise lands outside undo, outside the diff and outside what `loom scene --tx` can express —
> breaking the property that a human and an agent making the same edit produce the same diff.
> Editing an array-valued field is only possible by replacing the whole array, which is verified to
> collapse `[[node.components.VoxelVolume.ops]]` into a single-line inline array
> (`ops.rs:1039-1052`) and to rewrite that line on every stroke, destroying the reviewability the
> op-list representation exists for. Nothing in the vocabulary writes `[[asset]]` or `[[prefab]]`,
> so mesh import and prefab creation cannot be transactions. And `SpawnNode` cannot create a prefab
> instance, so the editor cannot place one.
>
> **Consequences.** `loom scene --tx` gains three capabilities and the agent gains them with it.
> `docs/format/README.md` needs no change — no field name, type, default, addressing or override
> syntax moves, so no `format` bump and no migration. `SpliceArray`'s index is fragile against a
> concurrent write, which the version token already rejects; within one transaction the editor must
> apply removals descending, the same way Delete already sorts deepest-first. The inspector's
> array-of-object fields become editable, which was blocked on this. Two ops is the smallest
> vocabulary that covers the three cases; a splice was chosen over separate append/remove/replace
> because it is one variant instead of three and reordering falls out of it.

**A second, unrelated fix belongs in the same commit series and is not an ADR:** `SetTransform`
must emit `f32` values through the shortest-round-trip path `prefab.rs:186` already uses, instead of
`f64::from` (`ops.rs:680`). It is a defect against `docs/format/README.md`'s "the authored value is
the source of truth", it is three lines, and grid snapping's defaults depend on it (§7).

---

## 14. What is not a `SceneOp` — the exemption list

Constraint survey §4.J asked the design phase to produce this list. **Here it is for authoring, in
full.** Everything not on it must be an op.

**Genuinely not authored state — ephemeral, discarded on reload:**

1. Selection, and the isolate/hidden sets (§5). Hiding is editorial, not a game property, and
   writing it would bloat every scene. Mitigated by a persistent "3 objects hidden" bar.
2. Camera position, tool mode, snap settings, grid visibility. `LOOM-IMPLEMENTATION-ORDER.md:459`
   already requires camera and selection to persist *outside* scene state.
3. The sculpt live preview (`Volume::edit`) and "preview to op N" (§10). Derived from the op list,
   never the source of it.
4. Marquee rubber band, gizmo hover, drag-in-progress state.

**A second file, and therefore a second step the UI must name:**

5. Writing a prefab `.loom` file when creating a prefab (§11). Two steps, stated in the dialog.
6. Creating or editing a `.rhai` script (§12). The file write is outside undo; the `SetField` that
   points at it is not.
7. Copying an imported mesh into the project. The `Declare` that names it **is** an op, so the
   scene half is undoable and the file half is not.

Items 5–7 all share one honest framing borrowed from ADR 0008: **say the step count out loud.**
"Undo restores the scene; the file stays" is the sentence, and every button that writes a second
file carries it.

**Deliberately deferred to a doc that owns them:** preview-before-commit for large transactions.
Note for whoever builds it that `Transaction::dry_run` (`ops.rs:107-109`) is honoured by the CLI and
**not** by `Session` — `commit` calls `ops::apply` and mutates regardless (`edit.rs:298-311`). An
in-editor preview must call `loom_scene::ops::apply` on the session's text directly and show
`Applied::diff` without committing.

---

## 15. Verification plan

Defined before building, per the operating rules. Every item is a command or an observation, not
"it should work".

**Unit tests, `cargo test --workspace`:**

- `SpliceArray` preserves the array-of-tables spelling: sculpt three ops into a scene written with
  `[[node.components.VoxelVolume.ops]]` and assert the output still contains that header and gains
  exactly three tables. This is the test that makes §10.2's argument enforceable.
- `SpliceArray` round-trips: append, then delete the same index, then assert byte-identical text.
- A snapped `SetTransform` writes `0.1`, not `0.10000000149011612` — the `SetTransform` twin of
  `unpacking_writes_the_authored_numbers_not_widened_ones` (`ops.rs:2208`).
- Group compensates: build a scene with a translated, rotated parent, group two children under a new
  node, assert every child's **world** position is unchanged within 1e-4.
- Sculpt coordinates: a stamp at a cursor hit on a translated and yawed volume produces an op whose
  `center` bakes material at the hit point.
- Preview equals truth (§10.5): `bake(ops)` versus `bake(ops[..1])` then `edit` for the rest,
  bit-identical fields. **If this test fails, the incremental preview is wrong and must be
  replaced with a re-bake on release.**
- Marquee: a node projected fully inside the rect is selected, one fully outside is not, one behind
  the camera is not.

**The M12 criteria, which must keep passing:** `edit.rs:457` (a twelve-op transaction is one
Ctrl+Z), `edit.rs:498` and `:514` (one gesture is one undo step, two gestures are two). Every new
gesture key — `sculpt:{node}:{epoch}`, the multi-select gizmo's, the array-element scrub's — gets a
test in that same shape, because the epoch discipline (`run.rs:898`) is easy to lose and silent when
lost.

**Hand-and-agent parity, which is the M12 exit criterion (`LOOM-BUILD-BRIEF.md:285`):** for each of
create-cube, drop-on-surface, distribute and sculpt-one-stamp, perform it in the editor and perform
the equivalent `loom scene --tx` / `loom place --op` from the shell, and diff the two resulting
files. They must be identical. Align, drop and array go through `place::resolve` precisely so this
holds by construction rather than by luck.

**`cargo xtask validate`:** the windowed half drives `loom run --frames n`, so the new tool layer
must not break `--frames` or `--play` (existing survey §14, item 12). A docked viewport that
resizes its offscreen image is a validation-message generator until it is right, and zero messages
is the gate.

**`cargo xtask image`:** authoring tools add no rendering path — gizmos and the brush cursor are egui
overlays, the sculpt preview goes through the existing terrain upload. **No new golden scene is
owed by this document.** The sculpt work does owe a scene to `SCENES`: a `.loom` whose voxel volume
was produced by a sculpt session, so the op-list path is exercised by something other than
hand-written ops.

**Not automatable, and stated as such:** whether the gizmo feels attached rather than geared, and
whether the sculpt list is comprehensible at forty ops. Those get a session with the human at the
end of each slice, and no gate can substitute.

---

## 16. What I could not verify

Stated plainly, because an unmarked guess is worse than an admitted gap. Nothing below was checked
by building — the design phase forbids it, and two of these need a run rather than a read.

1. **Whether `Scene::parse` accepts `VoxelVolume.ops` written as an inline array as well as
   `[[...ops]]` array-of-tables.** `ops` is `Vec<serde_json::Value>` and the TOML-to-JSON conversion
   should handle both, but I did not read the parse path for this field and `SpliceArray` must
   preserve whichever spelling is on disk. **Check before implementing the op.**
2. **Exactly what `toml_edit` prints for `f64::from(0.1_f32)`.** I verified that `SetTransform`
   widens (`ops.rs:680`) and that `prefab.rs:180-190` exists to avoid exactly that, and the
   prefab code's own doc comment supplies `1.399999976158142` as the observed result for `1.4`. I am
   inferring the same class of output for other non-binary fractions rather than having seen it.
   The fix is right either way; the *severity* is what I have not measured.
3. **The cost of re-baking a voxel volume during a sculpt stroke.** `Volume::edit` returns dirty
   chunks and `loom explode` remeshes exactly that way, so the mechanism is proven — but I have no
   number for a stroke at interactive rates on a realistic volume, and §10.5's incremental design
   rests on it. This is the measurement to take first in the sculpt slice, before the UI.
4. **Whether incremental `edit` and full `bake` actually agree bit-for-bit.** §10.5 proposes the
   test; I did not read `Volume::bake` and `Volume::edit` closely enough to predict the answer, and
   `bake` may do setup that per-op `edit` does not.
5. **Whether `SpliceArray` can write a nested `[[node.components.VoxelVolume.ops]]` array-of-tables
   through `toml_edit` while the enclosing `[[node]]` is itself an array-of-tables entry.** TOML
   permits the nesting and `toml_edit` models it as `Item::ArrayOfTables` inside the node's table,
   but I have not written the code and this is the one place the op could turn out to be harder
   than it reads.
6. **Whether egui's `TextEdit::multiline` undo is genuinely independent of the editor's shortcut
   handling** in the way §12 requires, or whether suppressing Ctrl+Z while it has focus needs more
   than the existing `wants_text_input` check.
7. **The frame cost of projecting every node's eight AABB corners per frame for the marquee.** At a
   few hundred nodes it is trivial; `LOOM-IMPLEMENTATION-ORDER.md:571` names egui frame-budget
   collapse on large scenes as a real risk, and the mitigation there is virtualization, which does
   not help a full-scene projection. Project only during an active marquee drag, and measure if a
   scene ever gets large enough to notice.
8. **Whether `place::resolve`'s `AlignTo` is what a user means by "distribute".** It spaces nodes at
   a fixed pitch centred on their own midpoint (`place.rs:199-238`), which is *array* semantics;
   "distribute evenly between the two outermost" is a different function. The tests confirm the
   former. If the latter is wanted it is a new `PlaceOp` variant, not an editor-side workaround.
