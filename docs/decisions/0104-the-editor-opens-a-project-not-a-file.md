# 0104 — The editor opens a project, not a file

Status: accepted · 2026-09-08

## Context

The editor could switch scenes — ADR 0093 added that, and it listed the `.loom`
files **beside the open one**. In this repository that means opening
`assets/games/deeper_demo.loom` offers you three scenes and cannot reach the
other 134, including every ocean, weather and water fixture the engine's look
was built on.

So `ocean_fft`, `squall`, `mirrorpool` and the rest were reachable only by
quitting and naming one on the command line. That is a viewer with a file open.
An editor opens a project.

There was also no way to open the editor at all without naming a file, so
"open the editor and pick something" was not a thing you could do.

## Decision

**The project root is found, not configured**: the nearest ancestor of the open
scene holding an `assets/` directory. Every `.loom` under it is the scene list.

There is no project file in this engine to read a root out of, and inventing one
would be a format to keep in step with nothing. `assets/` is the convention that
already exists — every scene resolves its meshes through it — so it is the
marker. A scene with no such ancestor falls back to its own directory, which is
exactly the old behaviour.

`loom run --edit` **with no scene** opens the project's own: the first scene
under `assets/games`, then anything. Landing in `alpha_cutout.loom` because it
sorts first would be technically a scene and practically a confusing place to
arrive.

The Project panel groups by folder, counts each, and filters, because 137 scenes
in a wrapped strip is a wall and the folder is most of what tells you what a
scene is.

## Consequences

Three quiet bugs surfaced building this, all of the same kind — something that
looks like an empty result rather than an error:

- **A relative base walks off the top.** `assets/test` has two parents and then
  the empty path, so the walk ended above nothing and the browser came back
  empty. Opening a test scene silently lost the scene list that is the point of
  this. The root walk canonicalises first.
- **A sorted path list is not grouped by folder.**
  `assets/test/prefabs/a.loom` sorts between `assets/test/ocean.loom` and
  `assets/test/squall.loom`, so `assets/test` appeared twice — with a duplicate
  egui id between them, which egui drew as red error text over the panel.
  Scenes sort by (folder, name).
- **Two spellings of one path.** The browser lists absolute paths; a scene
  opened as `assets/test/ocean_fft.loom` matched none of them, so nothing showed
  as open. `scene_path` is canonicalised once, at construction.

The walk is bounded — depth 6, 512 scenes, skipping `target/` and dotfiles —
because `target/` holds a copy of the asset tree per profile and would list
every scene three times.
