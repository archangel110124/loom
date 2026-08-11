<!-- Extracted from LOOM-IMPLEMENTATION-ORDER.pdf (downloaded 2026-08-11)
     with `pdftotext -layout`. Headings and the dependency graph were
     restored; every word is the original's. The extractor does not carry
     the PDF's bold and italic emphasis, so this copy has none. -->

# Loom — Implementation Order

The sequencing document. Everything designed since M12 — water, rain, wind, procedural scatter, editor work — put in the order
it should actually be built, with the reasoning for the order made explicit.

Companion docs (the what): loom-water-system.md , the rain plan, the wind plan, the PCG/editor research, loom-vulkan-backend.md ,
 loom-graphics-physics-frontier.md , loom-voxel-system.md , loom-terrain-generation.md . This document is the when, and it supersedes
the build orders inside each individual doc wherever they conflict.

Slot this at docs/design/LOOM-IMPLEMENTATION-ORDER.md and update the "Current phase" line in CLAUDE.md as you go.


## 0. The problem this document solves


Five separate research passes each produced a sensible build order in isolation. Followed independently they would cause four
specific kinds of waste:

1. Three divergent Rust↔Slang field implementations. Water, rain, and wind each independently specified "author the field in
   Rust, generate the Slang from it." Built three times, that's three codegen mechanisms and three chances to get it wrong.
2. Three occlusion queries. Rain needs sky exposure. Wind needs sheltering. Audio needs interior/exterior mix. These are one
   query.
3. Scatter built twice. Scattered instances are prefab instances. There is no prefab system.
4. Four visual systems built blind. Water, rain, wind, and vegetation are the four most visually regression-prone systems in the
   engine, and cargo test still has no pixel diff.

So the ordering principle is: build the shared substrate first, build the safety net before the things it protects, and
unblock what blocks the most.


## 1. Ordering principle


In priority order, a phase goes earlier if it:

1. Prevents rework. Anything whose absence forces you to build something twice.
2. Is a safety net for what follows. Verification before the thing being verified.
3. Is shared substrate. Built once, consumed by three or more systems.
4. Is upstream in the data flow. Wind feeds waves; waves don't feed wind.
5. Produces something runnable. Every phase ends in a demonstrable artifact, per the brief's rule.

Explicitly not an ordering criterion: how visually impressive it is. That's how rendering eats a project (brief §7.16).


## 2. Dependency graph


```text
PHASE 0 — substrate (no dependencies)
┌────────────────────────────────────────────────────────┐
│ S1 golden-image harness + deterministic fly-through    │
│ S2 Rust→Slang codegen + CPU/GPU agreement test         │
│ S3 voxel-SDF exposure / shelter query                  │
│ S4 prefab system                                       │
└────────────────────────────────────────────────────────┘
     │ S1,S2                      │ S3            │ S4
     ▼                            │               ▼
┌──────────┐                      │        ┌─────────────┐
│ P1 WIND │                       │        │ P5 SCATTER │
│ field    │                      │        └──┬───────┬──┘
└─┬──┬──┬──┘                      │           │       │
  │ │ └───────────┐               │           ▼       ▼
  │ │               │             │      ┌────────┐ ┌──────────┐
  ▼ │               ▼             ▼      │ P6 MESH│ │ P6 GPU   │
```

 ┌──────────┐ ┌────────────┐ ┌──────────┐ │ VEG       │ │ INSTANCE │
 │ P2 GRASS │ │ P3 WATER    │ │ P4 RAIN │ │ WIND │ │ CULL          │
 │ P1 + S1 │ │ wind→        │ │ wind→lean│ └────────┘ └──────────┘
 │ NEEDS NO │ │ spectra     │ │ S3→occl │
 │ S4 or P5 │ └────────────┘ └──────────┘
 └──────────┘

```text
P7 EDITOR ─── the viewport/inspector/gizmo half and the co-authoring half
              are independent of P1–P6; scatter live-preview needs P5.

CLOTH (flags) ── needs P1 only. Cheap. Slot opportunistically.
AUDIO ────────── needs S3 + P1/P4. Cheap. Slot opportunistically.


```

The one non-obvious edge: grass does not depend on the prefab system or the scatter system. A grass blade is procedurally
generated geometry in a compute shader, derived from a density field and a position hash — not a placed mesh instance. Trees and
rocks are prefab instances and so wait for S4/P5; grass does not. This is why it sits at Phase 2 rather than alongside mesh vegetation
in Phase 6.

## Phase 0 — Substrate (4–6 weeks)


Nothing here is glamorous and all of it is load-bearing. Do not skip ahead. Each item is justified by the specific rework it prevents.


### S1 — Golden-image regression harness (1 week)


Why first. The README already names this as a known gap. The next four phases are the four most visually regression-prone
systems you will ever add, and the current suite is "unit tests plus determinism hashes; image regressions are caught by eye."
Building water, rain, wind, and vegetation without a pixel diff means every numeric change to a shader is unverifiable.

What. - cargo test gains an image-comparison harness: render a fixed set of scenes headlessly at a fixed size, compare against
committed reference PNGs with a perceptual tolerance (not exact equality — driver updates shift pixels). - Commit references as
content-hashed artifacts, not in-tree binaries bloating history. - A --bless flag to accept new references deliberately. - A
deterministic camera fly-through that dumps a frame sequence. This is the part that matters most and the part most likely to
be skipped. A still PNG cannot catch unison sway, swimming vegetation, instant wind-direction snaps, impostor popping, wave-
direction snapping, or grass shimmer — every one of which is a motion artifact, and motion artifacts are the dominant failure mode of
all five systems in Phases 1–6 — and of grass above all.

Exit: a deliberate one-line shader change fails the image test; --bless accepts it; the fly-through produces a reviewable frame
sequence.


### S2 — Rust→Slang codegen + CPU/GPU agreement test (1 week)


Why here. Water, rain, and wind all specified this independently. Built once, it serves all three. Built three times, you get three
chances at the highest-severity failure mode in the whole backlog: a CPU field and a GPU field that each look internally correct while
silently disagreeing, so physics and visuals drift apart with no error anywhere.

What. - A build.rs step that emits Slang functions from a canonical Rust source for analytic field functions. You already run slangc
and spirv-val there. - A CPU/GPU agreement test that samples both implementations at a few hundred fixed (position, time)
points and asserts they match within a tight epsilon. Readback is fine in a test — determinism matters in the sim, not in a harness
asserting equality. - Pin the noise implementation used by both sides and treat its output as ABI. A crate version bump that changes
noise output silently invalidates every determinism hash.

Exit: one field function authored in Rust, its Slang generated, and a test proving they agree.


### S3 — Voxel-SDF exposure / shelter query (2–4 days)


Why here. Rain needs "is this point exposed to sky." Wind needs "how sheltered is this point." Audio needs "interior or exterior."
These are one function. Building it once means one deterministic value drives visuals, physics, gameplay, and audio mix — and it
means they can never disagree.

What. A CPU raymarch upward (or along a direction) through the i8 SDF chunks returning a smooth exposure fraction in [0,1], not a
boolean. Fixed integer iteration count — never a float-tolerance loop termination, which can diverge across platforms and break the
sim hash. In the deterministic sim, in the hash.

Why SDF and not ray queries: you have both, but the SDF march is deterministic, CPU-side, gives a smooth falloff rather than a
binary hit, and works over destructible terrain by construction (re-marching the current field). Ray queries remain the better answer
for a visual-only GPU occlusion pass if you later want per-drop precision.

Exit: exposure at a point under an overhang reads low; carving the overhang away with a CSG op raises it, in the same tick,
deterministically.


### S4 — Prefab system (1.5–2.5 weeks)


Why here. The format spec already specifies it and the parser refuses prefab = and extends = keys loudly rather than ignoring them
— so this is a designed, documented hole, not a new feature. More importantly it blocks Phase 5 outright: a scattered instance is
semantically a prefab instance (mesh + material + transform + per-instance overrides). Build scatter first and you will build it again.

What. Unity-style source reference plus explicit override deltas, per docs/format/README.md §5 and the main design doc §2.4: prefab =
"..." with a flat dotted-path override map; extends = for scene inheritance; apply_overrides / revert_overrides / unpack as first-class
ops. An override targeting a path that no longer exists is a loud warning with the orphaned value preserved, never a silent
drop.

Exit: editing a prefab updates 20 placed instances; per-instance overrides survive; the whole thing round-trips byte-identically and
undoes as one transaction.

Bonus payoff: this also fixes the "a floating crate is a node with MeshRenderer + collider + Buoyancy written out per instance"
verbosity that Phase 2 would otherwise inflict on every scene.

## Phase 1 — Wind field (1 week)


Why before water and rain. Wind is upstream in the data flow. Water waves should be derived from wind speed and fetch via an
ocean spectrum rather than hand-authored amplitudes; rain streaks must lean along the wind vector and drift with it. Build water first
and you hand-author wave parameters, then retrofit the derivation — which means re-tuning every authored ocean.

It is also the cheapest of the three field systems, because S2 already exists.

What. loom_wind : a pure deterministic wind_at(pos, t) -> Vec3 composed of a base directional vector (Beaufort-scale authorable), 2–
4 sinusoidal gust terms modulating magnitude, one octave of fBm turbulence, and an optional power-law height profile. Flat TOML
scalars, schemars-validated. Slang generated via S2. Sheltering multiplier from S3.

Deliberately deferred: curl noise. Plain fBm is visually sufficient for a wind field; add curl only if Phase 3/5 particle advection shows
visible sinks. Also deferred: terrain-driven ridge acceleration (cosmetic).

Exit: loom sim assertions on wind_at() at fixed positions and times pass identically in debug and release across 10k ticks; the S2
agreement test passes for the wind field; direction slew rate is bounded (asserted numerically, since a snap is invisible in a still).


## Phase 2 — Grass (3–5 weeks)


  Detail pending. A dedicated grass research pass is in flight; this phase is written from what the wind and PCG passes
  already established. Expect the specifics — exact vertex counts, the anti-aliasing verdict, measured costs — to be refined,
  not replaced. The position of this phase in the order is the load-bearing claim and is not expected to change.


Why this early — two independent reasons.

It has almost no prerequisites. Grass needs the wind field (P1) and the golden-image fly-through (S1). It does not need prefabs or
the scatter system, because a blade is generated geometry in a compute shader, not a placed mesh instance. Nothing else in the
backlog unblocks it, so there is no reason to wait.

It is the forcing function on the no-TAA decision. This is the real argument for pulling it forward. Sub-pixel grass blades are the
worst case of a problem that also affects water specular highlights and rain streaks: thin, high-frequency features that shimmer
without temporal accumulation. Nearly every shipped grass system leans on TAA, DLSS, or checkerboard reconstruction to hide it —
and this engine has none of those. If grass cannot be made stable without temporal AA, you need to know that in month
two, not month eight, because the remedy (adding a minimal non-temporal full-screen AA pass, or accepting a post-process stack
earlier than planned) would change the plan for water and rain too. Build grass early precisely because it is the hardest test of a
decision already made.

What

The compute-generated per-blade pipeline (Ghost of Tsushima is the reference — Eric Wohllaib, GDC 2021 Advanced Graphics
Summit):

 placement compute → per-tile blade generation from density + position hash
       ↓ blade buffer (bindless, addressed by device address)
 finalize compute   → frustum / distance / orientation cull, LOD select, compact
       ↓ compacted visible-blade buffer + draw args
 vkCmdDrawIndexedIndirect
       ↓
 vertex/fragment    → Bézier expansion, wind bend, shading


   Blade geometry: a cubic Bézier curve expanded into a triangle strip, width tapering toward the tip. Roughly 15 vertices at high
   LOD and 7 at low, per the Tsushima numbers.
   Per-blade payload: position, facing, wind strength at position, a per-blade hash, grass type, clump facing, clump colour, height,
   width, tilt, bend.
   Clumping via Voronoi cells. Clump identity drives coherent facing, colour, and height. This matters more for realism than blade
   quality does — uniformly random blades read as carpet.
   Stateless placement from position hashes (PCG, per Jarzynski & Olano), exactly as rain does. A blade is a pure function of its
   cell index and hash, so regeneration is free, order-independent, and identical across partial and full regeneration.
   Density from the terrain analysis you already have: slope, flow accumulation, curvature, erosion, and SDF distance. Lush
   grass in concave high-flow gullies, sparse on steep slopes, none on eroded rock — rules that other engines' users hand-paint.

Rendering-only, outside the sim hash. Same exemption as rain. Nothing in the grass pipeline may write anything the
deterministic sim reads.

Blades are never ECS entities. loom_ecs is Vec<Option<T>> ; a million blade entities would be pathological. The scene tree shows
one node for the grass field, never the blades — the same represent-the-generator principle as voxel op lists.

The anti-aliasing problem, which is the whole phase
Budget real time for this and treat it as the phase's actual risk. The non-temporal toolkit:

   MSAA — available in Vulkan and the baseline. Determine empirically which sample count actually helps versus just costing fill
   rate.
   Alpha-to-coverage, which composes with MSAA and gives order-independent cutout transparency.
   Minimum screen-space blade width clamping with opacity compensation — widen a blade to at least roughly one pixel
   and reduce its alpha to preserve apparent coverage. This is the single most important trick for distant grass and the standard
   answer to sub-pixel twinkling.
   Density falloff with distance while widening the survivors, so apparent density stays constant as blade count drops.
   True geometry rather than alpha-tested cards, which removes alpha-test aliasing entirely at the cost of thin-triangle
   geometric aliasing — likely the better trade here, but verify.

If that combination proves insufficient, the escape hatch is a single non-temporal full-screen AA pass (SMAA or CMAA2 class). That is
technically a post-process, so adding it is a scope decision to make deliberately rather than by drift — record it as an ADR if you take
it.

Lighting in a forward renderer

   Do not light blades with their true geometric normal. Flat blades lit honestly look harsh and wrong. Use outward-tilted or
   hemispherical normals, flip on front-facing, and blend toward the terrain normal. This is a case where the physically-correct
   answer is the wrong-looking one.
   Darken toward the blade base. The cheapest high-value trick available, and it substitutes for the SSAO you don't have.
   Do not put grass in the acceleration structure. Millions of wind-animated blades would force continuous BLAS rebuilds — the
   acceleration-structure cost, not the tracing cost, is what kills this. Ray-traced sun shadows stay for static geometry; grass gets
   analytic base darkening instead.
   Per-blade and per-clump colour variation, plus wetness from the rain system once Phase 4 lands.

Wind and interaction

   Bend the Bézier control points using the P1 field — do not translate whole blades.
   Derive phase from global world position, never chunk-local coordinates, or you get a visible seam wherever voxel chunks
   meet and phase jumps.
   Trample map: a top-down displacement render target entities write into, with a damped-spring restore so grass doesn't snap
   back linearly. Keep it visual-only unless hiding-in-grass becomes gameplay, in which case it needs a deterministic CPU grid in the
   hash.

Destructible terrain

Grass over a destroyed voxel region must not float. Dirty-region regeneration works the same way as scatter: a CSG op marks a grass
tile dirty and only that tile regenerates — byte-identical, because placement is position-hashed rather than order-dependent.

Exit criteria

   A grass field renders at target framerate with a plausible blade count.
   The S1 fly-through shows no shimmer, no LOD popping, no density popping, and no unison sway. This, not a still
   screenshot, is the real exit criterion — every grass failure mode is a motion artifact.
   Grass thins on steep slopes and thickens in gullies without any authored mask.
   Sim hashes are unchanged with grass enabled, proving the pipeline is genuinely decoupled.
   Destroying terrain under a patch leaves no floating blades.

Motion-only traps to watch

Every item here is invisible in a still PNG, which is why S1 comes first: shimmer and twinkling; LOD-tier popping; density popping as
tiles load; unison sway; "swimming" grass that slides across the ground instead of swaying in place; phase seams at chunk
boundaries; clump-pattern repetition visible only while moving; normal artifacts at grazing angles; blades intersecting other
geometry; grass on near-vertical surfaces.


## Phase 3 — Water (4–5 weeks)


Why here. Needs S2 (the Gerstner Slang mirror) and P1 (wind-driven spectra). Before rain because rain's ripple and splash work
lands on water surfaces, and because water is the more structurally significant system.

Order within the phase (from loom-water-system.md §6, revised for the new prerequisites):
 Step   Work

 W0     WaterBody + WaveSet schema; validator including the steepness limit

 W1     sample_water in Rust; analytic normals (never finite-differenced); unit tests

 W2     Slang generation via S2 + agreement test (now nearly free — S2 exists)

 W3     Wind→wave derivation: Pierson-Moskowitz / JONSWAP from wind speed + fetch, directional spreading, and direction slew ("wave inertia")

 W4     Water mesh: quadtree LOD tiles, camera-centered, one mesh for all bodies

 W5     Buoyancy pontoon solver, damping, drag against flow

 W6     Terrain depth queries, wave attenuation in shallows, shoreline

 W7     Submersion component; Rhai API; audio low-pass via S3; splash particles

 W8     Rivers from loom_terrain flow accumulation

 W9     CLI + MCP: loom water <scene> --at x,z


W3 moved up from a later position because deriving waves from wind is now available immediately, and hand-authoring amplitudes
first would mean re-authoring them later.

Exit: loom sim water_crate.loom --ticks 1800 — the crate floats and does not resonate; a wind speed change visibly changes sea
state with the wave direction lagging correctly.

Traps that bite here specifically: Gerstner steepness self-intersection (schema-validate with the computed limit in the error);
buoyancy resonance without damping (the 1800-tick assertion catches it, a screenshot cannot); the PM-spectrum 19.5 m vs JONSWAP
10 m reference-height confusion, which silently mis-sizes every sea state.


## Phase 4 — Rain (3–5 weeks)


Why here. Needs P1 (streak lean and drift) and S3 (sky exposure for occlusion, wetness gating, splash placement). Both now exist,
so rain gets cheaper than its standalone estimate.

What, in order. 1. CPU authoritative rain state — intensity scalar, wind vector from P1, exposure from S3 — in the deterministic sim,
TOML-authorable, sim-asserted. 2. Stateless GPU streak renderer: PCG-hashed positions from integer index + time, camera-locked
wrapping volume, additive blending (order-independent, no sorting, and the single biggest overdraw win), soft depth fade, lean
along the P1 wind vector. 3. Wetness in the forward material: porosity-based darkening, roughness reduction, accumulation gated by
exposure, and a two-rate dry-down (specular decays faster than albedo recovers). 4. Splashes decoupled from drops entirely —
spawned from exposure, never from per-drop collision. Ripple normal atlas first; seeding the shallow-water sim from W-phase work
later. 5. Audio: intensity crossfade plus the S3-driven low-pass.

Puddles come from loom_terrain flow accumulation gated by slope and curvature — flow accumulation alone marks drainage
convergence, not flat basins, so ungated puddles climb gentle inclines.

Exit: rain visibly stops under an overhang; carving the overhang open with a CSG op lets rain in, in the same tick; wetness
accumulates and dries; sim hashes unchanged with rain enabled (proving the visual layer is truly decoupled).

The structural rule for this phase: the GPU rain layer must never write anything that feeds the sim hash. If assertions go flaky
when rain is enabled, you have accidentally coupled them.


## Phase 5 — Procedural scatter (4–6 weeks, after S4)


Why here. S4 unblocked it. It also wants the terrain analysis that already exists — slope, flow accumulation, curvature, erosion —
plus SDF distance, which together let you author rules Unreal users have to fake with hand-painted masks.

What. 1. loom_scatter : deterministic Bridson Poisson-disk (fixed RNG stream and fixed active-list pop order — the vanilla algorithm
pops randomly and will silently break your hashes), plus Halton for fixed-count coverage. 2. Position-hashed per-instance
seeding. Derive each instance's RNG seed from its quantized world position, not from generation order. This is not a nicety — it is
what makes scatter order-independent, which is what makes parallel and partial regeneration produce byte-identical results. 3. Flat
TOML scatter rules bound to the terrain fields, spawning prefab instances. 4. Named intermediate outputs ( exclude_from = ["roads",
"water"] ) — this is how you recover the two or three genuinely-DAG features without a node graph. 5. Biome blending by scalar
priority + bounds. 6. Dirty-region incremental regeneration over destructible voxels. This is the part no engine does well
and the part your architecture makes tractable: a CSG op marks a scatter cell dirty, only that cell regenerates, and because seeds are
position-derived the result is identical to a full regen with no seams.

Do not build a node graph. The research settled this empirically: Unreal's PCG graphs are binary- .uasset -only with no text export
and no PCG-specific diff tool, and Epic's own recommended workflow is to partition content to avoid merge conflicts rather than solve
them. Blender's Geometry Nodes have the same problem. Two mature ecosystems both failed to make a node graph reviewable in a
diff. A linear rule list with named outputs is strictly better here.

Hierarchy rule: never put scattered instances in the scene tree or the ECS individually. loom_ecs is Vec<Option<T>> and a million
entities would be pathological — but more fundamentally, represent the generator, not the generated, exactly as voxels are op lists
rather than arrays. The tree shows one node: "pine_forest — 1.2M instances."

Exit: a scatter rule places vegetation on slopes under 22° away from riverbeds; the same seed reproduces byte-identically in debug
and release; destroying terrain under a patch regenerates only that cell.


## Phase 6 — Mesh vegetation wind + GPU instance culling (3–5 weeks)


Why here. This phase is mesh vegetation — trees, bushes, rocks — as distinct from grass (Phase 2). It needs P1 (the field) and P5
(instances to sway and cull). Both were blocked until now.

Vegetation wind (4–7 days). The Crytek GPU Gems 3 model: main bending scaled by normalized height with the normalize-to-
length step (skip it and the mesh tears at high wind), plus detail bending with stiffness and phase in vertex colors. Phase derived from
global world position — not chunk-local coordinates, or you get visible seams where voxel chunks meet and phase jumps. All of it
samples the P1 field, so a flag, a tree, and the rain agree about wind direction.

GPU instance culling (2–3 weeks). Compute frustum + Hi-Z occlusion cull writing a compact visible list, then
 vkCmdDrawIndexedIndirect . Rendering-only, never feeding the sim. Pad cull bounds to absorb Hi-Z's one-frame latency. Fits your
architecture well: the cull→draw-indirect dependency is exactly a barrier loom_render_graph should insert automatically, and instance
transforms in one SSBO addressed by device address is what bindless is for.

Impostors for distant props come after, and their popping is a motion artifact — which is why S1's fly-through exists.

Exit: a field of vegetation sways without unison, without seams at chunk boundaries, and the fly-through shows no popping; a million
instances render with culling in the sub-millisecond range.


## Phase 7 — Editor (6–9 weeks, slottable in parallel)


Why last in the list but not last in time. The viewport/inspector/gizmo half and the co-authoring half have no dependencies on
Phases 1–6 and can be interleaved whenever you want a break from graphics. Only the scatter live-preview tooling needs P5.

Order within the phase.

 Step   Work                                                                                                               Depends on

 E1     Render-to-texture viewport in an egui dock tab; camera nav (orbit/pan/fly, focus-on-selection)                     —

 E2     JSON-Schema → egui inspector walking schemars output                                                               —

 E3     transform-gizmo (not the abandoned egui-gizmo ) + snapping, emitting one compound SceneOp on drag release          E1

 E4     Transaction / activity feed: labelled entries with jump-to and revert                                              —

 E5     Batch approval gates and trust levels for destructive ops                                                          E4

 E6     Version-divergence banner surfacing the existing token model                                                       E4

 E7     Command palette issuing the same SceneOp s the agent uses                                                          —

 E8     Diff review in viewport: ghost the previous state, green/red for added/removed, paired with the text diff          E1, E4

 E9     Scatter live preview: parameter scrubbing that edits the TOML, seed re-roll, debug point overlay                   P5, E2


Two decisions worth locking now.

Approval batching, not per-op prompts. Per-op approval trains you to blind-approve — the exact regression the AI-coding-tool
ecosystem has already documented. Non-destructive ops apply immediately; destructive ones batch into one card ("Approve 10
deletions?"). Trust levels scoped by region or subtree.

Painted regions serialize as polygons or splines, never bitmaps. This is the round-tripping trap for E9 and it is the same
decision you already made for voxels: a painted mask is not diffable, so store the shape, not the raster. It round-trips cleanly and
stays reviewable.

Live-reload hardening (part of E1). Persist camera and selection outside scene state; debounce file-watch events, since an agent
writes in bursts; never reload mid-gesture — queue it until the drag or text edit completes; diff-and-patch the scene graph rather than
tearing down and rebuilding.

Exit: the same edit made by hand and by agent produces an identical diff; a twelve-op agent transaction undoes in one Ctrl+Z; an
edit made while the agent writes the same file is rejected with a reload prompt.


## Opportunistic inserts


Two things are cheap, high-value, and depend only on Phase 0–1. Slot them wherever you want a quick win.
Cloth — flags and ropes (5–8 days). Needs only P1. Your own fixed-step Verlet solver in loom_cloth reusing loom_particles
patterns — not rapier (no soft bodies) and not bevy_silk (Bevy-coupled, and it smooths wind by framerate, which is a determinism
red flag). A pinned edge, a small particle grid, per-triangle aerodynamic force. Very high visual impact per day spent.

Audio integration (3–5 days). Needs S3 and whichever of P1/P4 exists. Wind and rain loops crossfaded by intensity, low-passed by
the exposure fraction. Because loom_audio already traces real geometry, the interior/exterior mixing that other engines fake with
hand-placed volumes falls out of the query you already built.


## Phase 8 — Deliberately deferred


Not "never," but not now, and each for a stated reason.

 Item                            Why deferred

 Post-process stack              Unblocks SSR, lens raindrops, half-res transparency compositing, screen-space wet reflections. Large, and everything above
                                 works without it.

 SDFGI                           The right dynamic-GI answer for this engine, but a 3–6 month project. Ray-traced puddle reflections are the cheaper
                                 substitute.

 Dual Contouring                 Needed for sharp-cornered voxel structures. Nothing above requires it.

 Voxel LOD octree                Needed for the open world, not for contained levels.

 Mesh/task shaders for grass     Compute + indirect draw is better documented and lower risk. Optimize later if profiling demands.

 Hardware RT beyond ray          Destructible voxels force BLAS rebuilds rather than refits; the acceleration-structure cost, not the tracing, is the problem.
 queries

 Curl noise                      Add only if particle advection shows visible sinks.

 Shallow-water rivers/lakes      The height-field ripple sim covers hero water; full SWE is a later want.
 sim

 Archetype ECS                   Vec<Option<T>> is fine as long as scatter represents rules rather than instances. Revisit when profiling demands, not
                                 before.

 Editor plugin API               Low payoff at solo scale. The command palette and Rhai already give extensibility.


## Timeline, honestly


 Phase                                                                                                       Estimate

 0 — Substrate                                                                                               4–6 weeks

 1 — Wind field                                                                                              1 week

 2 — Grass                                                                                                   3–5 weeks

 3 — Water                                                                                                   4–5 weeks

 4 — Rain                                                                                                    3–5 weeks

 5 — Scatter                                                                                                 4–6 weeks

 6 — Mesh vegetation + culling                                                                               3–5 weeks

 7 — Editor                                                                                                  6–9 weeks

 Inserts (cloth, audio)                                                                                      1.5–2 weeks

 Total                                                                                                       30–44 weeks


That's roughly eight to eleven months of real evenings, and the estimate is optimistic in the way all such estimates are. Two honest
observations:

Phase 0 is 15–20% of the total and produces nothing visible. That is the correct allocation and it will feel wrong while you are
in it. The alternative is building four visual systems with no pixel diff, three codegen paths, three occlusion queries, and scatter twice.

If you only do four things, do S1 (golden images + fly-through), S2 (codegen + agreement test), P1 (the wind field), and P2
(grass). That is about six to eight weeks, it makes every subsequent visual system safer and cheaper, it answers the no-TAA question
while there is still time to act on the answer, and a wind-swept grass field is the single largest visible change per week spent
anywhere in this document.


## Resequencing triggers


Things that should make you deliberately deviate from this order.
If…                           Then…

The S2 agreement test         Stop. Do not build P1–P4 on a divergent foundation. Fix the noise/float determinism first.
can't be made to pass to a
tight epsilon

Determinism hashes            Stop that phase. This invalidates everything downstream that depends on assertions.
diverge between debug
and release in any phase

Prefabs turn out to be a      Consider a minimal prefab (source ref + flat override map, no extends ) to unblock P5, and defer inheritance.
4+ week job, not 2

You want a demo before        Reorder: S1 → S2 → P1 → P2 grass → cloth (flags). Wind + a swaying grass field + a flag is the most visually dramatic thing per
month four                    week spent, and skips water's five weeks.

egui's frame budget           Fix with row virtualization and rule-node collapsing first. That is not a signal to switch GUI frameworks.
collapses on large scenes

Compile times exceed ~1       Stop and fix crate boundaries. The agent's loop is cargo check ; degrading it hurts more than any feature helps.
minute warm

Grass cannot be made          This is the outcome Phase 2 exists to discover early. Decide deliberately between a single non-temporal full-screen AA pass
stable without                (SMAA/CMAA2 class) and pulling the post-process stack forward from Phase 8. Record it as an ADR. Do not proceed to water
temporal AA                   without a decision — wave specular has the same aliasing problem.

Grass turns out to be a 8+    Ship the near-field tier only (full-geometry blades, short draw distance, no card/impostor tiers) and defer distance LOD. The AA
week job                      question is answered by the near tier alone.

A phase's exit criterion is   You have mis-scoped the phase. Every phase ends in something that runs.
"it compiles"


## What to update as you go


   CLAUDE.md — the "Current milestone" line, every phase.
   docs/decisions/ — an ADR for each locked decision this document makes that isn't already recorded. At minimum: golden-image
  tolerance policy, the Rust→Slang codegen mechanism, SDF-over-ray-query for sheltering, no-node-graph-for-scatter, position-
  hashed instance seeding, and the grass anti-aliasing approach (including whether a non-temporal full-screen AA pass gets added).
  ADR 0003 (knowledge graph, proposed) — still unresolved. Nothing above depends on it, so it can stay open, but it should be
  either accepted or rejected rather than left indefinitely.
  The companion docs — where this document supersedes their internal build orders, note it there rather than silently diverging.
  docs/design/README.md already does this for the wgpu-era passages; same treatment.

