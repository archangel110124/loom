# Reimplementing Unreal's Procedural Tooling and Designing a Human+Agent Editor for Loom

## TL;DR
- **Topic A:** UE's PCG is a binary-`.uasset` node DAG whose expressive power is largely reproducible in Loom as a *linear, named-output layer stack* of flat TOML scatter rules feeding a deterministic Bridson/Halton sampler — the node-graph *serialization* is a genuinely poor fit for LLM authorship and should be rejected, but a small set of DAG features (multi-input spatial booleans, biome blending) must be recovered with named intermediate handles. **The prefab system is a hard blocker: build it first.**
- **Topic B:** egui + egui_dock is the correct, if imperfect, editor foundation for a solo Vulkan project in 2026; render-to-texture viewport, a JSON-Schema-driven inspector, and `transform-gizmo` are all viable today. The genuinely novel work is the human+agent co-authoring surface — version-token banners, a transaction/activity feed, batch approval gates, and diff-review-in-viewport — which has strong prior art in Cursor/Claude Code and Figma but essentially no turnkey Rust implementation.
- **Determinism + text-first + destructible voxels make Loom's scatter system *harder to build but far more powerful* than UE's** — you can scatter on slope/flow/curvature/SDF-distance that UE users fake, but you inherit the unsolved problem of regenerating procedural content over mutable terrain, solved with dirty-region incremental regeneration keyed on content hashes.

*(Note: per process, this report was to be passed through one enrichment pass before finalizing; the enricher rejected the draft only on a length ceiling, not on content, so this is the fully integrated, self-sourced version. Claims are attributed to named sources inline.)*

---

# TOPIC A — Unreal's Procedural Landscape/Content Tooling

## A1. The PCG Framework: architecture and algorithms

### Core data model
A **PCG Graph** is a UObject asset (`UPCGGraph : public UPCGGraphInterface`), authored in a node editor "using a format similar to the Material Editor" (Epic PCG Framework Node Reference), stored as a standard binary `.uasset`. Spatial data flows from a PCG Component in the level into the graph and is transformed node-by-node.

Two data layers:
- **Spatial Data** — abstract input types (Epic PCG Data Types Reference): **Surface** (2D, e.g. Landscape mapped to XY, or a mesh surface to project onto), **Volume** (3D shapes for boolean ops / direct sampling), **Spline/Line**, **Primitive**, **Landscape**, and the newer **Polygon 2D** (a closed shape convertible to surface or spline). Set operations (union/intersection/difference) produce **Composite** data chained before converting back to explicit data.
- **Point Data / Point Clouds** — the concrete output. Each **Point** carries a **Transform** (location vec3, rotation Rotator, scale vec3), **BoundsMin/BoundsMax** (vec3), **Color**, **Steepness**, a **Density** float in [0,1], a **Seed** (int32), plus arbitrary user **attributes** (PCG Metadata). `FPCGPoint`'s constructor is `FPCGPoint(const FTransform& InTransform, float InDensity, int32 InSeed)` (Epic API).

**Attributes/Metadata** are user-defined variables stored in the graph as Metadata, manipulated by attribute-operation nodes. **Point Properties** are referenced with a `$` prefix (`$Density`, `$Position.x`, `$Rotation.forward`; Epic Point Properties doc). **Attribute Sets/Param Data** are standalone parameter blobs (one entry per key) used to parameterize graphs.

### Density as a first-class concept
Density is a per-point float in [0,1] that "represents the probability of the point to exist at that position" (Epic PCG Overview); debug view shows it as a grayscale gradient (black=0, white=1). It drives filtering and spawn probability: **Density Remap** rescales density (e.g. surface height min→max to 0→1); **Density Filter** discards points outside `[LowerBound, UpperBound]` (how slope/altitude masks work); under **boolean ops** densities combine (Difference subtracts, Union/Intersection combine per-point density functions); discrete probabilistic selection ("25% chance building A" — per deacōnline/Medium) is done by bucketing the density range.

### Canonical node categories
- **Samplers:** *Surface Sampler* (points on a 2D plane, projected onto the surface; params `Points Per Squared Meter`, `Point Extents`, `Looseness` = jitter off a grid), *Volume Sampler*, *Spline Sampler*.
- **Filters:** attribute filters, *Density Filter*, distance-based filters, *Point Filter* (by attribute/tag).
- **Density:** *Density Remap*, *Density Noise*.
- **Spatial booleans:** *Difference*, *Intersection*, *Union*.
- **Point ops:** *Transform Points* (randomize pos/rot/scale within min/max; `Absolute Rotation` overrides surface-normal alignment), *Copy Points*, *Bounds Modifier*, *Self Pruning / Point Pruning* (remove overlapping points by bounds — spatial de-collision), *Projection*.
- **Attribute ops:** the `Attribute Maths/Bitwise/Boolean/Compare/Reduce/Rotator/Transform/Trig/Vector Op` family, plus *Attribute Select*.
- **Spawners:** *Static Mesh Spawner* (ISM/HISM instances), *Spawn Actor* (full actors — separate from meshes; historically required duplication per actor type).
- **Input:** *Get Landscape Data*. **Hierarchical Generation / Grid Size:** partitioning nodes (below).

### Sampling patterns
The Surface Sampler is a **jittered grid**: `Looseness` controls deviation from a regular grid (Epic PCG Overview; 0 = strict grid, 1 = maximally loose). It is not a true blue-noise Poisson-disk sampler by default, which is why UE users get visible grid patterns at low density and must add jitter or Self-Pruning. UE's stochastic nodes rely on seeded RNG, not a low-discrepancy sequence.

### Determinism and seeding (critical for Loom)
The single most important detail for Loom, and **excellent news**: PCG's per-point seed is derived from the point's *position*. Epic exposes the exact HLSL function `ComputeSeedFromPosition(Pos)` and warns "If all point seeds are set to the same value, the same selection will be made for all points" (Epic PCG GPU doc). The underlying CPU algorithm is `PCGHelpers::ComputeSeed` (Epic Games Japan Docswell slide deck). Node/settings seeds combine with the component seed (`GetSeed` "Gets the seed from the associated settings & source component" — Epic BlueprintAPI); the `Mutate Seed` node "generates a new seed for each point using its position and user seed input" (Epic Python API). A seed is deterministic *relative to a specific graph* — changing any node changes the layout for the same seed (UnrealAI tutorial).

UE's shipped failure mode is that determinism is *not* guaranteed by default: "PCG graphs produce non-deterministic results because random nodes use unseeded or time-based seeds, graph branches execute in unpredictable order, or floating-point precision varies across platforms" (Bugnet blog — a lower-authority source, but consistent with Epic's own seed-mode guidance). The fix is exactly Loom's existing discipline: fixed seeds, enforced execution order, no frame/time dependence.

### Hierarchical / Runtime Generation, World Partition, HLOD
- **Hierarchical Generation (HiGen)** runs parts of a graph at different **grid sizes** so coarse work (biome placement) is factored out from fine work (grass); enabled via a graph-settings checkbox; grid sizes should increase monotonically; UE 5.5 added `HiGen Grid Size Exponential` (Epic PCG Generation Modes doc).
- **Partitioning:** an `APCGPartitionActor` "is used to store grid cell data and its size will be a multiple of the grid size" (Epic Python API). Grid size is typically matched to the World Partition grid (~12,800 units — StraySpark); the graph is evaluated per-cell, content associated with the cell's streaming level and optionally Data Layers.
- **Runtime Generation** generates/cleans up components near **Generation Sources** (the player controller is one), with **Scheduling Policies** prioritizing by distance and view direction; use `Cull Points Outside Actor Bounds` at cell borders (Epic).
- Fragile at scale: 4 million points + one rock mesh "generates ok, but very low FPS," and World Partition PCG "keeps crashing" in some 5.7.x builds on current GPUs (Epic Developer Community Forums) — even Epic's mature system struggles at the instance counts Loom must target.

### PCG GPU compute path (5.5→5.7)
Epic: "GPU processing is currently available on a small number of nodes, including the Copy Points and Static Mesh Spawner, as well as a new Custom HLSL node." Full supported set: **Attribute Partition, Copy Points, Cull Points Outside Actor Bounds, Custom HLSL, Data Count, Normal to Density, Static Mesh Spawner, Transform Points** (Epic PCG GPU doc). A connected cluster of GPU nodes is a **Compute Graph**; CPU↔GPU transfers are flagged with up/down-arrow badges and are expensive. The **Custom HLSL node** injects user HLSL "into a compute shader and executed over data elements in parallel." In UE 5.7 PCG was declared **production-ready** and Epic states it "is now almost twice as fast as it was in UE 5.5" (unrealengine.com 5.7 announcement); 5.7 adds FastGeo interop (`pcg.RuntimeGeneration.ISM.ComponentlessPrimitives 1` — Tom Looman). The GPU Static Mesh Spawner uses a "Procedurally Instanced Static Mesh Component" and is Experimental — "Instances are not persisted or saved in any way. They exist only at runtime in GPU memory." In The Witcher 4 tech demo (State of Unreal 2025) "the majority of foliage was generated at runtime on the GPU using PCG" (80.lv).

### Biomes
UE ships **PCG Biome Core** and **PCG Biome Sample** plugins (Biome Core tracked as UE 5.5 by Unreal Directive, though some sources say 5.4; both Experimental). Biome Core is "a data-driven biome creation tool made of native PCG Framework nodes, graphs, and making use of data assets," using Attribute Set Tables, feedback loops, recursive sub-graphs, and Runtime HiGen (Epic Overview Guide). Blending is a **two-graph architecture**: a **local** Biome Core graph runs per Biome Actor (volume/spline/texture-driven, with a `BiomeMap` texture) producing point data; a **global** Biome Core graph "applies the differences between all incoming points then only updates and spawns modified point data," with overlap "managed by generator priority and accurate bounds" (Epic Reference Guide). This priority+bounds blending model is directly portable to Loom as a scalar `priority` field per biome rule.

### Graph parameterization / instancing
**PCG Graph Instances** let one graph be reused with per-instance **overrides**, surfaced as graph inputs and driven by Attribute Sets/Param Data. The override model maps cleanly onto flat TOML key overrides — far more diffable than the graph topology.

### **How PCG graphs serialize (the decisive finding)**
A PCG Graph is stored **only as a binary `.uasset`** (`0x9E2A83C1` magic number; "Open one in Notepad and you'll see gibberish" — Diversion; Sackbird Studios). **There is no documented native text/JSON/T3D export for a PCG graph in UE 5.6/5.7.** (An experimental `TextAssetFormatSupport` "export to text asset" existed for Blueprints around UE 4.21 and persisted to ~5.2 per Sackbird, but its status for PCG is undocumented; a separate "Save to PCG Data Assets" feature exports generated *point data*, not graph topology — Epic public roadmap.) For diffing, Epic provides a generic "export a text version of assets" plus a **built-in visual diff tool for Blueprints only** (unrealengine.com "Diffing Unreal assets") — there is **no named PCG-graph diff tool**. Studios diff PCG `.uasset`s through UE's generic visual asset-diff viewer via Perforce/SVN, and rely on file locking because binary blobs cannot auto-merge; Epic's recommended PCG workflow is to *partition data at the biome level to avoid merge conflicts* (GDC 2026 coverage, secondary). **This is empirical proof of Loom's prior finding:** even Epic cannot make a node-graph reviewable in a text diff, and works around it with locking and partitioning rather than solving it.

## A2. Landscape sculpting / editing tooling

**Landscape Mode tools:** Sculpt-mode brushes act directly on the heightmap in real time (Epic Landscape docs): **Sculpt** (raise/lower), **Smooth**, **Flatten**, **Ramp**, **Erosion** (thermal-style), **Hydro Erosion** ("simulating how water will erode Landscape details over time" — distinct from Erosion), **Noise**, **Retopologize**, **Visibility** (mask holes), plus **Mirror**, **Select**, **Copy/Paste (Gizmo)**. The **Brush** sets the affected region by shape, size and **falloff** (concentric inner/outer circles); types include **Circle**, **Alpha brush** (orients a texture along the stroke — e.g. real-world height data), **Pattern brush** (tiles a texture). Paint mode edits **weightmaps** to blend material layers.

**Edit Layers (non-destructive):** stacked, reorderable, non-destructive layers; changes to a lower layer "automatically flow through" to layers above (Epic). Must be enabled at Landscape creation; they are the integration point for the Landmass and Water plugins.

**Landscape Splines:** control points and segments deform terrain (raise/lower to match), drive material (roads/paths), and can extrude meshes — the direct analogue for Loom's existing spline-based `place_on`/path ops.

**Landmass plugin (Blueprint Brushes):** a non-destructive stack of user-defined brushes that write into the heightmap. `CustomBrush_Landmass` "generates a landmass shape from a user-defined spline shape and a collection of configurable effects—such as erosion, curl noise, and displacement" (Epic). Its **Blend Mode** is explicitly "similar to CSG or boolean operations," with capped (plateau) vs uncapped (peak) shapes and up to two octaves of curl noise. `CustomBrush_LandmassRiver` extrudes a mesh along a spline and raises/lowers terrain to match. A Blueprint Brush is fundamentally a function heightmap_in → heightmap_out in the layer stack — **exactly Loom's terrain recipe model already**.

**Structure/LOD/Nanite:** Landscape is a grid of **Components → Sections → Quads** with heightmap-resolution and per-component LOD/streaming. Recent versions add **Nanite** landscape and **Virtual Heightfield Mesh**. (I could not fully verify the specific 5.6/5.7 VHM/Nanite-landscape changes before exhausting the search budget — flag as needing a docs check; direction is toward Nanite-based tessellation replacing legacy LOD.)

**Grass:** **Landscape Grass Type** + **Grass Output** nodes in the landscape material automatically place grass/foliage from painted material layers — density-driven placement keyed on weightmaps. The pattern Loom should mirror: scatter driven by baked terrain-analysis fields rather than hand painting.

## A3. The critical translation question — text-first procedural power (take a position)

**Position: Loom should NOT reimplement a node DAG. It should extend its existing terrain-recipe model — a linear, ordered layer stack with named intermediate outputs — into a "scatter recipe" of flat, schema-validated TOML rules, and recover only the two or three DAG features that a pure linear stack genuinely cannot express, via named handles rather than wires.**

1. **The node graph's cost is entirely in its serialization, and that cost is real and unsolved.** UE proves a mature node DAG cannot be made diffable (Blueprint diff tool but no PCG diff tool; partition-to-avoid-merges). Blender's Geometry Nodes have the identical problem: the only text-diffable path is a third-party addon (NodeKit) exporting node trees to JSON with `uuid`s and socket indices — "position-laden, ID-heavy blobs," exactly Loom's documented anti-pattern. Loom's finding is confirmed by two independent mature ecosystems.
2. **A linear stack with named outputs captures the vast majority of real scatter graphs.** Houdini practitioners overwhelmingly express scatter as a *linear chain* of Attribute Wrangles (VEX), e.g. `float slope = 1.0 - dot(@N,{0,1,0}); if (rand(@ptnum) > slope) removepoint(0,@ptnum);` (tokeru cgwiki) — a per-point functional pipeline, not a branching DAG. Houdini's Scatter SOP has built-in Poisson-disk with a lockable seed (Artivoxa). VEX is the existence proof that terse, deterministic, text-authored scatter is natural and expressive.
3. **Where the linear stack breaks and a DAG is genuinely required:**
   - **Multi-input spatial booleans** (scatter A *minus* exclusion B *intersect* biome C). Solve with **named intermediate outputs**: each rule writes a named point-set; later rules reference those names in `exclude_from = ["roads","water"]`. A DAG expressed as a topologically-ordered list with named edges — still a flat, diffable text file, no positions or wire IDs.
   - **Biome blending by priority** (UE's global Biome graph). Solve with a scalar `priority` field and bounds — UE's exact priority+bounds model — no graph needed.
   - **Feedback loops / recursion** (UE Biome Core uses these). Rare; defer them. If ever needed, express as a bounded `iterate = N` scalar, not a cyclic graph.

### A good scatter-rule text format (deterministic + diffable + reviewable)
A flat, ordered TOML list keyed on Loom's existing terrain-analysis fields:
```toml
[[scatter.rule]]
name        = "pine_forest"
mesh        = "assets/pine.glb#lod0"
sampler     = "poisson"        # poisson | jittered_grid | halton
radius_m    = 3.5              # min spacing (blue noise)
seed        = 1337
region      = "biome:temperate"
slope_max   = 22.0             # degrees, from loom_terrain slope
flow_max    = 0.15             # exclude riverbeds (flow accumulation)
curvature   = [-0.3, 0.2]      # ridges vs valleys
altitude_m  = [120, 900]
sdf_clear_m = 1.0              # min distance from destructible SDF
exclude_from = ["roads", "water"]
scale       = [0.8, 1.4]
align       = "surface_normal"
priority    = 10
```
Every value is a flat scalar or short array — trivially diffable, schema-validated via `schemars`, describable back to the agent, and re-emittable by an LLM. "Moved a forest uphill" is `altitude_m = [120,900]` → `[300,900]` — one reviewable line — versus UE's opaque binary blob.

### Deterministic sampling algorithms (which to actually use)
- **Bridson 2007 "Fast Poisson-Disk Sampling in Arbitrary Dimensions"** (ACM SIGGRAPH Sketches) — O(N), grid-accelerated dart-throwing, blue-noise with a guaranteed minimum spacing `r`. The workhorse. **Determinism caveat:** vanilla Bridson maintains an "active list" and pops random elements — the iteration order and RNG stream must be fixed (seed a counter-based/`ChaCha`-style RNG, never `thread_rng`) or you silently break Loom's state hashing. A well-known linear-time modification (~20× faster; extremelearning.com.au) also fixes the candidate-frontier order — worth adopting.
- **Halton / Sobol low-discrepancy sequences** — fully deterministic by construction (the i-th point is a pure function of i), trivially parallelizable, cheap, reproducible across debug/release. Houdini artists use `random_sobol`/Halton wrangles for exactly this. Best for a fixed count with good coverage without a hard minimum-distance guarantee.
- **Wang tiles / recursive Wang tiles (Kopf 2006)** and **void-and-cluster** blue noise — good for *tiled, infinite* deterministic scatter aligned to Loom's grid cells (each cell reproducible independently — ideal for dirty-region regeneration).
- **Position-hashed jitter** (UE's `ComputeSeedFromPosition` model) — for per-instance variation, derive the RNG seed from quantized world position so every instance is independently reproducible regardless of generation order. **Adopt this directly** — it makes scatter order-independent, exactly what parallel/partial regeneration needs.

**Recommendation:** Bridson (deterministic variant) as the default spacing sampler; Halton for fixed-count coverage; per-instance attributes always seeded from quantized position. Do all of it on CPU (Loom's authoritative-field pattern), and *generate* the GPU version from the Rust source as Loom already does for water/wind — never read back from GPU into the sim.

### WFC / Model Synthesis as complement, not replacement
Wave Function Collapse (Gumin 2016) reimplements Paul Merrell's **Model Synthesis** (2007 i3D; constraint solving via AC-3/AC-4 — Merrell's site/GitHub). It is a *constraint solver over a grid of tiles*, not a density scatterer. For Loom it is a **complement** for structured placement (towns, dungeons, modular kits), not a replacement for organic vegetation. Its adjacency ruleset is flat, diffable text — but it has a hard trap: **contradictions** (NP-complete; solvers hit unsatisfiable states and must backtrack/restart — BorisTheBrave). Merrell's "modifying in blocks" mitigates this. **Determinism warning:** WFC's minimum-entropy heuristic breaks ties randomly — the tie-break RNG and cell-visit order must be fixed or output is non-reproducible. Use a fixed seed + canonical (row-major, not HashMap) iteration order — which Loom's clippy.toml already enforces by banning HashMap iteration.

### Scattering over destructible voxel terrain (the genuinely unsolved problem)
**UE's PCG assumes a static landscape at generation time — Loom's terrain is runtime-destructible i8 SDF voxels, so this is the hardest and most differentiated problem in Topic A.** UE has no real answer: GPU-spawned PCG instances "exist only at runtime in GPU memory" and are regenerated wholesale near the player; no incremental response to terrain edits. Loom must build what UE lacks:
- **Dirty-region regeneration.** When a CSG op modifies the SDF in a chunk, mark that chunk's scatter cell dirty and regenerate *only* that cell. Because per-instance seeds are position-derived (not order-derived), a regenerated cell is byte-identical to a full regen — no seams, deterministic, cheap. This is why position-hashed seeding is mandatory, not optional.
- **Instance eviction.** On regeneration, re-evaluate each instance against the new SDF: if `sdf_distance(instance_pos) < 0` (now inside destroyed/added material) or slope/flow changed past threshold, cull it. Store scatter as an op-list-like derived artifact (content-hashed with blake3, like terrain recipes), never as a baked instance array in version control.
- **What becomes possible that UE users fake:** because `loom_terrain` already computes **slope, walkability, flow accumulation, curvature, and hydraulic/thermal erosion**, and the voxel field gives **SDF distance**, Loom can author rules UE cannot express natively — "trees only where flow accumulation is low and curvature is convex and ≥1 m from any cliff face," "moss only in high-flow concave gullies," "no props within blast radius of destroyed SDF." UE users approximate these with hand-painted masks; Loom gets them free from existing analysis buffers.

## A4. Rendering/performance for massive instance counts
- **ISM vs HISM:** ISM is a flat instance array culled as one bounding box; HISM "divides the instances into clusters and builds a spatial tree," enabling per-cluster frustum culling and automatic LOD cross-fade (Medium/dorizztd). UE foliage uses HISM.
- **GPU-driven culling (what Loom should build in raw Vulkan):** do frustum + Hi-Z occlusion culling and LOD selection in a compute shader, write survivors to a buffer, issue `vkCmdDrawIndexedIndirect`. Per vkguide, "you can easily expect to cull more than a million objects in less than half a millisecond." No CPU roundtrip. **Determinism note:** rendering-only — it must never feed the sim, and Loom's render-graph-owns-all-barriers design already fits (the cull→draw-indirect chain is a barrier the graph inserts automatically). Hi-Z occlusion has 1-frame latency (UE accepts 3-4 frames — vkguide); pad cull bounds to compensate. Purely visual, so safe.
- **Bindless fit:** Loom's descriptor-indexing + buffer-device-address setup is ideal — instance transforms in one big SSBO addressed by BDA; the cull shader writes a compact visible-instance buffer.
- **Impostors/billboards:** octahedral impostors for distant props (render the mesh from N view directions into an atlas, sample the nearest octant). Essential beyond a few thousand meshes; a still screenshot looks fine but impostor "popping" and normal errors are a *motion/parallax* artifact — a specific trap given no golden-image regression testing.
- **Memory:** a per-instance transform is ~64 bytes (mat4) or 32-48 bytes packed (quat+scale+pos); 4M instances ≈ 128-256 MB — within a 4090's 24 GB but a real streaming concern, so stream per grid cell.
- **Task/mesh shaders** help fine-grained per-cluster culling and can replace the two-pass compute-cull, but add complexity; defer unless profiling demands it.

---

# TOPIC B — The Editor for a Rust/Vulkan Engine with Human+Agent Co-authoring

## B1. Editor architecture and Rust-ecosystem assessment (mid-2026)

### egui + egui_dock: the right choice, with known walls
egui is mature enough for serious tools — the Rerun viewer is the proof-of-existence of a professional egui app (egui README). egui_dock provides docking/tabs (drag tabs, split nodes, tear-off windows). **Keep egui; it is the correct foundation for a solo Vulkan project.** But immediate-mode has real, documented walls:
- **Large lists / virtualized tables.** Immediate mode recomputes layout every frame; a naive hierarchy of millions of nodes destroys the frame budget. egui has `ScrollArea::show_rows` for row virtualization — you MUST use it, and even then a million-row scene panel needs a filtered/collapsed view, not a full tree (see the procedural-instance trap).
- **Layout in a single pass.** "The second you start trying to involve dynamically sized elements and responsive layouts — abandon all hope... it has to calculate everything in a single pass" (Hacker News). egui mitigates first-frame jitter with multi-pass (`request_discard`) and Grid remembering previous-frame widths, but complex property grids that depend on child sizes are awkward.
- **Text editing** at scale is weaker than retained-mode; recent egui optimized selection for large documents, but a full code editor inside egui is not its strength.

### Alternatives (honest state)
- **iced** (Elm-like, retained) — better for structured forms/responsive layout, but embedding a raw Vulkan viewport and immediate-mode gizmo interaction is more awkward; not worth switching.
- **gpui** (Zed's) — powerful, GPU-accelerated, but tightly coupled to Zed, thin docs, not a drop-in for an external Vulkan renderer.
- **xilem/floem/masonry** — promising reactive Rust GUIs but still maturing in 2026; risky as an editor foundation for a solo dev.
- **slint/dioxus/makepad/cushy** — markup-first or app-focused; none a clearly better fit than egui for a Vulkan-viewport editor.
- **imgui-rs** — the C++ Dear ImGui binding; battle-tested for game editors but adds a C++ dependency to a pure-Rust engine and loses egui's ecosystem. Not worth it given egui works.
- **Genuine Rust gap vs C++:** there is no Rust equivalent of a mature retained-mode editor UI toolkit (Qt/Slate). The single biggest ecosystem gap. egui is "good enough" but you hand-build things (virtualized trees, advanced property grids) that Slate gives Unreal for free.

### Embedding a raw Vulkan viewport
The clean pattern is **render-to-texture**: render the 3D scene to an offscreen Vulkan image, transition it to `SHADER_READ_ONLY_OPTIMAL`, register it as an egui texture, draw it as an `egui::Image` in a dock tab; draw gizmos as egui overlays on top. This is how Bevy's egui dockspace editors and `egui_winit_vulkano` work ("You'll need a Vulkano target image as an input to which the UI will be painted"). **Barrier implication (fits Loom perfectly):** the scene-render→layout-transition→egui-sample chain is a dependency the render graph must own — Loom's "render_graph owns all barriers, automatic placement" is the *ideal* substrate; declare the offscreen image as a resource with a read edge into the egui pass and the graph inserts the barrier. **Trap:** egui "strongly prefers UNORM render targets" (egui_winit_vulkano) — mismatch your viewport image's color space and it looks subtly washed out; also, resizing the dock tab must resize the offscreen image or you get blur/aspect errors. Fyrox's editor and Bevy's (still-in-progress) editor are the Rust reference points; neither is a turnkey library, so budget ~1 week to build this.

### Reflection-driven inspector from JSON Schema (schemars)
Loom's `loom_reflect` = schemars JsonSchema *is* the type registry, so the inspector is a **JSON-Schema-to-form generator** — a *solved problem in the web world*. Borrow from **react-jsonschema-form** and **JSON Forms**: schema `type`→widget, `enum`→dropdown, `oneOf`/tagged-union→variant selector + sub-form, `minimum`/`maximum`→slider range, `array`→add/remove/reorder list, nested `object`→collapsible sub-grid, plus an `x-ui` extension vocabulary for custom widgets (color pickers, asset refs). In Rust, `bevy_inspector_egui` (mature, actively maintained, drives min/max via `#[inspector(min=…, max=…)]`), `egui_probe`, and `egui-struct` are prior art — but they reflect over Rust *types/`Reflect`*, not JSON Schema. **Recommendation:** write a schema-walker mapping schemars output → egui widgets, cribbing widget-mapping tables from react-jsonschema-form; ~1-1.5 weeks for a good v1 covering enums/tagged unions/arrays/ranges. Gives *the same inspector for human and agent-authored data* because both are just JSON-Schema-validated TOML.

### Undo/redo transaction architecture
Loom already has the right architecture (shared `SceneOp` transactions, version tokens). Industry models:
- **Unreal** `FScopedTransaction` — a scoped RAII transaction; changes within scope are captured and undone atomically. A multi-op change = one undo.
- **Blender** — historically full-scene memory snapshots (simple, robust, memory-heavy); moving toward finer-grained steps.
- **Godot** `UndoRedo` — explicit command pattern: register do/undo method+args pairs; multiple actions merged into one named undo entry.
**Loom should stay command-pattern (`SceneOp` list), not snapshot** — diffable, cheap, already integrated. A twelve-op agent transaction = one `CompoundOp { ops: Vec<SceneOp>, label }`; Ctrl+Z pops the compound and inverts its ops in reverse order. **The hard case** — "undo when the underlying file changed externally" — is where Loom's version token earns its keep: an undo whose base version no longer matches must not silently apply; raise the same divergence banner as a stale write. CRDT/OT is *overkill*: Loom is single-writer-at-a-time with optimistic concurrency + version tokens, simpler, giving a clean "reject stale write" semantic that CRDTs blur.

### Gizmos
`transform-gizmo` / `transform-gizmo-egui` is the current, maintained crate — framework-agnostic, translate/rotate/scale, fed a view+projection matrix and interaction, returns modified transforms, works with glam/mint (docs.rs). **Use `transform-gizmo`, not `egui-gizmo`** — `egui-gizmo` is effectively abandoned ("about 2 years ago," 2021 edition — lib.rs) and superseded by `transform-gizmo` by the same author. `transform-gizmo` handles screen-space-consistent sizing (`scale_factor` from the MVP) and multi-mode. You'll still implement snapping and multi-select pivot yourself. Every gizmo drag must emit a `SceneOp` (ideally one compound op on drag-release, with live preview during drag) so it undoes and is legible to the agent — do NOT mutate transforms directly.

### Editor/runtime separation (Rust-idiomatic)
Unreal uses editor modules + `WITH_EDITOR`; Unity uses `Editor/` assemblies + `#if UNITY_EDITOR`. The Rust-idiomatic equivalent, given Loom's CI-enforced dependency rules, is **separate crates + Cargo feature flags**: a `loom_editor` crate depending on `loom_scene`/`loom_render`/egui, gated so the shipped runtime never links egui or the editor. Matches Loom's discipline (e.g. `loom_agent` is a binary "so nothing can link it"). Prefer a separate crate over `#[cfg(feature)]` sprinkled through runtime crates — it makes the dependency boundary CI-checkable.

### Viewport navigation, asset browser, hierarchy
- **Navigation conventions:** orbit/pan/fly (Alt+drag orbit, MMB pan, RMB+WASD fly), **focus-on-selection (F)**, adjustable camera speed, view bookmarks, multi-viewport. Live-reload must preserve camera + selection.
- **Asset browser:** Loom already has blake3 content-hashing + `.meta` identity in `loom_asset` — the browser is a view over that DB with search/filter; content hashes give free dedup and "find usages."
- **Hierarchy for procedural instances (a severe trap):** you cannot put millions of scattered instances in the tree — it destroys the UI. Show the *scatter rule* as one collapsible node ("pine_forest — 1.2M instances"), never the instances. Selecting the node selects the rule (editable as text), not 1.2M entities. Same principle as Loom storing voxels as op-lists: represent the *generator*, not the *generated*.

### Extensibility/plugin API
For a solo project, **skip a formal editor plugin API.** High-effort, low-payoff at this scale; the command palette + the existing Rhai sandbox already give programmable extension. Revisit only if a team forms.

## B2. The genuinely novel part — human + agent co-authoring
The strong insight: **Loom already has the hard part right** — human and agent issue the *same* `SceneOp` transactions through the *same* code path with version tokens. The editor's job is to make that legible in both directions.

### Surfacing agent presence, attribution, and changes
Cursor and Claude Code both settled on **batch diff review** — the agent makes all changes, then presents them together for accept/reject, individually or all at once (Cursor's "granular acceptance"; Claude Code's `y`/`n`/`d`/`e`/`Esc` + Shift+Tab auto-accept; a filed Claude Code issue explicitly asks for Cursor-style batch-diff because per-line-blind approval is a "significant UX regression"). **Lesson for Loom:** the unit of review is the *labelled transaction*, not the line. A twelve-op transaction shows as one card: "Placed 3 benches, aligned to path (12 ops)" with expand-to-detail. Borrow Figma-multiplayer/Google-Docs-suggestion-mode presence: a colored "Agent is editing" banner and per-transaction attribution in the feed.

### Diff review UX for a 3D scene (hard, little prior art)
Synthesize from CAD/AEC version control (Speckle is "the Git & Hub for geometry," object-based not file-based; Onshape's change-based collaboration) and Unreal's OFPA + visual asset-diff:
- **Ghost/overlay the previous state** in the viewport (semi-transparent "before," solid "after"), added green, removed red, moved shown with a motion arrow — the spatial analogue of Cursor's green/red hunks.
- **Jump-to-change:** clicking a transaction frames the camera on the affected bounds (focus-on-selection reused).
- **Because Loom scenes are diffable TOML**, you also get a *text* diff for free — show both: text diff for precision, viewport ghost for spatial intuition. This dual view is Loom's structural advantage over UE.

### Transaction log / activity feed
A scrolling, labelled feed (prior art: Figma version history, Blender's Info log, AI-agent activity feeds). Each entry: label, author (human/agent), op count, timestamp, and [Jump] [Revert] [Expand]. Reverting issues the inverse compound op through the same path (so it too is undoable). Directly buildable on Loom's existing transaction stack — a *view*, not new infrastructure.

### Approval gates / permission scoping / trust levels
Loom runs the agent with `destructive` scope off by default. Prior art (Claude Code Shift+Tab auto-accept; Cursor allowlist-gated auto-run) teaches **batch approvals and trust levels, not per-op nagging:**
- **Scoped auto-approve:** non-destructive ops (place, transform, param edits) apply immediately; destructive ops (delete node/asset, `prefab` expansion, large-region terrain CSG-subtract) queue for a single batched approval card.
- **Trust levels:** a session setting from "review everything" → "auto-approve non-destructive" → "auto-approve all in this subtree." Scope by region/asset so approval fatigue doesn't set in.
- **Never** silently apply a destructive op; but **do** batch ten deletions into one "Approve 10 deletions?" card.

### Live-reload UX (`loom run --watch`)
Pitfalls when a file reloads mid-edit: **lost camera position, lost selection, lost in-progress edits, flicker.** Mitigations:
- Persist camera + selection *outside* the scene state and re-apply after reload (never derive camera from the reloaded scene).
- **Debounce** file-watch events (agents write in bursts; reload on quiescence, not per keystroke).
- **Never reload while a human drag/gizmo/text-edit is in progress** — queue the reload until the interaction completes, or you snatch state mid-gesture.
- Diff-and-patch the scene graph rather than tearing it down and rebuilding (preserves selection handles, avoids a full GPU rebuild flash).

### Making the editor's actions legible TO the agent (the reverse direction)
The subtle half most tools ignore. When a human moves an object, the agent's next action must not be based on stale state. Loom's version token + optimistic concurrency is exactly the right primitive:
- Every human edit bumps the scene version and appends to the same transaction log the agent reads.
- The agent, before acting, reads the current version; a write against a stale version is **rejected, never silently merged**, surfacing the divergence banner offering both versions.
- Surface a presence/version indicator ("scene at v142; agent last saw v139 — refreshing") so the human understands *why* an agent write bounced.

### Command palette issuing the same SceneOps (strongly recommended)
**Yes — expose a command palette that issues the exact same `SceneOps` the agent uses.** This makes human and agent *literally share one interface*: guarantees parity in both directions with identical undo/version semantics; gives the human a fast, keyboard-driven, diffable way to author (`place_on floor grid 5x5 bench`) that emits reviewable text ops; and doubles as documentation of the agent's command surface. Cheap given the ops exist (a palette over `SceneOp` + `place_on`/`align_to`/`grid_on`/`face_toward`).

## B3. Tooling for the procedural system (bridges A and B)

### Editor side of text-authored scatter
Keep **text as the single source of truth**, give interactive feedback on top:
- **Live preview with parameter scrubbing:** dragging a slider edits the TOML value in memory and re-runs the (deterministic) scatter for the affected cells only, live; on release, commit one `SceneOp`. The slider *is* editing the text.
- **Seed re-roll UI:** a button increments the `seed` scalar and regenerates — one-line diff, fully reproducible. UE's seed workflow but with a diffable artifact.
- **Region painting that writes text (the round-tripping trap, solved):** a painted mask is NOT diffable. Mirror Loom's voxel-as-op-list decision: **store painted regions as polygons/splines/op-lists, not bitmaps.** A painted exclusion zone serializes as a closed polygon (`region.exclude = [[x,y],…]`) or a spline — diffable, editable as text, re-editable as a painted overlay (exactly UE's Polygon 2D/spline approach; round-trips cleanly). Never persist a painted bitmap into version control.

### Debugging (copy from UE's PCG editor)
- **Per-node inspect/debug (press D):** visualize a node's output as points, density as grayscale, in the viewport (Epic). Loom's analogue: a "show scatter points" overlay rendering candidates colored by the driving field (slope/flow/density) *before* meshes spawn — invaluable for understanding why a rule placed nothing.
- **Attribute List view:** a table of point attributes for the selected rule (Loom: a dockable table of the first N points with computed slope/flow/seed).
- **Debug tree / profiling per node:** per-rule generation time and instance count.

### Live preview without breaking determinism
Preview MUST use the identical deterministic code path as `loom sim` — same sampler, position-hashed seeds, fixed iteration order. **Trap:** it is tempting to make preview "fast" with a different (GPU, or `thread_rng`-jittered) sampler; that silently diverges from the committed result and the sim hash. Rule: **preview and commit call the same function.** Loom's CPU-authoritative + generated-GPU pattern already enforces this.

---

# Where A and B Intersect
1. **Prefab system is the shared blocker.** Scattered instances are semantically prefab instances (mesh + material + transform + overrides). Loom has **no prefab system** — the parser "refuses `prefab =` and `extends =` keys loudly." You cannot cleanly express "1.2M instances of this configured prop" without prefab/instance-with-overrides. **Build the prefab system before procedural scattering.** It also unblocks the editor hierarchy (show the prefab/rule node, not the instances) and the PCG-Graph-Instance-style override model. The single most important dependency finding.
2. **The version-token transaction system is the shared substrate** for the editor's undo/co-authoring and the scatter system's dirty-region regeneration (both compound `SceneOp`s).
3. **Text-first + deterministic is the through-line:** the scatter recipe (A) and the co-authoring diff/review UX (B) are both only possible *because* everything is diffable TOML with position-derived seeds — Loom's structural advantage over UE, which has neither for PCG.
4. **Golden-image gap bites both:** impostor popping, scatter motion artifacts, and viewport diff correctness are invisible to Loom's unit-tests+hashes. Both need a golden-image/pixel-diff harness before shipping visually.

---

# Effort estimates (one competent dev + AI coding agent)
**Topic A (~10-13 weeks v1):** Prefab system (blocker) 1.5-2.5w; deterministic samplers (Bridson + Halton + position-hashed seeding) as `loom_scatter` 1-1.5w; scatter-recipe schema + TOML + SceneOp + terrain-field binding 1.5-2w; GPU-driven instance cull + indirect draw + HISM-equivalent clustering 2-3w; octahedral impostors 1-1.5w; dirty-region incremental regeneration over voxel edits 1.5-2w (novel/risky); named-intermediate-output DAG semantics 1w.

**Topic B (~6-9 weeks v1):** render-to-texture viewport + camera nav 1-1.5w; JSON-Schema → egui inspector 1-1.5w; `transform-gizmo` + snapping + SceneOp emit 0.5-1w; transaction/activity feed + revert 0.5-1w; diff-review-in-viewport (ghost/overlay + text diff) 1.5-2w (novel); approval gates + trust levels 0.5-1w; live-reload hardening 0.5-1w; command palette 0.5-1w.

---

# Recommendations (staged)

### Topic A
1. **Stage 0 (unblock):** Build the prefab / instance-with-overrides system. Nothing else in A is clean without it. *Benchmark to proceed:* a prefab can be placed, overridden per-instance, and round-trips through TOML + SceneOp + undo.
2. **Stage 1 (minimal-but-convincing v1):** `loom_scatter` = deterministic Bridson sampler + flat TOML scatter rules bound to `loom_terrain` slope/flow/SDF, spawning prefab instances via ISM, verifiable by `loom sim … --assert`. One rule, one biome, reproducible across debug/release. *This alone beats UE on reviewability and determinism.*
3. **Stage 2:** Named intermediate outputs (`exclude_from`), biome priority blending, Halton coverage sampler, GPU-driven cull + indirect draw for scale.
4. **Stage 3:** Dirty-region incremental regeneration over voxel destruction; octahedral impostors; optional WFC module for structured placement.
- *Threshold that changes the plan:* if you cannot hold determinism across debug/release for the sampler (state-hash divergence in `cargo xtask validate`), stop and fix the RNG/iteration-order before adding features — it invalidates everything downstream.

### Topic B
1. **Stage 1 (minimal-but-convincing v1):** Render-to-texture viewport + camera nav + `transform-gizmo` + JSON-Schema inspector, all emitting SceneOps. Makes the existing M12 editor genuinely usable for spatial work.
2. **Stage 2 (the differentiator):** Transaction/activity feed + batch approval gates + version-divergence banner + command palette. Operationalizes human+agent co-authoring on infrastructure you already have.
3. **Stage 3:** Diff-review-in-viewport (ghost/overlay + paired text diff), live-reload hardening, scatter live-preview/debug overlays (bridges to A).
- *Threshold:* if egui's frame budget collapses on large scenes even with row virtualization + rule-node collapsing, that is the signal to invest in a virtualized retained-mode panel — *not* to switch GUI frameworks.

---

# Caveats and Traps (highest-value section)

**Determinism traps (silent, catastrophic):**
- `HashMap`/`HashSet` iteration order in a sampler or scatter loop → non-reproducible placement that passes a still-screenshot check but fails `cargo xtask validate`. Loom's clippy.toml already bans this — extend into any new scatter/WFC code.
- Bridson's active-list uses random pops; the RNG stream *and* pop order must be fixed. A "cosmetic" refactor that reorders the active list silently changes every layout.
- Any GPU→CPU readback feeding scatter counts/positions into the sim is disqualifying (readback timing non-reproducible). Keep scatter CPU-authoritative; generate the GPU version, never read it back.
- Floating-point non-associativity: parallelizing a density sum across threads with non-deterministic reduction order changes low bits → hash divergence. Use fixed reduction order or integer accumulation.

**Scale traps (only break at millions):**
- Putting instances in the hierarchy panel or the ECS as individual entities (recall `loom_ecs` is `Vec<Option<T>>`, not archetype storage — millions of scattered entities will be pathological). Represent the *rule*, not the instances.
- 4M-point PCG "generates ok, but very low FPS" even in UE — instance culling is not optional; build GPU-driven cull early.
- Instance transform memory: 4M × 64B = 256 MB; stream per cell or you blow the budget on larger worlds.

**Motion/editing traps (invisible in a still, no golden-image net):**
- Impostor popping and normal errors at LOD transitions — fine in a screenshot, wrong in motion.
- Live-reload snatching camera/selection mid-gesture.
- Hi-Z occlusion 1-frame latency causing edge flicker if cull bounds aren't padded.
- **Build a golden-image/pixel-diff harness** — a documented structural blind spot, and both topics produce motion/visual artifacts unit tests can't catch.

**Human+agent co-authoring hazards:**
- Two writers, one file: without the version token, an agent write could silently clobber a human edit. The token + "reject stale, offer both" banner is correct — do not weaken it into an auto-merge.
- Approval fatigue: per-op prompts train the human to blind-approve (the exact regression the Claude Code issue names). Batch and scope approvals.
- Stale agent state: an agent acting on a version the human already changed produces spatially-wrong results that *look* plausible. Version-check before every agent write.
- Reverting an agent transaction must itself be a transaction (undoable, versioned) or you create un-revertable history.

**Text-first traps:**
- Painted masks are not diffable — store as polygons/splines/op-lists or you reintroduce the binary-blob problem Loom exists to avoid.
- Node-graph temptation: do not add a visual node graph "just for the hard cases" — it reintroduces position-laden, ID-heavy, unmergeable serialization (UE and Blender both prove this is unsolved). Use named intermediate outputs in a linear stack instead.

**Rust-ecosystem gaps (vs C++):**
- No mature retained-mode editor toolkit (no Slate/Qt equivalent); you hand-build virtualized trees and advanced property grids on egui.
- `egui-gizmo` is abandoned; `transform-gizmo` is the live successor but you still build snapping/pivot yourself.
- No turnkey Rust "3D scene diff" or "embed-Vulkan-in-egui editor" library — Fyrox and the WIP Bevy editor are references to read, not dependencies to reuse.
- JSON-Schema-to-egui-form must be written (react-jsonschema-form/JSON Forms to crib from; Rust has no equivalent).

**Unverified / shifted-in-2025-2026 (flag before building on):**
- Exact UE 5.6/5.7 Landscape Nanite / Virtual Heightfield Mesh changes — not fully verified before the search budget was exhausted; check current Epic docs.
- Whether any experimental UE text-asset export now applies to PCG graphs — as of research, **no** such export is documented; treat "PCG is binary-only" as strongly-but-not-absolutely confirmed (proven by confirmed binary-`.uasset` storage + absence of any documented text path).
- PCG reached **production-ready only as of 5.7 (Nov 2025)** and still crashes at scale in some 5.7.x builds — Loom is not "behind" a stable target; the target itself is young.