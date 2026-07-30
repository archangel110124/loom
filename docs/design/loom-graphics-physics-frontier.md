# Loom: Frontier Techniques for Graphics & Physics

**Companion to `ai-native-engine-design.md`** — what the state of the art actually looks like in
mid-2026, what's reachable from Rust today, and what to deliberately skip.

> **BACKEND OVERRIDE:** this doc was written assuming wgpu. The backend is now **Vulkan 1.3 via
> `ash`** — see `loom-vulkan-backend.md`, which supersedes every wgpu-specific claim below.
> The affected sections are A.1 (Slang target), A.2 (bindless), B.4 (mesh shaders), B.5 (ray
> tracing), and the Part D table. The underlying techniques are all unchanged and mostly become
> *easier*.

Organizing principle throughout: **an AI-authored engine has a different efficiency profile
than a human-authored one.** The agent renders constantly for verification, generates scenes
with no intuition for performance or physical plausibility, and iterates in tight loops. That
changes which optimizations matter and adds a category of robustness problem that human-authored
engines mostly dodge. Flagged as **[AI-relevant]** wherever it applies.

---

## Part A — Cross-cutting architecture

### A.1 Author shaders in Slang, not WGSL

**This is the highest-value, lowest-risk adoption on the entire list, and almost nobody building
a hobby engine knows about it yet.**

Slang is a shading language originally from NVIDIA, now an open-source Khronos project with a
working group and IP framework, built on roughly fifteen years of research. It compiles from a
single source to SPIR-V, HLSL, GLSL, MSL, **WGSL**, and CUDA/CPU compute targets.

Why it matters for Loom specifically:

- **WGSL is a weak authoring language.** It has no modules, no generics, no real code
  organization — Khronos' own comparison chart marks WGSL as lacking modular code management.
  Every wgpu project eventually invents a string-pasting preprocessor to work around this. Slang
  gives you actual modules, interfaces, and generics that are *pre-checked*, so you don't get the
  cascading incomprehensible errors C++ templates produce.
- **Modules compile offline to an IR and link at runtime.** That's a real answer to shader
  compile times, which are the second-worst iteration killer in a Rust graphics project after
  crate builds.
- **It's proven at scale, not aspirational.** Valve integrated Slang into Source 2 and shipped
  the generated SPIR-V in Counter-Strike 2 and Dota 2; per one write-up their migration needed
  about ten lines of change because Slang is effectively a superset of HLSL. Autodesk uses it for
  a single-source ray tracing codebase in Aurora.
- **Auto-differentiation is a first-class language feature**, which is what makes neural/learned
  material evaluation in shaders practical. Not something you need now — but it's the on-ramp if
  learned materials become real.

**Recommendation:** author in Slang, target **SPIR-V** directly. (This originally said "target WGSL
for wgpu" — obsolete: the backend is now Vulkan, so SPIR-V is the native target with no translation
step and access to features WGSL cannot express. See `loom-vulkan-backend.md`. Pleasingly, Slang +
SPIR-V is also the stack the current best-practice Vulkan reference uses.) One build step,
`slangc`, in your asset pipeline. Do this from Phase 1, because retrofitting a shader language after
you have 40 shaders is miserable.

Caveat worth respecting: portable authoring does not mean portable *behavior*. Each API and GPU
generation still exposes different capabilities and driver quirks, so you still test on the
hardware you ship to.

### A.2 Bindless — *superseded, see `loom-vulkan-backend.md` §5*

> Vulkan descriptor indexing plus buffer device address replaces this entire discussion. Buffers
> need no descriptors at all; textures live in one variable-count array indexed by `u32`. The
> wgpu binding-array limits below are historical context only.

Bindless means the shader indexes into a large array of resources rather than the CPU binding a
small set before each draw. WebGPU's default model is "bindful": shaders access only what's in
the currently bound bind groups, the CPU must know every resource a shader might need at
command-recording time, and the resource count stays under limits that can be fairly low. That
model was chosen so WebGPU v1 could run on hardware with a fixed number of resource registers.

The bindful model is what forces one draw call per material, which is what makes CPU-side draw
submission your bottleneck, which is what blocks GPU-driven rendering. So bindless is the
unlock for everything in §B.

The state of it in wgpu is better than most people assume. Per wgpu's changelog, binding arrays
now count against binding-array-specific limits rather than the standard per-stage texture
limits — and that change took Metal's binding array capacity from somewhere between 32 and 128
resources up to 500,000 sampled textures, with more efficient binding on Metal as well. It also
let legacy Intel GPUs go from about 1,800 bindless resources to a million.

One validation rule to design around: if a bind group contains a binding array, you can't use
dynamic offset buffers or uniform buffers in that bind group. That's inherited from Vulkan's
`UpdateAfterBind` descriptor rules. Plan your bind group layout accordingly — put bindless
material data in storage buffers, not uniforms.

A formal WebGPU bindless extension is still in proposal (gpuweb/proposals/bindless.md), so treat
the wgpu-native path as ahead of the web path. **Design implication: your desktop build can go
bindless now; your WASM build may need the bindful fallback for a while.** Keep the material
system abstracted over both.

### A.3 Determinism as a first-class architectural property **[AI-relevant]**

Ordinarily determinism is a networking or replay concern. In Loom it's a *verification* concern:
`run_scene` assertions are one of your two feedback channels (§2.10 of the main doc), and a
non-deterministic simulation makes every assertion flaky, which trains the agent to ignore
failures. That's worse than having no assertions.

What it takes, all of it cheap if you decide early and ruinous to retrofit:

1. **Fixed timestep, always.** Accumulator with a max-catchup clamp. Render interpolates; the
   simulation never sees a variable `dt`.
2. **Reject `-ffast-math` and equivalents.** Box3D makes a point of explicitly refusing fast-math
   optimizations because they break deterministic behavior. Rust doesn't enable it by default —
   just don't turn it on, and don't let a dependency turn it on for you.
3. **Deterministic iteration order everywhere.** No `HashMap` iteration in simulation code — use
   `IndexMap`, `BTreeMap`, or sorted `Vec`. This is the single most common source of "works on my
   machine" nondeterminism in Rust engines, and it hides for months.
4. **Seeded RNG stored in the scene**, not thread-local. An agent-authored scene with random
   spawn positions must replay identically.
5. **Stable entity allocation order.** Generational indices are fine; entity *IDs* must be
   assigned in a reproducible sequence for a given scene load.

### A.4 ECS refinements worth stealing

Beyond the archetype basics in the main doc:

- **Change detection as an agent-facing API.** Tick-based `Added<T>`/`Changed<T>` tracking already
  exists in Bevy. Expose it as `what_changed_since(tick)` — a diff of the running world is far
  cheaper feedback than re-reading a scene file, and it directly answers "did my edit do what I
  meant?" **[AI-relevant]**
- **Immutable components.** Bevy 0.16 added components that can't be mutated after insertion —
  you can't get a `&mut` to them, and the only way to change one is to insert a new instance on
  top. Perfect for anything with an invariant to protect: asset handles, IDs, anything the
  validator has already checked. The agent then physically cannot violate the invariant through
  a field write. **[AI-relevant]**
- **Hybrid Table/SparseSet storage.** Dense components in tables for iteration speed; churny
  tag-like components in sparse sets, because moving an entity between archetypes on every
  add/remove is the archetype model's known weak spot.
- **Graph-coloring parallelism.** Both Bevy's scheduler and Box2D v3's contact solver use the
  same idea: color the dependency graph, run same-colored work in parallel. Worth understanding
  once because it shows up in both your ECS and your physics.

---

## Part B — Graphics: efficient and robust

### B.0 The order to do this in

The techniques below are the state of the art, and adopting them in the wrong order will sink the
project. Phase them:

| Stage | What | Why then |
| --- | --- | --- |
| Phase 1 | Forward renderer, one draw per mesh, Slang shaders | You need pixels on screen to test the agent loop at all |
| Phase 6 | Bindless materials + indirect draws + frustum culling | The first real scaling step, and it's mostly CPU-side |
| Phase 7 | Visibility buffer + Hi-Z two-pass occlusion + meshlets | The Nanite-class win; big job, big payoff |
| Phase 7+ | Mesh/task shaders as an accelerated path | Available in Vulkan; compute path first for debuggability |
| Not now | Hardware ray tracing, neural materials | Not available or not ready — see §B.5 |

### B.1 GPU-driven rendering: the core shift

The traditional model has the CPU issue a draw call per object. GPU-driven rendering flips it:
the GPU decides what to render. The core idea behind meshlet rendering, as wgpu's own mesh-shading
spec puts it, is that the GPU decides how to render the many small parts of a scene instead of the
CPU issuing a draw call per small part or one inefficient monolithic draw for a large part.

Since about 2015 the field has moved toward using compute shaders to determine triangle visibility
before handing geometry to the rasterizer — Graham Wihlidal's GDC 2016 talk is the canonical
reference for that mindset. Treating draw operations as regular data means they can be pre-built,
cached, reused, and generated *on the GPU*, with the compute pass producing a visible-triangle list
that feeds the render pipeline.

Practical Rust path: a compute pass writes an indirect draw buffer; one `multi_draw_indirect` call
renders the scene. Combined with bindless materials (§A.2), the CPU's per-frame work becomes
roughly constant regardless of object count.

**[AI-relevant]** This matters more here than in a normal engine. `render_preview` runs the full
pipeline for a single frame, potentially dozens of times per agent task. A GPU-driven path makes
that near-free; a per-object-draw-call path makes your verification loop the slowest part of the
agent's workflow.

### B.2 Meshlets and virtual geometry — the practical Nanite subset

Split each mesh into meshlets (small clusters of triangles), then build a DAG: cluster the mesh,
group clusters, simplify each group into a smaller set of new clusters, and repeat. The result is a
tree whose leaves are the base mesh and whose root is a coarse approximation. At runtime you pick
clusters from *different levels* of the tree, so a near part of a mesh renders at high resolution
while a far part of the same mesh renders coarsely — unlike traditional LODs, which are all or
nothing per object. Bevy ships this as the `meshlet` cargo feature and describes it as their
Nanite-like system, giving far higher geometry density and freeing artists from hand-authoring LODs.

**The single most useful finding in this entire research pass**, from the Bevy virtual-geometry
discussion: meshlets + a visibility buffer + two-pass occlusion culling + GPU-driven rendering gets
you 60–70% of Nanite's benefit, and **mesh shaders are not required — only indirect indexed
draws.** Mesh shader support is a later performance addition with the compute path as fallback, not
a prerequisite.

That's the whole strategy. You do not need experimental GPU features to get most of this.

Hard-won implementation details from the Bevy work, which will save you weeks:

- **Meshlet size: 255 vertices / 128 triangles, not 64/64.** A vertex:triangle ratio at or below
  1:1 leaves most meshlets under-filled on triangles, which wastes the whole point.
- **Use texture atomics, not buffer atomics, for the visibility buffer.** Storing the visbuffer as
  an `R64Uint`/`R32Uint` storage texture and rasterizing with texture atomics is faster than a
  plain GPU buffer, largely because texture-like access patterns cache better. This became possible
  in wgpu/naga once u64/u32 storage-texture atomics landed.
- **Share one cluster buffer between software and hardware raster paths.** Nanite's trick: instead
  of allocating separate buffers, fill software-rasterized clusters from the left of one buffer and
  hardware-rasterized clusters from the right. Sized adequately, they never collide, and you halve
  the memory.
- **Software-rasterize tiny clusters.** For clusters covering few pixels, a compute shader
  iterating the bounding box (per-pixel or per-scanline depending on box size across the subgroup)
  and writing via `atomicMax()` beats the fixed-function rasterizer, which is inefficient at
  sub-pixel triangle sizes.
- `meshoptimizer` (Arseny Kapoulkine) is the library for meshlet building and simplification, and
  he contributed the determinism and performance improvements to Bevy's converter directly. Use it;
  don't write your own simplifier.

### B.3 Two-pass occlusion culling with a Hi-Z pyramid

The pairing for meshlets. Build a hierarchical depth pyramid — a compute pass generating
successive mip levels down to 1×1 — then test cluster bounds against it. Pass one renders what was
visible last frame; pass two builds the pyramid from that and tests everything else, catching newly
revealed geometry without a frame of latency. This is the Nanite occlusion approach and it's been
reimplemented independently enough times that the path is well-trodden.

### B.4 Mesh and task shaders: real gains, now first-class

> No longer experimental: `VK_EXT_mesh_shader` is available on the target hardware. Promoted from
> "defer, flag-gated" to a planned Phase 7 deliverable. The measured numbers below still stand.

When you're ready, this is the accelerated path. The mesh shader pipeline replaces the
vertex-tessellation-geometry chain with a programmable task-mesh pipeline, simplifying the
classic pipeline's 3 fixed-function and 5 programmable stages down to 2 fixed-function and 3
programmable. The task shader stage decides which meshlets are visible before *any* vertex
processing happens, and can also do LOD selection and frustum culling at meshlet granularity.

Measured results, so you can judge whether it's worth it:

- Frustum plus backface culling in a task shader typically eliminates 40–60% of meshlets before
  vertex processing, on a static view of a standard test mesh.
- Cone culling can nearly double performance on meshes where roughly half the meshlets face away
  from the camera — but does nothing on a mesh where nearly all meshlets face the viewer, like a
  rock. **Know your content; the win is geometry-dependent.**
- On a real scene (San Miguel), a depth-only prepass dropped from 2.41ms with vertex shaders to
  1.24ms with an amplification shader doing occlusion — about 48%. Traces showed the clipping/
  culling unit under less pressure, coarse z-culling doing less redundant work, and the mesh path
  spawning more warps with better streaming-multiprocessor utilization.
- Counterpoint worth knowing: a *pass-through* mesh pipeline with no culling stage is slightly
  **more** expensive than the vertex pipeline for depth-only passes. The culling is the win, not
  the pipeline itself. Don't port to mesh shaders and skip the culling.

**wgpu status:** experimental, behind `Features::EXPERIMENTAL_MESH_SHADER`, requiring
`enable wgpu_mesh_shader;` at the top of a WGSL program, adding two shader stages, with task and
mesh shaders able to use compute-available functionality including subgroup ops. wgpu's own docs
warn the features may have major bugs and are subject to breaking changes. So: build the compute
path first, gate the mesh path behind a feature flag, keep both.

Also worth knowing for a Vulkan-backend reality check: on AMD, RADV's NGG pipeline already converts
vertex workloads into pseudo-meshlets and does primitive culling in driver-generated shaders. Some
of your "win" may already be happening below you — measure against the real baseline, not the
theoretical one.

### B.5 What to skip, and why

- **Hardware ray tracing.** Not officially in WebGPU. wgpu-py's roadmap lists ray tracing
  extensions as promised-future, and one WebGPU roadmap analysis puts hardware RT at earliest 2027
  if ever, with the working group not having committed, and notes RT is blocked behind bindless
  anyway. Community compute-shader implementations (WebRTX) exist. Ignore for now.
- **Neural / learned materials.** Runtime evaluation of small neural networks inside shaders to
  compute material appearance is described as among the transformative 2026 techniques, moving past
  PBR parameters toward learned appearance representations. It's real research and Slang's
  autodiff is the on-ramp — but it is not what your project needs, and it would consume the entire
  budget.
- **Clustered/tiled deferred lighting** is *not* on the skip list but is a Phase-7 item: a compute
  shader reads tile-aligned G-buffer data, culls lights against a clustered grid, and accumulates
  lighting into a buffer the composition pass samples. Standard, well-documented, do it when you
  have more than a dozen lights.

---

## Part C — Physics: efficient and robust

### C.1 The headline recommendation: do not write this

Vendor it. `rapier3d` today. Then watch Box3D closely, because it may be the better long-term
answer and it is one month old.

### C.2 The solver landscape, and a correction to conventional wisdom

If you'd asked me to guess before researching, I'd have said XPBD — it's the technique that gets
written up most enthusiastically. **That would have been wrong for rigid bodies, and the
correction is well-documented.**

XPBD (Müller, Macklin, Chentanez, Jeschke, Kim) generalizes Position Based Dynamics by introducing
per-constraint *compliance* — the inverse of stiffness — which decouples effective stiffness from
solver iteration count and lets you interpolate principledly between hard and soft constraints. It
evolves Lagrange multipliers as state, which also yields force estimates. It's genuinely excellent
for cloth, soft bodies, and continuum mechanics, and it's robust with hard constraints because
those become effectively infinitely stiff.

For rigid bodies, three independent findings point the other way:

1. **Erin Catto's Solver2D**, a purpose-built rig comparing PGS, TGS, NGS, XPBD and others across
   tests designed to push them to failure, found that **TGS Soft surpasses nearly every other
   solver in nearly all tests — including beating XPBD at tasks XPBD is supposed to excel at.**
   Box2D v3 shipped it as the "Soft Step" solver, more stable than v2.4 in almost every way:
   higher mass ratios, longer chains, larger stacks. It falls slightly behind v2.4's NGS block
   solver on vertical stacks specifically, and Catto's judgment is that the performance tradeoff
   isn't worth it for that one case.
2. **Dirk Gregorius** (Valve, Rubikon — Half-Life: Alyx's physics) makes the structural argument:
   in an impulse solver, accumulated impulses converge toward actual contact forces and reach
   equilibrium with acting forces, whereas XPBD's accumulated positional pushes go to zero once
   penetration resolves — which makes warm-starting on the position level hard. He also notes
   friction isn't really a position constraint, and good friction matters enormously for contact
   and stacking. His shipped approach: **two-pass — solve on the velocity level, then resolve
   penetration on the position level.** That shipped in Half-Life: Alyx and Box2D has done it for
   over a decade.
3. **The Rust ecosystem already made this move.** The project formerly called Bevy XPBD rebranded
   to Avian and switched solvers, explicitly citing Catto's Solver2D results. Its maintainer
   describes TGS Soft as impulse-based with an XPBD-like substepping scheme plus soft constraints
   adding spring-like damping — and notes it's extremely close to what **Rapier** had recently
   switched to, with Rapier seeing substantial improvements from it.

So the modern consensus solver is: **impulse-based, sub-stepped, with soft constraints.** TGS
derives from the "Small Steps in Physics Simulation" line of work, and sub-stepping is what makes
soft constraints newly powerful — smaller timesteps allow higher Hertz values, so soft constraints
can appear rigid while costing about what Baumgarte stabilization costs and far less than NGS.
There's a further refinement from Ross Nordby (Bepu): soft-constraint parameters can be made
**mass-independent** using three decoupled parameters instead of two coupled to body mass
properties. Adopt that formulation if you ever touch solver internals — mass-independent tuning is
enormously easier to expose to an agent or a designer.

### C.3 Box3D: the most interesting new option, and the timing is unusual

Erin Catto released **Box3D on June 30, 2026** — about a month before this document. MIT-licensed,
written in C17 with a clean C API (samples and docs use C++20, but the core is pure C, which makes
it bindable from essentially anything).

Origin story matters here because it tells you what it's good at: it came out of real production
frustration at Kintsugiyama building *The Legend of California* on Unreal, where Chaos physics had
concrete failures — no gyroscopic torque, so thin objects spun unnaturally long; broken continuous
collision when falling trees hit terrain meshes; performance collapse at hundreds of thousands of
objects. The team initially planned to fork Jolt, but on Dirk Gregorius's advice built instead on
a simplified project called Rubikon-Lite, then progressively replaced nearly all of it — APIs,
data structures, algorithms — with Box2D-derived code.

The features that matter for Loom:

- **Cross-platform determinism**, meaning identical inputs produce identical outputs regardless of
  platform. This is rare and it's exactly what §A.3 needs. It explicitly refuses `-ffast-math`.
- **Multithreaded contact solving via SIMD and graph coloring**, parallelizing physics islands
  safely.
- **~916KB release binary on macOS** — small enough that the community immediately flagged it as
  viable for WebAssembly. Relevant to your WASM ambitions.
- Triangle mesh collision, height-field collision, and baked compound collision, the last of which
  saves memory on large worlds.
- Already in production use: Facepunch's s&box has reportedly been on it for about a year, plus the
  Esoterica engine. Glenn Fiedler endorsed it in the Hacker News thread.

**The honest caveats.** It's alpha (0.1), documentation is incomplete enough that you should plan
to read headers and sample code, and it's C — so you're writing and maintaining FFI bindings, in a
project whose entire safety story rests on Rust's type system. Catto himself has said he isn't
trying to compete with other engines; it's tailored to his game's needs. And at least one observer
argues it's essentially a stripped-down Rubikon extended into 3D rather than something
architecturally novel.

**Verdict: `rapier3d` for Phase 1** — Rust-native, no FFI, already on a TGS-Soft-like solver, good
enough. **Re-evaluate Box3D at Phase 6**, when determinism and WASM size actually bind. Lock a
commit hash either way.

### C.4 Continuous collision: the robustness bar **[AI-relevant]**

Tunneling is the failure mode that makes physics feel broken, and it's the one an agent will
trigger constantly by authoring thin geometry and fast movers without knowing better.

Box2D v3's approach is a **hybrid of speculative contacts and time-of-impact**, and the reported
result is that it prevents almost all dynamic-versus-static tunneling. For bullets it also handles
dynamic-vs-kinematic and dynamic-vs-dynamic, though less robustly — a deliberate performance/
robustness balance.

Take the same position: guarantee no tunneling against static geometry (which is where an agent's
mistakes land — walls, floors, terrain), accept best-effort for dynamic-vs-dynamic.

### C.5 The problem nobody has solved, and why it's *your* problem

Catto names it directly in a recent interview: **physics sandbox games where the content is
uncontrolled remain an unsolved problem.**

Read that against your architecture. An AI agent authoring scenes *is* an uncontrolled-content
generator. It will produce the exact configurations physics engines handle worst — extreme mass
ratios, degenerate collision shapes, hundred-unit-long thin boxes, deeply nested compound bodies,
objects spawned interpenetrating — and it will produce them with no intuition that anything is
wrong, because nothing in a text scene file looks unusual.

**The insight this leads to: in an AI-authored engine, physics robustness is substantially a
validation problem, not a solver problem.** Put physical sanity checks in the same validator that
enforces the schema (§2.9 of the main doc), because that's where the agent gets structured
feedback it can act on:

| Check at authoring time | Rationale |
| --- | --- |
| Mass ratio between jointed/contacting bodies exceeds ~100:1 | Warn. High mass ratios are the classic stacking/chain failure, and TGS Soft's headline strength is handling them *better*, not perfectly. |
| Collider dimension outside ~0.01–100 world units | Warn. Degenerate scale wrecks floating-point contact generation. |
| Colliders spawned interpenetrating | Error with the overlap depth — the agent can fix this, and `scene_measure` already computes it. |
| Thin collider (min extent < speculative margin) on a dynamic body | Warn and suggest the CCD flag. |
| Compound body nesting depth > 3 | Warn. Baked compounds exist for a reason. |
| Dynamic body with no collider, or collider with no mass | Error. Almost always an agent mistake. |

Each of these costs an afternoon and prevents a class of "the physics is broken" reports that would
otherwise be unattributable. And each one is a message that teaches the agent something it can't
learn from a render.

### C.6 Efficiency checklist

- **Sleeping/island management.** A stationary body should cost nothing. Non-negotiable for
  agent-generated scenes with hundreds of static props.
- **Graph coloring for parallel islands** — same technique as Box3D's contact solver and your ECS
  scheduler.
- **Broadphase:** BVH or sweep-and-prune, and `rapier` already handles this. Don't hand-roll.
- **Substepping over iteration count.** The whole TGS insight: 4 substeps × 1 iteration generally
  beats 1 step × 4 iterations, because sub-stepping is what lets soft constraints be stiff.
- **Physics on a fixed timestep decoupled from render** (§A.3), with render interpolation between
  states.

---

## Part D — Summary: adopt, defer, skip

| Technique | Verdict | When |
| --- | --- | --- |
| Slang for shader authoring | **Adopt** | Phase 1 — cheapest big win on this list |
| Fixed timestep + determinism discipline | **Adopt** | Phase 1 — cannot be retrofitted |
| `rapier3d` | **Adopt** | Phase 1 |
| Physical sanity checks in the validator | **Adopt** | Phase 3 — pairs with the agent loop |
| Change detection as agent API | **Adopt** | Phase 3 |
| Immutable components for invariants | **Adopt** | Phase 3 |
| Bindless materials + indirect draws | **Adopt** | Phase 6 |
| Clustered lighting | **Adopt** | Phase 7 |
| Meshlets + visbuffer + Hi-Z two-pass culling | **Adopt** | Phase 7 — 60–70% of Nanite, no mesh shaders needed |
| `meshoptimizer` for meshlet building | **Adopt** | Phase 7 — with it, don't write a simplifier |
| Software raster for tiny clusters | Defer | After meshlets work |
| Mesh/task shaders | **Adopt** | Phase 7 — `VK_EXT_mesh_shader` available; build compute path first for debuggability |
| Box3D | **Defer, watch** | Re-evaluate at Phase 6 for determinism + WASM size |
| Writing your own physics solver | **Skip** | Not the interesting part of this project |
| XPBD for rigid bodies | **Skip** | Superseded by TGS Soft for this use case |
| Hardware ray tracing | **Defer (not skip)** | Available via `ash`, but destructible voxels invalidate acceleration structures constantly — see Vulkan doc §8 |
| Neural/learned materials | **Skip** | Research-stage; would consume the whole budget |

---

## Sources

Graphics: wgpu mesh shading spec and CHANGELOG (gfx-rs/wgpu) · gpuweb bindless proposal ·
jms55's Virtual Geometry in Bevy 0.14/0.15/0.16 series · Bevy virtualized-geometry discussion
#10433 · Bevy 0.14 and 0.16 release notes · Interplay of Light on meshlets and mesh shaders ·
AMD GPUOpen meshlet compression · Metal by Example on mesh shaders and meshlet culling ·
Vulkan/Slang task-and-mesh-shader practical guide · Khronos Slang announcement and shader-slang.org ·
Vulkan Documentation Project on Slang · Kaelan's WebGPU roadmap analysis.

Physics: Macklin et al., *XPBD: Position-Based Simulation of Compliant Constrained Dynamics* ·
Müller et al., *Detailed Rigid Body Simulation with Extended Position Based Dynamics* (2020) ·
Erin Catto, *Solver2D* (box2d.org) and *Releasing Box2D 3.0* · Catto, *Announcing Box3D*
(June 30, 2026) and coverage at 80.lv, byteiota, Developers Digest, Groundy · Dirk Gregorius,
GameDev.net thread on XPBD contact manifolds · Avian (formerly Bevy XPBD) issue #346 ·
*Position Based Rigid Body Simulation* (WSCG 2023) · Catto interview on Box2D and
*Legends of California*.
