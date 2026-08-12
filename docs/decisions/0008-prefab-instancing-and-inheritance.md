# ADR 0008 — Prefab instancing, overrides and scene inheritance

- **Date:** 2026-08-11
- **Status:** accepted
- **Decision touched:** implements `docs/format/README.md` §5 and
  LOOM-IMPLEMENTATION-ORDER.md S4

## Context

§5 of the format spec has described prefab instances since M1 and nothing read them. The parser
**refused** `prefab`, `extends` and `[node.overrides]` rather than ignoring them, for a reason worth
repeating: a key the parser does not know is a key it *ignores*, so a prefab instance node arrived
with no components at all — it drew nothing, lit nothing, and the scene validated clean.

S4 also blocks Phase 5 outright. A scattered instance *is* a prefab instance — mesh plus material
plus transform plus per-instance overrides — so building scatter first means building this twice.

## Decision

Unity's delta model: the consuming file stores a **source reference plus explicit modifications**,
never a copy of what it placed. A copy is the one arrangement in which editing a prefab cannot
update what was placed.

### Resolution produces an ordinary scene

`loom_scene::prefab::resolve` returns a `Scene` with no prefab keys left in it. This was the
load-bearing choice: the renderer, `measure`, physics and the ECS understand prefabs **without
knowing they exist**, and `unpack` is the same code writing its result into the file instead of
keeping it in memory. The alternative — teaching every consumer about instances — is the same work
repeated per consumer with a fresh chance of disagreeing.

The flattened scene is a derived artifact. Byte-identical round-trip remains a property of the
source file, which resolution never touches.

### A library is keyed by `id`, never by alias

An alias is file-local. Two files may use one word for different prefabs, and the same prefab under
two words. Keying a shared library by alias would make a prefab's meaning depend on which file
loaded first.

The same reasoning forces something §5 does not mention: **asset aliases must be merged**.
Flattening a prefab that calls its texture `"wood"` into a scene that calls a *different* texture
`"wood"` has to keep them apart, so the merge dedups on asset id and renames the loser. Without it,
instancing silently repaints geometry.

### Setting a field on an instance writes an override

An instance declares no components — the parser rejects a node carrying both, because two sources
for one component have no rule about which wins. So `SceneOp::SetField` on an instance writes into
`[node.overrides]`. The editor's inspector and `loom scene --tx` therefore need no idea which kind
of node they are looking at; without this they would each need the branch, and a caller that forgot
it would produce a file the parser refuses.

### `apply-overrides` is two files, and says so

Promoting a deviation into the prefab writes the prefab (which gains the value) and the instance
(which loses the override). A `Transaction` is scoped to one scene, so this is **two transactions
and two undo steps**, and the command reports `undo_steps: 2` rather than implying one. Inventing a
cross-file transaction to make one command look tidy would put the version-token discipline —
already the most delicate thing in the format — under a second, weaker mechanism.

The prefab is written first. Doing the instance first could strip the overrides and then fail to
record them, which destroys the author's work: the never-do #15 shape of mistake.

Everything still goes through `SceneOp` and `apply_to_file`, so never-do #16 holds — there is no
second code path an editor could take, and a multi-op prefab transaction undoes in one Ctrl+Z
because the undo payload is the whole previous text.

### `unpack` needs the prefabs, so `apply_with` takes a library

`apply` cannot know what an instance stood for. `apply_with` takes a `Library`; `apply` calls it
with an empty one, and an unpack against an empty library reports `prefab_library_required` rather
than the misleading "prefab not found". `apply_to_file` builds the library **inside the lock**, from
the scene as it then stands — which is also how the editor gets unpack for free.

**A correction to record:** the first S4 commit claimed `loom_scene` "deliberately cannot read
files". It never was true — `edit.rs` reads, writes and locks. The prefab loader now lives in
`loom_scene::prefab::library_for` rather than being duplicated in the CLI.

### `extends` merges field by field

A derived scene declaring `[node.components.Light] intensity = 5.0` means *change the intensity*.
Replacing whole components would make every change a full re-declaration — and §4 omits defaults, so
a full re-declaration is not even writable.

**The one thing it cannot express** is resetting an inherited transform to exactly identity. Omitted
*is* identity (§4), so "wrote identity" and "wrote nothing" are indistinguishable in the file, and
the rule is that a non-default transform wins. Overriding a single axis works; anything non-identity
works. Unpacking is the escape hatch.

`extends` is legal on the root only. Inheritance is a property of the scene; a mid-tree `extends`
would be a prefab instance spelled differently, and two spellings for one thing is how a format
grows a dialect.

### Orphans warn, cycles fail

An override targeting a path the prefab no longer has is a **warning with the value preserved**,
surfaced by `loom validate` under `overrides`. A prefab that renamed a child should not make twenty
scenes fail to load, and it must not silently discard what the author wrote — Unity's handling of
this is a known pain point and reproducing it would be a choice.

Instancing and inheritance cycles are refused with the **whole cycle path**, because "a includes b
includes a" is fixable and "a is in a cycle" is a search.

## Two defects the end-to-end run caught

Both were invisible to the unit tests written before them, and both are now pinned by one:

- **`1.4` came back as `1.399999976158142`.** A `Transform` holds `f32` and JSON holds `f64`, so
  serialize-then-convert replaced the author's number with noise — in a format whose stated premise
  is that the authored value is the source of truth. `f32::to_string` gives the shortest decimal
  that round-trips.
- **Defaults were written out**: `rot_euler = [0,0,0]` and `scale = [1,1,1]` onto nodes that never
  had them, against §4.

## Consequences

- Phase 5 scatter builds on instancing rather than reinventing it.
- The verbose "a floating crate is a node with MeshRenderer + collider + Buoyancy written out per
  instance" shape that Phase 2 would otherwise inflict on every scene is now one line per placement.
- **Any new command that reads a scene must go through `prefab_load::for_reading`.** The parser now
  accepts `prefab`, so a command that skips resolution reintroduces the silent-empty-node bug the
  refusal used to prevent. This is the single most likely way to regress S4.
- Comments inside a prefab do not survive into a scene that instanced it. There is nowhere sensible
  to put them and no one to read them there.

## Human approval

Not required — this implements a section of the format spec that was already normative, rather than
changing a locked decision.
