---
name: loom-scene-authoring
description: Use when hand-authoring or editing a Loom .loom scene — a test scene, a building, a landform, a demo — or when a scene validates clean and draws nothing, or draws the wrong thing. Covers the op-composition traps, framing, and what the loader silently ignores.
---

# Authoring a `.loom` scene

The format is normative in `docs/format/README.md`. **Do not learn it from
here, and do not restate it there.** This is the procedure and the traps.

**Scene authoring is the one activity in this project with no gate at all.** A
`.loom` file changes no Rust and no Slang; `cargo xtask validate` renders only
`SCENES` and `cargo xtask image` compares only `GOLDEN`, so a scene in neither
is invisible to both. `3f3368d` says so in its own commit message. The failure
mode is therefore silent, and it is expensive: `ef9fe95` lists **nine rules it
cost a render each to learn**.

## Order of operations

1. **`loom describe <TypeName>`** for a component's JSON Schema — never from
   memory. To list the known types, ask for a name that does not exist:
   `loom describe ?` returns `unknown_component_type` with a `hint` listing all
   of them. (The built-in usage text claims a bare `loom describe` lists them;
   it does not — it prints usage and exits 2.)
2. Author the file.
3. **`loom validate <scene.loom>`** — exit 1 prints JSON errors and the version
   token.
4. **`loom measure <scene.loom> [--node <path>]`** for bounds and overlaps, and
   `--shape` for how rocky a shape is. Both answer questions a render cannot,
   before you spend a render.
5. **Render, and open the PNG.**
6. If the subject moves, use the `watch-loom` skill. *"Smoke authored at the top
   of a chimney shaft is smoke inside the cap. Found by a 20-frame contact
   sheet; the still had nothing to show either way."*

## Op composition — the traps that cost a render each

- **`subtract` applies to everything present, and an extended plane is
  infinite.** Op order across volumes is load-bearing and must be checked
  **arithmetically**, not by looking. In `croft.loom` the byre's roof spheres
  run *first*, and the main roof's after, which is safe only because the byre
  sits under both planes extended by 0.57 m at its tightest corner.
- **A fillet smaller than one voxel does not exist** — the mesher cannot place a
  vertex for it.
- **A world-axis `elongate` cannot follow a wall rotated off the axes.** A bank
  behind an 18°-rotated house drifted 2.8 m over the building's length and
  wrapped in front of the door. Use a `capsule` laid parallel.
- **A `VoxelVolume` carries one material plus one slope-keyed layer, so a
  material break is a *volume* break.** A building is three volumes (ground,
  walls, roof), not one.
- **Voxels where the geometry is boolean, box primitives where it is a plane.**
  A window is one `subtract`; in primitives it is four piers, a header and a
  sill. But a voxel surface at 0.16 m cannot hold a true arris, and a fascia
  edge is what defines a modern roof — so slabs, columns, glazing and paving
  are primitives.
- **A pitched roof out of ops that cannot tilt**: a box, minus two spheres of
  radius 300 tangent to each pitch — planar to under a centimetre across the
  pitch.
- **A height map coarser than its voxels** wastes both (`bee7a94`); **glazing
  smaller than the holes it fills** leaves a gap (`d7fb433`).

## Framing — half the defects

- **Aiming the lens at a building's centre is not centring the building.**
- A three-quarter view needs **both faces near 45°**. A first attempt gave the
  façade 31° and the gable 77°, and the doorway foreshortened to nothing.
- **A thin plume against a bright sky must be darker than its background.** At
  0.36 grey it was indistinguishable from the cloud deck.
- **A scene authoring a `Grass` field owes its ground the colour of grass** —
  the density falloff thins the far field to nothing, and brown soil then reads
  as ploughed earth from any orbit.
- **A `Grass` rectangle's edge is a hard line** and no falloff hides it. Run
  every field off the flat onto a brow, where the slope rule culls the boundary
  first.
- A wall standing clear of the turf on nothing is invisible in the shot you
  framed it in. Found the byre at 9.25, not at its floor.
- A 4 m slab is a light trap: ambient is the only thing lighting under it.

## What the loader refuses, and the one thing it does not

Classes of load-time refusal (the list is in the code and in ADRs, not here):
format version from the future; `Transform` spelled as a component; a
`[node.overrides]` with no `prefab`; a GPU emitter without `additive`, on a
`RigidBody`, a second one, or an undersized pool (ADR 0047); a cascade with no
water; cinematic water without an extent or over budget (ADR 0059).

**The one that bites: a key the parser does not understand is a key it
ignores.** The parser used to skip `prefab`, so an instance arrived with no
components, drew nothing, and **validated clean**. Any command that reads a
scene must go through `loom_cli::prefab_load::for_reading`.

## The CLI contract

**An unknown flag or a bare positional is a failed invocation** (exit 2), and
that is deliberate: `778fd62` is a stray positional that wrote to the wrong file
and reported ok. `loom render scene.loom out.png` used to exit 0, report
`"out": "render.png"`, and write to the working directory. Every output path is
a `--out` value.

## Assertable feedback instead of eyeballing

```bash
loom sim <scene> --assert 'wind@x,y,z.speed >= 3'   # also rain@, wetness@, water@
loom water <scene> --at <x,z|x,y,z> [--sim <ticks>]
loom terrain <scene|recipe.toml> --from <x,z> --to <x,z> --max-slope <deg>
loom measure <scene> [--node <path>] [--shape]
```

`loom terrain` is the only assertable feedback a landscape has — a gorgeous
range with 3% buildable ground is a failed level and no hillshade reveals it.
`loom water --at` without `--sim` answers at t = 0, which is the one moment
every wave is at the same phase.

## When the human may have the viewer open

They may have `loom run <scene> --edit` open; it reloads on file change, so
your edits appear live. Use `--dry-run` first on anything large — it prints the
diff and writes nothing. Make the write conditional: `expect_version` inside the
transaction JSON for `loom scene --tx`, `--expect-version <tok>` for
`loom place`. **Never force a stale write and never merge two divergent scene
states** (never-do #15). Label the transaction usefully — it shows up in their
log panel and in the git history.

## Registering it

A scene earns a `GOLDEN` row only if it covers a rendering path nothing else
does. `croft.loom` is in `SCENES` and deliberately not in `GOLDEN`, with the
reason written out volume by volume — that is the standard to meet. See
`loom-gate-a-rendering-path`.
