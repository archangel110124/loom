# 0108 — The editor can author weather and water

Status: accepted · 2026-09-09

## Context

Asked plainly: *can a human put shapes in a scene, add and remove and tune rain,
wind and water, and change the FFT sea's numbers, from inside the editor?*

Most of the machinery said yes. `Rain`, `Wind` and `WaterBody` are registered
types, so Add Component offers them off the registry with no second list to keep
in step; the ✖ removes one; the Inspector edits fields generically with schema
ranges and enum dropdowns; Create offers five primitives. On paper, done.

Measured, three things were not.

## What was broken

**1. Add Component could not add rain or water at all.**

The menu writes every field's schema default. Four types of thirty have a field
whose default is `null` — `Rain.duration`, `WaterBody.{extent, fetch, flow,
swell}`, `Environment.cloud_type`, `Joint.limits` — and TOML has no null, so the
op layer refuses it as `unrepresentable_value`. A transaction is all-or-nothing,
so the refusal took the other fifteen `WaterBody` fields with it and the human
got a console line and no component.

The two types a person reaches for first were the two that failed.

A null default *means* the field is absent, and absence in TOML is a key that is
not written. So the defaults walk skips it. Pinned by a test over every
registered type — not "every type applies", because a `Joint` on a bare node is
legitimately refused and that is the validator working, but "no type emits a
value TOML cannot hold". Fault-injected: putting the null back fails it on
`Environment`.

**2. The FFT sea's numbers could not be edited.**

`WaterBody.swell` is `{direction, fetch, u10}` — the three numbers that *are*
the spectrum. It reached the Inspector as the literal text
`{"direction":[0.25881904,0.9659258],"fetch":420000.0,"u10":18.0}` in read-only
grey, and so did `optics`, `flow` and `waves`. The one thing this editor exists
to do is change a value, and for a whole class of fields it could only describe
one.

Nested objects are now edited as their own small grid, with the sub-schema
resolved through `$defs` so a nested number carries the same range and the same
tooltip it would at the top level. `null` says *not set* rather than printing
the word `null` in the grey a real value uses.

**3. Writing a nested object shredded the author's file.**

The inspector writes the whole object — one number changed means
`WaterBody.swell` is set to all three. `SetField` replaced the item, which
turned `[node.components.WaterBody.swell]` and the eight lines of comment above
it explaining why those numbers are those numbers into a one-line inline table.

That is exactly the reformatting this layer edits a format-preserving DOM to
prevent, and it would have happened on the first swell anyone touched. A header
sub-table is now walked and its keys updated in place, decor carried the same
way the scalar path already carried it; an inline table is still replaced as one
value, because that is one line whose own decor survives. Keys absent from the
object are removed, so "set" still means set rather than quietly meaning merge.

The `swell` diff went from twelve lines to one.

## Also

`loom describe` with no argument said, in its own usage text, that it *"lists
the known types"*. It printed the usage and exited 2. Which is a small lie about
a small command, except that "what can I add to this scene?" is the first
question an agent asks and the answer was nowhere. It lists them now, off the
same registry the menu reads.

The Create menu's five primitives were a copy of `loom_asset::primitives::NAMES`
with nothing holding them together — `loom_editor` cannot depend on
`loom_asset`, and acquiring an edge to share five strings costs more than it
saves, so a test in `loom_cli` (which sees both) asserts they agree and that
every offered name actually builds.

The component-copy button rendered as a tofu box: `⧉` is not in the bundled
font. It says `copy`.

## Consequences

An optional nested object that is absent still cannot be *created* from the
editor — `Swell`'s fields are deliberately "refused at load rather than
defaulted into silence", so there is no default a button could write that would
load. Copying the component from a node that has one brings it across, and the
CLI or the agent can author one. Inventing plausible physics numbers in a button
is the thing this project does not do.

## Addendum, after the critic — 2026-09-09

A fable critic graded the editor 7/10 against the bar of 8 and found three
things this ADR had missed or half-fixed.

**The whole of a component, not the part written down.** The Inspector walked
the node's *stored* table, so a field the author never wrote did not appear —
and since TOML cannot hold a null, every optional field of every component was
invisible **by construction**. `WaterBody` showed seven of sixteen. `swell`,
`fetch`, `flow` and `extent` were not "read-only" as this ADR first said; they
were simply absent, with no way to reach them at all. It now walks the union of
the stored keys and the schema's, both alphabetical so they interleave in place
rather than appending a second block, with an unwritten field showing the value
that is actually in effect — its schema default.

An optional field whose default is `null` shows **not set** with a `set` button
that writes the schema's own `minimum` — the smallest value its author declared
legal, so nothing is invented here. `Rain.duration` becomes a shower of zero
seconds you drag out; `WaterBody.fetch` becomes one metre you drag up. An
optional *object* gets no button, and that is deliberate: `Swell`'s fields are
documented as refused at load rather than defaulted into silence, so any value a
button could write would be rejected, and a button that always fails is worse
than none — the mistake ADR 0101 already fixed once in "+ add". The hover names
the two paths that do work: copy the component from a node that has one, or
author it with `loom scene --tx`.

**An intermittent crash in the command the docs verify water with.**
`sample_water` asserts, debug-only, that the cascade is asked about the tick it
was evolved to. `loom water` and the `water@` assertion path each computed
`ticks / 60.0` while the step accumulated `tick * (1.0 / 60.0)`. Those are a
last bit apart at some tick counts and equal at others: `--sim 300` panicked
with `t = 5.0000005` against `t = 5`, and `--sim 301` was fine. The fix already
existed in `submerge_eye`, whose comment names these two callers by name and
explains the arithmetic — written, and never applied to them. Both now ask the
ocean for its own instant, and `loom water` reports that instant, since its
output documents `seconds` as what the waves were evaluated at.

**A design doc that had become false.** `docs/design/EDITOR-SCOPE.md` still said
four tabs render "not built yet", that there is no snap, no hierarchy filter and
no profiler in the UI. All four were built between then and now. It opens by
promising every claim in it was verified — so it had become the exact failure it
was written to avoid, and a reader came away with four false beliefs. It now
carries a superseded header naming what changed and where; its comparison
against Unity, Unreal and Godot, and the gaps it lists, still stand.

## Second addendum, after the re-grade — 2026-09-09

The critic came back 7.5/10 and found one new defect of its own making — mine,
strictly: the union of stored and schema fields made an old bug ubiquitous.

**An empty array was a dead row.** `Iterator::all` is vacuously true on nothing,
so `[]` matched the "all numbers" arm, drew zero drag boxes, and rendered as a
blank line — shadowing the arm that owns `+ add`. That is every
`Environment.stages` before its first stage, every `VoxelVolume.ops` before its
first op, every `Buoyancy.pontoons` before its first pontoon, and any
object-array whose last entry was just deleted. The bug predates the union
change; the union change is what put an empty array on screen for every unwritten
array field in the engine.

An empty array cannot answer for itself, so `array_is_numeric` asks the schema:
object items, or the untyped `items: true` a voxel recipe declares, route to the
object arm where `+ add` lives; only a declared numeric item stays numeric, so
nothing offers to append a table into a colour.

**The nested grid got the same union**, for the same reason one level up did: a
`Swell` authored without `fetch` — legal, absent means unlimited — showed two of
its three fields with no way to reach the third.

**The "no button" hover was giving a confident wrong answer.** It told anyone
hovering `WaterBody.extent` that its "fields are refused rather than defaulted",
which is true of `Swell` and meaningless about a two-component vector. A nullable
list and a nullable group now say different things, because they are absent for
different reasons.

**And glTF was already supported.** The critic ranked "glTF import" as the single
biggest thing missing, and `import_gltf` has been in `loom_asset` — a dependency,
a tested path, and the extension dispatch every `MeshRenderer` goes through —
since long before the drop gesture existed. What was wrong was one whitelist in
`import_file`: it took `.obj` and `.png` and turned glTF away at the door, so the
engine could load a format its own editor refused to import. `.glb` and `.gltf`
are accepted now, and because a `.gltf` is a manifest rather than a model, the
buffers and images it names beside itself are copied with it — a relative,
flat, non-`data:` URI only, with anything else reported rather than silently
dropped.

That last one is worth naming as a category: the sharpest finding of two review
rounds was not a missing feature. It was a capability the engine already had and
one line of the editor refused.

## Third addendum, after the third pass — 2026-09-09

7/10. The score went *down*, correctly: the previous round's fix shipped a
defect worse than the one it closed, and two claims in this very ADR were
false.

**"+ add" wrote scenes the engine will not load.** `sorted_insertion` returned
`Some((0, 0.0))` for *any* empty array — a test asserted it — so the `at` that
belongs to a `MoodStage` was injected into the blank entry of every empty array
in the engine. On `VoxelVolume.ops` that produced `ops = [{ at = 0.0 }]`, which
the op layer **accepted** and `loom validate` then refused with
`missing field 'kind'`. The button's own comment said "appending something
invalid would be worse: the transaction would be rejected and the button would
look broken" — describing a better outcome than the one that shipped, which was
accepted *and* broken. Making every unwritten field visible is what put that
button on screen everywhere.

Two changes. `sorted_insertion` asks the item schema whether it declares an
`at`, instead of assuming an empty array is a stage list. And "+ add" is offered
only where the editor can build an entry the engine will load — an item schema
with `properties`. Measured across every object-array in the engine:
`Buoyancy.pontoons` and `Scatter.exclude` accept `{}`, `Environment.stages`
takes its `at`, and `VoxelVolume.chunks` and `VoxelVolume.ops` accept nothing
the editor can construct. `ops` is `Vec<serde_json::Value>` because its
vocabulary lives in `loom_voxel`, which `loom_scene` may not depend on — so the
schema genuinely cannot describe an entry, and the honest answer is a disabled
button that says so.

That exposes something worth naming: **the op layer can write a scene
`loom validate` rejects.** `apply` re-parses with `Scene::parse`; the voxel-op
check lives in the CLI, on the far side of a dependency boundary `loom_scene`
cannot cross. Two validators, and the op layer is the weaker one. Not fixed
here — the boundary is deliberate — but the button is no longer the place that
discovers it.

**The array `$ref` was never followed.** `field_schema` resolves a field's own
`$ref` and stops, so an array of structs arrived with
`items: { "$ref": "#/$defs/MoodStage" }` and every question about an entry — has
it properties, does it carry an `at`, what is this number's range — got no
answer from a reference nothing followed. Two of those decide whether a button
corrupts a scene, so `resolve_items` follows it.

**And the glTF claim in the second addendum was half true.** The engine did
always read glTF, and the whitelist was a real bug. But `import_gltf` iterated
`document.meshes()` and never looked at the scene graph: a file placing one
pyramid mesh at two nodes six metres apart imported as **one pyramid at the
origin**. Every node transform discarded, every instance collapsed, every mesh
in the file merged whether a scene referenced it or not. Blender writes a node
per object, so that was wrong for essentially every real export — and wrong
silently. "The engine could always read glTF" was true only of single-mesh,
origin-anchored geometry: the fixture, exactly.

It walks the scene graph now, baking each node's accumulated transform into the
vertices, with normals through the **cofactor** of the upper 3×3 — the inverse
transpose without the determinant, which a direction does not need. `#Name`
selects one node's subtree, the same fragment `import_obj_object` has, and it
reaches both importers now instead of only the OBJ one. A missing `NORMAL`
attribute is computed as flat normals per the spec, which is what the comment
beside that code already claimed while the code filled a constant `[0, 1, 0]` —
every face lit as though it faced up.

Fixtures: `two_pyramids.gltf` (one mesh, two nodes, ±3 m) and `pyramid.glb`.
Nothing in this repository had ever loaded a `.glb`, the format anything real
ships in.

**Three more documentation claims were false and are corrected**: this ADR said
sidecar URIs outside the flat folder were "reported rather than silently
dropped" and they were silently dropped (they are reported now); the
array-of-objects comment credited fixing `WaterBody.waves`, which is a
`WaveSet` object and never reaches that arm; and `EDITOR-SCOPE.md`'s superseded
header still listed `.obj`/`.png` as the whole of import.

Still open, honestly: the per-wave Gerstner list. `WaterBody.waves.waves` is an
object-array one level below a nested object, where `object_fields` edits only
scalars — and `SetField` splits its field name once, so it cannot address that
path either. Authoring individual Gerstner waves is `loom scene --tx` work.
