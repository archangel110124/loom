# ADR 0014 — Rain drops stay stateless, and what would change that

- **Date:** 2026-08-14
- **Status:** **accepted** — as a *deferral with a trigger*, not as a closed door. The trigger is
  written down in "When to revisit" and it is specific.
- **Decision touched:** Phase 4 step 2's stateless streak renderer. Does not move a locked decision.

## The question

Can the engine simulate real in-world rain droplets — each with a position and velocity, colliding
with the world — without costing performance?

Asked because Phase 4 step 2's rain is *stateless*: a drop's position is a closed-form function of
its integer index and the time, with no state anywhere. Steps 3–5 (wetness, splashes, audio) are
about to be built on top of it, so if the foundation is wrong it is cheaper to know now.

## What the performance answer turns out to be

**Yes, affordably — and performance is not the interesting part of the question.**

Measured here, at 1920x1080 with 4x MSAA:

| | forward pass | rain |
| --- | --- | --- |
| 64,000 drops | 0.105 ms | **0.016 ms** |
| 160,000 drops (40 mm/h) | 0.104 ms | **0.036 ms** |

The literature says where the cost actually lives, and it is not simulation. Latta's *Building a
Million-Particle System* (GDC 2004) identifies **fill rate and overdraw** as the limiting factor when
particles are large and overlapping — and notes that the concern *recedes* as particles get smaller.
Raindrops are about as small as particles get, which is exactly what our 0.036 ms is showing.

The historical bottleneck was never GPU arithmetic: it was **CPU→GPU transfer**, which caps
CPU-driven particle systems at roughly 10,000 particles per frame. Keeping both simulation and
rendering GPU-resident removes it. A stateful version here would be a position/velocity buffer of a
few megabytes and one compute dispatch — cheaper, almost certainly, than the draw already is.

So cost is not the reason to stay stateless.

## Why this engine is unusually well-placed for the stateful version

Niagara offers two GPU collision modes:

- **Scene depth** — cheap, but screen-space: a drop does not exist behind the camera or off-screen.
- **Distance field** — world-correct, but the distance field has to be *generated* from meshes, and
  users routinely fight to get it working.

**Loom's terrain is already a signed distance field.** The representation Unreal users must bake is
the one this engine treats as authoritative. A GPU drop colliding against the voxel SDF is one fetch
against the same surface `sample_water`, `sky_exposure` and the collider already agree on. That is a
genuine structural advantage and it is the reason this ADR is a deferral rather than a rejection.

## Why we are not doing it yet

**The visible problem is not statelessness.** The artifact the human actually reported — rain that
reads as a *texture sliding back and forth* under camera motion — is a hash problem: the drop field
was a lattice of period `RAIN_BOX`, so the identical arrangement repeated every 72 m horizontally and
32 m vertically. Going stateful would not have fixed it. Putting the cell index in the hash does.

**The believability is not in the drops.** At rain viewing distances individual drops are not
resolvable. What a viewer perceives is streaks, **wet surfaces**, and **splashes** — Phase 4 steps 3
and 4. Streaks over a bone-dry world read as scratches on the lens no matter how good the streaks
are, and no amount of per-drop simulation changes that. The cheap steps are the convincing ones.

**Rewriting first would be optimising an unmeasured problem**, which is the mistake this project has
now made twice and caught twice — the placement compute pass deferred by GPU timings in P2, and the
projected grid rejected by arithmetic in ADR 0013.

## The honest cost of staying stateless

One correctness gap, and it should not be glossed:

**Per-drop occlusion uses W6's baked height field, which cannot express an overhang.** It is nearly
right because rain falls from above, so the topmost surface in a column is what a drop meets — but
rain blown sideways under a ledge still stops at that ledge's column, and mesh geometry stops no rain
at all. The whole-layer CPU `sample_rain` at the eye covers the gross case (walk under cover and the
rain thins), but it is per-camera: a camera half under a ledge thins *all* the rain, including what
falls in the open five metres away.

SDF-collided drops would fix that properly, and would give splashes a true collision point instead of
spawning them from exposure.

## When to revisit — the trigger

Build steps 3 and 4 first. Then reconsider **if any of these is true**:

1. Wetness and splashes are in, and drops still read as wrong under a moving camera.
2. A scene needs rain to behave correctly under mesh geometry — a bridge, a gantry, anything not in
   the voxel volume — because the height field will never handle it.
3. Splashes need to land where drops actually hit rather than where exposure says they should.
4. Sideways-driven rain under a ledge becomes visible enough to matter.

Any one of those is sufficient. None of them is an aesthetic judgement about the streaks themselves.

## What the change would look like, so the estimate is real

- A drop buffer (position, velocity), ping-ponged or updated in place.
- One compute dispatch per frame, integrating and colliding against the voxel SDF.
- A splash append buffer feeding an indirect draw — which is step 4 done properly rather than
  approximated.
- **No readback, ever.** `loom-water-system.md` §5.1 is explicit that GPU readback destroys
  determinism, and Phase 4's structural rule is that the GPU rain layer must never write anything
  feeding the sim hash. A GPU-resident drop system satisfies both by construction, provided nothing
  ever reads it back.

The rendering does not change. That is the point: the draw is already the cheap part, so the
stateful version is an addition rather than a restructuring, and deferring it costs nothing but the
gap named above.

## Sources

- Latta, *Building a Million-Particle System*, GDC 2004 —
  <https://media.gdcvault.com/gdc04/slides/building_a_million.pdf> and
  <https://www.gamedeveloper.com/programming/building-a-million-particle-system>
- Niagara distance-field collision in practice —
  <https://forums.unrealengine.com/t/niagara-gpu-particle-collision-distance-mesh-fields-not-working/235386>
- UE5 dynamic rain and global wetness —
  <https://yelzkizi.org/ue5-dynamic-rain-system-niagara-global-wetness/>
- Optimising particle system rendering — <https://realtimecollisiondetection.net/blog/?p=91>

Timings are reproducible with `LOOM_GPU_TIMING=1` at `b145b7a`.
