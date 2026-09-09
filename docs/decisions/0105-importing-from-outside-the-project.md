# 0105 — Importing from outside the project

Status: accepted · 2026-09-08

## Context

`SceneOp::Declare` has existed since the op layer did, and its own doc comment
says why: *"Authoring these was CLI-only, which meant the editor could reference
an asset but never introduce one — so importing a mesh or creating a prefab
could not be an editor action at all."*

The op was written. Nothing in the editor ever called it. A model you had was a
model you could not use without editing the scene file by hand.

## Decision

**Drop a file on the window.** `.obj` becomes a mesh asset, `.png` a texture,
`.loom` opens that scene instead. The declaration goes through `transact`, so an
import is one Ctrl+Z and one History entry like any other edit.

- **Handled as a winit `DroppedFile` event, not egui's `dropped_files`.** egui's
  is read inside the panel closure, which is `FnMut` and may run twice in a
  frame; importing a model twice because the layout settled is a bug nobody
  would think to look for. Winit delivers one event per file, once.
- **A file from outside the project is copied in first.** A scene referencing
  `/home/somebody/Downloads/boat.obj` works on one machine, and `loom pack`
  would have nothing to fold in. A file already under the root is declared
  where it lies.
- **Never silently overwrite.** Two files called `boat.obj` from two folders are
  two models; the second one landing on the first would change every scene using
  the first. The copy is renamed rather than clobbering.
- **Only formats this engine reads.** Declaring a `.fbx` would validate and then
  fail at mesh-build time, somewhere the human is no longer looking. OBJ and PNG
  are what `loom_asset` loads, so they are what import accepts.
- The path written into the scene is **relative to the scene**, so the project
  moves as one directory. `relative_path` is the fifteen lines `pathdiff` would
  have been a dependency for.

## Consequences

The alias is the file stem, sanitised to a TOML bare key and deduplicated
against what the scene already declares — so `my model (2).obj` declares as
`my_model__2_` rather than producing a file that will not parse.

Discoverability is one line in the Project panel: *"drop a .obj or .png on the
window to import one"*. A gesture nobody is told about is a gesture nobody uses.
