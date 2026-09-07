# ADR 0092 — Character throughput, measured rather than assumed

- **Date:** 2026-09-07
- **Status:** **accepted**. Closes the last of the nine subsystems scoped as
  "what is missing for a full-blown game engine".
- **Decision touched:** **ADR 0072 is upheld.** A character stays separate rigid
  parts on a node hierarchy. Skinning is still not in this engine.
- **Adds:** no code. A number.

## 1. Why this was on the list

The survey said "character throughput" was missing, and the plan attached to it
was skinning plus a per-frame BLAS refit — refit being the thing that would fix
ADR 0072's actual objection, which is that a skinned surface is displaced in the
vertex shader and therefore every ray sees it at its rest pose.

That is a large change: the asset loader, the scene schema, the vertex shader,
and a per-frame acceleration-structure rebuild. It also overturns a decision
that was made carefully and is load-bearing for reflections, shadows and RTAO.

**So the first question is not "how do we make characters cheaper" but "how
expensive are they".** Nobody had measured.

## 2. What was measured

Generated scenes with 1, 4, 8, 16 and 32 deckhands — the shipped prefab, 43
nodes and 27 mesh leaves each, with all fourteen joint scripts running.

Simulation, 300 ticks, wall clock:

| deckhands | total | marginal per character |
|---|---|---|
| 1 | 44 ms | — |
| 4 | 135 ms | |
| 8 | 273 ms | |
| 16 | 552 ms | |
| 32 | 1174 ms | **36.5 ms / 300 ticks** |

Linear, at **0.12 ms per character per tick**.

Windowed, 90 frames, simulation running:

| deckhands | CPU mean | CPU worst |
|---|---|---|
| 1 | 0.443 ms | 29.8 ms |
| 8 | 0.908 ms | 41.6 ms |
| 32 | 2.706 ms | 78.7 ms |

About **0.073 ms per character per frame** on the render side, also linear.

## 3. The conclusion

Roughly **0.19 ms of CPU per character per frame**, both halves together. A
16.67 ms frame at 60 Hz therefore holds on the order of **85 characters** if
they were the only thing running, and comfortably **30** alongside everything
else a scene does.

DEEPER is co-op for a handful of players and the things in the water with them.
**The puppet is not the bottleneck, and skinning would not buy anything this
game can spend.** ADR 0072 stands, unchanged, on evidence rather than on
inertia.

If a scene ever wants a crowd — a harbour, a horde — this ADR is the number to
re-measure against, and the skinning-plus-refit plan is the thing to cost then.

## 4. What the measurement did surface

**The worst frame grows with the crowd**: 29.8 ms at one deckhand, 78.7 ms at
thirty-two. That is a load-time spike, not a steady-state cost — the mean is
flat and small — and it is almost certainly the acceleration structure being
built for the first time. Nothing in this project measures it, so it is written
down here rather than left as folklore.

**A prefab is not relocatable.** Scripts declared *inside* a prefab resolve
against the directory of the *scene* that instantiates it, not the prefab's own
directory. Instantiating `assets/prefabs/deckhand.loom` from a scene outside
`assets/` fails on `../scripts/deckhand_hips.rhai`. Every shipped scene lives
one directory below `assets/`, so nothing in the repository trips over it. It
is still wrong, and it will bite the first person who keeps a prefab library
somewhere else.

Neither is fixed here. Both are real, both are recorded, and both are cheaper to
fix once someone has decided they matter.
