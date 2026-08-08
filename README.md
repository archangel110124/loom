# Loom

A 3D game engine in Rust where an LLM agent is a first-class author, not a code-completion
bolt-on. The agent composes scenes, sculpts destructible voxel terrain, and writes gameplay
scripts through a CLI tool API — against diffable text files, schema-validated on load, and
verified by two channels it can read on its own: a headless render to PNG and a deterministic
headless simulation with assertions.

The bet is that the constraint which makes an engine agent-authorable is the same constraint
that makes it reviewable by a human: if every authored artifact is text and every change is a
labelled transaction, then an agent's work is a git diff and a hash, not an opaque `.uasset`.

`assets/games/proving_ground.loom` is the argument in one file — a playable FPS arena (level,
lighting, weapon, enemy behaviour, win condition) with no build step and no engine code, whose
win/lose condition is checkable headlessly:

```bash
loom sim assets/games/proving_ground.loom --ticks 3600 \
    --assert "status == lost" --assert "events.damage >= 1"
```

## The three properties everything follows from

1. Everything authored is **diffable text**, schema-validated on load.
2. The agent can **see and test its own work** — headless PNG render, deterministic headless sim.
3. The runtime is **deterministic**, so those assertions are trustworthy.

Determinism here is a verification concern, not a networking one. A flaky assertion trains an
agent to ignore failures, which is worse than having no assertions. `clippy.toml` enforces it
mechanically: `HashMap`/`HashSet` and `rand::thread_rng` and `Instant::now` are `disallowed-types`
/ `disallowed-methods` workspace-wide.

## Architecture

16 workspace members — 15 library/binary crates plus `xtask`. Roughly 33,500 lines of Rust across
57 files (comments and tests included), 344 external crates in the lockfile.

| Crate | What it does |
| --- | --- |
| `loom_reflect` | Type registry + schema-driven validation. `schemars` `JsonSchema` **is** the registry entry (ADR 0004). Depends on nothing in-workspace. |
| `loom_scene` | `.loom` (TOML) parse/serialize, `SceneOp` transactions, version tokens, semantic placement. Depends only on `loom_reflect`. |
| `loom_ecs` | Entity storage, transform propagation, fixed-timestep loop, state hashing. |
| `loom_render` | Vulkan 1.3 via `ash`. The **only** crate permitted to import `ash`. |
| `loom_render_graph` | Pass declarations, resource lifetimes, automatic barrier placement. Owns every barrier. |
| `loom_asset` | glTF + PNG import, `.meta` identity, blake3 content hashing, primitive generation. |
| `loom_input` | winit events → action maps from TOML. |
| `loom_physics` | `rapier3d` with `enhanced-determinism`, voxel colliders, raycasts, character controller. |
| `loom_script` | `rhai` host, sandboxed with hard operation/depth/size limits. |
| `loom_voxel` | `i8` SDF chunks, CSG op lists, Surface Nets meshing behind a `Mesher` trait. |
| `loom_terrain` | Recipe → heightmap → SDF, erosion, bake-and-hash, slope/walkability analysis. |
| `loom_particles` | Deterministic particle simulation. |
| `loom_audio` | `cpal` playback plus acoustics measured by tracing the real geometry. |
| `loom_cli` | The `loom` binary: 10 subcommands, plus the viewer and editor. |
| `loom_agent` | `loom-mcp` — an MCP server over stdio that shells out to the `loom` CLI. A **binary**, so nothing can link it. |
| `xtask` | `cargo xtask validate` — drives the real `loom` binary as a subprocess over 14 scenes and fails on any Vulkan validation message. |

Three dependency rules are enforced by `scripts/check-deps.sh`, which runs first in the green
script: `loom_reflect` has no in-workspace deps, `loom_scene` may depend only on `loom_reflect`,
`loom_agent` is depended on by nothing, and nothing outside `loom_render*` imports `ash`.

## Notable decisions

- **Vulkan 1.3 via `ash`, no abstraction layer.** No wgpu, no RHI, no portability shim. Isolation
  instead: a future backend is a rewrite of `loom_render`, not a diffusion through the codebase.
  Instance requests `vk::API_VERSION_1_3`; the device check rejects anything below it.
- **Dynamic rendering only.** No `VkRenderPass`, no `VkFramebuffer`.
- **Descriptor indexing + buffer device address.** No per-draw descriptor sets. The two places that
  do bind a set — the bindless texture array and the acceleration structure — say why in the source.
- **`gpu-allocator`.** No direct `vkAllocateMemory`.
- **Hardware ray queries for sun shadows**, not a ray tracing pipeline: the rays are secondary
  visibility, answered inline from the fragment shader already shading the pixel.
- **Slang → SPIR-V in `build.rs`**, and every emitted module is checked with `spirv-val`. A shader
  compile failure fails the build with full output; it is never swallowed.
- **`rapier3d`** with `enhanced-determinism`, voxel colliders for terrain, no trimesh colliders on
  dynamic bodies.
- **`rhai`** compiled with `no_time`, so `timestamp()` is not merely disabled but absent — a script
  that could read the wall clock would make the simulation depend on machine speed and quietly
  break every `--assert`.
- **Voxels serialize as op lists**, never raw arrays: a 67-million-voxel terrain is a few lines of
  text that diffs.
- **Fixed timestep.** Render interpolates; the sim never sees a variable `dt`.
- **Scene writes carry a version token.** A stale write is rejected and never merged — the editor
  and the agent issue the same `SceneOp` transactions through the same code path, so a twelve-op
  agent transaction undoes in one Ctrl+Z, and a divergence raises a banner offering both versions.

## Build

Linux x86_64 only, by design. Prerequisites beyond the pinned toolchain (`rust-toolchain.toml`
pins Rust 1.97.1, edition 2024):

| Need | Why |
| --- | --- |
| `clang` + `mold` | `.cargo/config.toml` links with mold |
| `slangc` | shader compilation in `build.rs`. Not packaged by any distro — install from the Slang GitHub releases |
| `spirv-tools` (`spirv-val`) | validates every emitted SPIR-V module |
| `vulkan-validation-layers` | the second green check has nothing to report without it |
| A Vulkan 1.3 device | dynamic rendering, descriptor indexing, buffer device address, ray query |

Note that Fedora's `slang` package is S-Lang, the terminal library — unrelated. Check for the
`slangc` binary, not the RPM.

```bash
cargo build --workspace
```

## Run

```bash
loom validate assets/test/office.loom            # schema-validate, print version token
loom describe MeshRenderer                       # a component's JSON Schema
loom render assets/test/office.loom --out /tmp/o.png --size 1280x720
loom sim assets/test/walker.loom --ticks 600 --assert "positions.Walker.y > 0.4"
loom scene s.loom --tx tx.json --dry-run         # print the diff, write nothing
loom place s.loom --op op.json                   # on top of / aligned to / facing / grid on
loom measure s.loom --node Room/Desk             # bounds and overlaps, no render needed
loom terrain s.loom --from 0,0,0 --to 40,0,40 --max-slope 35
loom explode s.loom --at 4,1,0 --radius 3 --out /tmp/boom.png
loom run assets/games/proving_ground.loom --edit # editor: hierarchy, inspector, gizmos, Play
```

Every subcommand prints one line of JSON and uses the exit code as the coarse signal (`0` ok,
`1` the thing was invalid, `2` the invocation was wrong). An unrecognised flag is a failed
invocation, not a no-op — a silently ignored `--dry-run` once wrote a file for real.

`loom-mcp` wraps the same commands as MCP tools over stdio.

## Tests

The project's own definition of done, quoted from `CLAUDE.md`:

```bash
cargo clippy --workspace -- -D warnings   # 1. clean
cargo xtask validate                      # 2. ZERO Vulkan validation messages — REAL as of today
cargo test --workspace                    # 3. golden images + determinism hashes match
```

> **`cargo check` passing is not done.** Rust's compiler catches nothing that matters in Vulkan —
> missing barriers, wrong image layouts, use-after-free of in-flight resources, out-of-bounds
> bindless indices all compile fine. The validation layers are the real compiler.

`scripts/green.sh` runs all three plus the dependency-rule check. There are 415 `#[test]`
functions. `cargo xtask validate` additionally simulates a scene under both the debug and release
binaries and fails if the state hashes disagree.

## Status

103 commits. All milestones M0–M12 of `docs/design/LOOM-BUILD-BRIEF.md` §6 are complete, including
the editor. What that means concretely: a scene loads from text, renders offscreen or in a window,
simulates deterministically under physics and scripts, carves destructible voxel terrain, plays
audio with geometry-derived acoustics, and can be edited by a human and an agent at the same time
through one shared transaction log and one undo stack.

Honest gaps:

- **CI enforces two of the three green checks.** GitHub's hosted runners have no GPU and no
  validation layers, so `cargo xtask validate` runs locally only. It skips loudly rather than
  passing vacuously when no Vulkan device is present.
- **Tested on one GPU.** RTX 4090 on the NVIDIA 610 driver series, Fedora 44. AMD and Intel are untested;
  the build brief calls this out as a known blind spot (§7.14).
- **`cargo test` has no golden-image comparison** despite the wording of check 3 above. The suite
  is unit tests plus determinism hashes; image regressions are caught by eye and by `xtask validate`
  not crashing, which is weaker than a pixel diff.
- **No prefab system.** `docs/format/README.md` §5 specifies one; the parser refuses `prefab =`
  and `extends =` keys loudly rather than ignoring them, so no scene can silently depend on a
  feature that does not exist.
- **`loom_ecs` is not archetype storage.** It is `Vec<Option<T>>` indexed by entity — marked as a
  deliberate shortcut in the source, to be revisited when profiling demands it.
- **Only Surface Nets meshing.** Dual Contouring (for sharp corners) has a `Mesher` trait waiting
  for it and no implementation. No LOD octree.
- **Deferred rendering work:** SDFGI and a post-processing stack were pushed past M12 on purpose.
- **The knowledge graph is undecided** — see ADR 0003. It is the one item from M12's list with no
  implementation and no resolution.
- **No gamepad support.** `loom_input` handles winit keyboard/mouse only; the planned `gilrs`
  integration is absent.
- Every crate is `version = "0.0.0"`, `publish = false`. This is not a library you depend on yet.

## Going deeper

`docs/design/README.md` is the entry point and routes by milestone. It also flags which passages of
the six design documents are superseded — three of them predate the Vulkan decision and still
discuss wgpu at length, which is exactly the kind of trap that gets written down here rather than
retroactively tidied away.

- `docs/design/LOOM-BUILD-BRIEF.md` — the operational plan. §2 locked decisions, §6 milestones,
  §7 the traps. Authoritative on what is locked.
- `docs/format/README.md` — the normative `.loom` format spec, written before the parser.
- `CLAUDE.md` — the always-loaded rules the agent works under: locked decisions, 16 never-dos,
  the definition of green.
- `docs/decisions/0001-rust-edition-2024.md` — edition 2024, and a locked-table entry that had
  drifted silently.
- `docs/decisions/0002-companion-doc-precedence.md` — which design doc wins, and every conflict
  between them, recorded rather than fixed in place.
- `docs/decisions/0003-knowledge-graph-deferred.md` — **proposed, awaiting a decision.**
- `docs/decisions/0004-schemars-instead-of-a-reflect-derive.md` — why there is no
  `#[derive(Reflect)]`: `schemars` already emits all four things the design doc wanted from one.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option, matching the `license = "MIT OR Apache-2.0"` declaration in the
workspace manifest. Contributions are accepted under the same dual license.
