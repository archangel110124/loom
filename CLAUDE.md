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
this project was verified by a human opening the PNG. It renders eight scenes
chosen for coverage of *rendering paths* — mesh, bindless textures, voxels,
alpha particles, additive particles, an authored-dark environment, a whole
game, and vertex-shader-generated grass — and compares them to
`tests/references/` with a calibrated tolerance. **Adding a rendering path
means adding a scene to `GOLDEN`**, or the gate reports a full pass without
ever having looked at it; grass shipped two slices before anyone noticed.

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

> **Phase 0 complete; P1 done. P2 (grass) in progress — blades render and bend in the wind.** See
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
> **P2 slice 1 landed: `loom_grass` placement.** A blade is a pure function of its coordinates —
> position-hashed off the frozen `loom_field::noise::hash`, Voronoi clumping, coverage computed
> from slope/rock/flow rather than a painted mask. That purity is what makes dirty-region
> regeneration byte-identical, and there is a test asserting a crater leaves its neighbours
> untouched. **Nothing draws yet.**
>
> **Slice 2: grass renders.** Cubic-Bézier blades, 15 verts near / 7 far, generated on the CPU
> into an ordinary mesh — `assets/test/meadow.loom`. **That order is deliberate and worth keeping
> in mind:** the phase's risk is AA, shimmer is sub-pixel geometry under a moving camera, and a
> mesh answers that with no compute pass or indirect draw. The machinery cannot change the answer;
> the answer may change the machinery. Blade normals are deliberately *not* geometric — tilted
> outward and blended toward the ground, because an honestly-lit flat blade reads as paper.
>
> **Slice 4: blades are generated in the vertex shader and bent by P1's wind.** 42 verts per blade
> from `SV_VertexID`, no vertex or index buffer; the CPU uploads *what a blade is* and the GPU
> decides where its triangles are, which is the whole reason wind can move it. **The bend is on the
> control points, never the base** — a blade whose base moves is grass sliding across the ground,
> the "swimming" artifact. The clump hash phase-shifts the sway so neighbours do not move in
> lockstep. Wind and the camera position both live in the environment buffer because the push block
> is at its 128-byte guarantee.
>
> **Slice 6: MSAA, and it is the tool that works.** Measured flicker on `meadow`:
>
>     1x  0.424      2x  0.387      4x  0.354      8x  0.318
>
> Steady, diminishing, no cliff. **4x is the setting** (`MSAA_SAMPLES` in `renderer.rs`) — 8x buys
> another 10% for double the bandwidth and can be revisited when there is a frame budget to argue
> against. The offscreen path rasterises into a transient multisampled pair and resolves into the
> colour target, so the readback and the golden images still see one sample per pixel. Every
> pipeline builder takes its sample count as a parameter, because a pipeline's rasterisation
> samples must match the attachment, and getting that wrong is four validation errors rather than
> a visual bug.
>
> **The viewer drew at one sample until after P4, and that was a measurement bug, not a setting.**
> Every AA number in this project — the MSAA table above, the density-falloff rows, the blade-width
> sweep — was taken on the *offscreen* path at 4x, while the window the human actually judges grass,
> water and rain in had none of it. It is the same defect as a metric that frames a scene not
> containing the subject, and it survived far longer: measuring the filter somewhere the filter is
> not. The window now rasterises into the same multisampled pair and resolves into the scene target,
> so the two paths agree. Measured on `meadow` at 1440x900, high-frequency energy over the grass
> region **0.0436 → 0.0333 (−24%)**, forward pass **0.114 → 0.167 ms**. **Rain and the editor UI
> stay single-sample** — both draw into the resolved target, after it.
>
> **The multisampled images go through the render graph** like everything else (never-do #4). They
> start UNDEFINED every frame; skipping the transition is a validation error, and it was one. The
> barrier-list test in `lib.rs` now names all four transitions, which is how that ownership stays
> visible rather than assumed.
>
> **Slice 9: grass reads the terrain it stands on, and the engine can finally measure itself.**
>
> `grass_blades` passed `loom_grass` a flat constant `Ground`, so grass never responded to terrain at
> all — the slope and flow rules had been implemented and tested since slice 1 with nothing feeding
> them. A `GroundGrid` per field now marches the voxel SDF down to the surface (half-voxel steps,
> then eight bisections — the `i8` field saturates at one voxel and carries no usable distance
> further out) and answers with bilinear height, a central-difference normal, and a concavity proxy
> for flow. **`loom_grass` still has no `loom_voxel` dependency**; the `&dyn Fn` closure is the seam.
> A column with no surface returns `rock = 1.0`, which zeroes coverage — that is the
> no-floating-blades path, and it falls out of the same query rather than being a special case.
> The grid is not faith-based: the naive per-candidate march measures **2.98 s against 0.14 s**.
>
> **The slope boundary wanders, and that is the difference between a hillside and a shaved ring.** A
> cutoff on slope — at any threshold, softened by any amount — draws grass to a clean curve across a
> smooth hill, and a clean curve is the synthetic tell. `coverage` now perturbs the *threshold* with
> low-frequency noise on world position. Widening the fade band instead only blurs the curve; that
> was tried first. **The wander only ever subtracts**, so `slope_cutoff` keeps its documented meaning
> exactly — grass stops *entirely* there. A symmetric version let grass survive past the cutoff and
> the existing test caught it within a minute. Flat ground is untouched by construction, and the
> manifest proves it: only `grass_slope`'s reference hash moved.
>
> **GPU timestamps exist** (`LOOM_GPU_TIMING=1`, per render-graph pass, `Renderer::last_pass_times`).
> They say **grass costs 0.054 ms** for 45,460 blades at 1080p/4x MSAA — 0.3% of a 16.7 ms frame. The
> whole forward pass of every scene in this project is 0.05–0.11 ms. **So the placement compute pass
> and `vkCmdDrawIndirect` are not justified by GPU cost**; density could rise ~10x first. The
> milliseconds are calibrated against the readback pass, whose duration is bounded by the bus:
> 13.5–14.0 GB/s measured against a PCIe 4.0 x8 ceiling of 15.75 GB/s, linear over a 16x range.
> The printed field is labelled `graph`, not `total`, because it is the sum of graph passes and
> about **2% of a frame** — the TLAS rebuild is a separate submit and the PNG encode is CPU.
>
> **The offscreen harness's ~30 ms/frame is the PNG encoder**, not the engine: ~10 ms fixed plus
> ~11 ms per megapixel, with GPU readback only 0.61 ms of it. It never measured the engine in either
> direction.
>
> **Adding a rendering path means adding a scene to `SCENES` and `GOLDEN`.** `meadow` was missing for
> two slices and `grass_slope` for one, and in both cases the gate reported a full pass without ever
> rendering the thing under test. Now 19 scene runs and 9 references.
>
> ## The AA investigation, re-run on an instrument that can see the grass
>
> `cargo xtask shimmer` now holds the camera still at the scene's authored eye and advances the
> simulation, so it measures twinkle at rest. Three scenes with no animated geometry score **exactly
> 0.000**, which is the control it never had. Baseline at 4x MSAA, 640x400:
> **`meadow` 3.059, `grass_slope` 1.755** (these moved from 2.712/1.545 at `1062550`, when grass
> took its colour from the authored `Material` — the field is painted brighter and the metric
> measures absolute pixel differences, so **flicker is not invariant to brightness**. The table
> below was taken on the darker field and its rows remain valid against each other. Normalising
> flicker by mean brightness was tried and **does not work** — it still scales, more steeply, because
> the numerator is grass and the denominator is the whole frame. Reverted; see ADR 0010. What stands
> is the narrow rule: **never compare two AA numbers across a change in colour or lighting.**)
>
>     MSAA          1x 3.888   2x 3.000   4x 2.712   8x 2.502
>     density falloff at 4x:   on 2.712   off 2.715      <- no effect at all
>     blade width at 4x:  0.020 2.712   0.035 2.635   0.060 2.338   0.100 1.973
>
> **MSAA survives re-measurement.** Monotonic, ~36% from 1x to 8x, and 4x is still the right pick.
>
> **The density falloff's win was entirely the deletion artifact.** 2.712 against 2.715 — zero. It
> remains worth keeping as an LOD and cost measure, and it helps `grass_slope` by ~6%, but **it is
> not an anti-aliasing tool** and the table that said it was, was photographing an empty field.
>
> **Widening blades genuinely reduces twinkle**, monotonically, 27% for 5x width — so "minimum
> screen-space width clamping is measured worse, twice, decisively" was *also* an artifact of the
> broken instrument, and the research pass that called it the most important trick for distant grass
> was probably right. It was deleted on bad evidence. 0.1 m blades are a leaf rather than grass, so
> the usable form is the screen-space clamp that widens only sub-pixel blades — **that is the next
> thing to build, and it should be judged on this instrument.**
>
> **Two cautions on reading the number.** It is strongly resolution-dependent — `meadow` is 2.712 at
> 640x400 and 0.539 at 1920x1080 — because the artifact *is* sub-pixel geometry, so the low
> resolution is a deliberate stress test rather than a representative frame. And flicker on animated
> geometry conflates twinkle with legitimate motion: a blade really does change pixels when it bends.
> The 0.000 controls prove the camera is static; they do **not** establish that 2.712 is bad in
> absolute terms. Resolution scaling is the discriminator, and it says sub-pixel twinkle dominates.
>
> ## ⚠ THE AA NUMBERS BELOW ARE INVALID. READ THIS FIRST.
>
> **`cargo xtask shimmer` and `cargo xtask flythrough` ignore the scene's authored camera.** They
> frame whole-scene bounds — for `meadow`, about 38 m back — and render at 480x300. At that distance
> the 55 m density falloff has deleted the entire field, and a surviving blade is ~0.3 px wide. Open
> `target/xtask-shimmer/meadow_0000.png`: **it is a bare green slab with a stone on it. There is no
> grass in the frame at all.**
>
> So every row of the table below measures *how thoroughly the falloff removes grass from the shot*,
> not how stable grass is. **The winning variant won by deleting the subject.** 0.137 is the flicker
> of an empty plane.
>
> **Measured at `meadow`'s own camera, with the camera completely static and one tick of wind between
> frames:**
>
>     meadow        0.539        cave (no animated geometry)  0.000
>     grass_slope   0.324
>
> `cave` at exactly 0.000 is the control that proves the instrument and the static camera are sound.
> **Grass twinkles at rest.** Cutting wind speed 10x only brings `meadow` to 0.361, so this is not
> coherent motion cancelling imperfectly — it is twinkle.
>
> **The AA question this phase exists to answer — can thin geometry be stable without temporal
> accumulation — is still open.** MSAA's sample-count curve was taken before the falloff existed, so
> those rows saw real (sub-pixel) grass and are probably sound; every row involving the cull is not.
> Re-measure at an authored camera before trusting anything here.
>
> **The general lesson, which cost a night:** a metric that frames a scene automatically will
> silently stop containing the subject, and then it rewards whatever removes the subject fastest.
> The human found this by running `loom run` and seeing no grass; no gate in this project can detect
> an absent feature.
>
> **Slice 7 (numbers invalid, see above): density falloff appeared to win the AA investigation.**
> Every number is `cargo xtask shimmer`'s flicker on `meadow` at 4x MSAA, each row one change from
> its neighbour:
>
>     no cull (every blade)              0.354
>     hard cull, 12% surviving at range  0.234
>     + soft fade                        0.214
>     + fading all the way to none       0.137   <- shipped
>     soft fade + alpha-to-coverage      0.212
>     soft fade + minimum-width clamp    0.419
>
> **0.354 → 0.137 on top of MSAA**, and the mechanism is one screenful of shader. Blades thin with
> distance, chosen by a stable hash of the blade's own position so the survivors never change as
> the camera moves, and each one leaves by shrinking uniformly to a point across a band 8% of the
> population wide.
>
> **The previous round blamed the wrong tool, and the lesson is the general one.** It measured a
> minimum-width clamp and a hard cull *together*, got 0.431, and concluded the cull's pop was the
> suspect. Separated, the cull is the single largest win available and the widening is what nearly
> doubles the flicker. Two tools measured as one is not a measurement. The clamp is now **deleted,
> not gated** — the research pass calls it the most important trick for distant grass, and on true
> geometry blades it is measured worse twice, decisively, with the confound removed.
>
> **Fading to *none* rather than to a floor is most of the win** (0.214 → 0.137). A floor leaves a
> fixed fraction of blades scattered across the far field, and a sparse scatter of sub-pixel blades
> is noisier than either a full field or an empty one — each is a lone twinkling pixel with nothing
> around it to average against.
>
> **That only works because the ground under a grass field is authored the colour of grass**, and
> that is a scene rule rather than an engine one. `meadow`'s soil was brown; from the flythrough's
> orbit the thinned field read as ploughed earth. Dark green fixes it and still reads as shadowed
> ground between blades close up. **Any scene with a `Grass` field owes its ground the same.**
>
> **Alpha-to-coverage bought 0.002 and was removed with it.** The geometric shrink has already
> taken a blade's area to zero by the time its opacity would matter, so there was nothing left for
> coverage to do. The pipeline state, the extra varying and the RGB-only write mask it needed all
> came back out.
>
> **A pan cannot measure a pop; only a dolly can**, and the dolly is swamped by near-field parallax
> — six variants landed within 2% of each other on it. So the fade's *smoothness* is still unmeasured;
> what is measured is that thinning the field helps enormously and that a soft edge is worth a
> further 9% over a hard one.
>
> **`meadow` is in `GOLDEN` now.** It was not, and the image gate had been reporting seven matches
> without looking at a single blade — grass is the only rendering path whose geometry exists solely
> in the vertex shader, so nothing else covers it.
>
> **Slice 5 (superseded, kept for the reasoning):** minimum screen-space width clamping makes
> flicker worse on its own — 0.424 with nothing, 0.479 with the pixel floor. Rasterisation without
> multisampling is binary at the pixel centre, so widening a sub-pixel blade recruits *more* pixels
> into the flicker rather than steadying it. The hypothesis that it needed MSAA underneath was
> tested and also wrong.
>
> **`cargo xtask shimmer` now reports flicker, not changed pixels**, because changed pixels was
> fooled by exactly this experiment: widening blades covers more screen, changes more pixels, and
> read as a regression when the question was stability. Flicker is `|b - (a+c)/2|` over three
> frames — coherent motion is near-linear and cancels, a pixel that twinkles does not. `loom
> flicker a b c` exposes it. **Use the flicker column; the changed% column is context only.**
>
> **Slice 3: `cargo xtask shimmer` measures the phase risk as a number** — mean fraction of
> pixels changing between consecutive frames of a slow pan, using the same calibrated comparison
> the image gate uses. **Only ever compare a scene against itself under a different setting**; it
> counts legitimate parallax too, so across scenes it means nothing. Baseline at 1× MSAA:
> `meadow` 3.26% mean.
>
> **The order changed, and the investigation is what changed it.** Of the five non-temporal AA
> tools, only MSAA is testable against a baked mesh: minimum screen-space width clamping and
> density falloff are both **camera-dependent**, so they cannot exist until blades are generated
> in a vertex shader. Alpha-to-coverage is largely moot for true geometry blades, and
> geometry-over-cards is already decided. So the GPU path is not *after* the AA work — it is a
> **prerequisite** for most of it.
>
> **Next:** P3 water. Vertex-shader blade generation, MSAA, the AA toolkit and the terrain response
> are done. **The non-temporal toolkit proved sufficient** — 0.354 → 0.137 with density falloff on
> top of 4x MSAA — so the full-screen AA pass that was budgeted for is not needed, and adding one is
> now a scope decision needing an ADR rather than a contingency already agreed.
>
> **The placement compute pass and indirect draw are deferred, on evidence.** They are the design
> doc's next item, and the GPU timestamps say grass is 0.3% of a frame, so they would optimise the
> part that is already free. They will be wanted when placement has to be *dynamic* — a field larger
> than one CPU bake, streaming as the camera moves — which is a scale argument, not a cost one.
> Doing it also needs an answer to where the placement rules live: hand-porting Voronoi clumping and
> the position hash into Slang is the CPU/GPU divergence S2 and ADR 0006 exist to prevent, and S2's
> `Expr` tree is a scalar-field language that cannot express neighbourhood search or struct output.
> **That deserves an ADR.**
>
> **Grass is rendering-only and outside the sim hash**, the same exemption rain gets. Blades are
> never ECS entities and the scene shows one node for the field, never the blades. **Verified, not
> assumed**: the determinism hash is `b478ea4ac2622d32` across every change in P2 slices 5–7.
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
>
> **P4 rain drops are stateful — ADR 0017, and it supersedes ADR 0014's deferral.** A drop has a
> position and a velocity in a GPU buffer, `rain_sim.slang` advances it a fixed tick at a time and
> **collides it against the collision world** — every voxel volume unioned with every static
> `BoxCollider`, baked by `loom_rain::collide` into an `R8_SNORM` 3D image. So a **mesh** roof now
> stops rain, which the baked height field could never do (ADR 0014's trigger 2), and a splash is a
> collision the simulation resolved rather than a place exposure said a drop should have reached
> (trigger 3). `assets/test/rain_gantry.loom` is the scene that can only pass with this, and it is
> in both gates. Splashes feed an **indirect draw** from a count that never leaves the device; the
> CPU crowns in `loom_cli::particles` are gone. Cost is 0.022 ms to simulate and 0.033 ms to draw at
> 1920x1080, against 0.036 ms for the whole stateless layer.
>
> **The render graph owns buffer barriers now**, because it had to: a compute pass writing the drop
> buffer and a vertex shader reading it in the same command buffer need a dependency, and a missing
> one draws last frame's rain, which looks almost right. `BufferId`/`BufferAccess`/`pass_with` —
> never-do #4 covers buffers as well as images, and `plan_full` makes the buffer barriers as
> testable as the layout transitions.
>
> **What state costs is that a frame is no longer a pure function of its tick**, which is the same
> objection ADR 0010 used to reject TAA. The golden gate survives it because a headless still seeds
> deterministically and advances to `--sim N` in **one** dispatch — verified byte-identical across
> three processes — but the viewer and the offscreen path now agree only while the camera is still.
> `Renderer::set_rain_tick` going backwards re-seeds.
>
> **It did not fix the reported motion artifact, and that was characterised first.** Rain is 97% of
> the frame-to-frame temporal noise in `rain_impact` under a walking camera; the cause is that the
> rain pass draws into the **resolved single-sample target after the MSAA resolve**, so distant
> sub-pixel streaks are the one thing in the frame with no anti-aliasing at all, and that the near
> field is drawn at a constant *world* width that makes a drop half a metre away a 19-px bar.
> **The near field is fixed** — `RAIN_NEAR_MIN`/`RAIN_NEAR_FULL` fade the nearest 0.6–2.5 m out,
> which drops the layer's frame-to-frame brightness swing from 13.4% to 7.4% and costs 65 drops of
> 131,072; the boundary fade could never have covered it, because a drop 30 cm from the eye is in
> the *middle* of the block. **The sub-pixel half is not**: a screen-space width floor with matching
> alpha needs alpha-to-coverage, which needs the rain pass to have samples, and it has none by
> design. None of it needs state. **`cargo xtask shimmer` cannot measure any of this** — at its 0.2 s step a drop
> falls 1.6 m and consecutive frames share no streaks. Use `loom render --dolly <m>`, added for it:
> the fly-through could only *pan*, and a pan is chosen precisely because it makes no parallax.

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
