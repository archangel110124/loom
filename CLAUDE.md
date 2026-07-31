# Loom — Project Rules

An AI-native 3D game engine in Rust. An LLM agent is a first-class author: it composes scenes,
sculpts destructible voxel terrain, and writes scripts — through a tool API, against text files,
with schema validation and visual + simulation verification.

Full plan: `docs/design/LOOM-BUILD-BRIEF.md`. Traps: §7 of that document. Read it before Vulkan work.
Five companion docs sit alongside it — **start at `docs/design/README.md`**, which routes by
milestone and flags the superseded wgpu-era passages you must not build against.

Platform: Fedora 44, RTX 4090 (power-capped to 300W), Vulkan 1.3 target on a 1.4 loader.
No web target. Single developer + agent.

---

## Three properties everything follows from

1. Everything authored is **diffable text**, schema-validated on load.
2. The agent can **see and test its own work** — headless PNG render, deterministic headless sim.
3. The runtime is **deterministic**, so those assertions are trustworthy.

---

## Locked decisions — ask before changing any of these

| | |
| --- | --- |
| Graphics | Vulkan 1.3 via `ash`. No wgpu. **No portability/RHI abstraction.** |
| Render passes | **Dynamic rendering only.** |
| Binding | **Descriptor indexing + buffer device address.** |
| Shaders | Slang → SPIR-V via `build.rs`. |
| Memory | `gpu-allocator`. |
| Barriers | Owned by the render graph. |
| Physics | `rapier3d`; **voxel colliders** for terrain. |
| Scripting | `rhai`, sandboxed with hard op/depth limits. Agent never writes engine Rust. |
| Voxels | `i8` SDF, 32³ chunks, **op-list serialization** — never raw voxel arrays. |
| Timestep | Fixed. Render interpolates. Sim never sees variable `dt`. |
| Agent API | **CLI first, MCP second.** |
| Build order | **Headless offscreen render + PNG before the swapchain.** |
| Human oversight | Read-only viewer + `loom run --watch` at M5.5. Editing at M12. |
| Concurrency | Scene writes carry a **version token**. Stale writes are rejected, never merged. |

Changing one of these requires an ADR in `docs/decisions/` and human approval.

---

## Definition of green — all three, every time

```bash
cargo clippy --workspace -- -D warnings   # 1. clean
cargo xtask validate                      # 2. ZERO Vulkan validation messages
cargo test --workspace                    # 3. golden images + determinism hashes match
```

**`cargo check` passing is not done.** Rust's compiler catches nothing that matters in Vulkan —
missing barriers, wrong image layouts, use-after-free of in-flight resources, out-of-bounds bindless
indices all compile fine. The validation layers are the real compiler. They panic in debug builds by
design; do not downgrade that to a log line.

---

## Never do this

1. Never create `VkRenderPass` or `VkFramebuffer` — dynamic rendering only.
2. Never allocate per-draw descriptor sets — descriptor indexing + buffer device address.
3. Never call `vkAllocateMemory` — `gpu-allocator` only.
4. Never place a barrier outside the render graph.
5. **Never write `ash` calls from memory.** Read the vendored source first:
   `ls ~/.cargo/registry/src/*/ash-*/src/` or `cargo doc -p ash --open`. The API has churned across
   versions and recalled shapes are confidently wrong.
6. Never float a dependency version — pin exactly, add with `cargo add`.
7. Never use `HashMap`/`HashSet` iteration or `thread_rng` in simulation code (clippy enforces).
8. Never read the wall clock in simulation code.
9. Never let `build.rs` swallow a shader compile error.
10. Never put a trimesh collider on a dynamic rigid body.
11. Never serialize raw voxel arrays into a scene file.
12. Never introduce a trait with one implementation. Pre-authorized: `Mesher`, ECS system.
13. Never refactor code marked `// STABLE` without an ADR.
14. Never build a portability abstraction over Vulkan.
15. Never force-write a scene file against a stale version token, and never auto-merge two divergent
    scene states — reject and reload. Silently destroying the human's edits is the worst bug class in
    this project.
16. Never give the editor its own undo stack. It issues the same `SceneOp` transactions the agent
    does, through the same code path — a twelve-op agent transaction must undo in one Ctrl+Z.

---

## Style

Modern Vulkan is **short**. The reference is Sascha Willems' `HowToVulkan2026`: a lit, textured,
multi-object scene in a few hundred lines of one file, using dynamic rendering + descriptor indexing
+ buffer device address + Slang. Most Vulkan material in training data predates 1.3 and is 5–10×
longer. **If generated code is much longer than that per feature, it's the obsolete style — stop and
reconsider.**

Name every Vulkan resource via `VK_EXT_debug_utils`. It makes validation messages and Nsight captures
readable, which is what makes them usable as agent feedback.

Small commits, one concern each, each producing something that runs. A 2,000-line Vulkan-init commit
that doesn't work is nearly undebuggable.

---

## Dependency rules (CI-enforced)

- `loom_reflect` and `loom_scene` depend on nothing else in the workspace.
- `loom_agent` is depended on by nothing.
- **Nothing outside `loom_render*` imports `ash`.**

Violating these makes the project unbuildable by month four and destroys `cargo check` times, which
is the agent's iteration loop.

---

## Working alongside a human

From M5.5 the human may have `loom run --watch` open while you work — the viewer reloads on file
change, so your edits appear live. Two consequences:

- **Label every transaction usefully.** The label shows up in their log panel and in the git history.
  "Block out office: 14 nodes" beats "update scene".
- **Prefer `--dry-run` first** on anything large. It prints the diff without writing, which is how
  the human reviews before it lands.
- **Expect version-token rejections** and handle them by re-reading and re-applying, not by forcing
  the write.

## Current milestone

> **M0–M12 complete, editor included.** See §6 of the build brief.
>
> All milestone exit criteria met, and M12's body as well: `loom run --edit` has a scene tree, an
> inspector generated from the type registry, a transaction log, and click-to-select.
> Still not built from M12's list: gizmo handles, multi-selection, the asset browser, and the
> knowledge-graph view (which depends on ADR 0003, still undecided). `loom` has: validate, describe, render (--sim), sim (--assert),
> run (--edit), scene (--tx), place (--op), measure, terrain, explode. `loom-mcp` wraps them.
>
> Post-M12 work, in the brief's own order of priority: shadows / SDFGI / the post stack (all
> deliberately deferred past M12 by §7.16), Dual Contouring behind the existing `Mesher` trait for
> sharp-cornered structures, LOD octree for the open world, and archetype ECS storage when
> profiling demands it (ADR 0003 names the trigger).
>
> The knowledge graph is still deferred — ADR 0003, option 1, awaiting a decision.

---

## When stuck

1. Read the real source (vendored crate, not memory).
2. Turn on more validation — sync validation, GPU-assisted validation.
3. Render it to a PNG. Use the `ids` and `collision` debug modes.
4. Capture in Nsight/RenderDoc.
5. Bisect — this is why commits are small.
6. Write the failing test first, then fix.
7. Ask the human, especially for anything locked above.
