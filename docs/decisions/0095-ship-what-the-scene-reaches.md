# ADR 0095 — Ship what the scene reaches

- **Date:** 2026-09-07
- **Status:** **accepted** and built. Closes the follow-up left open in
  ADR 0089 §7.
- **Adds:** `loom pack --from <scene>`, and `cargo xtask dist` uses it.

## 1. What packing everything cost

ADR 0089 packed the whole `assets/` tree: **355 files, 240 MB, a 207 MB
archive**. Of the 137 scenes in it, roughly 130 are test fixtures DEEPER never
opens.

Walking from the game's own scene ships **90 files and 8.8 MB — a 21.5 MB
archive**. Ten times smaller.

## 2. The reason is not the bytes

A disk is cheap and 207 MB is not a crisis. **The walk earns its place by
failing the build when a scene names a file that is not there.**

Without it, an asset alias that resolves to nothing is invisible at build time:
the renderer substitutes a box and carries on, which is right at runtime — ADR
0089's degrade-don't-crash — and means a missing mesh ships as a grey box the
player sees and nobody can explain. `loom validate` catches it if somebody runs
it on that scene. Nothing made it a condition of shipping.

Now it is. `loom pack --from` collects every missing reference and refuses in
one go, naming all of them.

## 3. The edges

- `[[prefab]]` declarations — which is also how `extends` resolves, so scene
  inheritance is covered by the same edge.
- `[[asset]]` declarations, minus procedural primitives: `deeper_demo` declares
  `path = "box"`, which this engine *builds* and no one can ship. The same
  escape `alias_report` has, for the same reason.
- The three components whose `path` names a file: `Script`, `GameRules`,
  `Bindings`.
- Meshes are read directly and do not pull in `.mtl`, so an OBJ is a leaf.

## 4. The bug the verification caught

The first working version packed 90 files, and the packed build then failed to
load its first prefab.

**Paths were stored unnormalised.** A prefab found as
`assets/games/../prefabs/jib_vi.loom` was written under the key
`games/../prefabs/jib_vi.loom`, while a lookup normalises lexically and asks for
`prefabs/jib_vi.loom`. The file was in the pack, under a name nothing would ever
request.

The scene format is full of `..`, so that was every prefab in the game. It is
exactly the class of defect that only appears in the shipped artifact — the
loose tree resolves `..` through the filesystem and never notices — which is why
the check below runs the packed build rather than inspecting the pack.

## 5. How it is checked

`scripts/green.sh` packs with `--from`, puts the pack beside a copy of the
binary, and runs the same scene **from `/`** with no assets directory anywhere
near it. Same `state_hash` as the loose tree or the row fails.

Verified beyond the gate row while building this: the dist renders
**0 differing pixels of 614,400** against the loose tree, so the walk misses
nothing the renderer reads either — which `state_hash`, being physics-only,
cannot see.

## 6. What this does not do

- **One scene per dist.** `GAME_SCENE` in `xtask` names it. A game with several
  entry scenes needs the walk seeded with each; the function already takes a
  starting point, so that is an argument, not a rewrite.
- **No unused-asset report.** It knows what is reachable and says nothing about
  what is not. "These 265 files ship in no build" is a different tool.
