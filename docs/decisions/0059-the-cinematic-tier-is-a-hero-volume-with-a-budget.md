# ADR 0059 — The cinematic tier is a hero volume with a budget, and the height field is the water system

- **Date:** 2026-08-19
- **Status:** **proposed** — needs human approval before merge.
- **Constrains:** ADR 0053 and ADR 0057. It changes no mechanism in either. It
  writes down what the tier is *for*, and enforces it at load rather than in
  prose.

## Context

The human's report was that cinematic water runs at 30–40 fps on an RTX 4090
and that a much weaker card is therefore out of the question. The ruling that
came back from the review was unanimous on the architecture — keep the
APIC/multigrid solver, change nothing about the family — and unanimous on the
scoping: **demote it by contract from "the water system" to a budgeted hero
volume.**

The number that settles it is a pair, not an opinion. The same authored event —
a crate down a 55° ramp into a pool — measured on this machine, quiet, 1440x900,
`loom run --frames 300 --play`:

```text
   plough.loom             (deterministic height field)    145.7 fps
   plough_cinematic.loom   (APIC/multigrid volume)          36.6 fps
```

After the work in ADR 0058 and the frame fixes shipped alongside it,
`plough_cinematic` is at 141.5 fps and vsync-bound — so the tier is now
affordable *at this size*. That is exactly why the budget has to be written
down now rather than after the first scene that asks for a lake.

## Decision

### 1. The deterministic height-field/wavelet tier is the water system

Every water surface in a scene is `simulation = "deterministic"` unless there is
a specific reason it cannot be. It carries force, `--assert`, the sim hash and
the whole of `loom_water`; it is what a level is made of.

### 2. The cinematic tier is one hero volume per scene, refused at load

`loom_scene` refuses a second `WaterBody` with `simulation = "cinematic"`, with
the reason in the message. The engine builds exactly one `FluidSolver` — with
its own Vulkan device, its own particle buffer and its own synchronous readback
inside the fixed step — so a second would silently not solve. That is the
failure mode ADR 0047's emitter refusals exist to replace, and the same pattern
answers it.

### 3. A cell budget, with the number in the error

`MAX_CINEMATIC_CELLS = 65,536`, refused at load. The grid is 64 cells along the
longest axis (ADR 0057), so the budget is really a statement about the *shape*
of the domain: a long shallow tank at 6.4 × 1.6 × 3.2 is 32,768 cells, and a
cube of the same length is 262,144 — eight times the cost for the same hero.

65,536 is twice what `plough_cinematic` uses, and `plough_cinematic` measures
**3.2 ms a tick** on an RTX 4090. At 60 Hz that is 19% of a frame before
anything is drawn. Anything past the budget is asking for the whole frame, and
the message says so with both numbers in it.

**The budget is stated in cells and not in milliseconds**, which is honest about
what is being bounded. Per-tick cost is dominated by dispatch *count* — 296 of
them, most on multigrid levels holding a few hundred cells — and that count is a
function of the level structure, not of the cells in it. What the cell budget
actually bounds is the particle count, the P2G gather and the CPU marching
cubes, all of which are linear in cells. Until the coarse-level dispatch fusion
lands, shrinking a domain does not buy back much, and the budget should not
pretend otherwise.

### 4. Compositing is presentation-only

Where a cinematic volume sits inside a scene that also has deterministic water,
the volume's marched surface is drawn over the height field's. The height field
keeps sole authority over force, `water@x,z` and every `--assert` outside the
box. ADR 0053 §5 already says the tier does not leak into deterministic bodies;
this states the drawing half of it.

## What was ruled out, and why it is written here

**Changing the solver family.** PBF, DFSPH and MLS-MPM were all rejected. The
solver is not the frame cost when it is healthy: measured on the viewer, the
GPU render graph is 0.56 ms and the solver's own tick is 3.2 ms, of which most
is fence wait and dispatch launch. The frame was a CPU particle sort, three
blocking fences and a serial CPU march, and those are what the commits around
this ADR fixed. Replacing a working solver to fix a sort would have been the
expensive way to learn that.

**Shrinking the domain as a performance lever.** Measured flat in domain size —
see §3. It is a *quality and memory* lever today and becomes a cost lever only
after the coarse-level fusion.

## Consequences

- A scene may hold one cinematic volume of at most 65,536 cells. Every scene in
  this repository already satisfies both, and none re-blesses.
- `loom_scene` gains `MAX_CINEMATIC_CELLS` and two refusals, and duplicates two
  lines of grid arithmetic from `loom_render::fluid_grid`, because `loom_scene`
  depends on nothing else in the workspace and that is CI-enforced. The test
  pins the two together with a real scene's numbers.
- The tier's scope stops being a paragraph in a companion document that a scene
  author never reads, and becomes a message that arrives when they try it.
