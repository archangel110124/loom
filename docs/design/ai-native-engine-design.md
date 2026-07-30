# Designing an AI-Native Game Engine

**Research findings from Unity, Unreal, and Godot — and a concrete implementation plan in Rust**

Working codename: **Loom**. Rename it whatever you like; the name appears in crate paths below.

---

## 0. Executive summary

The goal is an engine where an AI agent is a first-class author: it has full structural
context on the project, and it can build scenes, place prefabs, and wire behavior itself.

The research conclusion is short and slightly surprising: **the AI capability is almost
entirely determined by boring architectural decisions made in the engine's data layer.**
Not by the model, not by the prompt, not by the tool count. Godot — the smallest and
simplest of the three engines — is the one agents work best in today, purely because its
scenes are text files. Unreal has vastly more capability exposed and agents struggle in it,
because its assets are opaque binaries.

So the design below optimizes for one property above all others: **every authored thing is
diffable text, schema-validated on load, and describable back to the agent.**

### Decisions at a glance

| Concern | Decision | Borrowed from |
| --- | --- | --- |
| Type metadata | Derive-macro reflection into a runtime type registry | Unreal (`UPROPERTY`/UHT) |
| Authoring model | Node tree, composition over inheritance | Godot |
| Runtime storage | Archetype ECS under the tree | Bevy / Unity DOTS |
| Scene format | Text, TOML-flavored, defaults omitted, stable UUIDs | Godot `.tscn` |
| Prefab instancing | Source reference + explicit override deltas | Unity `PrefabInstance` |
| Asset identity | Content-addressed UUID in sidecar `.meta` | Unity `.meta` GUIDs |
| Project knowledge | SQLite graph, derived cache, never source of truth | (new) |
| Agent interface | MCP server, catalog-mode tools, transactional | Prior art consensus |
| 3D placement | Semantic verbs (`place_on`, `align_to`), never raw coordinates | (new — see §2.8) |
| Agent-authored behavior | Sandboxed Rhai scripts as hot-reloadable data | (new — see §2.12) |
| Verification | Headless render → image, **and** deterministic headless sim | Blender MCP + Unreal PIE |
| Code iteration | Data hot-reloads always; code via host/worker dylib split | Rust gamedev practice |

---

## Part 1 — How the three engines actually work

### 1.1 Unity: GameObject + Component, and a GUID-shaped serialization problem

Unity's authoring model is a flat-ish hierarchy of GameObjects, each a bag of Components.
Nothing exotic. What matters for us is the **serialization layer**, because that's the layer
an agent has to touch.

Unity serializes scenes (`.unity`) and prefabs (`.prefab`) as YAML — but only if you set
asset serialization to Force Text; binary is the default because it keeps the project
lighter. Every asset carries a GUID defined in a sidecar `.meta` file living next to the
original with the same name plus `.meta`. Cross-asset references are `{fileID, guid, type}`
triples. Scripts are no exception: a MonoBehaviour's YAML type is always just
`MonoBehaviour`, and the actual script is referenced by the GUID of the script's `.meta`
file — the script is treated as just another asset.

Two consequences worth stealing and one worth avoiding.

**Steal: the prefab override model.** When a prefab is instanced in a scene, Unity does not
serialize the prefab's GameObjects and components into the consuming file. Instead it adds a
`PrefabInstance` object with two key properties: `m_SourcePrefab` (the reference) and
`m_Modifications` (the deltas). Referenced objects inside the instance appear as *stripped*
placeholders — a header marked `stripped` carrying only enough identity to resolve the
source. This is the correct design. It means "the enemy prefab got a new collider" propagates
to 400 placed enemies, and a per-instance patrol radius survives.

**Steal: sidecar identity.** Content moves, gets renamed, changes file extension. A stable
UUID in a sidecar file survives all of it. Unity's own docs note you can fix lost references
by transplanting the old GUID into the new asset's meta file — the identity is the meta, not
the path.

**Avoid: the density.** Unity's docs are blunt that asset files aren't designed for manual
modification and won't produce helpful errors when you break them. And practitioners have
measured the agent consequence: a simple scene with a few GameObjects produces hundreds of
lines of dense, ID-heavy YAML, so an agent editing Unity scenes directly has to produce valid
GUID references, serialized MonoBehaviour data, and correctly linked meta files. That's a
high bar to clear on every edit.

There's a second Unity lesson buried in their runtime-serialization package: the Asset
Database only exists in the Editor, so player builds have no way to resolve an asset from a
GUID and need pre-baked AssetPacks mapping GUID/fileID pairs to references. **Design
implication: decide up front whether your UUID→asset resolution works at runtime, or you
will bolt on an ugly parallel system later.** We resolve this in §2.6.

### 1.2 Unreal: reflection is the load-bearing wall

Unreal's architecture is a class hierarchy: `UObject` at the base, `AActor` for anything
placeable in a level, `UActorComponent` for modular capability. An Actor deliberately does
not store transform or mesh data directly; it's a container that holds Components which
define its physical presence.

But the thing that actually makes Unreal *work* — the editor, Blueprints, serialization,
networking, GC — is the **reflection system**, and this is the single most important idea to
port.

In ordinary C++, type information is a compile-time affair. Unreal recovers runtime
introspection with the `UCLASS`, `USTRUCT`, `UPROPERTY`, `UFUNCTION`, and `UENUM` macros,
which annotate types and members in headers and can carry additional specifier keywords.
UnrealHeaderTool scans the codebase before compilation and generates reflection metadata,
producing a `UClass` object per class that holds full type information available at runtime.
The property system is itself a type hierarchy — `UField` → `UStruct` → `UClass`,
`UScriptStruct`, `UFunction`, `UEnum`, `UProperty` — and you can retrieve a type with
`UTypeName::StaticClass()` or an instance's type with `Instance->GetClass()`.

Marking a member `UPROPERTY` is what exposes it to reflection, introspection, garbage
collection, serialization, *and* Blueprint access simultaneously. One annotation, five
subsystems. Members left unmarked are invisible to all of them.

The GC and safety story rides on the same rails: any UObject not reachable in the reference
graph is collected, objects are retained by being a UProperty or living in an engine
container like `TArray`, and when an Actor or Component is destroyed, all references visible
to the reflection system are automatically nulled to prevent dangling pointers.

Editor tooling also rides on it. Unreal's own guidance is that `set_editor_property` is
preferred over direct assignment because it goes through the property system — firing
on-property-changed events and flagging the object dirty. **That is exactly the interface an
agent needs**, and it exists only because reflection exists.

Where Unreal hurts an agent: **assets are binary.** A `.uasset` can't be diffed, merged, or
hand-repaired the way a text file can, which raises the stakes of every mutation. Unreal
also has a real transaction system — the undo stack — and an automation layer that ignores it
is genuinely dangerous.

### 1.3 Godot: the scene tree, and the format that accidentally won the AI era

Godot's model is nodes in a tree. Every element — sprite, camera, collision shape, UI
control — is a node of a specific type, and node types form an inheritance chain
(`Sprite2D` → `Node2D` → `CanvasItem` → `Node`) so a node inherits its ancestors' properties
and methods. Spatial nodes inherit transform from their parent, so moving a parent moves
everything under it.

Godot's own design philosophy doc is careful to say **nodes are not components**: nodes are
part of a tree and always inherit from their parents up to `Node`, and most nodes work
independently of one another. The composition happens at the *scene* level, not by bolting
components onto an entity.

And a scene is just a tree of nodes saved to a file, which can be instanced inside another
scene exactly like adding a single node. Godot's docs draw the distinction from prefabs
explicitly: unlike prefabs in other 3D engines, you can **inherit from and extend** scenes —
create a `Magician` that extends your `Character`, modify `Character` in the editor, and
`Magician` updates too. The architecture encourages structure that mirrors the game's design
rather than an imposed pattern like MVC.

The `.tscn` format is where the AI story lives. It's an INI-like text format with five
ordered sections: file descriptor, external resources, internal resources, nodes, and
connections. The descriptor is a single line like `[gd_scene load_steps=1 format=2]`.
Headings look like `[<resource_type> key=value ...]`, followed by zero or more `key = value`
pairs whose values can be complex types — Arrays, Transforms, Colors. Nodes are declared with
`[node name="Cube" type="Spatial" parent="."]`; the first node has no `parent` entry and is
the scene root, other nodes give a path relative to (but excluding) the root, and `"."` means
direct child. Exactly one root is required or import fails. Resources are the data that make
up nodes — a `MeshInstance3D` has an accompanying `ArrayMesh` resource, which may be internal
or external to the file. External resources are referenced with `res://`-prefixed paths.

Two details show real format maturity: **properties equal to their default value are not
stored at all**, keeping files compact; and TSCN is compiled to binary `.scn` in
`.godot/imported/` on import, so the text format costs nothing at load time.

One honest caveat from Godot's own contributor docs: documentation on sub-resource formats is
largely absent, and some of it can only be discovered by reading engine source. A format is a
contract; if you don't document it, you don't have one.

### 1.4 Bevy: the Rust ECS reference implementation

Bevy is archetype-based, like Flecs, Unity DOTS, and Unreal Mass. An archetype is a unique
combination of components, and a world holds exactly one archetype per unique combination;
spawning a single entity creates or updates an archetype, and adding a component *moves* the
entity to a different archetype. Storage groups entities by component composition so
iteration walks contiguous arrays — a hundred enemies have their `Position`, `Speed`, and
`Target` laid out contiguously, so a query over `Position` + `Velocity` is an array walk with
no virtual dispatch and few cache misses.

Bevy runs a hybrid storage model: Tables for fast iteration, SparseSets for components added
and removed frequently. If every component in a query is table-stored it takes a dense path;
otherwise it goes through archetype indirection. Entities use a generational index allocator,
and archetype indices themselves carry a generation so query caches can be invalidated
correctly.

The part that matters most for us is the scheduler. Because component access is declared in
the type system, the borrow checker proves at compile time that two systems can't mutate the
same component simultaneously, and Bevy's scheduler exploits that to parallelize
automatically — two systems touching disjoint components just run in parallel. It even warns
about *ambiguous* system pairs that conflict on data access with no defined ordering.

A useful reality check from a mid-2026 survey: Bevy is the open-source star of Rust gamedev
but shipped commercial titles count on two hands, while Unity still leads mobile/indie/VR,
Unreal leads AAA, and Godot owns a strong slice of indie 2D. Building your own is a
learning-and-control decision, not a gap-filling one.

### 1.5 What the agent-tooling ecosystem has already learned

This is the most directly applicable research, because people have spent two years failing at
exactly your problem in existing engines. Agent editing is settled infrastructure now, not a
hack: MCP SDKs were around 97 million monthly downloads by March 2026, the protocol sits under
the Linux Foundation's Agentic AI Foundation, and Epic shipped an experimental first-party MCP
plugin in UE 5.8.

Four hard-won lessons:

**1. Text scenes are the whole ballgame.** Godot is described as the sleeper precisely because
text-based `.tscn` scenes let an agent meaningfully edit a project *without the editor even
running* — something neither Unreal nor Blender can offer. A server with an offline `.tscn`
parser can read, diff, and reason about scenes file-side. Scaffolding entire game skeletons is
where Godot agents shine, because the whole result is inspectable text. And text files plus git
make everything trivially reviewable and revertible.

**2. A blind agent fails, no matter how good the tools are.** Blender is the *easiest* engine
to automate — `bpy` covers essentially the whole application — and it's still where agents fail
most, because 3D work fails silently without eyes. An agent can execute fifty geometrically
valid operations and produce a mesh that looks like a melted shopping cart, with every tool call
returning success. Text read-back of object names, vertex counts, and transforms does not tell
you the chair looks like a chair. The conclusion the ecosystem reached: a **vision feedback
loop** — capture the viewport, render a preview, hand the image back to the model — is the
single most important capability, because it converts "plausible operations" into "verified
results."

**3. Closed-loop read-back catches the agent's own mistakes.** In Unreal, an agent that can
call something like `describe_graph` to read a Blueprint back after editing catches its own
errors; one that can't is editing blind. Same principle, cheaper channel.

**4. Tool count is a liability, not a feature.** A server exposing hundreds of tools naively
dumps tens of thousands of tokens of schema into every session before the agent does anything.
The mitigation in production is *catalog mode*, which cuts a fresh session's tool-definition
cost by roughly 95%. Related: multi-step programs should execute as **one editor transaction
with one undo step**, and tools should carry graduated permission scopes (read / scene /
destructive). Long operations need background jobs with progress streaming, or the agent just
stalls.

One more small but telling detail: smart type coercion — accepting `Vector2(100, 200)`,
`[100, 200]`, and `{"x": 100, "y": 200}` interchangeably — eliminates a whole class of agent
retry loops. Be liberal in what you accept.

---

## Part 2 — The implementation plan

### 2.1 The keystone: reflection via derive macros

Unreal needed an entire external preprocessor to get runtime type metadata into C++. In Rust
you get it from a proc macro, and this is the biggest single reason Rust is right for this
project.

One derive generates everything downstream:

```rust
#[derive(Component, Reflect)]
#[loom(doc = "Damages entities that overlap it")]
pub struct DamageZone {
    #[loom(range = 0.0..=1000.0, doc = "Damage per second applied on overlap")]
    pub dps: f32,

    #[loom(default = "Layer::Enemy", doc = "Which collision layer is affected")]
    pub target_layer: Layer,

    #[loom(asset, doc = "VFX spawned on first contact")]
    pub hit_effect: Option<AssetId<Effect>>,
}
```

From that single declaration, the macro emits:

1. `serde::Serialize` / `Deserialize` — so the scene format is derived, not hand-maintained
2. A **JSON Schema** entry — so the agent's tool API is derived, not hand-maintained
3. An **inspector widget** — a slider bounded by `range`, an enum dropdown, an asset picker
4. A **type registry entry** at startup — `TypeId` → name, fields, docs, defaults, constraints
5. A **doc string** the agent can query — `describe_component("DamageZone")` returns real text

The registry is Unreal's `UClass` table, built at compile time instead of by a code generator.
Registration is a one-liner per type in a plugin's `build()`.

**The rule this creates: the agent's API surface is never written by hand.** Add a component,
and the agent can immediately place it, set its fields, be told what the fields mean, and be
prevented from writing an out-of-range value. Every hand-maintained agent tool is a tool that
will drift out of sync with the engine by Thursday.

The `range` and `default` attributes do double duty as **validation** — see §2.9.

### 2.2 Authoring model: node tree over ECS storage

This is the one place the three engines genuinely disagree, and both sides are right about
different things.

- Godot's nodes are explicitly *not* components; they're a tree, and composition happens at
  scene granularity.
- Bevy/DOTS composition is components on a flat entity, with archetype storage for speed.

Godot's tree is better for **authoring and for agent reasoning** — hierarchy is how humans
and models describe scenes ("put the lamp on the desk in the office"), transform inheritance
falls out naturally, and a tree serializes to readable text. ECS is better for **runtime**.

**Loom does both, in layers:**

```
Authoring layer:  Node tree — named, hierarchical, parent-relative transforms
                  ↓ (flattened on load)
Runtime layer:    Archetype ECS — entities, components, parallel systems
```

A node is an entity plus a `Name`, a `Parent`, and a `Children` list. The tree is real, but it
is *component data*, not a separate object graph. Transform propagation is a system that walks
hierarchy depth-first and writes `GlobalTransform` from `LocalTransform`. This is roughly what
Bevy already does, and it's the right compromise: authors and agents see Godot, the CPU sees
DOTS.

Composition is per-node (components attached to a node), because forcing Godot's
"one capability per node" model on an agent multiplies node counts and makes hierarchies
deeper for no benefit.

### 2.3 Scene format: `.loom` — text, sparse, stable

Godot's format, tightened up, with the documentation gap fixed. TOML-flavored because Rust
already parses it well and it's less whitespace-brittle than YAML for machine editing.

```toml
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

# ── external references ─────────────────────────────
[[asset]]
key = "office_desk"
id  = "a41f...c9"          # resolved via the asset DB, never by path
path = "assets/props/desk.glb"   # informational only; a hint for humans

[[prefab]]
key = "lamp"
id  = "7b22...41"
path = "prefabs/desk_lamp.loom"

# ── node tree ───────────────────────────────────────
[[node]]
name = "Office"
# no parent field == scene root; exactly one node may omit it

[[node]]
name   = "Desk"
parent = "Office"
transform = { pos = [0.0, 0.0, -2.5], rot_euler = [0.0, 90.0, 0.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "office_desk" }

  [node.components.BoxCollider]
  half_extents = [0.8, 0.37, 0.4]

# ── prefab instance with override deltas ────────────
[[node]]
name   = "DeskLamp"
parent = "Office/Desk"
prefab = "lamp"
transform = { pos = [0.55, 0.74, 0.1] }

  [node.overrides]
  "Light.intensity"   = 420.0
  "Light.color"       = [1.0, 0.92, 0.78]
  "Flicker.enabled"   = true
```

Format rules, all deliberate and all documented in the repo from commit one:

- **Defaults are omitted on write.** Straight from Godot: properties equal to default aren't
  stored. Diffs then show intent, not noise — which is exactly what you need when reviewing an
  agent's commit.
- **Nodes are addressed by path** (`Office/Desk`), not by index. Paths are stable under
  reordering; indices aren't. Sibling names must be unique — enforce on write with a clear
  error, don't silently rename.
- **Assets and prefabs are referenced by UUID with a local `key` alias.** The agent writes
  `asset = "office_desk"`, never a raw UUID and never a path. This is the single biggest
  usability win over Unity's `{fileID, guid, type}` triples.
- **Overrides are a flat dotted map**, per §2.4.
- **Exactly one root.** Godot's rule; it makes instancing composable.
- Scenes may be instanced inside scenes, recursively, with cycle detection at load.

Text is the shipping format too — no separate binary. If load time ever becomes a real problem,
add a build-step cache the way Godot compiles `.tscn` to `.scn`; don't pay that complexity
before you have the problem.

### 2.4 Prefab instancing: Unity's delta model, Godot's inheritance

Straight port of `PrefabInstance`: the consuming file stores a **source reference plus explicit
modifications**, never a copy of the prefab's contents. Change the prefab, every instance
updates; per-instance tweaks survive.

The dotted-path override syntax (`"Light.intensity"`) is deliberately agent-friendly: it's a
flat key-value map, so setting an override is one operation with no tree surgery, and the
schema from §2.1 validates the value.

Also adopt Godot's **scene inheritance**: a prefab can declare `extends = "character.loom"`,
add nodes and components, and stay live-linked to its base — so editing `Character` updates
`Magician`. This is more useful than Unity prefab variants because the extension is a full
scene, not a diff blob.

Three operations the editor and the agent both need, first-class:

| Operation | Meaning |
| --- | --- |
| `apply_overrides` | Push instance changes back into the prefab |
| `revert_overrides` | Drop instance changes, snap back to prefab |
| `unpack` | Break the link, inline the contents, become plain nodes |

Rules to hold the line on: an override targeting a path that no longer exists in the prefab is
a **loud warning with the orphaned value preserved in the file**, never a silent drop. Unity's
handling of this is a known pain point; don't reproduce it.

### 2.5 Runtime: archetype ECS, borrowed wholesale

No innovation needed here — Bevy's design is correct and well-documented.

- Archetype storage: entities grouped by component composition, contiguous arrays per archetype
- Hybrid Table/SparseSet, because tag-ish components churn and dense components iterate
- Generational entity indices, so a stale `EntityId` fails loudly instead of aliasing
- Systems declare component access in their signature; the scheduler derives the parallel
  execution graph from that and warns on ambiguous ordering

The one deviation worth making for this project: **make change detection a first-class,
queryable thing the agent can read.** Bevy already has `Added<T>` / `Changed<T>` with tick-based
tracking. Expose it as a tool: `what_changed_since(tick)` gives the agent a diff of the running
world, which is far better feedback than re-reading the whole scene.

### 2.6 Assets: content-addressed, with a resolvable runtime index

Unity's `.meta` sidecar idea is right; its Editor-only Asset Database is the trap. Solve it once,
at the start.

- Every imported asset gets `<file>.meta` containing a UUID, an import config, and a content
  hash
- The UUID is the only identity that appears in scene files; paths are advisory only
- The import pipeline produces normalized runtime formats (glTF → mesh buffers, PNG → GPU
  texture) in a cache directory keyed by content hash, so re-import is skipped when the hash
  matches
- **A `manifest.bin` mapping UUID → cached artifact is generated on every build and shipped
  with the game.** This is Unity's AssetPack, but automatic and non-optional, so runtime
  resolution works identically in editor and in a shipped build. No parallel system, no
  surprise at ship time.
- Missing asset = a magenta placeholder plus a warning, never a panic. An agent working
  against a half-imported project should degrade, not crash.

### 2.7 The knowledge graph: SQLite, derived, disposable

Your Obsidian-graph instinct maps onto a real need, with one correction: separate the
**visualization** from the **index**.

The index is a SQLite database, derived entirely from files on disk:

```sql
CREATE TABLE node (
  id          TEXT PRIMARY KEY,   -- uuid
  kind        TEXT NOT NULL,      -- scene|prefab|asset|component_type|script|system
  name        TEXT NOT NULL,
  path        TEXT,
  content_hash TEXT
);

CREATE TABLE edge (
  src   TEXT NOT NULL REFERENCES node(id),
  dst   TEXT NOT NULL REFERENCES node(id),
  kind  TEXT NOT NULL,   -- instantiates|child_of|references_asset|extends
                         -- attaches|reads_component|writes_component|emits|listens
  meta  TEXT             -- json: node path, field name, etc.
);

CREATE INDEX edge_src ON edge(src, kind);
CREATE INDEX edge_dst ON edge(dst, kind);
```

Three properties that matter more than the schema:

1. **It is a cache, not a source of truth.** Delete it, rebuild from files in seconds. The
   moment the graph can hold state the files can't, you've built a second database to keep in
   sync and you will lose.
2. **It's rebuilt incrementally by a file watcher.** Touch one scene, re-index one scene.
3. **It answers the questions context windows can't hold.** This is the actual point:

```sql
-- What breaks if I change this prefab?
SELECT DISTINCT n.path FROM edge e JOIN node n ON n.id = e.src
WHERE e.dst = :prefab_id AND e.kind IN ('instantiates','extends');

-- Which assets does nothing reference? (agent cleanup task)
SELECT n.path FROM node n WHERE n.kind='asset'
  AND NOT EXISTS (SELECT 1 FROM edge WHERE dst = n.id);

-- Two-hop neighborhood: the context pack for "work on the office scene"
WITH RECURSIVE nb(id, depth) AS (
  SELECT :scene_id, 0
  UNION SELECT e.dst, nb.depth+1 FROM edge e JOIN nb ON e.src = nb.id WHERE nb.depth < 2
) SELECT DISTINCT n.* FROM node n JOIN nb ON n.id = nb.id;
```

**This is the answer to "full context."** Full context does not fit in a context window and
never will. What the agent gets is *retrieval*: the two-hop neighborhood of whatever it's
touching, assembled on demand. That's a strictly better outcome than a giant dump, because it
also works when the project is 4,000 files.

The force-directed visualization sits on top of the same tables and is **for you** — spotting
orphaned assets, circular prefab dependencies, a scene that references half the project. Build
it second; it's a view, not infrastructure.

### 2.8 The agent interface: MCP, catalog-mode, transactional

Since the near-term target is Claude Code, expose the engine as an **MCP server**. That's the
settled interface, and it means the same server later serves a local model with no rework.

**Tool design, informed directly by what the ecosystem got wrong.**

Do *not* ship 300 flat tools. Use the catalog pattern: a handful of always-present tools, with
the rest fetched on demand.

```
Always loaded (10 tools):
  scene_query        — read the tree; supports path globs and component filters
  scene_edit         — batched mutations, one transaction (see below)
  scene_place        — semantic placement ops (see below) — the 3D workhorse
  scene_measure      — bounds, raycast, overlap tests; numeric self-verification
  describe_type      — schema + docs for a component type, from the registry
  list_types         — component types by category, names and one-liners only
  graph_query        — the SQL questions in §2.7, as named parameterized queries
  render_preview     — the eyes (§2.10)
  run_scene          — headless play, N ticks, returns assertions + trace
  tool_catalog       — fetch full schemas for a named tool group on demand
```

**Semantic placement is the most important 3D decision in this document.** Do not make the
agent compute world coordinates. It will get them wrong constantly, and that is precisely the
failure that puts a monitor floating 30cm above a desk. Give it geometry-aware verbs and let
*engine* code do the arithmetic:

```rust
pub enum PlaceOp {
    /// Raycast down onto the target's upper surface; align by AABB face.
    PlaceOn   { node: NodePath, surface: NodePath, anchor: Anchor2 },
    /// Distribute nodes along an axis with fixed spacing or fixed span.
    AlignTo   { nodes: Vec<NodePath>, axis: Axis3, spacing: Spacing },
    /// Flush one node's face against another's.
    SnapTo    { node: NodePath, target: NodePath, face: Face },
    /// Yaw the node so its forward vector points at the target.
    FaceToward{ node: NodePath, target: NodePath, up: Axis3 },
    /// Lay out on a grid in the surface's local frame.
    GridOn    { prefab: PrefabRef, surface: NodePath, rows: u32, cols: u32, pitch: Vec2 },
}
```

"Six desks in two rows with a monitor on each" is then four calls that are **correct by
construction**, instead of forty coordinate writes that need visual correction afterward. The
burden moves from the model's spatial reasoning — its weakest faculty — onto deterministic
geometry code, which is your strongest.

`scene_measure` is the cheap companion: `query_bounds`, `raycast`, `check_overlaps`,
`distance_between`. It lets the agent verify numerically before spending a render, and it
catches interpenetration that a single camera angle would hide.

**Every mutation is a transaction with one undo step.** This is non-negotiable — Unreal's
transaction system exists for a reason, and an automation layer that ignores the undo stack is
dangerous. A twelve-step blockout is one entry in the undo stack and one revert.

```rust
pub struct Transaction {
    pub label: String,          // "Block out office: 14 nodes"
    pub ops: Vec<SceneOp>,
    pub dry_run: bool,
}

pub enum SceneOp {
    SpawnNode  { parent: NodePath, name: String, prefab: Option<PrefabRef> },
    Reparent   { node: NodePath, new_parent: NodePath },
    SetTransform { node: NodePath, transform: TransformPatch },
    AttachComponent { node: NodePath, type_name: String, value: Value },
    SetField   { node: NodePath, field: DottedPath, value: Value },
    SetOverride { node: NodePath, field: DottedPath, value: Value },
    RemoveNode { node: NodePath },
}
```

Three properties on top:

- **`dry_run`** validates and returns the diff it *would* write, touching nothing. Cheap way
  for the agent to check itself before committing.
- **Scopes** on the token/session: `read` / `scene` / `destructive`. `RemoveNode` and asset
  deletion need `destructive`, and it isn't on by default.
- **Liberal input coercion.** Accept `[0,1,0]`, `{"x":0,"y":1,"z":0}`, and
  `"Vec3(0,1,0)"` for the same vector. This single decision removes an entire category of
  retry loops.

Long operations (import, bake, headless test runs) go through a job queue with progress
streaming, so the agent gets partial results instead of blocking.

### 2.9 Validation: schema at the boundary, plus a hook net

Two layers, and they're cheap because §2.1 already generated the schemas.

**Layer 1 — the type system is the validator.** Every scene load and every `scene_edit` op
deserializes into concrete Rust types. Malformed data cannot become a live object because the
type won't construct. Range and enum constraints from the derive attributes are checked at the
same boundary. Rejections return **structured, actionable errors**:

```json
{
  "error": "field_out_of_range",
  "node": "Office/Desk/DeskLamp",
  "field": "Light.intensity",
  "value": 40000.0,
  "constraint": "0.0..=10000.0",
  "hint": "Interior lights are typically 100-800. Did you mean 400?"
}
```

That `hint` field is doing real work. A rejection message is the agent's teacher, and a good
one converts a retry loop into a single correction.

**Layer 2 — a Claude Code hook as the outer net.** Wire a `PostToolUse` hook matched on
`Write|Edit` that runs `loom validate --staged`. A hook that exits 2 blocks and feeds its
stderr back to Claude as an error message; with `continueOnBlock: true` on `PostToolUse`, the
rejection surfaces as a tool result the model can read and retry from. Net effect: Claude Code
physically cannot leave a malformed scene file on disk.

```json
{
  "hooks": {
    "PostToolUse": [{
      "matcher": "Write|Edit",
      "hooks": [{
        "type": "command",
        "command": "$CLAUDE_PROJECT_DIR/.loom/validate.sh",
        "continueOnBlock": true
      }]
    }]
  }
}
```

For a local model later, this is where **grammar-constrained decoding** slots in — GBNF or
XGrammar generated from the same JSON Schemas, masking invalid tokens during generation so
malformed output is impossible rather than merely rejected. Same schemas, three enforcement
points. Note the caveat from the constrained-decoding literature, though: format correctness
and semantic correctness are independent problems requiring independent solutions. A grammar
guarantees the JSON parses; it does nothing about the lamp being inside the wall.

### 2.10 The eyes: headless render feedback

This is the capability that decides whether the whole project works, and it's the one most
likely to get deprioritized because it feels like a nice-to-have. It isn't. The Blender
experience is unambiguous: an agent can run fifty valid operations, get "success" every time,
and produce garbage, because text read-back of names and transforms cannot tell you whether
the thing looks right.

```
render_preview(scene, camera: NamedCam | Orbit { yaw, pitch, dist }, size, mode)
  → PNG (base64, returned inline to the model)

  modes: shaded | wireframe | collision | overdraw | ids
```

Design notes that make it actually useful:

- **Multi-angle by default.** One render is a lie; return front / three-quarter / top. Objects
  inside other objects are invisible from exactly one angle.
- **`collision` and `ids` modes matter more than pretty shading.** The failure modes are
  intersecting geometry and misparented nodes, and a flat-color-by-entity-id pass makes both
  obvious.
- **Renders through the same Vulkan path as the game**, headless to an offscreen target. Not a separate
  code path, or it will diverge and start lying.
- **Cheap and fast.** 512×512, one frame, no post. It'll be called constantly.

Pair it with the non-visual channel, which is cheaper and catches different bugs:

```
run_scene(scene, ticks: 600, assert: [...])
  → { assertions: [...], warnings: [...], trace: [...] }
```

Deterministic fixed-timestep headless simulation, with the agent able to assert things like
"player never falls below y = -1" or "enemy count reaches 0 within 30s." This is Unreal's PIE
automation, which is exactly the capability Godot agents are noted as lacking. Having both
puts you ahead of all three engines on verification.

### 2.11 Iteration speed: the thing that kills Rust engine projects

The real cost of Rust here is compile time — the graphics dependency tree is heavy and cold builds
run minutes. The architecture already mostly solves it, but be deliberate:

**Tier 1 — data changes: no compile at all.** Scene files, prefabs, materials, tuning tables
all hot-reload through the file watcher. The agent's entire loop lives here. This is why the
data-driven decision in §2.3 is a *performance* decision as much as a correctness one.

**Tier 2 — game code: host/worker dylib split.** Game logic lives in a crate built both as a
`dylib` (development) and an `rlib` (shipping, behind a feature flag). Host and worker
communicate through a trait object. This works because the same compiler and same flags on both
sides produce a matching ABI — Rust has no stable ABI, so this is a **development aid only, not
a plugin architecture.** Ship the static build. `hot_lib_reloader` is the well-trodden crate
here; `dexterous_developer` goes further with serialization-based schema evolution across
reloads, and `subsecond` (from the Dioxus team) takes the more aggressive route of intercepting
the link phase, diffing assembly between compiles, and patching symbols in the running process
— reported around 300–900ms on a Mac. Treat subsecond as promising and experimental; start with
`hot_lib_reloader`.

**Tier 3 — engine core: accept the rebuild.** Keep the core crate genuinely thin so this is
rare.

### 2.12 Scripting: Rhai, sandboxed, as hot-reloadable data

The agent authors behavior, not just layout. This breaks the invariant that made everything
above safe — a component field can be schema-validated, and code cannot. So the containment has
to come from somewhere else.

**Embed Rhai, not Lua.** Rhai is Rust-native with no unsafe FFI boundary, integrates with
`serde`, and — decisively for this project — supports hard resource limits: max operations, max
call depth, max string/array size. When the *agent* is writing the code, an accidental
`while true {}` must be a caught error, not a hung engine. Lua via `mlua` is faster and more
widely known, but the FFI surface is unsafe-heavy and sandboxing is manual.

**Keep the Rust/script boundary absolute.** The agent never writes engine Rust, never touches
`Cargo.toml`, never adds a dependency. It writes `.rhai` files against a registered API. Three
consequences that all point the same direction:

- Scripts are **data**: text files, watched, hot-reloaded, no compile step. Tier-1 iteration
  (§2.11) survives intact — this is the whole reason not to let the agent write Rust.
- The registered surface *is* the sandbox. No filesystem, no network, no process spawning,
  because those functions were never registered. A call to anything unregistered fails at
  script-compile time with a structured error.
- The type registry from §2.1 does double duty: components are exposed to Rhai from the same
  metadata that generates the schemas and the inspector. One source of truth, three consumers.

```rust
// Registration derived from the reflection registry, not hand-written
engine.register_script_api(|api| {
    api.component::<Transform>()      // get/set fields, respecting §2.1 constraints
       .component::<Light>()
       .query("nearby", Query::sphere)
       .event("on_enter", "on_exit", "on_tick")
       .limits(Limits { ops: 100_000, depth: 32, ..default() });
});
```

**Verification shifts channels.** This is the part worth internalizing: a render tells you a
script *looks* fine while it is leaking entities on frame 900. Visual feedback verifies
placement; only simulation verifies behavior. So `run_scene` with assertions is no longer a
late-phase nicety — it is co-equal with `render_preview` and lands in the same phase, and the
fixed-timestep determinism it needs has to be designed in from Phase 1 rather than retrofitted.

A script failure returns the same structured shape as a validation error — script path, line,
the registered alternatives if the call was unknown — because the rejection message is the
agent's teacher.

### 2.13 Crate layout

```
loom/
├── loom_reflect/      # derive macros + type registry            ← build first
├── loom_ecs/          # archetype storage, queries, scheduler
├── loom_scene/        # .loom parse/serialize, prefabs, overrides ← and this
├── loom_asset/        # import pipeline, .meta, manifest, cache
├── loom_render/       # ash/Vulkan: forward renderer, headless target — the ONLY crate
│                     # permitted to import ash
├── loom_render_graph/ # pass declaration, resource lifetimes, automatic barriers
├── loom_input/        # winit event mapping
├── loom_physics/      # rapier3d integration; do not write this yourself
├── loom_script/       # rhai host: API registration, sandbox limits, hot reload
├── loom_graph/        # SQLite indexer + file watcher
├── loom_agent/        # MCP server, transactions, tool catalog
├── loom_editor/       # egui inspector, driven by the registry
└── loom_cli/          # loom new | run | validate | index | render
```

Dependency discipline: `loom_reflect` and `loom_scene` depend on nothing else in the tree.
Everything else may depend on them. `loom_agent` depends on many but is depended on by none —
the agent layer must be removable, or you'll never be able to tell whether a bug is in the
engine or the agent.

---

## Part 3 — Build order

Sequenced so each phase produces something demonstrably working, and so the risky bet gets
tested early rather than late.

### Phase 0 — Reflection and scene format (no rendering)
**Weeks 1–2.** `loom_reflect` derive macro; type registry; `.loom` parse/serialize round-trip;
5–6 components (`Transform`, `Name`, `MeshRenderer`, `BoxCollider`, `Light`, `Script`).
CLI: `loom validate`, `loom describe <Type>`.
**Exit:** a hand-written `.loom` file round-trips byte-identically, and a bad field produces a
useful error. No window yet — this is the foundation everything else is generated from.

### Phase 1 — Window, ECS, render
**Weeks 3–13** (revised for Vulkan — see `loom-vulkan-backend.md` §11 for the week-by-week).
winit + ash; swapchain; gpu-allocator; render graph; descriptor indexing; pipeline cache;
forward renderer; glTF mesh loading; the ECS with transform
propagation; scene load → screen. **Fixed-timestep game loop from day one** — determinism is a
prerequisite for §2.10 assertions and cannot be retrofitted cheaply.

Because this is 3D-first, add a **primitive library**: box, cylinder, plane, sphere, capsule with
a handful of materials, generated procedurally. Without it, 3D blockout is blocked on having art,
and the agent has nothing to compose. Blockout with primitives is how human level designers work
anyway.
**Exit:** `loom run office.loom` shows the desk and the lamp; frame timing is deterministic
across runs.

### Phase 2 — Prefabs and overrides
**Week 6.** Instancing, delta overrides, `extends`, apply/revert/unpack, cycle detection.
**Exit:** editing `lamp.loom` updates 20 placed instances; per-instance intensity survives.

### Phase 3 — The agent loop *(the real milestone)*
**Weeks 8–12.** `loom_agent` MCP server; `scene_query` / `scene_edit` / `describe_type`;
**`scene_place` + `scene_measure`** (§2.8); transactions with undo and dry-run;
**`render_preview`**; **`run_scene` with assertions**; `loom_script` with the sandbox and hot
reload; the Claude Code validation hook.

Both verification channels ship here, not later. Placement needs eyes; behavior needs
simulation. Shipping only one gives you an agent that is confidently wrong in the other half.

**Exit:** the test that matters —

> *"Block out a high-school computer lab: 6 desks in two rows, a monitor on each, a teacher desk
> facing them, and overhead lights. Then make the monitors turn on when the player walks into
> the room."*

Claude Code produces the layout, looks at its own multi-angle render, notices the monitor
floating above desk three and fixes it, writes the trigger script, and proves it fires with a
headless assertion. Unassisted.

**If this doesn't work, stop and fix the loop before building anything else.** It is the
load-bearing assumption of the entire project, and everything in Phases 4–6 is worthless without
it. Budget the extra time here rather than compressing it — this phase is now doing double duty
because both the 3D and the scripting decisions land in it.

### Phase 4 — Assets and the graph
**Weeks 10–12.** Import pipeline, `.meta`, manifest, hash cache; SQLite indexer + watcher;
`graph_query`; two-hop context packs.
**Exit:** *"what would break if I changed the desk prefab?"* answers correctly on a project
with 200+ files.

### Phase 5 — Editor
**Weeks 13–16.** egui inspector generated entirely from the type registry; scene tree panel;
gizmos; the force-directed graph view.
**Exit:** you can do a task by hand *and* by agent, and the results are indistinguishable in
the file.

### Phase 6 — Headless testing, hot reload, WASM
**Ongoing.** `run_scene` with assertions; the host/worker dylib split; the WASM build target.

**Rough shape: a working AI-authored engine at Phase 3, around 9 weeks of real evenings.**
Phases 4–6 are what make it pleasant.

---

## Part 4 — Risks, honestly

| Risk | Severity | Mitigation |
| --- | --- | --- |
| **The agent is a bad 3D level designer even with working eyes** | Highest | The actual open question, and 3D-first raises it. Phase 3 tests it early. Mitigate with semantic placement ops (§2.8) so spatial arithmetic never reaches the model, plus multi-angle renders and reference images in context. Accept that agent-as-blockout-artist may work while agent-as-art-director doesn't. |
| **Agent-authored scripts accumulate silent rot** | High | Behavior bugs don't show up in renders. Every agent-written script needs a `run_scene` assertion committed alongside it — treat "script with no assertion" as a lint failure. Rhai resource limits catch runaway loops; they don't catch wrong logic. |
| Scope creep into rendering | High | You are not building a renderer, you're building an authoring loop. Forward renderer, primitives + glTF, no shadows until Phase 6. Vendor `rapier3d`. |
| Compile times destroy the loop | Medium | Thin core crate; data changes never compile (§2.11). Measure cold and warm build times at Phase 1 and treat regressions as bugs. |
| Format churn breaks saved scenes | Medium | `format = 1` from day one, plus a migration function per bump. Godot's undocumented-subresource problem is a cautionary tale — document the format in the repo from commit one. |
| Overrides get subtly wrong | Medium | Unity's model is proven; copy it exactly rather than improvising. Orphaned overrides warn loudly and preserve the value. |
| Building this instead of building games | Real | Bevy, Godot, and a Godot MCP server all exist today. This project is worth doing for control and understanding, not because there's a gap. Know which one you're chasing. |

---

## Part 5 — Decisions made, and what's still open

### Settled

| Question | Decision | Consequence |
| --- | --- | --- |
| 2D or 3D first | **3D** | Semantic placement ops (§2.8) become mandatory, not optional. Primitive library required in Phase 1. Multi-angle renders, not single. |
| Agent authors scripts | **Yes** | Rhai sandbox (§2.12). `run_scene` assertions promoted to Phase 3. Fixed-timestep determinism required from Phase 1. |
| Physics | **Vendor `rapier3d`** | Writing your own 3D collision is a semester, not a sprint, and it isn't the interesting part of this project. |
| Scripting language | **Rhai over Lua** | Rust-native, no unsafe FFI, hard resource limits — which matter specifically because the agent writes the code. |

### Still open

1. **How much art pipeline do you want to own?** 3D means assets. The primitive library covers
   blockout, but a real game needs meshes, and the import path (glTF only? materials? animation?
   skinning?) is a whole subsystem. Recommendation: glTF static meshes in Phase 1, skinned
   animation deferred past Phase 6.
2. **Can the agent create *new component types*, or only new scripts?** Scripts are data and
   hot-reload. A new component type is Rust and requires a Tier-3 rebuild. Letting the agent
   define components *in script* (a dynamic component backed by a property bag) keeps the loop
   fast but weakens the type-safety story that §2.9 depends on. This is the next real fork in
   the road, and it's worth deferring until you've watched the Phase 3 agent work for a while.
3. **Networking, ever?** If yes, it changes the ECS design (deterministic lockstep vs
   state replication) and it's much cheaper to know now than to bolt on later. If it's a
   single-player engine, say so out loud and stop paying design tax for it.
