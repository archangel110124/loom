# Loom — Build Brief

**A front-to-back implementation document for Claude Code.**
Companion docs: `ai-native-engine-design.md`, `loom-graphics-physics-frontier.md`,
`loom-voxel-system.md`, `loom-terrain-generation.md`, `loom-vulkan-backend.md`.

This document is the operational one. The others are the reasoning; this is the plan, the
guardrails, and — at length, because it's what you asked for — the traps.

---

## 0. How to use this

Read §2 (locked decisions) and §7 (traps) before writing any code. §6 is the milestone plan; work
one milestone at a time and do not start the next until the current one's exit criterion passes.
`CLAUDE.md` in the repo root is the condensed always-loaded version.

**The single most important structural instruction:** every milestone ends with something that
*runs* and produces an artifact a human or an agent can inspect. No milestone ends at "it compiles."

---

## 1. What is being built

A 3D game engine in Rust where an AI agent is a first-class author. The agent composes scenes,
places prefabs, sculpts destructible voxel terrain, and writes gameplay scripts — through a tool API,
against text files, with schema validation and visual + simulation verification.

Three properties define the architecture. Everything else follows from them:

1. **Everything authored is diffable text**, schema-validated on load.
2. **The agent can see and test its own work** — headless render to PNG, deterministic headless
   simulation with assertions.
3. **The runtime is deterministic**, so those assertions are trustworthy.

Target platform: Fedora 44, NVIDIA RTX 4090 (power-capped to 300W), Vulkan 1.3 target on a 1.4
loader. No web target. Single developer plus agent.
<!-- Corrected 2026-07-30: originally read "Arch Linux". Reality wins (§7.13); CLAUDE.md was
     already corrected at M0. -->

**New here? Read [`README.md`](README.md) first** — it maps the six design docs and flags which
passages are superseded.

---

## 2. Locked decisions — do not relitigate

If a session proposes changing any of these, stop and ask the human. These were each researched;
re-deciding them mid-build costs weeks.

| Area | Decision |
| --- | --- |
| Language | Rust, **2024 edition** (was 2021 — see ADR 0001), pinned toolchain |
| Graphics API | **Vulkan 1.3 via `ash`.** No wgpu. No portability abstraction layer. |
| Render pass model | **Dynamic rendering only.** Never create `VkRenderPass` or `VkFramebuffer`. |
| Binding model | **Descriptor indexing + buffer device address.** No per-draw descriptor sets. |
| Shaders | **Slang → SPIR-V**, compiled by `build.rs`. Never hand-write SPIR-V or GLSL. |
| Memory | `gpu-allocator`. Never call `vkAllocateMemory` directly. |
| Barriers | Owned by the render graph. Never hand-place a barrier outside it. |
| Physics | `rapier3d`, with **voxel colliders** for terrain. Never a trimesh collider on a dynamic body. |
| Scripting | `rhai`, sandboxed with hard op/depth limits. The agent never writes engine Rust. |
| Scene format | TOML-flavored text, defaults omitted, nodes addressed by path, prefab override deltas |
| Voxel representation | Quantized `i8` SDF, 32³ chunks, uniform-chunk collapse, op-list serialization |
| Voxel meshing | Surface Nets first (`fast-surface-nets`), Dual Contouring later behind a `Mesher` trait |
| Terrain | Recipe → baked heightmap artifact → SDF. Erosion baked, never re-simulated at load. |
| Timestep | Fixed, always. Render interpolates. Simulation never sees variable `dt`. |
| Agent interface | CLI first, MCP server wrapping the CLI second |
| Ordering | **Headless offscreen rendering before the swapchain** (§7.1) |
| Human oversight | **Read-only viewer + `--watch` at M5.5**, long before the editable editor at M12 |
| Concurrency | Every scene file write carries a **version token**; stale writes are rejected, never merged silently (§7.17) |

**Provisional — revisit only at the named gate:**

| Thing | Revisit at |
| --- | --- |
| Box3D instead of rapier | M10, if determinism or binary size binds |
| Mesh shaders (`VK_EXT_mesh_shader`) | After meshlet compute path works |
| Ray tracing (`rayQuery` for static-only shadows) | After SDFGI is evaluated |
| `VK_EXT_descriptor_buffer` | If descriptor pool management becomes annoying |
| Agent authoring new component types | After watching the agent work for a while |

---

## 3. Repo layout

```
loom/
├── CLAUDE.md                  # always-loaded rules (§ separate file)
├── docs/
│   ├── decisions/             # ADRs — one file per decision, append-only
│   ├── format/                # THE SCENE + RECIPE FORMAT SPEC. Write it before the parser.
│   └── design/                # the five companion docs
├── crates/
│   ├── loom_reflect/          # derive macros + runtime type registry     [M1]
│   ├── loom_scene/            # .loom parse/serialize, prefabs, overrides [M1]
│   ├── loom_ecs/              # archetype storage, queries, scheduler     [M3]
│   ├── loom_render/           # ash/Vulkan — ONLY crate that imports ash  [M2]
│   ├── loom_render_graph/     # passes, resource lifetimes, barriers      [M4]
│   ├── loom_asset/            # import, .meta, manifest, content hashing  [M5]
│   ├── loom_input/            # winit + gilrs → action maps               [M6]
│   ├── loom_physics/          # rapier3d integration                      [M7]
│   ├── loom_script/           # rhai host, sandbox, hot reload            [M8]
│   ├── loom_agent/            # MCP server over the CLI                   [M9]
│   ├── loom_voxel/            # SDF chunks, CSG ops, redistancing         [M10]
│   ├── loom_voxel_mesh/       # Mesher trait; surface_nets, dual_contour  [M10]
│   ├── loom_terrain/          # recipe eval, bake, erosion                [M11]
│   ├── loom_editor/           # read-only viewer + watch mode            [M5.5]
│                             # editing, gizmos, undo                     [M12]
│   └── loom_cli/              # loom new|run|validate|render|scene|voxel
└── assets/
    ├── shaders/               # .slang sources
    └── test/                  # golden images, fixture scenes
```

**Dependency rule, enforced by CI:** `loom_reflect` and `loom_scene` depend on nothing else in the
workspace. `loom_agent` is depended on by nothing. Nothing outside `loom_render*` imports `ash`.
Violating these is how the project becomes unbuildable in month four.

**Write `docs/format/` before the parser.** Godot's own contributor docs admit their sub-resource
format documentation is largely absent and can only be discovered by reading engine source. A format
without a spec is not a contract, and an agent authoring against an unspecified format will invent
variations.

---

## 4. Toolchain and the iteration loop

```bash
# Pinned. Do not float versions.
rust-toolchain.toml → channel = "1.8x.x"   # pin the actual current stable at init

# Fast linking — non-negotiable for iteration speed
# .cargo/config.toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]

# Commands Claude Code should use
cargo check --workspace          # the fast loop — seconds
cargo clippy --workspace -- -D warnings
cargo test --workspace           # includes golden-image + determinism tests
cargo xtask validate             # runs the engine headless with validation layers
```

**Definition of "green" — all three, every time:**

1. `cargo clippy -D warnings` clean
2. **Zero Vulkan validation messages** (including synchronization validation)
3. Golden-image tests match

`cargo check` passing is not green. See §7.3.

---

## 5. The three verification channels

Build these early; they are how the agent knows anything.

| Channel | What it catches | Available from |
| --- | --- | --- |
| **Schema validation** | Malformed scenes, out-of-range fields, bad references | M1 |
| **Headless render → PNG** | Wrong placement, black screens, intersecting geometry, broken materials | **M2** |
| **Deterministic headless sim + assertions** | Wrong behavior, leaks, scripts that break on frame 900 | M3 |
| **Human observation — viewer + `--watch` + git diff** | Everything the other three miss: bad taste, wrong intent, plausible-but-pointless work | **M5.5** |

The fourth channel is the one that catches the failures automation structurally cannot. A scene can
be schema-valid, render correctly, and pass every assertion while being a bad level. Text scenes mean
every agent edit is also a readable git diff — better change visibility than Unity or Unreal can
offer, where a scene edit is an opaque blob.

Channel 2 lands at M2, before the swapchain. That inversion is deliberate and explained in §7.1.

---

## 6. Milestones

Each has a **runnable** exit criterion. Commit at each. Do not proceed on a failing gate.

### M0 — Skeleton (2–3 days)
Workspace, pinned toolchain, mold, clippy config, CI running the three green checks, `docs/format/`
stub, ADR template, `CLAUDE.md`.
**Exit:** `cargo check --workspace` and CI both pass on an empty workspace.

### M1 — Reflection + scene format (2 weeks)
`#[derive(Reflect)]` generating serde impls, a JSON Schema entry, a type-registry entry, and doc
strings. `.loom` parse/serialize round-trip. 6 components: `Name`, `Transform`, `MeshRenderer`,
`BoxCollider`, `Light`, `Script`. `loom validate`, `loom describe <Type>`.
**Exit:** a hand-written `.loom` file round-trips byte-identically; an out-of-range field produces a
structured error naming the field, the value, and the constraint.

### M2 — Vulkan headless (5 weeks) — *the hard one*
Instance, device, queue families, validation layers. `gpu-allocator`. Slang→SPIR-V in `build.rs`.
Dynamic rendering. **Offscreen render target + PNG writeout.** Pipeline cache. Descriptor indexing
array + buffer device address plumbing.
**Exit:** `loom render triangle.loom --out t.png` writes a PNG containing a triangle, with zero
validation messages. **No window yet.**

### M3 — ECS + fixed timestep (2 weeks)
Archetype storage, queries, transform propagation (`Local` → `Global`), fixed-timestep loop,
`loom sim <scene> --ticks N` printing a deterministic state hash.
**Exit:** the same scene simulated twice produces identical state hashes. Two different machines
would too (can't test, but no `HashMap` iteration or `thread_rng` in sim code — see §7.5).

### M4 — Render graph (3 weeks)
Pass declaration, transient resource allocation, **automatic barrier and layout-transition
placement**, queue ownership transfers. Read `caldera` first.
**Exit:** a 3-pass graph (depth prepass → forward → post) renders correctly headless with sync
validation enabled and silent.

### M5 — Assets + swapchain (3 weeks)
`.meta` sidecars with UUIDs + content hashes, import cache, `manifest.bin`. glTF static meshes.
Procedural primitive library (box, cylinder, plane, sphere, capsule). *Then* winit + swapchain +
present.
**Exit:** `loom run office.loom` opens a window showing a glTF mesh and a primitive; `loom render`
still produces the same image headless.

### M5.5 — Read-only viewer + watch mode (1.5 weeks) — *cheap, do not skip*

The human's window into the agent's work, ~13 months before the editable editor. It is this cheap
only because `loom_reflect` already generates inspector widgets from the type registry (§2.1 of the
design doc) — that was always the point of the registry.

- egui + `egui_dock`: scene tree panel, component inspector, fly camera
- **Read-only.** No gizmos, no editing, no undo. Those are M12.
- **`loom run --watch`** — file watcher reloads the scene on disk change
- Transaction log panel: labels, timestamps, node counts, and the diff for each
- Version-token plumbing in the scene loader, even though nothing writes yet (§7.17)

The payoff: leave `loom run --watch office.loom` open on one monitor while Claude Code works in a
terminal on the other, and **the world updates live as the agent edits it.** Near-free, because the
file watcher is already required for script and data hot reload.

**Exit:** with the viewer open, an agent-issued `loom scene` command visibly changes the world within
a second, and the transaction appears in the log panel with its label and diff.

### M6 — Input + camera (1.5 weeks)
Action/context/modifier/trigger model over winit + gilrs. Fly camera.
**Exit:** rebindable actions loaded from TOML; camera flies.

### M7 — Physics (2 weeks)
`rapier3d`, fixed-step, colliders from components, character controller.
**Exit:** a capsule walks on a mesh floor; determinism hash from M3 still stable with physics active.

### M8 — Scripting (2.5 weeks)
`rhai` host, API registration generated from the type registry, hard op/depth limits, file watcher
hot reload, structured script errors.
**Exit:** a `.rhai` file rotates a cube; editing it takes effect without restart; a script attempting
file I/O **fails a test that asserts it fails** (§7.8).

### M9 — Agent loop (4 weeks) — *the gate that matters*
`loom_cli` subcommands for every scene mutation. Transactions with one undo step and `--dry-run`.
`loom render` multi-angle. `loom sim --assert`. **Then** the MCP server wrapping the CLI.
`scene_place` semantic ops (`place_on`, `align_to`, `grid_on`, `face_toward`). `scene_measure`
(bounds, raycast, overlaps). Claude Code validation hook.
**Exit:** *"Block out a computer lab: 6 desks in two rows, a monitor on each, a teacher desk facing
them, overhead lights. Then make the monitors turn on when the player walks in."* — produced,
self-corrected from its own render, behavior proven by a headless assertion. **If this fails, stop
and fix the loop. Everything after is worthless without it.**

### M10 — Voxels (6 weeks)
`i8` SDF chunks, uniform collapse, CSG op list, redistancing (Fast Sweeping), Surface Nets meshing
with neighbor-aware chunk borders, rapier voxel colliders, off-thread remesh + recollide with a
per-frame swap budget, debris pool with convex colliders and a hard cap.
**Exit:** carve a tunnel at a stable framerate with no seams, no tunneling, no frame hitches.

### M11 — Terrain (4 weeks)
Recipe parse; fBm/ridged/multifractal with domain warping and analytical derivatives; art layers
(spline carve, flatten, peak, escarpment, corridor); particle hydraulic + thermal erosion, baked and
content-hashed; `terrain_analyze` returning slope/flow/hillshade PNGs and buildability stats.
**Exit:** *"a mountain valley with a buildable plateau for a fort and a walkable path from the south"*
— agent authors it, reads its own slope map, adjusts, verifies the path.

### M12 — Editing (4.5 weeks — reduced; the viewer shell exists from M5.5)
Turn the M5.5 viewer editable: gizmos and manipulators, multi-selection, property editing through the
inspector, asset browser, the knowledge-graph view — and **the same transaction/undo system the agent
uses**, not a parallel one.

That shared transaction system is the requirement to hold the line on: if the agent performs twelve
operations and the human presses Ctrl+Z, it must undo as **one** step. Two undo stacks that disagree
about history is a bug class with no clean fix, so the editor issues the same `SceneOp` transactions
the MCP layer does, through the same code path.

Concurrent-edit handling (§7.17) becomes load-bearing here rather than theoretical.
**Exit:** the same edit made by hand and by agent produces an identical diff; a twelve-op agent
transaction undoes in one Ctrl+Z; an edit made while the agent writes the same file is rejected with a
reload prompt rather than silently clobbering either side.

---

## 7. THE TRAPS

The part you asked to focus on. Ordered by how much time each will cost if missed.

### 7.1 Claude Code cannot see a window — invert the build order

**The trap:** every Vulkan tutorial builds instance → device → swapchain → present → triangle. If you
follow that, the first thing an agent produces is a window it cannot look at. It will report success
on a black screen, repeatedly, for days, because "it runs without errors" is the only signal
available.

**The fix:** build **offscreen rendering and PNG writeout before the swapchain.** Render to a
`VkImage`, copy to a host-visible buffer, write a PNG. That's ~150 extra lines at M2 and it means the
agent has eyes from week five instead of week ten. The swapchain moves to M5, where it's a
convenience for the human rather than the only output path.

This is the highest-leverage single decision in this document. It also means `render_preview` — the
agent-facing tool from the design doc — is not a feature bolted on later; it's the primary render
path from the start, which guarantees it never diverges from the real one.

### 7.2 `ash` API churn will produce confident, wrong code

**The trap:** `ash` tracks Vulkan header releases and has broken its API repeatedly — extension
module paths moved (`ash::extensions::khr::Swapchain` → `ash::khr::swapchain`), `Entry::new()` became
`Entry::load()`, builder patterns and lifetime handling changed. Training data contains several
mutually incompatible generations of `ash` code, all of which look plausible. Claude Code will mix
them, and the errors are confusing because they're type errors deep in FFI signatures.

**The fix, as a hard rule:** *never write `ash` calls from memory.* Before writing Vulkan code in a
session, read the actual vendored source or generated docs:

```bash
cargo doc --open -p ash          # or:
ls ~/.cargo/registry/src/*/ash-*/src/
```

Pin `ash` exactly (`=x.y.z`, not `^`). Same rule for `gpu-allocator`, `rapier3d`, `rhai`, and
`fast-surface-nets`. Add every dependency with `cargo add`, never by hand-editing `Cargo.toml`.

### 7.3 "It compiles" is a dangerous success signal in Vulkan

**The trap:** Rust's compiler catches nothing that matters here. Missing barriers, wrong image
layouts, destroying a resource still in flight, out-of-bounds bindless indices, unsynchronized
queue access — all compile fine, and many *appear* to work on one driver. Claude Code's default
feedback loop is `cargo check`, which is exactly blind to the entire class of bugs Vulkan introduces.

**The fix:** make the validation layers the real compiler.

- `VK_LAYER_KHRONOS_validation` on in every dev build, plus **synchronization validation** and **best
  practices** sub-layers explicitly enabled.
- Route the validation callback to `panic!` in debug builds. Not a log line — a panic. A warning that
  scrolls past is a warning that gets ignored for three weeks.
- `cargo xtask validate` runs the engine headless with validation and exits nonzero on any message.
  Wire it into CI and into the definition of green.
- Run GPU-assisted validation periodically (it's slow) for bindless bounds checking.

Also: name every resource with `VK_EXT_debug_utils`. It costs a string per object and turns
validation messages from hex-handle soup into `buffer "voxel_chunk_staging"`. That readability is
what makes validation output usable as *agent* feedback rather than human-only noise.

### 7.4 Vulkan training data is a decade of obsolete style

**The trap:** the overwhelming majority of Vulkan material predates 1.3. Claude Code will
reflexively write `VkRenderPass`, `VkFramebuffer`, per-frame descriptor set allocation, and manual
`VkAttachmentDescription` chains — hundreds of lines of ceremony that dynamic rendering deleted.
Modern Vulkan is a few hundred lines for a lit textured scene; the legacy style is a few thousand.

**The fix:** explicit prohibitions in `CLAUDE.md` (they're there). Specifically forbid
`VkRenderPass`, `VkFramebuffer`, per-draw descriptor sets, and `vkAllocateMemory`. When in doubt,
the reference is Sascha Willems' `HowToVulkan2026` — single file, dynamic rendering, descriptor
indexing, buffer device address, Slang. If generated code is much longer than that per feature,
it's the old style.

### 7.5 Determinism breaks silently and surfaces weeks later

**The trap:** determinism is a requirement with no compile-time enforcement and no immediate
symptom. `HashMap` iteration order, `rand::thread_rng()`, `f32` accumulation order across threads,
work-stealing schedulers, and `Instant::now()` in simulation code all produce code that works
perfectly and makes every headless assertion subtly flaky — six weeks later, when the agent starts
ignoring failing tests because they're "just flaky."

**The fix — make it mechanical, not vigilant:**

```toml
# clippy.toml
disallowed-types = [
  { path = "std::collections::HashMap", reason = "non-deterministic iteration — use IndexMap in sim crates" },
  { path = "std::collections::HashSet", reason = "same — use IndexSet" },
]
disallowed-methods = [
  { path = "rand::thread_rng", reason = "seed RNG from the scene, never thread-local" },
  { path = "std::time::Instant::now", reason = "simulation must not read the wall clock" },
]
```

Plus a determinism test from M3 onward: run any scene twice, assert identical state hashes. Run it
in CI. It's the only thing that catches this.

### 7.6 Compile times will eat the iteration loop if crate boundaries are wrong

**The trap:** the agent's loop is `cargo check`. If `loom_reflect` (a proc-macro crate everything
depends on) changes, everything rebuilds. If `loom_render` ends up depended on by `loom_scene`, a
shader change rebuilds the scene parser. Get this wrong and by month three every iteration is a
four-minute wait, which degrades agent effectiveness far more than it degrades a human's.

**The fix:** enforce the dependency rules from §3 in CI (`cargo-deny` or a simple xtask check).
Measure cold and warm `cargo check` at M2 and treat regressions as bugs with the same seriousness as
test failures. `mold`, `sccande`/`sccache`, and `opt-level = 1` for dev dependencies.

### 7.7 Shader errors escape the Rust type system

**The trap:** Slang compiles in `build.rs`. If the build script swallows `slangc` failures, or if
shader/Rust struct layouts silently diverge, `cargo check` passes and you get corrupted rendering
with no error anywhere.

**The fix:**
- `build.rs` fails the build on any `slangc` non-zero exit. Print the full compiler output.
- Run `spirv-val` on every output.
- **Generate the Rust structs from the Slang source, or the Slang from Rust — one direction, one
  source of truth.** Two hand-maintained struct definitions with `#[repr(C)]` on one side will
  diverge, and the symptom is garbage on screen with no diagnostic. This is worth the tooling.
- Assert `size_of` and field offsets in tests for every shared struct.

### 7.8 The scripting sandbox needs an adversarial test, not a review

**The trap:** the sandbox is "safe because we only registered safe functions." That's a claim about
absence, and absence isn't testable by reading the code. One careless `api.register_fn` on something
that touches the filesystem and the agent can write anywhere.

**The fix:** a test module that *attempts* escapes and asserts each fails — file open, process spawn,
network, infinite loop (asserts the op limit trips), deep recursion (asserts the depth limit trips),
huge allocation. Add a case every time you register a new API surface. Also: registration should be
generated from the type registry, not hand-written, so the surface is auditable in one place.

### 7.9 Voxel chunk boundaries — the guaranteed bug

**The trap:** surface extraction reads one voxel past the chunk boundary. Forget it and you get
cracks at every seam, on every edit. It's the most common bug in every voxel implementation, and
it's invisible in a single-chunk test.

**The fix:** write the test before the mesher. Two adjacent chunks, a sphere spanning the boundary,
assert the meshes are watertight across the seam (no unmatched edges). Same for the dirty-chunk
propagation: an edit must dirty the touched chunk *and its neighbors*.

Related traps in the same area, each worth a test:
- **`i8` SDF quantization scale.** Getting the factor wrong produces a surface that's subtly offset
  or has stair-stepping. Round-trip test: quantize → dequantize → assert within tolerance.
- **Op-list ordering.** Subtract-then-union ≠ union-then-subtract. The agent will assume
  commutativity. Document it and test a non-commutative pair.
- **Redistancing after CSG.** Skipping it degrades normals silently — the geometry looks right and
  the shading is wrong.

### 7.10 Building the agent interface with the agent is circular

**The trap:** Claude Code building the MCP server that Claude Code will use. Sessions get confused
about which side of the boundary they're on, and the MCP layer is awkward to test without an agent
driving it.

**The fix:** **CLI first, MCP second.** Every scene mutation is a `loom scene ...` subcommand with
structured JSON output, testable from `cargo test` and from a shell. The MCP server is then a thin
adapter over commands that already work. This also means a human can do everything the agent can,
which is the right property anyway.

### 7.11 Premature abstraction

**The trap:** given an engine, Claude Code will build trait hierarchies, generic resource managers,
and a `Renderer` trait with one implementation. On a Vulkan renderer, abstraction that isn't paying
for itself actively obstructs — you end up with an RHI, which is the exact thing §0 of the Vulkan doc
rejected.

**The fix:** a rule — **no trait until there are two implementations.** The only pre-authorized
traits are `Mesher` (Surface Nets and Dual Contouring, both planned) and the ECS system trait. Write
the concrete thing twice before generalizing.

### 7.12 Losing the thread across sessions

**The trap:** this is a year-long project across hundreds of sessions with no shared memory between
them. Decisions get re-litigated, working code gets refactored, and the same trap gets rediscovered
three times.

**The fix:**
- `CLAUDE.md` — short, dense, always loaded. Locked decisions and prohibitions.
- `docs/decisions/` — one ADR per decision, append-only, numbered. When a session wants to change a
  locked decision, it writes an ADR proposing it and asks the human.
- **Mark stable modules.** A `// STABLE: do not refactor without an ADR` header on code that works.
  Claude Code will otherwise improve things that were fine.
- Small commits, one milestone concern each. A 2,000-line Vulkan-init commit that doesn't work is
  nearly impossible to debug; ten 200-line commits each producing a visible artifact is tractable.

### 7.13 Trusting the design docs as spec

**The trap:** the five companion docs contain recommendations of varying confidence. Some are
research-backed and locked; some are my judgment and may be wrong. A session treating all of it as
specification will implement something the human didn't intend.

**The fix:** §2 of this document is the authority on what's locked. Anything not in that table is
provisional and open to question. When a doc and reality conflict, reality wins and the doc gets an
ADR noting the correction.

### 7.14 NVIDIA-only blindness

**The trap:** one GPU, one driver. Descriptor limits, queue family layouts, and barrier behavior all
differ on AMD's RADV and Intel. Code that works perfectly here may not run at all elsewhere, and
there is no way to find out.

**The fix:** accept it deliberately rather than accidentally. Don't query capabilities and then
hardcode the 4090's answers — query and branch even if only one branch is ever taken locally.
Synchronization validation catches a real fraction of what would break elsewhere. If distribution
ever matters, budget real time; if it never does, note that in an ADR so a future session doesn't
"fix" it.

### 7.15 Physics robustness is an authoring problem here

**The trap:** the agent will generate physically pathological scenes — 1000:1 mass ratios, hundred-
unit thin colliders, bodies spawned interpenetrating — with no idea anything is wrong, because
nothing in a text scene file looks unusual. The symptom is "the physics is broken," unattributable.

**The fix:** put physical sanity checks in the schema validator at M7, not later. Mass ratio > ~100:1
warns; collider dimensions outside ~0.01–100 units warn; interpenetration at spawn errors with the
overlap depth; dynamic body with no collider errors. Each is an afternoon and each prevents a class
of bug reports. The full table is in §C.5 of the graphics/physics doc.

### 7.16 Scope creep into rendering

**The trap:** rendering is the most seductive part of an engine and the least relevant to this
project's thesis. Shadows, PBR, GI, and post-processing will absorb unlimited time while the agent
loop — the actual point — stays unbuilt.

**The fix:** M9 is the gate. Nothing beyond a forward renderer with directional light ships before
it. Shadows, SDFGI, and the post stack are post-M12. If a session proposes "quick" lighting work
before M9, it's the trap.

### 7.17 Split-brain: the human and the agent editing at once

**The trap:** the human has a scene open in the editor with unsaved in-memory state; the agent writes
the same file from a terminal. Whoever saves last wins and the other's work vanishes with no error.
This is the single worst bug class in a human+agent workflow because it destroys work *silently* and
erodes trust in the whole setup — after it happens twice, the human stops letting the agent touch
anything.

**The fix — optimistic concurrency, designed in at M5.5 rather than retrofitted at M12:**

- Every scene file load returns a **version token** (content hash is fine).
- Every write passes the token it read. A write against a stale token is **rejected**, returning the
  current content and version so the caller can merge or reload.
- The editor handles rejection by prompting a reload, never by force-saving.
- The agent handles rejection by re-reading and re-applying its transaction — which is automatic and
  invisible, because its edits are `SceneOp`s against paths rather than whole-file writes.
- **Never auto-merge.** A silent merge of two divergent scene states produces something neither party
  intended and is worse than a rejection.

Plumb the token through the loader at M5.5 while nothing writes yet. It's five lines then and an
architectural change later.

**Related:** run the agent with the `destructive` scope **off by default**. Node deletion and asset
removal require explicit human sign-off; everything else flows freely. Recoverable mistakes should be
frictionless, unrecoverable ones should not be.

---

## 8. Never do this

A consolidated prohibition list; also in `CLAUDE.md`.

1. Never create `VkRenderPass` or `VkFramebuffer` — dynamic rendering only.
2. Never allocate per-draw descriptor sets — descriptor indexing + buffer device address.
3. Never call `vkAllocateMemory` — `gpu-allocator` only.
4. Never place a barrier outside the render graph.
5. Never write `ash` calls from memory — read the vendored source first.
6. Never float a dependency version. Pin exactly. Add with `cargo add`.
7. Never use `HashMap`/`HashSet` iteration or `thread_rng` in simulation code.
8. Never read the wall clock in simulation code.
9. Never treat `cargo check` passing as done — validation-clean and golden images too.
10. Never let `build.rs` swallow a shader compile error.
11. Never put a trimesh collider on a dynamic rigid body.
12. Never serialize raw voxel arrays into a scene file — op lists only.
13. Never introduce a trait with one implementation.
14. Never refactor code marked `// STABLE` without an ADR.
15. Never change a §2 locked decision without asking the human.
16. Never build a portability/RHI abstraction over Vulkan.

---

## 9. When stuck

In order:

1. **Read the actual source.** Vendored crate source in `~/.cargo/registry`, not recalled API shape.
2. **Turn on more validation.** Sync validation and GPU-assisted validation catch most Vulkan bugs
   outright, with a specific message.
3. **Render it.** A PNG answers most "is it working" questions in one command. Use the `ids` and
   `collision` debug modes.
4. **Capture it.** Nsight Graphics and RenderDoc work natively; named objects (§7.3) make captures
   readable.
5. **Bisect the commit.** Small commits (§7.12) make this cheap. This is why they're small.
6. **Write the failing test first**, then fix. Especially for the voxel and determinism classes,
   where the bug will otherwise recur.
7. **Ask the human.** Particularly for anything touching §2. Guessing at a locked decision costs
   more than a question.
