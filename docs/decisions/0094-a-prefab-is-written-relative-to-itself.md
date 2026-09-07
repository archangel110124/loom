# ADR 0094 — A prefab is written relative to itself

- **Date:** 2026-09-07
- **Status:** **accepted** and built.
- **Decision touched:** none. This is a defect in path resolution, recorded as
  an open item in ADR 0092 §4 and fixed here on the human's instruction.

## 1. The bug

A prefab declares its scripts the way any file declares its neighbours:
`deckhand.loom` says `../scripts/deckhand_hips.rhai`, meaning *beside my own
folder*. The scene that instances it resolved that string against **its own**
directory instead.

Every scene in this repository sits one level under `assets/` — `assets/games`,
`assets/test` — and every prefab sits in `assets/prefabs`. So `../scripts/…`
means `assets/scripts/…` from either of them, the two readings agree, and
nothing has ever noticed.

A prefab kept anywhere else breaks. It breaks as **a missing script**, not as a
path problem, which is the kind of error somebody spends an afternoon on.

Found while generating throughput scenes outside `assets/` (ADR 0092): a scene
in `/tmp` instancing the shipped deckhand failed with
`"../scripts/deckhand_hips.rhai": No such file or directory`.

## 2. Decision

**Rebase a prefab's file paths onto the consuming scene's directory, once, as
the prefab is loaded.** `collect` already knows both directories at every level
of nesting; it now rewrites the text before `Scene::parse` sees it.

Three components have a `path` field that names a file — `Script`, `GameRules`,
`Bindings` — checked against `components.rs`. Everything else with a `path` is a
*node* path, and rewriting one of those would break a reference rather than fix
one.

## 3. Why text in, text out

The obvious implementation mutates the parsed prefab. `Scene` keeps a
`DocumentMut` and a `Vec<Node>` that must agree, exposes neither mutably, and is
right not to.

So this is a pure `&str -> String` transform over `toml_edit`. It cannot leave
the document and the nodes disagreeing, it needs no new mutable surface on
`Scene`, and it is testable without a filesystem.

## 4. Relative, never absolute

The tempting shortcut is to rewrite each path to its absolute resolved form.
That is wrong here, and the reason is two calls away: `expand_instance` feeds
the **unpack** op, which writes the expanded nodes into the author's `.loom`
file. An absolute path there is correct on exactly one machine.

So the rewrite produces a relative path from the consuming scene to the target,
computed lexically — no filesystem, no symlink resolution — and declines
(leaving the path alone) when the two directories share no root and there is no
honest relative answer.

## 5. The regression this nearly shipped

The first version replaced the whole key with `Table::insert`, which takes the
key's decor with it — and the key's decor is where the author's indentation
lives. A rewritten `path` came back flush against the margin while its
neighbours stayed indented.

This project has explicit tests for keeping a human's comments and formatting
through an edit, and this would have quietly violated that for every prefab a
scene loads. The fix mutates the *value* and restores its decor; there is now a
test asserting the indentation survives.

## 6. What is checked

- The case that has always worked — `assets/prefabs` to `assets/games` — rebases
  to itself. If that changed, every scene in the repository would break at once.
- The case that was broken: a prefab in `/lib/prefabs` reaches its own scripts
  from a scene in `/game/assets/games`.
- An absolute path is left alone.
- Only the three file-path components are rewritten; a `CharacterController`
  beside them is untouched.
- Same directory in and out is the identity, byte for byte.
- End to end: a scene in `/tmp` instancing the shipped deckhand by absolute path
  now loads 46 entities, where it used to fail on a missing script.
