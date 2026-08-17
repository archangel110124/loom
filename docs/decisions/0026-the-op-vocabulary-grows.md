# ADR 0026 — The op vocabulary grows: `SpliceArray`, `Declare`, `SpawnNode { prefab }`

- **Date:** 2026-08-16
- **Status:** **accepted**
- **Decision touched:** none locked. `loom_scene::ops::SceneOp` goes from nine
  variants to eleven, plus one optional field on an existing one
  (`ops.rs:47-162`). **No `format` bump** — every construct these ops write was
  already legal in `docs/format/README.md`; what changes is that the editor can
  author it.
- **Implements:** `docs/design/editor/PLAN.md` §3 row 0026, and closes its risk
  R2 (the `toml_edit` spike) and R14 (splice against a prefab instance).

## Context — three things the editor could not author

**An array field could only be replaced whole.** `SetField` converts its JSON
payload with `json_to_toml` and assigns the result as an `Item::Value`
(`ops.rs:960`, `ops.rs:977-983`). A JSON array becomes a
`toml_edit::Value::Array`, and a value array in TOML is inline by construction.
So setting `VoxelVolume.ops` collapses every `[[node.components.VoxelVolume.ops]]`
header into a single line and takes the human's comments on those entries with
it. `assets/test/ground.loom` and `assets/test/cave.loom` each carry four such
headers today, and a sculpt brush emits one op per stroke. The one system whose
authored form exists *specifically* to be diffable would have produced a
one-line diff per stroke, forever.

**Declarations were CLI-only.** The editor could reference an asset alias and
never introduce one, so "import this mesh" and "make this a prefab" could not be
editor actions at all.

**`SpawnNode` could only make a mesh node.** Dropping a prefab into the viewport
had no op behind it.

## The spike came first, and it is kept as a test

`crates/loom_scene/tests/toml_edit_contract.rs` was written before any of this,
as the stop-and-redesign gate PLAN §5 asks for: *can a splice edit
`[[node.components.VoxelVolume.ops]]` in place, preserving whichever spelling is
on disk?* The answer is yes, and the tests stayed because the answer is a
property of a pinned dependency (`crates/loom_scene/Cargo.toml:14`,
`toml_edit = "=0.25.13"`) rather than of anything in this repository. A version
bump that changed it would otherwise surface as scene files quietly reformatting
themselves under the editor.

Six properties are pinned: the two spellings are distinguishable after parsing
(`toml_edit_contract.rs:75`), an append re-emits a header rather than collapsing
(`:91`), append-then-delete is **byte-identical** to where it started (`:118` —
undo re-applies an inverse transaction rather than restoring a snapshot, so an
op that round-trips with a whitespace difference makes every undo a diff), a
middle insert keeps order and spelling (`:140`), the inline spelling splices as
an ordinary array (`:167`), and `Scene::parse` reads both spellings identically
(`:190`).

## One splice op, not three named ones

Append, remove and replace are the same operation with different arguments, and
naming them separately costs three ways:

- **Every caller would pick one, and the inspector would have to know which.**
  The callers are the sculpt brush, `WaterBody.waves`, `Buoyancy.pontoons`,
  `Scatter.excludes`, `Scatter.remove`, `FoliagePaint.strokes`,
  `SplatPaint.strokes`, `PaintLayer.strokes`, the ground layer, mesh import,
  prefab creation, the prefab browser drop and the duplicate-an-instance fix.
  One array-of-object row in the inspector serves all of them only if there is
  one op.
- **The inverse of a splice is a splice.** Undo is re-application of an inverse
  transaction; with three ops, undo needs a table mapping each to its opposite,
  and `append` inverts to `remove` while `replace` inverts to itself.
- **The destructive classifier would grow three arms** instead of one predicate
  (see below).

`ops.rs:87-101`. `index`, `remove`, `insert` — an append is `remove: 0` at the
current length, a deletion is an empty `insert`, a replace is both.

## The spelling on disk is preserved by branching on the item variant

Not by a policy, and not by a flag stored anywhere. The two spellings parse to
two different `toml_edit::Item` variants, so **the variant is the spelling**, and
the apply arm matches on it: `Some(Item::ArrayOfTables(existing))`
(`ops.rs:1047`) for the header form, `Some(item)` falling through to
`as_value().and_then(Value::as_array)` (`ops.rs:1061-1071`) for the inline form
and for plain scalar arrays.

The alternatives were both worse:

- **A flag on the op** ("write headers") makes the caller responsible for
  knowing how a file it did not write is spelled — and the human may change that
  between the read and the write.
- **A normalisation policy** ("always headers", "always inline") rewrites arrays
  in files nobody was editing, which is the diff noise a format-preserving DOM
  exists to prevent.

The header branch edits **entry by entry** — `remove(at)` in a loop, then
`insert(at + offset, …)` (`ops.rs:1050-1056`) — rather than rebuilding the array,
so the decor of every entry the op did not touch survives. That is what makes
the round-trip byte-identical, and it is why the comment on op 0 is still there
after op 2 is spliced in (`ops.rs:1515`,
`splicing_keeps_the_array_of_tables_spelling_and_the_comments`).

## A splice against a prefab instance materialises the resolved array as an override

`ops.rs:1012-1016`: if the node is an instance, read what the field currently
resolves to, splice that, and write the whole result back as one override.

The other readings all fail, and it is worth writing down how:

- **"Index 3 means the prefab's index 3."** The result then changes silently
  when the prefab changes. A carve recorded at index 3 lands somewhere else in
  the recipe after someone else edits the prefab — the scene file did not
  change and the geometry did.
- **"Write a component onto the instance."** An instance owns no components;
  that is what makes overrides well-defined. A node with both has two sources
  for one field and no rule about which wins, which is exactly the state
  `Scene::parse` rejects and exactly what `SpawnNode` refuses to author
  (`ops.rs:772-784`, error `mesh_and_prefab`).
- **"Refuse."** Sculpting or painting a prefab-instanced terrain chunk is a
  plausible first user action, which is why this was decided before the op
  shipped rather than after the first bug report.

Materialising makes the edit mean what the human saw on screen when they made
it. `ops.rs:1608` asserts the instance grows no `[node.components]`, that order
is preserved across the pre-existing entry, and — the check that matters — that
the result still parses, so the override is *legal* rather than merely present.

**The honest limit.** `resolved_array` (`ops.rs:542-578`) reads the instance's
own override and **returns empty when there is none**, rather than reaching into
the prefab: `apply_one` deliberately sees only the document it is editing, and
the prefab library is out of scope for every op except `UnpackPrefab`. So
splicing a field an instance has never overridden starts from nothing instead of
from the prefab's list. The editor always sends the array it displayed, so the
case it hits is the case that works; a hand-written transaction against an
un-overridden field should `SetField` the whole array first. Pulling the library
in here would give this layer a dependency on files outside the one document,
which is a larger change than the bug it fixes.

**What it costs.** An override is a dotted key holding a value
(`set_override`, `ops.rs:521-525`), so a spliced field on an instance is inline
by construction and there is no operation that returns it to header form. A
prefab instance with a long spliced `ops` array has a long line in it. That is
the price of the instance model, not of this op.

## A dotted field path is impossible, not merely unsupported

`SpliceArray` takes an integer `index` and an array field name rather than a
path like `VoxelVolume.ops.3.radius`, and the reason was verified rather than
assumed: `SetField` splits its field name **once** (`ops.rs:905`) and uses the
remainder as a literal TOML key (`ops.rs:983`). `VoxelVolume.ops.3.radius`
therefore names a *field called* `ops.3.radius` on the `VoxelVolume` component.
It never reaches the third entry; it writes a key nothing reads and the scene
still validates.

Reaching into an array element would need a path grammar this format's field
names do not have. That is a different op, not a longer string.

## Bounds clamp; a missing field errors

Both bounds are clamped, in all three code paths: `splice_values`
(`ops.rs:535-540`) for the prefab route, and `min`/`saturating_sub` in the
header and inline branches (`ops.rs:1048-1049`, `ops.rs:1073-1074`). An index
past the end appends.

The distinction is between a **race** and a **mistake**. An editor that issues
"append at index N" is right about its intent and possibly stale about the
array's length — another write may have shortened it between the read and the
send — and rejecting turns an ordinary race into an error dialog for an
operation whose meaning was never in doubt. A *missing field* is not a race: it
means the op names something that is not there, and clamping cannot invent an
array to splice. So `unknown_field` (`ops.rs:1085`) and `unknown_component`
(`ops.rs:1025`, `ops.rs:1036`), and `ops.rs:1573` asserts both halves in one
test — `index: usize::MAX, remove: 99` appends cleanly, `VoxelVolume.nope`
fails with `op_index: Some(0)`. A transaction is all-or-nothing, so the failure
leaves the scene untouched.

## `remove > insert.len()` is a net deletion

Recorded here, on the op, rather than only in ADR 0038's classifier, so the rule
is discoverable from the thing that triggers it — the doc comment on the variant
says so too (`ops.rs:84-86`).

The classifier keys on **net loss**, not on `remove > 0`, and the difference is
the whole point: `remove: 1, insert: [one entry]` is how you edit an array
element in place — retuning a sculpt stamp, replacing a paint stroke — so a gate
on `remove > 0` would fire during routine editing, and a gate that fires
constantly is the blind-approve regression arriving through the mechanism built
to prevent it.

## `Declare`, and why the duplicate check is an error

`ops.rs:1093-1137`. `kind` is `asset` or `prefab` and nothing else. A duplicate
alias within one file is rejected (`ops.rs:1105-1116`, `duplicate_alias`),
because a file where one word means two things is a file the loader resolves by
silently taking one of them.

`id` is optional and is the prefab's stable identity: a library is keyed by
`id`, **never** by the alias, because aliases are file-local and two files may
use one word for different prefabs (ADR 0008). `path` is relative to the scene.

`Declare` and `SpawnNode { prefab }` are issued **in one transaction**, which is
what dropping a prefab into the viewport actually is — and it is not merely
tidy: the validator correctly refuses an instance whose alias is undeclared, so
either op alone is invalid and only the pair applies (`ops.rs:1704`). One
transaction, one Ctrl+Z, per never-do #16.

## What this forecloses

- **No op edits inside an array entry.** Changing one field of one voxel op is a
  whole-entry replace. Per-field editing inside an array needs the path grammar
  the section above shows this format does not have.
- **A prefab instance's spliced array is inline from then on.** Nothing converts
  it back.
- **Nothing at this layer reads the prefab library**, so a splice against a
  never-overridden field on an instance starts empty. Changing that is a
  scope change for `apply_one`, not a tweak.
- **`SpawnNode` will not author a node that is both a mesh and an instance**,
  which also means "give this instance its own extra mesh" has no op; the answer
  is a child node or `UnpackPrefab`.

## How it would be reversed

`SceneOp` is a serde-tagged enum, so deleting a variant is a compile error at
every caller — the reversal is loud, which is the property that matters.
`SpliceArray` reverts to whole-array `SetField` at exactly the cost named in
Context. `Declare` reverts to hand-editing or the CLI. `SpawnNode.prefab` is
`#[serde(default, skip_serializing_if = "Option::is_none")]` (`ops.rs:61-62`),
so removing it changes no serialised transaction that did not use it.

The one part that is not cheap to reverse is the prefab-instance
materialisation: scenes already written that way hold override arrays that no
longer correspond to a component table, and un-writing them is a migration
rather than a revert.

## Human approval

Not required. No locked decision moves, no `format` bump, and every file these
ops produce is one a human could have typed.
