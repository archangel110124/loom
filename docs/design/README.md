# Design docs — read this first

Six documents, ~2,600 lines. **You do not need to read them all.** Three of them contain
superseded backend material that will make you build the wrong thing if you read it cold.

Precedence and the full list of corrections: [ADR 0002](../decisions/0002-companion-doc-precedence.md).

## Before you read anything else

- **There is no wgpu in this project.** The backend is Vulkan 1.3 via `ash`. Three docs below still
  discuss wgpu at length because they predate the decision — `loom-vulkan-backend.md` supersedes
  every one of those passages.
- **There is no web/WASM target, permanently.** The design doc's Phase 6 WASM deliverable is void.
  Do not hedge any decision toward it.
- **Milestones are M0–M12** (build brief §6). The design doc's "Phase 0–6" is an older numbering for
  the same project — mapping in ADR 0002. Use M-numbers in commits and conversation.
- **Every `= "*"` in a code sample is illustrative.** Pin exactly, add with `cargo add`.

## The documents

| Doc | What it is | Status |
| --- | --- | --- |
| **`LOOM-BUILD-BRIEF.md`** | The operational plan: locked decisions (§2), milestones (§6), traps (§7). | **Authoritative.** §2 decides what is locked. |
| **`loom-vulkan-backend.md`** | Vulkan 1.3 / `ash`. Bindless, multi-queue, pipeline cache, validation. | Current. Supersedes all wgpu claims elsewhere. |
| **`loom-voxel-system.md`** | SDF chunks, Surface Nets → Dual Contouring, rapier voxel colliders, op lists. | Current, except §3.1 and §7 wgpu mentions. |
| **`loom-terrain-generation.md`** | Recipe → heightmap → SDF. Erosion, art layers, `terrain_analyze`. | Current, except §9 wgpu mention. |
| **`ai-native-engine-design.md`** | The origin document. Reflection, scene format, prefabs, MCP, verification. | Architecture current; Phase plan and WASM stale. |
| **`loom-graphics-physics-frontier.md`** | Meshlets, visbuffer, occlusion culling, solver landscape. | Techniques current; §A.2, §B.4, §B.5 are wgpu-era. |

## Routing by milestone

- **M1 — reflection + scene format** → design doc §2.1 (derive macro), §2.3 (`.loom` format),
  §2.4 (prefab overrides), §2.9 (validation errors). Write `docs/format/` before the parser.
- **M2 — Vulkan headless** → Vulkan doc entire, brief §7.1–7.4. Read the vendored `ash` source
  before writing a single call.
- **M3 — ECS + fixed timestep** → design doc §2.2, §2.5; graphics doc §A.3 (determinism), §A.4.
- **M4 — render graph** → Vulkan doc §4, §6; read `caldera` first.
- **M5 — assets + swapchain** → design doc §2.6; Vulkan doc §9 (pipeline cache).
- **M5.5 — viewer + watch** → design doc §2.1 (registry drives the inspector); brief §7.17
  (version tokens — plumb them here, five lines now, an architecture change later).
- **M7 — physics** → graphics doc §C.2 (solver), **§C.5 (the sanity-check table brief §7.15 cites)**,
  §C.4, §C.6.
- **M8 — scripting** → design doc §2.12; brief §7.8 (the sandbox needs adversarial tests).
- **M9 — the agent loop, the gate** → design doc §2.8 (semantic placement — the most important 3D
  decision), §2.10 (verification channels), Part 4 risks. Brief §7.10, §7.16.
  Note: `graph_query` is deferred — [ADR 0003](../decisions/0003-knowledge-graph-deferred.md).
- **M10 — voxels** → voxel doc entire. §5.2 (op lists, never voxel arrays), §5.3 (redistancing),
  brief §7.9 (chunk seams — write the test before the mesher).
- **M11 — terrain** → terrain doc entire. §4.4 (bake and hash, never re-simulate), §7 (analysis).
- **M12 — editing** → design doc §2.8 transactions; brief §7.17 and never-do #16 (one undo stack,
  shared with the agent).

## Environment prerequisites — checked 2026-07-30

M0/M1 need nothing beyond the pinned toolchain. **M2 is blocked on three things this box does not
have yet**, all cheap to install but painful to discover in week five:

| Need | Status | Source |
| --- | --- | --- |
| `VK_LAYER_KHRONOS_validation` | **MISSING** — `/usr/share/vulkan/explicit_layer.d/` is empty | `dnf install vulkan-validation-layers` (1.4.341.0-2.fc44, exactly matches the installed loader) |
| `slangc` (Slang → SPIR-V) | **MISSING** | Not packaged for Fedora. GitHub releases (`shader-slang/slang`) or the Vulkan SDK. |
| `spirv-val` (brief §7.7) | **MISSING** | `dnf install spirv-tools` (2026.1-1.fc44) |

**Trap:** Fedora's `slang` package — which *is* installed here — is **S-Lang, the terminal extension
library**. Completely unrelated to the Slang shader language. `rpm -q slang` succeeding proves
nothing; check for the `slangc` binary.

Present and correct: mold, clang, `vulkan-loader` 1.4.341, RTX 4090 on driver 610.43.03 at the
deliberate 300W cap. Loader is 1.4, target is Vulkan 1.3 — as intended (Vulkan doc §1).

Without the validation layer, **definition-of-green check #2 cannot run at all** — `cargo xtask
validate` would pass vacuously, which is worse than failing. Install before writing M2 code.

## Where the docs disagree with each other

Recorded in ADR 0002 rather than fixed in place — the reasoning record is worth more intact than
retroactively tidied. Known conflicts: wgpu vs Vulkan, WASM alive vs dead, Phase-vs-M numbering,
crate splits (`loom_voxel_physics`, `loom_terrain_erode`, `loom_terrain_analyze`, `loom_graph` all
appear in per-doc crate lists but not in brief §3).

Per brief §7.13: **§2 of the build brief is the authority on what is locked. Anything not in that
table is provisional. When a doc and reality conflict, reality wins and the doc gets an ADR.**
