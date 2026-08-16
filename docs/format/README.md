# The `.loom` format — specification

**Status: normative for M1.** Version `format = 1`.

This is a contract. If the parser and this document disagree, that is a bug in one of them —
fix it and say which. Godot's own contributor docs concede their sub-resource format is only
discoverable by reading engine source; the whole point of writing this first is to not repeat that.

Scope: the scene format, in full. The terrain recipe and voxel op-list share the encoding rules in
§1–2 but their layer/op vocabulary is specified at M11/M10 — see §10.

---

## 1. Encoding

- **UTF-8**, no BOM. A BOM is an error, not something to strip.
- **LF line endings.** CRLF is an error. (Single-platform project; no reason to accept both.)
- Exactly **one trailing newline** at end of file.
- The file is **valid TOML v1.0.0**. Not "TOML-flavored" — valid. If the TOML parser rejects it,
  it is not a `.loom` file.

> **Known doc errata:** `loom-terrain-generation.md` §2 and `loom-voxel-system.md` §5.2 show
> `octaves = 5; lacunarity = 2.0` — semicolon-separated pairs, which **TOML does not accept**
> (verified against `tomllib`, 2026-07-30). One key per line. Those examples are illustrative and
> wrong; this document wins.

Floats: TOML requires a fractional part or exponent, so `90` is an *integer*. A field typed `f32`
always emits at least one fractional digit — `90.0`, never `90`. Reading `90` into an `f32` field
is accepted (liberal in what we accept, §7) and normalizes to `90.0` on write.

**NaN and ±infinity are rejected by the validator** on every field. TOML can express them; nothing
in a transform, colour, or intensity means them, and they poison the determinism hashes M3 depends
on. Rejection message names the field.

### 1.1 Coordinate system and rotation

**Right-handed, Y-up, −Z forward, +X right.** This is glTF's convention, and matching it means the
M5 mesh importer needs no axis conversion — the most common source of silently mirrored or
90°-rotated assets in every engine that picked something else.

**Rotation is authored as euler angles in degrees, and the authored value is the source of truth.**

```toml
transform = { pos = [0.0, 0.0, -2.5], rot_euler = [0.0, 90.0, 0.0] }
#                                                   ^X    ^Y    ^Z
```

- **Order: intrinsic Y-X-Z** — yaw about Y, then pitch about X, then roll about Z. Written in the
  array as `[pitch_x, yaw_y, roll_z]` so the array index matches the axis. Gimbal lock sits at
  pitch = ±90°, which is "looking straight up or down" — the singularity every camera controller
  already handles, rather than an arbitrary one.
- **Conversion to quaternion is one-directional**, at the ECS boundary. The scene layer never
  converts a quaternion back to euler, so **round-trip is exact by construction rather than by
  luck**. Quaternions are a runtime representation and never appear in a `.loom` file.
- **Canonical form normalizes each angle into `(-180.0, 180.0]` by adding or subtracting integral
  multiples of 360.** Pure addition — no trig, no matrix, no drift.

> `ponytail:` normalization is modulo-360 per axis only, *not* full canonicalization of the euler
> triple. A pitch outside ±90° is a legal rotation expressed unusually, and reducing it would mean
> going through a quaternion — reintroducing exactly the drift this design avoids. Two different
> euler triples can therefore denote the same rotation and hash differently. Upgrade path if that
> ever matters: canonicalize at author time in the `SceneOp` layer, where a quaternion is already
> in hand, never in the emitter.

---

## 2. Canonical form

Canonical form is what makes the M1 exit criterion testable:

> **A canonical `.loom` file round-trips byte-identically.**

Non-canonical files are *accepted* and normalized on write. `loom fmt` canonicalizes in place —
the gofmt model. A byte-identical round-trip is not claimed for arbitrary input, because
"defaults are omitted on write" (§4) makes that impossible: a hand-written explicit default is
deleted by design.

Canonical rules, all of them:

1. **Section order** is fixed: `[scene]`, then `[[asset]]`, then `[[prefab]]`, then `[[node]]`.
2. **Key order within a table** is declaration order from the type registry, not alphabetical.
   Registry order is the field order in the Rust struct, which is stable and reviewable.
3. **Node order** is depth-first, parents before children, siblings in insertion order.
   Reordering siblings is a real edit and shows in the diff; it is not normalized away.
4. **Defaults are omitted.** A field equal to its registered default is not written (§4).
5. **Indentation:** two spaces per nesting level for sub-tables under a node. Top-level tables are
   not indented. Indentation is cosmetic in TOML and load-bearing for human review.
6. **Floats:** shortest representation that round-trips exactly, with a mandatory fractional digit.
   `-0.0` normalizes to `0.0`.
7. **Inline tables** for fixed-shape value types only: `transform`, and any registered type whose
   fields all fit on one line under 100 columns. Everything else is a sub-table.
8. **Arrays** are inline when under 100 columns, one element per line otherwise.
9. **One blank line** between top-level tables; none inside a table.
10. **`name` and `transform` are always written as node-key sugar**, never as
    `[node.components.Name]` / `[node.components.Transform]` (§3). Both spellings parse; only one
    is canonical.
11. **Euler angles are normalized** into `(-180.0, 180.0]` per axis (§1.1).

### Comments and hand formatting survive writes

The write path is **format-preserving** (a `toml_edit`-style DOM), not a serde re-emit. An agent
`SetField` mutates one value and leaves every comment, blank line, and key order untouched.

This is not a nicety. `CLAUDE.md` never-do #15 — *silently destroying the human's edits is the
worst bug class in this project* — is violated on day one if every agent write reflows the file and
deletes annotations. It also keeps agent diffs to the lines actually changed, which is what makes
the human's git-diff review channel (brief §5, channel 4) work at all.

**Consequence for §2.1 of the design doc:** the derive macro still generates serde impls, and those
still drive schema, validation, and the type registry. They do **not** drive the emitter. Reading
is serde; writing is a format-preserving DOM guided by the registry. Two paths, one source of truth
for what the fields *are*.

---

## 3. Scene file structure

```toml
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

# ── external references ──────────────────────────────
[[asset]]
key  = "office_desk"
id   = "a41f0c2e-1b3d-4e5f-8a90-0c1d2e3f4a59"
path = "assets/props/desk.glb"   # advisory only — never resolved by path

[[prefab]]
key  = "lamp"
id   = "7b22e910-2c4d-4a6b-9f81-3e5d7a9c0b41"
path = "prefabs/desk_lamp.loom"

# ── node tree ────────────────────────────────────────
[[node]]
name = "Office"
# no `parent` key == scene root

[[node]]
name      = "Desk"
parent    = "Office"
transform = { pos = [0.0, 0.0, -2.5], rot_euler = [0.0, 90.0, 0.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "office_desk" }

  [node.components.BoxCollider]
  half_extents = [0.8, 0.37, 0.4]
```

### `[scene]`

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `format` | integer | yes | Format version. `1`. A file with a higher version than the binary understands is a **load error**, never a best-effort parse. |
| `id` | UUID string | yes | Stable scene identity. Survives rename and move. |

### `[[asset]]` / `[[prefab]]`

`key` is a file-local alias; `id` is the UUID that is the real identity; `path` is a **hint for
humans and nothing else** — never resolved, never trusted, never used to load. Unity's lesson
(design doc §1.1): identity is the UUID, not the path.

The agent writes `mesh = { asset = "office_desk" }` — an alias. It never writes a raw UUID and
never writes a path. This is the single biggest authoring-ergonomics win over Unity's
`{fileID, guid, type}` triples.

`key` must be unique within the file, match `[A-Za-z_][A-Za-z0-9_]*`, and resolve to a declared
entry. An unresolved alias is an error naming the alias and listing the declared keys.

### `[[node]]`

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | yes | Unique among siblings. Sugar for the `Name` component. |
| `parent` | node path | no | Absent ⇒ this is the root. **Exactly one node may omit it.** |
| `transform` | inline table | no | Sugar for the `Transform` component. Defaults to identity, and is omitted when identity. |
| `prefab` | prefab alias | no | Makes this a prefab instance (§5). |

`parent` and `prefab` are structural — they describe the tree, not the node's data, so they are
genuinely node keys rather than sugar. `name` and `transform` are data, and desugar to components.

Zero roots or two roots is an error. Godot's one-root rule, and it is what makes scenes composable
as instances.

### Node paths

Slash-separated names from the root, **including** the root: `Office`, `Office/Desk`,
`Office/Desk/DeskLamp`. The root is addressed by its own name.

- Sibling names must be unique. Collision is an **error with both paths named**, never a silent
  rename.
- A name containing `/`, or leading/trailing whitespace, or empty, is an error.
- Names are compared by exact bytes. No case folding, no Unicode normalization.
- `parent` must name an already-declared node. Forward references are an error — this makes the
  file readable top-to-bottom and makes cycles unrepresentable rather than merely detected.

### Components

`[node.components.<TypeName>]` — one sub-table per attached component. `<TypeName>` must be
registered; an unknown type is an error listing near-matches by edit distance.

A node may hold at most one component of a given type. Field values are validated against the
registry entry (§6).

### `name` and `transform` are sugar for real components

`Name` and `Transform` are registered components like any other — they are two of M1's six. The
node keys are **file-level sugar** for them, and nothing else in the system knows the difference:

| Sugar in the file | Desugars to |
| --- | --- |
| `name = "Desk"` | `[node.components.Name]` with `value = "Desk"` |
| `transform = { pos = [...], rot_euler = [...] }` | `[node.components.Transform]` with those fields |

This is the only special case in the parser and the emitter, and it buys uniformity everywhere
else. Addressing is component addressing, with no second scheme to remember:

```
overrides     "Transform.pos" = [0.55, 0.74, 0.1]
SceneOp       SetField { node: "Office/Desk", field: "Transform.pos", ... }
CLI           loom describe Transform
inspector     generated from the registry entry, like every other component
rhai          node.Transform.pos
```

Rules:

- **Both spellings parse; canonical form always writes the sugar.** Otherwise there would be two
  canonical forms for one scene, and the version token (§8) would disagree with itself.
- Declaring both `name = "X"` and `[node.components.Name]` on one node is an error naming both.
- `name` is required on every node (§3). `transform` defaults to identity and is omitted when
  identity, per §4 — so a node that is not moved costs zero lines.

---

## 4. Defaults are omitted

A field equal to its registered default is not written. Straight from Godot; the reason is that
diffs then show intent rather than noise, which is what makes an agent's commit reviewable.

Two consequences worth stating plainly:

- **Absent ≡ default.** There is no third state at the scene-file level. A field is either written
  with a non-default value or it is absent and takes the default.
- **Prefab overrides are the exception** (§5). There, "absent" and "explicitly set to the prefab's
  value" genuinely differ, and the override map records presence explicitly.

---

## 5. Prefab instances and overrides

> **Implemented.** `prefab`, `[node.overrides]` and `extends` are read by
> `loom_scene::prefab` and expanded before any command looks at the tree. The
> three operations are `loom prefab <unpack|revert-overrides|apply-overrides>`,
> and setting a field on an instance writes an override rather than a
> component.
>
> Two behaviours worth knowing, both recorded in ADR 0008: `apply-overrides`
> writes **two** files and is therefore two undo steps, not one; and `extends`
> merges field by field, which means resetting an inherited transform to
> exactly identity is the one edit it cannot express (omitted *is* identity, so
> the two are indistinguishable in the file).

Unity's `PrefabInstance` delta model. The consuming file stores a **source reference plus explicit
modifications** — never a copy of the prefab's contents.

```toml
[[node]]
name      = "DeskLamp"
parent    = "Office/Desk"
prefab    = "lamp"
transform = { pos = [0.55, 0.74, 0.1] }

  [node.overrides]
  "Light.intensity" = 420.0
  "Light.color" = [1.0, 0.92, 0.78]
  "Flicker.enabled" = true
```

`[node.overrides]` is a **flat map of dotted paths to values**. Flat, so setting an override is one
operation with no tree surgery. Keys are `ComponentType.field`, or
`Child/Path::ComponentType.field` to reach inside the instanced sub-tree.

- A prefab instance node declares no `[node.components.*]` of its own. Component data comes from
  the prefab; deviations go in `overrides`.
- **An override targeting a path that no longer exists in the prefab is a loud warning, and the
  orphaned value is preserved in the file.** Never a silent drop. Unity's handling of this is a
  known pain point and reproducing it would be a choice.
- `extends = "<prefab alias>"` on a scene's root node makes the whole scene an extension of another
  (Godot scene inheritance). Editing the base updates the extension.
- Instancing cycles are detected at load and reported with the full cycle path.

Three operations are first-class for both the editor and the agent, and go through the same code
path (`CLAUDE.md` never-do #16): `apply_overrides`, `revert_overrides`, `unpack`.

---

## 6. Validation, and what a rejection looks like

Two layers, both cheap because the derive macro already generated the schema.

**Layer 1 — the type system is the validator.** Every load and every `SceneOp` deserializes into
concrete Rust types. Malformed data cannot become a live object because the type will not
construct. `#[loom(range = ...)]` and enum constraints are checked at the same boundary.

Rejections are **structured and actionable**. This shape is normative:

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

The `hint` is doing real work — a rejection message is the agent's teacher, and a good one turns a
retry loop into one correction. Every constraint that can carry a hint should.

Error codes for M1: `parse_error`, `format_version_unsupported`, `duplicate_sibling_name`,
`multiple_roots`, `no_root`, `unknown_parent`, `unknown_component_type`, `unknown_field`,
`field_out_of_range`, `field_type_mismatch`, `unresolved_alias`, `non_finite_float`, and
`not_implemented` for the §5 keys above.

`invalid_voxel_op` came later, and for the same reason `not_implemented` exists: a `VoxelVolume`'s
`ops` ride on the component as free-form JSON, so layer 1 never looks inside them. An op whose
`kind` nothing recognises used to be dropped on the way to the bake — the volume baked *short*, drew
a plausible surface, validated clean, and got faster for it. The volume is refused whole, naming the
op's index and its `kind`.

`unresolved_alias` is reserved for an alias that **nothing declares** — a typo in the scene, which
is what the agent controls. A declared alias whose file is merely absent is reported as an
`asset_file_missing` warning instead: the text is right and the workspace is incomplete, which is
an ordinary state during import and not grounds to reject a scene. The renderer substitutes a unit
box either way (design doc §2.6, degrade rather than crash) — which is right for a render and
useless as feedback, because a scene full of stand-in boxes looks exactly like one that loaded.

Every error carries the node path and, where applicable, the byte span in the source file.

**Layer 2 — a `PostToolUse` hook** running `loom validate --staged`, wired at M9. Exit 2 blocks the
write and feeds stderr back as a tool result. Net effect: the agent physically cannot leave a
malformed scene on disk.

---

## 7. Liberal in what we accept

Accepted interchangeably for a `Vec3` field, and normalized to the first form on write:

```
[0.0, 1.0, 0.0]        { x = 0.0, y = 1.0, z = 0.0 }        "Vec3(0, 1, 0)"
```

Integers coerce to floats where a float is expected. This removes a whole category of agent retry
loops for the cost of a few `From` impls, and costs nothing in the file because canonical form
normalizes it all anyway.

Coercion never *widens* validation: a coerced value is range-checked exactly like a native one.

---

## 8. Version tokens

Every load returns a token; every write presents the token it read. A write against a stale token
is **rejected** — returning current content and version so the caller reloads and re-applies.
**Never auto-merged.** (Brief §7.17; `CLAUDE.md` never-do #15.)

- The token is the **BLAKE3 hash of the file's canonical bytes**, lowercase hex.
- Same hash function as the asset content-hash pipeline (§2.6 of the design doc). One hash for the
  project, chosen once.
- Computed on canonical bytes, so cosmetic reformatting does not invalidate a token, and a
  semantic change always does.

**Plumb this through the loader at M5.5 while nothing writes yet.** Five lines then; an
architectural change at M12.

---

## 9. Stability guarantees

**May change without a migration:** canonical formatting rules (§2) — reformatting is not a
semantic change; hint text; error message wording. Adding a new optional field with a default,
or a new component type.

**Requires a `format` bump and a migration function:** renaming or removing a field or component
type; changing a field's type or its default; changing node addressing; changing override key
syntax.

Migrations are one function per bump, they run on load, and they are tested by keeping a fixture
scene per historical version in `assets/test/`. `format = 1` from day one so there is never an
unversioned file in the wild.

---

## 10. Recipes and op lists — deferred, but constrained now

Terrain recipes (M11) and voxel op lists (M10) are separate documents sharing §1–2's encoding.
Their vocabulary is specified at those milestones. Three constraints bind now, because they are
what the scene format has to accommodate:

1. **Never serialize raw voxel arrays.** A 512³ volume is 134M voxels. Scenes store the *recipe* —
   the ordered op list — and the volume is baked from it on load. (`CLAUDE.md` never-do #11.)
2. **Op lists are explicitly ordered and non-commutative.** Subtract-then-union ≠
   union-then-subtract. The schema docs must say so, or the agent will assume commutativity.
3. **Baked artifacts are content-hash-keyed and never in version control.** The recipe is
   authoritative for authoring; the baked artifact is authoritative for determinism.

---

## 11. Settled, and where

Both questions this spec opened were closed on 2026-07-30:

- **Rotation** — euler degrees on the wire, authoritative; quaternion derived one-way at the ECS
  boundary; intrinsic Y-X-Z; right-handed Y-up −Z-forward. §1.1.
- **`name` / `transform`** — node-key sugar over the real `Name` and `Transform` components, which
  restores brief M1's six-component list and keeps addressing uniform. §3.

Nothing in this document is open. If a decision here turns out wrong, it gets an ADR and a `format`
bump per §9 — not a silent edit.
