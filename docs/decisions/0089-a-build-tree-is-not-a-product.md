# ADR 0089 — A build tree is not a product

- **Date:** 2026-09-07
- **Status:** **accepted** — seventh of the nine subsystems scoped as "what is
  missing for a full-blown game engine", at the general-engine bar.
- **Decision touched:** none relaxed. In particular **LOOM-BUILD-BRIEF §3 still
  holds**: `loom_scene` depends on `loom_reflect` and nothing else.
- **Human decision this records:** a self-contained Linux directory first, then
  asset packing — *"A then C"* — and that packing covers everything the running
  game opens.

## 1. The gap

Everything before this proved the engine works on a machine that has the
repository, a Rust toolchain and a shader compiler. A player has none of those.
There was no answer to "how do I give this to someone", and no command that
produced anything a stranger could run.

## 2. Decision

`cargo xtask dist` produces `target/dist/deeper/` — the binary, one
`assets.pack`, and a launcher — and a `deeper-linux-x86_64.tar.zst` beside it.
`loom pack <dir> <out>` folds a tree into the archive and is a real command
rather than a build-script detail.

**Shaders do not ship.** `build.rs` compiles the Slang to SPIR-V and
`include_bytes!`s it into the binary, so `assets/shaders` is source nothing
opens at runtime. Notes (`.md`) stay out for the same reason. Everything else
the running game reads — 341 files: scenes, prefabs, meshes, textures, scripts,
audio — is in the pack.

## 3. Why the paths did not have to change

Scene files reference their neighbours relatively — `../meshes/gleamsprat_chrome.obj`,
`../prefabs/jib_vi.loom`. So the tree only has to keep its *shape*: there is no
root to configure and no working directory to be in. That is why the launcher
can `exec` from anywhere, and why the check below runs from `/`.

Pack keys are the same paths relative to the assets root. A lookup normalises
the path lexically — the scene format is full of `..` and a key cut before
resolving them would miss every prefab in the game — finds the last `assets`
component, and takes what follows.

## 4. The format, and what it deliberately is not

```text
"LOOMPACK"  u32 version  u32 count
count x { u16 key_len, key, u64 offset, u64 length }
blobs
```

- **No compression.** The distribution archive already compresses; decompressing
  twice to read one mesh is work nobody asked for.
- **No memory map.** A positional read per asset is simpler, and assets load
  once. Each read opens its own handle, so the pack is thread-safe by
  construction rather than by a lock.
- **Sorted entries**, so packing the same tree twice gives the same bytes. A
  build output that changes without its input changing is one nobody can check.

## 5. A hook, not a dependency

`loom_scene` resolves prefabs, which means it opens files. The pack reader lives
in `loom_asset`, and **`loom_scene` may not depend on it** — the brief makes it a
leaf, and that rule is the reason the crate graph is still legible.

The two honest ways out were to relax the rule, or to thread a source through
every prefab signature. Both spend a lot to satisfy a rule that exists to keep
this crate small. So `loom_scene` exposes `set_reader`, and whoever mounts a
pack fills it in — set once at startup, like a logger. `loom_scene` still
depends on `loom_reflect` and nothing else, and `scripts/check-deps.sh` agrees.

## 6. How it is checked

The silent failure mode is a new `std::fs::read` somewhere on the load path: it
works perfectly in a checkout, which has the loose tree, and breaks only the
packed build — which is the only build anybody outside this repository runs.

So `scripts/green.sh` packs the tree, puts the pack beside a copy of the binary,
and runs the same scene **from `/`, with no assets directory anywhere near it**.
Same `state_hash` as the loose run or the row fails. That one comparison covers
scenes, prefabs, meshes and scripts; `cargo xtask dist` plus a one-frame render
covers textures.

## 7. What this does not do

- **Linux only.** A Windows cross-build of a Vulkan binary is a different piece
  of work and was explicitly not asked for yet.
- **No reachability walk.** Every asset is packed, including test scenes DEEPER
  never opens. Walking the scene graph would shrink the archive *and* catch a
  scene referencing a file that is not there — the obvious next step, and not
  built speculatively.
- **No versioning of the pack against the binary.** The format carries a version
  and refuses one it does not know; it does not check that the assets and the
  executable came from the same build.
- **Saves are not packed.** A pack is read-only and written at build time; a save
  is written by the player, to a path they chose.
