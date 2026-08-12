# Loom — Project Rules

An AI-native 3D game engine in Rust. An LLM agent is a first-class author: it composes scenes,
sculpts destructible voxel terrain, and writes scripts — through a tool API, against text files,
with schema validation and visual + simulation verification.

Full plan: `docs/design/LOOM-BUILD-BRIEF.md`. Traps: §7 of that document. Read it before Vulkan work.
**Sequencing after M12: `docs/design/LOOM-IMPLEMENTATION-ORDER.md`** — it is the *when*, and it
supersedes the build orders inside the companion docs wherever they conflict. Nine companion docs
sit alongside them — **start at `docs/design/README.md`**, which routes by phase and flags the
superseded wgpu-era passages you must not build against.

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

## Definition of green — all four, every time

```bash
cargo clippy --workspace -- -D warnings   # 1. clean
cargo xtask validate                      # 2. ZERO Vulkan validation messages
cargo test --workspace                    # 3. unit tests + determinism hashes match
cargo xtask image                         # 4. renders match their reference PNGs
```

`scripts/green.sh` runs all four. Check 4 is S1 of the implementation order and
is new: until it existed, "golden images" was aspirational and every render in
this project was verified by a human opening the PNG. It renders six scenes
chosen for coverage of *rendering paths* — mesh, bindless textures, voxels,
alpha particles, additive particles, an authored-dark environment — and
compares them to `tests/references/` with a calibrated tolerance.

**`cargo xtask image --bless` accepts new references.** Do it deliberately and
read the diff: `tests/references/MANIFEST.txt` records each reference's hash,
so a re-blessing is a readable line in a commit rather than an opaque binary.

**`cargo xtask flythrough` is not a gate and is the more important half.** It
dumps sixteen orbiting frames per scene, advancing the simulation between them.
Shimmer, popping, unison sway and a wind direction that snaps instead of
turning are all invisible in a still, and they are the dominant failure mode of
every system queued in Phases 1–6.

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

## Current phase

> **Phase 0 complete; P1 (wind) done. P2 (grass) is next.** See
> `docs/design/LOOM-IMPLEMENTATION-ORDER.md`, which is the sequencing document for everything
> after M12 and **supersedes the build orders inside the companion docs wherever they conflict**.
>
> Phase 0 is S1 golden-image regression harness, S2 Rust→Slang codegen + CPU/GPU agreement test,
> S3 voxel-SDF exposure/shelter query, S4 prefab system. None of it is visible and all of it is
> load-bearing: it is 15–20% of the total and it prevents building water, rain, wind and grass on
> a foundation with no pixel diff, three divergent field implementations, three occlusion queries
> and scatter written twice.
>
> **S1 delivered** the fourth green check and `cargo xtask flythrough` — ADR 0005 records the
> tolerance and the measurement behind it. Exit criterion run end to end: a one-line shader change
> fails the gate, `--bless` accepts it, reverting restores the references.
>
> **S2 delivered** `loom_field` — a field is one expression tree, `Expr::eval` walks it on the CPU
> and `build.rs` emits `assets/shaders/generated/fields.slang` from it, so the two cannot implement
> different formulas. `cargo test` proves they agree numerically: worst absolute difference 4.5e-5
> over 512 samples against a 1e-3 threshold. ADR 0006 records the mechanism, the three traps pinned
> along the way, and the rule that **noise, when a field needs it, is implemented inside
> `loom_field` rather than taken from a crate** — a crate bump must never be able to change the sim
> hash. Nothing uses noise yet.
>
> **Authoring a new field means adding it to `loom_field::all()`.** Never hand-write a field in
> Slang, and never edit the generated file — that changes the GPU alone, which is the divergence
> the generator exists to make impossible.
>
> **S3 delivered** `loom_voxel::exposure` — one CPU march of the SDF answering "how open is this
> direction" as a fraction, for rain, wind and gameplay alike. ADR 0007 records why it is an SDF
> march rather than a ray query, and the two constants the tests chose rather than the design
> (half-voxel steps, an asymmetric occupancy ramp — a one-voxel roof leaks without either).
> **Rain and wind take sheltering from here; neither grows its own.** Audio's `openness` is
> deliberately *not* unified with it: audio casts against the collision world, this marches only
> the voxel volume, and swapping it would read every non-voxel scene as wide open.
>
> **S4 delivered** prefabs in full — ADR 0008. `[[prefab]]`, `prefab = "<alias>"`,
> `[node.overrides]` and `extends` all resolve through `loom_scene::prefab`, and
> `loom prefab <unpack|revert-overrides|apply-overrides>` are the three §5 operations.
> `assets/test/prefab_room.loom` is a worked example. A library is keyed by **`id`, never the
> alias** — aliases are file-local, and two files may use one word for different prefabs.
>
> **Setting a field on a prefab instance writes an override, not a component**, so the inspector
> and `loom scene --tx` need no idea which kind of node they hold. `apply-overrides` writes two
> files and is therefore **two undo steps**; everything else is one.
>
> **The load path is a correctness requirement, not tidiness, and it is the likeliest way to
> regress S4.** The parser used to refuse `prefab` precisely because a key it does not understand
> is a key it *ignores* — the instance arrived with no components, drew nothing, and validated
> clean. Any new command that reads a scene must go through `prefab_load::for_reading`.
>
> **P1 delivered** the wind field — ADR 0009. A `Wind` component authors it, `loom_field::wind`
> is the tree, the Slang is generated, and `loom_field::wind::Wind` samples it with S3 sheltering.
> `loom sim --assert "wind@x,y,z.speed >= v"` checks it from the CLI.
>
> **The only thing wind visibly drives is particles**, via `wind_response` on `ParticleEmitter` —
> a coupling toward the air's velocity, **zero by default** so every scene authored before wind
> renders byte-identically. `assets/test/windy.loom` is the demo and is in both gates. Grass (P2)
> is what makes the field carry a landscape.
>
> **Two rules that outlive P1.** `loom_field::noise` is **frozen ABI** — an integer lattice hash,
> written in Rust and Slang side by side and compared *exactly* by the agreement test, because a
> `frac`-based hash amplifies a last-bit difference into a different number. And the 10k-tick wind
> hash is pinned in a test that `cargo xtask validate` also runs in **release**; changing a field
> means re-pinning it in the same commit, deliberately.
>
> **Do not skip ahead.** The order after it is P1 wind → P2 grass → P3 water → P4 rain →
> P5 scatter → P6 mesh vegetation + culling, with P7 editor slottable in parallel. Grass sits at
> P2 rather than with the other vegetation because it is the forcing function on the no-TAA
> decision, and that answer changes the plan for water and rain if it comes out badly.
>
> The grass research pass has landed and sharpens that: geometry blades plus MSAA gets *most* of
> the way, and residual specular and edge shimmer in motion is **genuinely unsolved without
> temporal accumulation**. Budget for the possibility that a single non-temporal full-screen AA
> pass (CMAA2 / SMAA 1x) has to be added, and record it as an ADR if it is. It also confirms S1
> from the outside: motion artifacts are the exact failure class grass is worst at and the exact
> one a still PNG cannot see.
>
> The four post-M12 items previously listed here (shadows/SDFGI/post stack, Dual Contouring, LOD
> octree, archetype ECS) are all in Phase 8 — deferred, each with a stated reason.

### What M0–M12 already delivered

> **M0–M12 complete, editor included.** See §6 of the build brief.
>
> All milestone exit criteria met, and M12's body as well. `loom run --edit` has a hierarchy with
> drag-to-reparent and multi-selection, an inspector generated from the type registry (including
> Add Component), transform gizmos with Move/Rotate/Scale, an asset panel, a console, a transaction
> log, click-to-select, and Play/Pause/Step/Stop.
>
> **The window is a live view of the file.** It polls the scene four times a second, so a
> transaction the agent applies through the CLI appears in the viewport while the human watches.
> A divergence between unsaved edits and a changed file raises a banner offering both versions and
> merges neither (never-do #15).
>
> Gestures coalesce: a gizmo drag or a scrubbed slider is **one** undo step, not one per frame.
>
> Still not built from M12's list: the knowledge-graph view, which depends on ADR 0003 — still
> undecided. `loom` has: validate, describe, render (--sim), sim (--assert),
> run (--edit), scene (--tx), place (--op), measure, terrain, explode. `loom-mcp` wraps them.
>
> Built after M12 and not in the brief: raycasting and blast force with cover, a character
> controller whose movement model is a script, explosions (one-shot emitters, additive fire),
> a scene-authored `Camera`, first-person play with pointer capture, a deterministic event log
> with damage on it, `GameRules` with win/lose and assertions, a `Hud`, navigation probed from the
> collision world with A*, enemies, ray-traced acoustics with playback, and `Environment` so a
> scene can set its own sun, sky and fog. `assets/games/proving_ground.loom` is a whole game.
>
> The knowledge graph is still deferred — ADR 0003, option 1, awaiting a decision. The
> implementation order asks for it to be accepted or rejected rather than left open; nothing in
> Phases 0–8 depends on it.

---

## When stuck

1. Read the real source (vendored crate, not memory).
2. Turn on more validation — sync validation, GPU-assisted validation.
3. Render it to a PNG. Use the `ids` and `collision` debug modes.
4. Capture in Nsight/RenderDoc.
5. Bisect — this is why commits are small.
6. Write the failing test first, then fix.
7. Ask the human, especially for anything locked above.
