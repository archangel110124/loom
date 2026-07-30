# Loom: Vulkan Backend

**Fifth companion doc, and an override.** Where the earlier docs assume wgpu, this document
supersedes them. Ripple effects are listed explicitly in §13.

Decision: **Vulkan 1.3 core via `ash`, no portability abstraction.**

---

## 0. What this buys and what it costs

**Gained — everything the graphics doc wanted but couldn't have:** mesh and task shaders as a
first-class path, hardware ray tracing, full descriptor indexing, buffer device address, timeline
semaphores, explicit multi-queue control, and a real pipeline cache. On an RTX 4090 under current
NVIDIA drivers, every extension named in this document is available.

**Lost — the browser target, permanently.** `ash` has no WASM story. That was the original argument
for wgpu, and it's now off the table: no link you can send someone, no zero-install demo, nothing a
student could open in a tab. If that ever matters, it becomes a second renderer, not a config flag.
Worth being clear-eyed that this is the real price.

**No RHI abstraction layer.** Tempting, and wrong here. An RHI trait that could also sit on wgpu
would force every design decision toward the lowest common denominator — which is precisely the
ceiling you're escaping. Instead: keep `loom_render` genuinely isolated (nothing outside it imports
`ash`), so a future backend is a rewrite of one crate rather than a diffusion through the codebase.
Isolation, not abstraction.

---

## 1. Target version: 1.3 baseline, 1.4 opportunistically

**Require Vulkan 1.3.** Everything structurally important is core there: dynamic rendering,
buffer device address, descriptor indexing, timeline semaphores, and synchronization2. That's the
version where Vulkan stops being painful.

**Use 1.4 features when present.** Vulkan 1.4 (December 2024) folded previously optional extensions
into the core spec as mandatory — push descriptors, dynamic rendering local reads, and scalar block
layouts — along with maintenance extensions up to and including `VK_KHR_maintenance6`, and
guaranteed support for 8K rendering with up to eight separate render targets plus assorted limit
increases that previously varied by implementation.

One 1.4 change is directly load-bearing for your voxel work: it introduced **new implementation
requirements for streaming transfers**, specifically so that portable applications can stream large
quantities of data to a device *while simultaneously rendering at full performance*. That is exactly
the voxel-chunk-upload-during-gameplay problem, now a spec guarantee rather than a vendor hope.

---

## 2. Modern Vulkan is far less code than the tutorials imply

Most Vulkan tutorials date from 2016–2019 and teach the verbose path. Ignore them. The current
reference is Sascha Willems' **HowToVulkan2026**, which demonstrates that Vulkan 1.3 can build a
functional rasterization application in **a single source file of a few hundred lines** — and not a
bare triangle either, but multiple lit and textured 3D objects with mouse rotation — deliberately
avoiding abstraction layers that would obscure the underlying mechanisms. Its stack is
dynamic rendering, descriptor indexing, and buffer device address, on SDL, VMA, and **Slang**.

That last detail is a nice payoff: the shader-language decision from §A.1 of the graphics doc was
made for other reasons, and it turns out to be what the current best-practice Vulkan reference also
uses.

Three features do all the boilerplate-killing:

1. **Dynamic rendering** — eliminates `VkRenderPass` and `VkFramebuffer` objects entirely, removing
   the historically cumbersome step of specifying precise attachment layouts and their
   synchronization up front. This alone deletes a few hundred lines and most of the confusion.
2. **Descriptor indexing** with `VK_DESCRIPTOR_BINDING_VARIABLE_DESCRIPTOR_COUNT_BIT` — replaces
   traditional per-set descriptor management with a bindless approach that scales to any number of
   textures.
3. **Buffer device address** — shaders fetch data directly from a buffer pointer, sidestepping
   descriptor bindings for buffers altogether. Your scene data, meshlet buffers, and voxel chunk
   data become plain pointers in Slang. This is the single biggest simplification for GPU-driven
   rendering.

Add **synchronization2** (cleaner barrier API) and **timeline semaphores** (§7) and you have a
Vulkan that is verbose but not baroque.

---

## 3. Crate stack

```toml
ash              = "*"   # raw Vulkan bindings — the foundation
ash-window       = "*"   # surface creation from a raw window handle
gpu-allocator    = "*"   # GPU memory suballocation (the VMA equivalent; AMD's VMA
                         # bindings via vk-mem are the alternative)
winit            = "*"   # windowing (unchanged from the wgpu plan)
raw-window-handle= "*"
rspirv           = "*"   # SPIR-V reflection, if you want automatic descriptor layouts
```

Shader toolchain: **Slang → SPIR-V directly.** This is strictly better than the wgpu path — no naga
translation step, no WGSL as an intermediate, and full access to SPIR-V features that WGSL cannot
express (mesh shader stages, ray tracing stages, subgroup ops, 64-bit atomics, pointers).

Editor UI: `imgui-rs-vulkan-renderer` (records into a command buffer you supply, manages per-frame
vertex/index buffers, supports frames-in-flight and custom textures) or an egui-ash integration.

**Read `caldera` (sjb3d) before writing your renderer.** It's a Rust/Vulkan experiment repo whose
core crate implements a render graph over Vulkan with **automatic memory allocation of temporary
buffers and images** and **automatic placement of barriers and layout transitions**, plus a
procedural macro for descriptor set layouts, async resource loading via Rust `async`/`await`, and
live shader reload. That is close to exactly the layer you need to build, already worked out in
Rust.

---

## 4. The layers wgpu was giving you for free

This is the honest accounting of what you now own.

| Layer | What it involves | Help available | Effort |
| --- | --- | --- | --- |
| Instance/device/queue setup | Extension + feature negotiation, physical device selection | Boilerplate, well-documented | 3–5 days |
| Swapchain | Creation, recreation on resize, present modes, image acquisition | `ash-window` | 3–5 days |
| Memory allocation | Suballocation, memory type selection, defragmentation | **`gpu-allocator`** covers this | 2–3 days to integrate |
| Descriptor management | Pools, sets, layouts, update-after-bind | Mostly avoided via §6 | 1 week |
| Synchronization | Barriers, layout transitions, queue ownership, fences/semaphores | Render graph automates it | **2–3 weeks** |
| Pipeline management | PSO creation, caching, dynamic state | `VkPipelineCache` (§10) | 1 week |
| Command buffers + frames in flight | Pools, per-frame resources, recording | Standard patterns | 1 week |
| Staging/transfer | Upload rings, async transfer queue | §7 | 1–2 weeks |
| Render graph | Pass declaration, resource lifetimes, automatic barriers | **`caldera` as reference** | 3–4 weeks |

**Total: roughly 8–11 weeks of infrastructure before you're at feature parity with `wgpu::Device`.**
Synchronization and the render graph are the two that actually hurt; everything else is typing.

The render graph is not optional in Vulkan the way it was optional in wgpu. wgpu tracked resource
state and inserted barriers for you. Now that's yours, and doing it manually per-pass is how
projects accumulate subtle, hardware-specific corruption bugs. Build the graph early — it's the
thing that makes the rest safe.

---

## 5. Bindless, done properly

Replaces §A.2 of the graphics doc, which was about working around wgpu's constraints.

**One giant descriptor set, indexed by integer.** Create a descriptor set with a variable-count
array of combined image samplers sized to your texture budget (100k+ is fine on a 4090), flagged
`UPDATE_AFTER_BIND` and `PARTIALLY_BOUND`. Materials store a `u32` texture index. Shaders index the
array. No per-draw descriptor binding, ever.

**Buffers don't need descriptors at all.** Buffer device address means a buffer is a 64-bit pointer
you push as a constant or embed in another buffer. Your scene structure becomes a pointer graph the
GPU walks — meshlet buffers pointing at vertex buffers pointing at material structs:

```hlsl
// Slang — buffer device address makes GPU-driven rendering read like CPU code
struct Meshlet { uint vertexOffset; uint triangleOffset; uint vertexCount; uint triangleCount; }
struct DrawData {
    Meshlet*  meshlets;      // a pointer, not a descriptor
    float3*   positions;
    Material* materials;
    uint      meshletCount;
}
[[vk::push_constant]] DrawData* draw;
```

This is the thing that makes the meshlet/visibility-buffer design from §B.2 of the graphics doc
straightforward instead of a fight. It was not available to you before.

The newer alternative is `VK_EXT_descriptor_buffer`, which eliminates descriptor pools entirely and
treats descriptors as plain memory. Cleaner, well supported on NVIDIA, less battle-tested across
vendors. Start with descriptor indexing; migrate if pool management annoys you.

---

## 6. Multi-queue: a concrete win for the voxel design

wgpu exposed one queue. Vulkan exposes what the hardware actually has, and your 4090 has a graphics
queue, dedicated compute queues, and a **dedicated transfer queue backed by separate copy engines**.

That maps directly onto the destructible-voxel pipeline:

```
Graphics queue  → render the frame
Async compute   → voxel Surface Nets/DC meshing, erosion sim, particle update
Transfer queue  → voxel chunk uploads, texture streaming, staging copies
                  (runs on copy engines, does not steal graphics throughput)
```

Coordinate with **timeline semaphores** — monotonically increasing counters instead of binary
signal/wait pairs, which makes multi-queue dependency chains expressible without a semaphore zoo.

The gotcha, and it will bite you: **queue family ownership transfers.** A resource written by the
transfer queue and read by the graphics queue needs an explicit ownership transfer via paired
release/acquire barriers, or you get undefined contents that manifest as intermittent corruption on
some drivers and not others. Put this in the render graph so it's handled once, correctly, rather
than remembered at each call site.

This is where Vulkan 1.4's streaming-transfer guarantees earn their keep: the "stream large data
while rendering at full performance" requirement is the thing your voxel world needs to be portable
rather than NVIDIA-specific.

---

## 7. Mesh shaders: promoted from deferred to planned

`VK_EXT_mesh_shader` is available on your hardware today. The graphics doc gated the meshlet
pipeline behind "experimental in wgpu, keep the compute path as fallback" — that caveat is gone.

Revised position: build the compute-based meshlet culling path first anyway, because it's simpler to
debug and the numbers from §B.4 show the *culling* is the win, not the pipeline. But target mesh
shaders as a real Phase 7 deliverable rather than an "if wgpu ever ships it" hope. Keep the compute
path only if you care about running on non-NVIDIA hardware — which for a personal project on one
machine, you may not.

---

## 8. Ray tracing: now accessible, still deferred — for a different reason

The earlier "skip it" was about API access: WebGPU doesn't have it. That reason is void. `ash`
exposes `VK_KHR_acceleration_structure`, `VK_KHR_ray_tracing_pipeline`, and `VK_KHR_ray_query`, and
the 4090 has the hardware.

**The new reason to defer is your own architecture, and it's a better reason.** Hardware ray tracing
requires acceleration structures — a BLAS per mesh, a TLAS over the scene. Destructible voxel
terrain invalidates the BLAS *every time the player blows a hole in something*. Rebuilding
acceleration structures for regenerated voxel meshes, every frame that destruction occurs, is
expensive and scales with geometry. A destructible world is close to the worst case for RT.

So the conclusion from the graphics doc survives, on firmer ground: **SDFGI-style dynamic GI remains
the right call**, because you already have an SDF and it needs no acceleration structure and no bake.

Where RT *is* worth using, in bounded form:
- **Ray-traced shadows against static geometry only** — a TLAS containing props and structures but
  not terrain, rebuilt only when structures change.
- **`rayQuery` in compute** for discrete queries (AO, contact shadows, reflection probes) without a
  full RT pipeline.
- **Offline baking acceleration**, if you ever want a lightmap path — RT makes baking dramatically
  faster even if runtime is rasterized.

Note also that `rayQuery` in a compute shader is a much smaller commitment than a full ray tracing
pipeline with shader binding tables. Start there if you start at all.

---

## 9. Pipeline cache: a solved problem now

The graphics doc flagged shader-compilation stutter as a genuine gap, since wgpu's precompiled
shader / pipeline cache support was an open tracking issue. In Vulkan it's `VkPipelineCache`:

1. Create a pipeline cache at startup, populated from a file on disk if present.
2. Pass it to every `vkCreateGraphicsPipelines` / `vkCreateComputePipelines` call.
3. Serialize with `vkGetPipelineCacheData` on shutdown.
4. **Key the cache file by driver version + device ID + a hash of your shader set** — a stale cache
   is silently ignored at best and a correctness hazard at worst.
5. Ship a pre-warmed cache built on your target hardware.

Roughly 40 lines, and it eliminates the first-use compile hitch that plagues DX12/Vulkan titles.
Do it in Phase 1, not later, because retrofitting means auditing every pipeline creation site.

---

## 10. Validation and debugging — better than what you had

This is an underrated upgrade. wgpu's validation is good; Vulkan's is more thorough and more
configurable.

- **`VK_LAYER_KHRONOS_validation`** in every debug build, non-negotiable. Enable the
  **synchronization validation** and **best practices** sub-layers explicitly — sync validation in
  particular catches exactly the barrier and queue-ownership bugs from §6 that would otherwise
  present as random corruption.
- **GPU-assisted validation** for descriptor indexing bounds checks. Slow, but run it periodically;
  an out-of-bounds bindless index is otherwise a silent garbage read.
- **`VK_EXT_debug_utils` object naming.** Name every buffer, image, pipeline, and command buffer.
  This costs a string per resource and transforms RenderDoc and Nsight Graphics captures from
  hex-handle soup into something readable. Nsight works natively on your hardware.

**[AI-relevant]** Named objects plus structured validation output is a **diagnostic channel the
agent can read.** Route the validation callback into the same structured-error format as your schema
validator (§2.9 of the main doc), and a GPU error becomes something the agent can act on —
"pipeline `voxel_mesh_pass` read descriptor index 4096, array size 4096" is actionable feedback in a
way that a driver crash is not. Wire this up when you build the render graph.

---

## 11. Revised Phase 1

The cost of this decision, stated plainly.

**Old Phase 1 (wgpu): weeks 3–6.**
**New Phase 1 (Vulkan): weeks 3–13.**

| Weeks | Work |
| --- | --- |
| 3 | Instance, device, queue families, extension/feature negotiation, validation layers wired up |
| 4 | Swapchain + recreation; `gpu-allocator` integrated; first cleared frame |
| 5 | Slang → SPIR-V build step; first triangle via dynamic rendering; pipeline cache (§9) |
| 6–7 | Descriptor indexing setup; buffer device address; push constant plumbing |
| 8–10 | Render graph: pass declaration, resource lifetimes, automatic barriers and layout transitions |
| 11 | Transfer queue + timeline semaphores; staging ring |
| 12 | glTF loading; primitive library; textured meshes on screen |
| 13 | ECS transform propagation; fixed-timestep loop; deterministic frame timing |

**Everything downstream shifts by about six weeks, including the Phase 3 agent-loop gate.** That's
the real price of this decision — not the difficulty, the delay before you learn whether the core
thesis works. If that trade bothers you, the alternative is wgpu for Phase 1–3 purely to reach the
gate faster, then rewrite the renderer. I'd take the six weeks: rewriting a renderer mid-project
while also building agent tooling is worse than starting on the right one.

---

## 12. Ripple effects — what's now wrong in the earlier docs

| Doc | Section | Status |
| --- | --- | --- |
| graphics/physics | §A.1 Slang | Target **SPIR-V**, not WGSL. Simpler — one less translation. |
| graphics/physics | §A.2 Bindless | Superseded by §5 here. The wgpu binding-array limits discussion no longer applies. |
| graphics/physics | §B.1 GPU-driven | Still correct; buffer device address makes it easier than described. |
| graphics/physics | §B.4 Mesh shaders | No longer experimental. Promoted to planned (§7). |
| graphics/physics | §B.5 Skip list | Hardware RT moves off "unavailable" to "available but deferred for architectural reasons" (§8). |
| graphics/physics | Part D table | Mesh shaders: adopt at Phase 7. RT: defer, not skip. |
| main design | §2.10 render_preview | Headless render is now an offscreen Vulkan target — same code path principle, different API. |
| main design | §2.11 hot reload | Compile-time concern unchanged, but Vulkan validation makes the dev build slower; keep a validation-off profile for perf testing. |
| main design | §2.13 crate layout | `loom_render` is `ash`-based; add `loom_render_graph`. Nothing outside `loom_render*` imports `ash`. |
| main design | Phase 1 | Weeks 3–6 → weeks 3–13 (§11). |
| voxel system | §7 GPU meshing | `silk-clouds` was a wgpu reference; the Vulkan equivalent uses indirect draw + device address, and the **async compute queue** makes GPU meshing genuinely better than described. |
| voxel system | §3.1 off-thread advantage | Still true and now stronger: dedicated transfer queue means chunk uploads don't compete with rendering. |
| terrain | §4 erosion on GPU | Async compute queue means erosion can run without stalling the frame. |

---

## 13. Risks specific to this choice

| Risk | Mitigation |
| --- | --- |
| **`unsafe` everywhere, in a project whose safety story is the type system** | Wrap resources in RAII types with `Drop` impls immediately — never hold a raw `vk::Buffer`. Confine `unsafe` to `loom_render`. Validation layers always on in dev. |
| **Synchronization bugs that only appear on other hardware** | Synchronization validation layer, always. Render graph owns all barriers. Accept that you cannot fully test this on one GPU. |
| **NVIDIA-only testing** | You have one vendor. AMD's RADV and Intel behave differently, especially around descriptor limits and queue families. If you ever distribute this, budget real time for it; if it's personal, ignore it deliberately rather than accidentally. |
| **`ash` API churn** | Pin the version. `ash` tracks Vulkan header releases and does break. |
| **No web target, forever** | Named in §0. If a browser demo ever matters — including for anything you'd want other people to just click and try — that's a second renderer, so decide now whether you're truly fine with that. |
| **Six-week delay to the Phase 3 gate** | Accepted deliberately (§11). Don't let Phase 1 scope-creep past week 13 — the gate is what matters. |

---

## Sources

Sascha Willems, *HowToVulkan2026* (github.com/SaschaWillems/HowToVulkan2026) and coverage of it ·
Khronos Vulkan 1.4 release announcement and specification notes · `ash`, `ash-window`,
`gpu-allocator`, `rspirv` crate documentation · `imgui-rs-vulkan-renderer` documentation ·
sjb3d/caldera (Rust Vulkan render graph, automatic barriers, async loading, shader live reload) ·
Nikita Black, *Vulkan with Rust by example* series · shader-slang.org (Slang SPIR-V target) ·
Vulkan Documentation Project on descriptor indexing, dynamic rendering, and buffer device address.
