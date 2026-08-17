# Editor rework — survey of binding constraints

**This document exists to stop the design phase producing something that cannot be built.** It is
the constraint register: every rule in `CLAUDE.md`, the build brief, the implementation order and
the twenty-one ADRs that narrows the design space of a ground-up editor rework, quoted, with the
consequence stated. Then the list of things the user has asked for that **cannot be built without
a new ADR**, each pinned to the exact rule it collides with.

Nothing here is a design proposal. Where a resolution is obvious it is named as such and marked
*no ADR needed*, so the later phases do not spend approval budget on decisions that were already
made.

Sources read in full: `CLAUDE.md`, `docs/design/LOOM-BUILD-BRIEF.md`,
`docs/design/LOOM-IMPLEMENTATION-ORDER.md`, `docs/design/README.md`,
`docs/design/loom-pcg-and-editor.md`, `docs/format/README.md`, and ADRs 0001–0021.

---

## 0. Precedence, so a conflict has an answer before it arises

ADR 0002 fixes the order and it is binding: `CLAUDE.md` → `docs/decisions/` (newest applicable) →
`LOOM-BUILD-BRIEF.md` §2 → `loom-vulkan-backend.md` → the subsystem docs → the two origin
documents (`docs/decisions/0002-companion-doc-precedence.md:49-57`). `LOOM-IMPLEMENTATION-ORDER.md`
is authoritative on *order* and **supersedes the build orders inside every companion doc**
(`docs/design/README.md:36`). So where `loom-pcg-and-editor.md` proposes an editor build order it
loses to the implementation order's Phase 7 list, and where either disagrees with `CLAUDE.md` it
loses outright.

Two documents matter most for this rework and are easy to miss: **`docs/format/README.md` is a
normative spec, not a design note** ("Nothing in this document is open", `docs/format/README.md:435`),
and **`LOOM-IMPLEMENTATION-ORDER.md` §Phase 7 already locked two editor decisions** before any code
was written (`docs/design/LOOM-IMPLEMENTATION-ORDER.md:449-461`).

---

## 1. The three properties, and what each forbids an editor

`CLAUDE.md` states three properties and says "Everything else follows from them". They are the
sharpest constraints on this rework because two of the four painting systems the user wants sit
directly across them.

**1. "Everything authored is *diffable text*, schema-validated on load."** The editor may not
create authored state that is not `.loom` TOML (or another schema-validated text artifact). This
is not a preference; the whole verification story — git diff as the fourth verification channel
(`docs/design/LOOM-BUILD-BRIEF.md:164`) — rests on it.

**2. "The agent can see and test its own work."** Anything the editor can author, the agent must be
able to author and inspect through the CLI. A capability reachable only by mouse is a capability
the agent cannot verify, and it breaks the parity that M12's exit criterion asserts: *"the same
edit made by hand and by agent produces an identical diff"* (`docs/design/LOOM-BUILD-BRIEF.md:285`).

**3. "The runtime is deterministic, so those assertions are trustworthy."** Editor play-mode must
step the same fixed timestep the headless sim does, and nothing the editor does may enter the sim
hash. The pinned hash is `b478ea4ac2622d32` (`CLAUDE.md`, current-phase block; ADR 0017, 0018,
0019 all re-verify it).

---

## 2. Rules that constrain the editor, quoted

### 2.1 The undo rule — the single hardest constraint

> "16. Never give the editor its own undo stack. It issues the same `SceneOp` transactions the
> agent does, through the same code path — a twelve-op agent transaction must undo in one Ctrl+Z."
> — `CLAUDE.md:106`

**Consequence: every editor action must be expressible as a `SceneOp`, or it is outside undo
entirely.** This is enforced by the shape of the code, not by discipline.
`crates/loom_scene/src/ops.rs:47-98` defines exactly nine operations — `SpawnNode`,
`SetTransform`, `SetField`, `RemoveNode`, `RenameNode`, `ReparentNode`, `RemoveComponent`,
`RevertOverrides`, `UnpackPrefab`. `Applied::undo` is a `String`
(`crates/loom_scene/src/ops.rs:126-128`) and it is **the entire previous scene text**;
`Session::undo` restores it by `std::mem::replace` on the session's text
(`crates/loom_scene/src/edit.rs:314-323`).

So the undo stack is a stack of scene files. **An editor action that changes anything other than
the scene text is invisible to it and cannot be undone by Ctrl+Z.** That is the fact that decides
the fate of texture painting, vertex-colour painting and splat painting in §4.

The escape hatch that already works, and is the model to copy: `VoxelVolume.ops` is a free-form
JSON array carried on a component (`docs/format/README.md:328-332`), so terrain sculpting is
`SetField` on that array — a brush stroke becomes an *op appended to a text list*, undoable for
free. Never-do #11 ("Never serialize raw voxel arrays into a scene file") is the same rule read
from the other side: represent the generator, not the generated.

### 2.2 Gesture coalescing

> "Gestures coalesce: a gizmo drag or a scrubbed slider is **one** undo step, not one per frame."
> — `CLAUDE.md` (M0–M12 block)

Already built: `Session::apply_coalescing(transaction, gesture)` pops the previous undo entry when
the gesture key matches (`crates/loom_scene/src/edit.rs:282-306`), and `run.rs` builds gesture keys
from node/axis/field plus a `gesture_epoch` bumped on every mouse-up
(`crates/loom_cli/src/run.rs:198-201, 1716-1730, 1781, 1992`). **The new editor must keep using
this API rather than batching frames itself**, and every new continuous interaction (a brush drag,
a colour-picker scrub, a marquee) needs a gesture key that is stable for the gesture and unique
across gestures. The comment at `edit.rs:277` records why the epoch exists: an agent write landing
mid-drag must not be swallowed into the human's undo entry.

### 2.3 Version tokens and split-brain

> "15. Never force-write a scene file against a stale version token, and never auto-merge two
> divergent scene states — reject and reload. Silently destroying the human's edits is the worst
> bug class in this project." — `CLAUDE.md:103`

> "Every write passes the token it read. A write against a stale token is **rejected** … **Never
> auto-merge.**" — `docs/design/LOOM-BUILD-BRIEF.md:530-541`

Built and load-bearing. The token is the BLAKE3 hash of canonical bytes
(`docs/format/README.md:384`); `Transaction::expect_version` carries it
(`crates/loom_scene/src/ops.rs:110-113`); `apply` checks it *first*, before any work
(`crates/loom_scene/src/ops.rs:196` — "Version first: re-applying against a scene that moved under
you is the whole point of the check"); `TransactionError::current` returns the current content for
the caller to re-apply against and says in its own doc comment "**Never merged for them**"
(`crates/loom_scene/src/ops.rs:141-144`). `apply_to_file` holds a lock across read-apply-write
because the naive version let two processes both report `ok: true` while one silently erased the
other (`crates/loom_scene/src/edit.rs:90-110`). Writes are atomic via write-tmp-then-rename
(`crates/loom_scene/src/edit.rs:37-70`).

**Consequences for the rework:** the divergence banner is not optional UI polish, it is the
mechanism (`crates/loom_cli/src/run.rs:652`, `1292`, `1582`). `Session::reload` clears both undo
and redo, "because offering it anyway would let a user undo their way onto someone else's work"
(`crates/loom_scene/src/edit.rs:395-403`) — so **any new editor state that is derived from scene
history must survive a reload or be discarded with it**. And the new editor must keep the
"persist camera and selection outside scene state" and "never reload mid-gesture" rules from
`LOOM-IMPLEMENTATION-ORDER.md:459-461`.

### 2.4 Prefabs, and the two-file exception

ADR 0008 makes `SceneOp::SetField` on a prefab instance write into `[node.overrides]` rather than a
component, "so the inspector and `loom scene --tx` need no idea which kind of node they hold"
(`docs/decisions/0008-prefab-instancing-and-inheritance.md:46-52`). **The new inspector must not
grow a branch for instances** — that branch is exactly what ADR 0008 spent the design to avoid.

Two things the UI must surface honestly:

- **`apply-overrides` writes two files and is therefore two undo steps.** "the command reports
  `undo_steps: 2` rather than implying one"
  (`docs/decisions/0008-prefab-instancing-and-inheritance.md:54-60`). An inspector button that
  says "Apply to prefab" must say so, or Ctrl+Z will half-undo.
- **Any new command or panel that reads a scene must go through `prefab_load::for_reading`**
  (`docs/decisions/0008-prefab-instancing-and-inheritance.md:121-123`;
  `crates/loom_cli/src/prefab_load.rs`). This is named as "the single most likely way to regress
  S4": the parser now *accepts* `prefab`, so a reader that skips resolution gets a node with no
  components that draws nothing and validates clean.

### 2.5 The scene format is a normative contract

`docs/format/README.md` binds the editor in ways that are easy to break with a nice UI:

- **Defaults are omitted** (§4, `:237-247`). A field equal to its registered default is not
  written. An inspector that writes every field it displays will bloat every scene it touches and
  destroy the reviewability the format exists for. ADR 0008 records this as a defect that actually
  shipped: `rot_euler = [0,0,0]` and `scale = [1,1,1]` written onto nodes that never had them
  (`docs/decisions/0008-prefab-instancing-and-inheritance.md:113-115`).
- **`f32` must round-trip as the author wrote it.** ADR 0008 again: `1.4` came back as
  `1.399999976158142` because a `Transform` holds `f32` and JSON holds `f64`
  (`docs/decisions/0008-prefab-instancing-and-inheritance.md:109-112`). Any new numeric widget
  goes through the same `f32::to_string` shortest-round-trip path.
- **Comments and hand formatting survive writes** (§2.1, `docs/format/README.md:99`;
  `crates/loom_scene/src/ops.rs:157-159` — "the document is edited as a format-preserving DOM").
- **`name` and `transform` are node-key sugar for the `Name` and `Transform` components**
  (`docs/format/README.md:206-233`). Addressing is uniform: `Transform.pos` is a component field
  everywhere — override key, `SceneOp::SetField`, inspector, rhai. The new editor must not invent
  a second addressing scheme for transforms.
- **Asset references are aliases, never paths and never UUIDs** (`docs/format/README.md:158-169`).
  An asset browser that writes a path into a scene is writing an invalid scene.
- **A change to field names, types, defaults, addressing or override syntax needs a `format` bump
  and a migration function** (`docs/format/README.md:395-407`). Any new component the editor
  introduces is additive and free; renaming one is not.
- **Zero roots or two roots is an error, and `parent` may not forward-reference**
  (`docs/format/README.md:176`, `:195`). A hierarchy panel that supports drag-to-reparent must
  keep declaration order valid on write.

### 2.6 Crate boundaries, CI-enforced

> "`loom_reflect` and `loom_scene` depend on nothing else in the workspace. `loom_agent` is
> depended on by nothing. **Nothing outside `loom_render*` imports `ash`.**" — `CLAUDE.md:129-131`

Enforced mechanically by `scripts/check-deps.sh`, which `scripts/green.sh` runs first: it walks
`cargo tree` for `loom_reflect` and `loom_scene`, checks nothing depends on `loom_agent`, and
greps every `.rs` file outside `crates/loom_render*` for `use ash` (`scripts/check-deps.sh:17-52`).

**Consequences.** All Vulkan and all egui-to-Vulkan plumbing stays in `loom_render`, which is why
`loom_cli/src/panels.rs:17` reads `use loom_render::egui;` — the UI crate reaches egui through a
re-export (`crates/loom_render/src/lib.rs:64`). The new editor keeps that shape. The inspector is
generated from `loom_scene`'s schemars registry, which may depend on `loom_reflect` and nothing
else — so **schema-driven widgets cannot be implemented by teaching `loom_scene` about egui**. And
a command palette must issue `SceneOp`s directly; it may not link `loom_agent`.

Brief §7.6 is blunt about why this matters: "the agent's loop is `cargo check`", and a resequencing
trigger says **compile times exceeding roughly one minute warm are a stop-and-fix condition**
(`docs/design/LOOM-IMPLEMENTATION-ORDER.md:574`).

### 2.7 Dependencies

> "6. Never float a dependency version — pin exactly, add with `cargo add`." — `CLAUDE.md:94`

Every dependency in the workspace already uses `=x.y.z` (`crates/loom_render/Cargo.toml:9-21`,
`crates/loom_cli/Cargo.toml`). The toolchain is pinned to 1.97.1 with **`targets =
["x86_64-unknown-linux-gnu"]` only** (`rust-toolchain.toml`), and `.cargo/config.toml` configures
clang+mold for that one target.

> "12. Never introduce a trait with one implementation. Pre-authorized: `Mesher`, ECS system."
> — `CLAUDE.md:100`

A `Tool` or `Brush` trait is legitimate only once there are two concrete brushes to satisfy it,
and a `Panel` trait for a dock system is the exact shape brief §7.11 warns about.

### 2.8 Rendering rules that bind a viewport

These bite the moment the viewport becomes a docked render-to-texture tab rather than the whole
window.

- **Dynamic rendering only; no `VkRenderPass`/`VkFramebuffer`** (`CLAUDE.md:96`, never-do #1). The
  existing egui layer already complies — "Drawn with dynamic rendering into the swapchain image
  the scene just wrote, so there is no second pass and no render-pass object (never-do #1)"
  (`crates/loom_render/src/ui.rs:10`).
- **Barriers are owned by the render graph** (never-do #4), and this now covers buffers as well as
  images (`docs/decisions/0017-raindrops-become-stateful.md:90-102`). A scene-image → sample-in-egui
  dependency is a graph edge, not a hand-placed barrier. The barrier-list test in
  `loom_render_graph`'s `lib.rs` names every transition and must keep doing so
  (ADR 0018 consequences: "the barrier-list test names all eleven").
- **The frame is HDR and collapsed once.** Colour targets are `R16G16B16A16_SFLOAT`; `tonemap.slang`
  writes an `_SRGB` attachment; the chain is **forward → tonemap → UI → CMAA2 → present**
  (`docs/decisions/0018-the-frame-is-computed-in-float.md:38-50, 75-82`). **The editor UI is
  deliberately drawn after tonemap and before CMAA2**, because CMAA2 is a display-referred filter.
  A docked viewport changes what egui samples, and that ordering is the thing to preserve.
- **MSAA is 4x and the UI is single-sample.** `MSAA_SAMPLES` is `TYPE_4`
  (`crates/loom_render/src/renderer.rs:422`); `CLAUDE.md` records "**Rain and the editor UI stay
  single-sample** — both draw into the resolved target, after it."
- **The window and the offscreen path must agree, and this project has paid three defects for
  letting them drift.** "The viewer drew at one sample until after P4, and that was a measurement
  bug, not a setting" (`CLAUDE.md`); ADR 0018's consequences record a fourth — "the forward pass
  wrote a different destination depending on an environment variable — **in the window, which is
  where the human judges everything**, and that class of offscreen/window divergence has cost this
  project three defects" (`docs/decisions/0018-the-frame-is-computed-in-float.md:179-183`). **Any
  viewport restructuring must keep `loom render` and `loom run` rendering the same pixels.**
- **The TLAS holds meshes only.** Grass, water, rain, fire and smoke are vertex-shader geometry and
  cannot be hit by a ray; "**Anything that wants to be reflected has to become an `Object`**"
  (`docs/decisions/0019-secondary-rays-from-the-fragment-shader.md:330-336`). This is a decal
  constraint (§4.D).
- **No temporal accumulation, ever.** ADR 0010 rejects TAA because "determinism, agent-verifiable
  renders and single-frame golden images all assume a frame is a pure function of its state"
  (`docs/decisions/0010-non-temporal-aa-is-insufficient.md:79-83`); ADR 0018 refuses auto-exposure
  for the same reason (`:84-89`). No editor viewport effect may accumulate across frames.

### 2.9 Determinism rules that bind play mode

- **Fixed timestep, always; the sim never sees variable `dt`** (`CLAUDE.md` locked table).
- **No `HashMap`/`HashSet` iteration and no `thread_rng` in simulation code**, and **no wall clock
  in simulation code** — enforced by `clippy.toml`.
- **The presentation loop is exempt and the exemption is scoped, not blanket.** `run.rs` reads
  `Instant::now` under a targeted `#[allow(clippy::disallowed_methods)]` with a comment explaining
  that frame pacing and file polling are "exactly what wall time is for"
  (`crates/loom_cli/src/run.rs:385-393`). **The new editor copies that pattern rather than
  weakening the lint for the crate.**
- **Rendering-only systems are outside the hash and must stay outside it.** Grass, rain and the
  post stack all carry this exemption explicitly (`CLAUDE.md`; ADR 0017 `:104-127`). Any editor
  overlay, gizmo, or preview must not write anything the sim reads.
- **The rain buffer is stateful, so a viewer frame is a function of the tick *sequence*, not the
  tick.** "The viewer and the offscreen path agree only while the camera is still"
  (`docs/decisions/0017-raindrops-become-stateful.md:129-136`). An editor that scrubs a timeline
  backwards must re-seed (`Renderer::set_rain_tick` going backwards re-seeds) — a timeline scrubber
  is therefore not free.

### 2.10 Agent-facing rules the editor inherits

- **CLI first, MCP second** (`CLAUDE.md` locked table; brief §7.10). A feature reachable only from
  the editor breaks the property that "a human can do everything the agent can", and its inverse.
- **The `destructive` scope is off by default**; node deletion and asset removal need explicit
  sign-off (`docs/design/LOOM-BUILD-BRIEF.md:544-546`). `SceneOp::RemoveNode`'s own doc comment says
  "Requires the `destructive` scope (§7.17)" (`crates/loom_scene/src/ops.rs:72`).
- **Approval batching, not per-op prompts — already locked.** "Per-op approval trains you to
  blind-approve … Non-destructive ops apply immediately; destructive ones batch into one card"
  (`docs/design/LOOM-IMPLEMENTATION-ORDER.md:451-453`).
- **Label every transaction usefully.** "'Block out office: 14 nodes' beats 'update scene'"
  (`CLAUDE.md`, working-alongside-a-human section). Labels appear in the log panel and in git
  history, so every UI action needs a written label, not a generated one.
- **Represent the generator, not the generated.** "never put scattered instances in the scene tree
  or the ECS individually … The tree shows one node: 'pine_forest — 1.2M instances'"
  (`docs/design/LOOM-IMPLEMENTATION-ORDER.md:390-392`). `Scatter` and `Grass` are already authored
  as rules (`crates/loom_scene/src/components.rs:959-1051`), so the hierarchy panel must never
  expand them.

### 2.11 The verification obligations the rework inherits

All four green checks apply to editor work: `cargo clippy --workspace -- -D warnings`,
`cargo xtask validate` (zero validation messages), `cargo test --workspace`, `cargo xtask image`
(`CLAUDE.md`, definition of green; `scripts/green.sh`).

Two of these bite specifically:

- **"Adding a rendering path means adding a scene to `GOLDEN`"** — `CLAUDE.md` states it and gives
  the evidence ("grass shipped two slices before anyone noticed"). `xtask/src/main.rs:41` holds 43
  `SCENES` and `:253` holds 28 `GOLDEN` references, each with a comment justifying its inclusion or
  exclusion. **Decals are a new rendering path and therefore owe a golden scene.**
- **The validation layers panic in debug builds by design and must not be downgraded to a log
  line** (`CLAUDE.md`). A docked viewport that resizes its offscreen image every frame is a
  validation-message generator until it is right.

`cargo xtask flythrough` is "not a gate and is the more important half"; `cargo xtask shimmer`
scores flicker at the authored camera. Neither measures UI, but both will regress if the viewport
restructuring changes what the window renders.

`cargo xtask` gates are a cross-worktree singleton and serialise (`xtask/src/main.rs:216`,
`GateLock`), which matters only for the design phase's own hygiene: do not run them in parallel.

### 2.12 Things that constrain the *process*, not the code

- **No `// STABLE` markers exist anywhere in `crates/`** (verified by grep). Never-do #13 therefore
  does **not** block replacing `panels.rs`, `run.rs`, `gizmo.rs`, `hud.rs`, `scene_view.rs` or
  `materials.rs`. The rework is free at the source level.
- **Small commits, one concern each, each producing something that runs** (`CLAUDE.md`, style).
  A 3,000-line editor commit that does not run is the failure mode brief §7.12 names.
- **`cargo check` passing is not done**, and the exit criterion of any phase may not be
  "it compiles" (`docs/design/LOOM-IMPLEMENTATION-ORDER.md:584`).
- **egui frame budget collapsing on large scenes is a "fix with row virtualization and rule-node
  collapsing" signal, explicitly not a signal to switch GUI frameworks**
  (`docs/design/LOOM-IMPLEMENTATION-ORDER.md:571`). This forecloses "replace egui" as a response to
  performance, without a new ADR.

---

## 3. What already exists, and what the M12 exit criteria oblige the replacement to keep

The rework replaces UI, layout and interaction while keeping SceneOps, `.loom`, the CLI and the
agent API (the user's decision 1). The M12 exit criteria remain in force and the replacement must
re-meet all three (`docs/design/LOOM-BUILD-BRIEF.md:284-286`):

1. the same edit made by hand and by agent produces an identical diff;
2. a twelve-op agent transaction undoes in one Ctrl+Z (pinned as a test:
   `crates/loom_scene/src/edit.rs:457`);
3. an edit made while the agent writes the same file is rejected with a reload prompt rather than
   silently clobbering either side.

The gesture test (`edit.rs:498 a_dragged_gesture_is_one_undo_step`) and
`edit.rs:514 two_gestures_stay_two_undo_steps` are the other two that must keep passing.

Current shape, for reference: the editor is `crates/loom_cli/src/{run.rs, panels.rs, gizmo.rs,
hud.rs, scene_view.rs, materials.rs}` — roughly 4,400 lines — over `loom_render`'s `Ui` and
`Viewer`. egui 0.35.0 / egui-ash-renderer 0.12.0 / egui-winit 0.35.0 / winit 0.30.13, all pinned
in `crates/loom_render/Cargo.toml:9-21`. **egui draws over the scene in the same pass, into the
swapchain image** (`crates/loom_render/src/ui.rs:1-10`) — there is no render-to-texture viewport
today, which is Phase 7's E1 and the largest structural change the Unity-like layout implies.

Two inherited facts worth knowing before designing panels:

- **24 component types are registered by hand** in one function
  (`crates/loom_scene/src/components.rs:1695-1723`). ADR 0004 set the revisit threshold at
  "roughly 20 component types, or the first time a type is added and someone forgets to register
  it" (`docs/decisions/0004-schemars-instead-of-a-reflect-derive.md:60-65`). **That threshold has
  been passed.** The fix it names is additive — a small derive emitting only a registration entry
  via `inventory`/`linkme` — and needs no consumer change, because the registry API is the seam.
- **Primitives are `["box", "plane", "sphere", "cylinder", "capsule"]`**
  (`crates/loom_asset/src/primitives.rs:10`). The user wants sphere, cube, capsule, plane **and
  quad**; `quad` does not exist and is an afternoon in `primitives.rs` plus a name in `NAMES`.

---

## 4. What requires a new ADR

Each item names the rule it collides with, why the collision is real, and the shape of the decision
that has to be recorded. **None of these should be designed around silently.**

### A. UV texture painting — writes binary data that the undo stack cannot see

**Collides with:** property 1 ("everything authored is diffable text"); never-do #16 by way of
`Applied::undo` being the previous *scene text* (`crates/loom_scene/src/ops.rs:126-128`,
`crates/loom_scene/src/edit.rs:314-323`); `LOOM-IMPLEMENTATION-ORDER.md:455-457` — "**Painted
regions serialize as polygons or splines, never bitmaps.** This is the round-tripping trap … a
painted mask is not diffable, so store the shape, not the raster."

**Why it is real and not pedantry.** Imported PNGs already live in `assets/textures/` in version
control, so "binary in the repo" is not itself the objection — the objection is that a *painted*
texture is authored state that changes on every stroke, and it has no representation in the scene
text. Three concrete failures follow: Ctrl+Z cannot undo a stroke (there is no text to restore);
`loom scene --tx` cannot express one, so the agent cannot author or review it, breaking property 2
and the M12 identical-diff criterion; and each stroke rewrites a content-hashed asset, invalidating
its `.meta` hash (`crates/loom_asset/src/meta.rs`) and every downstream bake.

There is also a **runtime plumbing gap**: `loom_render`'s material array is built once and uploaded
once — `Materials::new` allocates a fixed descriptor count and uploads every texture in the
constructor, and the module exposes no public update path
(`crates/loom_render/src/material.rs:135-214`, everything `pub(crate)`). Painting needs a way to
re-upload or write a storage image, which is new Vulkan surface owned by the render graph.

**The ADR must decide:** whether a paint stroke is a text op-list on a component (the voxel
precedent — diffable, undoable, agent-authorable, and re-rasterised on load), or a binary artifact
outside the undo/diff system with a stated exemption; and if the latter, what replaces Ctrl+Z, what
the agent sees, and where the bytes live relative to git.

### B. Vertex-colour painting — changes the vertex layout and has no text home

**Collides with:** property 1; never-do #16 (same mechanism as A); and the vertex format itself.
`loom_asset::Vertex` is `position/normal/uv` only (`crates/loom_asset/src/mesh.rs:12-19`). Adding a
colour channel changes the layout every pipeline and every shader reads, and `#[repr(C)]` layouts
in this project are pinned by tests on principle (brief §7.7; ADR 0021 pinned `MaterialData`'s
layout for exactly this reason, `docs/decisions/0021-...:76-81`).

Additionally, per-vertex colour is data about an *imported mesh asset*, not about a scene node — so
even if the layout change is accepted, there is no authored artifact for it to live in without
inventing one.

**The ADR must decide:** whether vertex colour is stored as a sidecar text artifact keyed by asset
id, as a component on the node, or is dropped in favour of the splat/material-layer system, which
covers most of the same use cases; plus the vertex-layout change, its shader ripple, and its golden
re-bless.

### C. Material-layer / splat painting — the mask is the exact thing Phase 7 forbade

**Collides with:** `LOOM-IMPLEMENTATION-ORDER.md:455-457` verbatim ("never bitmaps"); never-do #11's
represent-the-generator principle; property 1.

**Why it is real.** A splat weightmap is a raster by definition. The already-locked answer is to
store the *shape* — polygons, splines, or a stroke list — and rasterise on load, which is
diffable, agent-authorable and undoable through `SetField`. That answer is cheap here because
`Grass`, `Scatter` and `VoxelVolume` all already author rules rather than results
(`crates/loom_scene/src/components.rs:959-1051`; `docs/format/README.md:411-422`).

**The ADR must decide:** the stroke/region vocabulary and its schema, how it rasterises (CPU bake
like `HeightField`, or generated Slang like `loom_field` — ADR 0006 and ADR 0011 are the two
precedents and they chose differently for stated reasons), and whether the rasteriser is inside the
sim hash. If the answer is "bitmap after all", that is a direct amendment to a locked Phase 7
decision and needs human approval, not just an ADR.

### D. Decals — a new rendering path, a new component, and a TLAS question

**Collides with:** `CLAUDE.md`'s "**Adding a rendering path means adding a scene to `GOLDEN`**";
never-do #4 (the decal pass's barriers belong to the graph); ADR 0018's fixed pass ordering
(forward → tonemap → UI → CMAA2 → present); ADR 0019's "**Anything that wants to be reflected has
to become an `Object`**" (`:330-336`); and ADR 0010's no-temporal-accumulation rule if screen-space
decals are considered.

**Why it needs an ADR rather than just code.** Where the decal draws decides three things at once:
whether it is anti-aliased (the MSAA resolve happens before the UI and after the forward pass —
rain draws after the resolve and is the one thing in the frame with no AA at all,
`docs/decisions/0017-...:151-160`), whether it appears in reflections (only meshes are in the
TLAS), and whether it is in HDR or display-referred. Those are not tuning choices.

**The ADR must decide:** projected-in-the-forward-pass vs a deferred screen-space pass, the
component schema, the pass's position in the chain, its AA behaviour, and the golden scene that
covers it.

### E. New UI dependencies — docking, gizmos, icons

**Collides with:** never-do #6 (pin exactly, add with `cargo add`); the crate-boundary rules
(§2.6); and `LOOM-IMPLEMENTATION-ORDER.md:571`, which forecloses switching GUI framework as a
performance response.

A Unity-like docked layout means `egui_dock` (or hand-rolled docking); Phase 7's E3 names
**`transform-gizmo`, explicitly "not the abandoned `egui-gizmo`"**
(`docs/design/LOOM-IMPLEMENTATION-ORDER.md:434`; the research doc's reasoning is at
`docs/design/loom-pcg-and-editor.md:170`). Today `gizmo.rs` is 280 hand-written lines, so adopting
the crate is an addition, not a swap. "Sleek and good-looking" will also want an icon font or SVG
set, which is a new binary asset class and a licence question.

**The ADR must decide:** the exact pinned versions and their egui-0.35 compatibility, which crate
each dependency lands in (egui-adjacent code must not pull `ash` outside `loom_render*`), whether
the hand-rolled gizmo is replaced or kept, and the asset/licence answer for icons and fonts.

### F. Stripping the editor from the runtime build — a crate split the repo does not have

**Collides with:** the repo layout, which is the *plan* rather than reality. Brief §3 lists
`loom_editor/` (`docs/design/LOOM-BUILD-BRIEF.md:107-108`), but no such crate exists — the editor
lives inside `loom_cli` and egui is an unconditional dependency of `loom_render`
(`crates/loom_render/Cargo.toml:11-13`). **So today every runtime build links egui and the editor.**

The research doc already recommends the shape — "separate crates + Cargo feature flags … Prefer a
separate crate over `#[cfg(feature)]` sprinkled through runtime crates — it makes the dependency
boundary CI-checkable" (`docs/design/loom-pcg-and-editor.md:173`) — but adopting it changes the
crate graph, adds a rule to `scripts/check-deps.sh`, and requires deciding what the shipped runtime
binary *is*, since `loom` is currently one binary carrying validate/render/sim/run/play together.

**The ADR must decide:** the crate split (`loom_editor`, and whether a separate `loom_runtime`
binary exists), how egui becomes optional in `loom_render` without a second render path (ADR 0018's
"the viewer's scene image is now unconditional" is a warning against feature-gated render paths),
and the new CI rule that keeps the boundary honest.

### G. Windows cross-compilation from Fedora

**Collides with:** `rust-toolchain.toml`, which pins one target; `.cargo/config.toml`, which
configures clang+mold for that target only; brief §7.14 ("NVIDIA-only blindness … accept it
deliberately rather than accidentally … If distribution ever matters, budget real time; if it never
does, note that in an ADR"); and never-do #14 (no portability abstraction — Vulkan on Windows is
fine, but a compatibility layer is not).

Concrete unknowns the ADR must resolve rather than discover: `slangc` runs in `build.rs` and must
still run when cross-compiling (`crates/loom_render/build.rs`); `loom_audio` pulls `cpal` and thus
ALSA on Linux and WASAPI on Windows; `ash` loads the loader at runtime, `ash-window`/`winit` need
the Windows backend; `spirv-val` must be present; and there is no Windows machine here to test on,
so the ADR must state what "supported" means when it cannot be verified. Brief §7.14 asks for
exactly this to be written down.

**The ADR must decide:** target triple (`-gnu` vs `-msvc`), how shaders and assets are laid out in
the shipped folder, what is tested and what is merely built, and whether the golden gate runs at
all on the second target.

### H. Projects, the Hub, and templates — a new authored artifact class

**Collides with:** property 1 (a project manifest is authored state, so it must be diffable text,
schema-validated on load); `docs/format/README.md` §9's stability guarantees, which govern any
format this project ships; and the absence of any project concept in the CLI today — there is no
`loom new` (verified against `crates/loom_cli/src/main.rs:36-176`) and no notion of a project root.

Also unresolved: a Hub's *recents list* is user state, not project state, so it does not belong in a
project file at all — it needs a location (XDG config), and that is a first for this repo.

**The ADR must decide:** the project manifest's schema and whether it lives in `docs/format/`
alongside `.loom` (it should, by §9's logic); what a "template" is — a directory copied, or a
prefab/`extends` chain, given `extends` already provides scene inheritance
(ADR 0008); where the recents list and editor preferences live; and whether `loom new` becomes a
CLI subcommand, which property 2 argues it must.

### I. The render-to-texture viewport — restructuring where the frame lands

**Collides with:** ADR 0018's pass ordering and its consequences paragraph on offscreen/window
divergence (three defects already paid); never-do #4 (the scene-image → egui-sample dependency is a
graph edge); and the format/colour trap the research doc names — "egui 'strongly prefers UNORM
render targets' … mismatch your viewport image's colour space and it looks subtly washed out; also,
resizing the dock tab must resize the offscreen image or you get blur/aspect errors"
(`docs/design/loom-pcg-and-editor.md:157`).

This is Phase 7's E1 and it is the one structural change the Unity-like layout forces. It is
borderline whether it needs its own ADR or is simply implementation under never-do #4 — **it needs
one**, because it changes what the window renders relative to `loom render`, and that equivalence is
what the golden gate's authority rests on.

**The ADR must decide:** where the viewport image sits in the chain (before or after tonemap and
CMAA2), how resize is handled without validation errors, and what test proves the window and the
offscreen path still agree.

### J. Anything that turns an editor gesture into engine state that is not a `SceneOp`

Stated as a general rule so the design phase catches the cases this survey did not enumerate:
**terrain sculpting is safe** (it appends to `VoxelVolume.ops`, which is text — `SetField`), and so
are placement, transforms, component edits, prefab operations and scatter/grass parameter scrubbing.
**Anything else — creating an asset, importing a mesh, renaming a file, editing a script, changing
project settings — is outside the scene text and therefore outside undo.** Each such action either
gets a text artifact and a `SceneOp`-equivalent, or it gets an explicit, documented exemption. The
design phase should produce that list; this survey establishes that the list must exist.

---

## 5. What does *not* need an ADR

Recording these so the design phase does not manufacture approval work.

- **Replacing the editor UI wholesale.** No `// STABLE` markers exist; never-do #13 does not fire.
- **A Unity-like panel layout, a command palette, an activity feed, batch approval, a divergence
  banner, jump-to-change, diff review in the viewport.** All are Phase 7 E1–E8 and already planned
  (`docs/design/LOOM-IMPLEMENTATION-ORDER.md:428-447`); the two decisions worth locking were
  already locked there.
- **Primitive creation, including adding `quad`.** `primitives.rs` already has the other four;
  `SpawnNode` already takes a mesh alias (`crates/loom_scene/src/ops.rs:49-55`).
- **A base scene for new projects, and sample first/third-person templates.** These are `.loom`
  files; `assets/games/proving_ground.loom` is the existing proof that a whole game is one.
- **End-user documentation.** Prose in `docs/`.
- **Making the component registry auto-registering.** ADR 0004 pre-authorised it as additive at
  exactly the threshold now passed (`docs/decisions/0004-...:60-65`).
- **Row virtualization and collapsed rule-nodes in the hierarchy.** Named as the correct response to
  egui frame-budget pressure (`docs/design/LOOM-IMPLEMENTATION-ORDER.md:571`).

---

## 6. Open questions inherited, not created

- **ADR 0003 (knowledge graph) is still `proposed` and needs a human decision.** M12's
  knowledge-graph view depends on it, and ADR 0003 says plainly that if the index is never built,
  "that clause in M12 must be struck too — a view over a nonexistent index is not a feature"
  (`docs/decisions/0003-knowledge-graph-deferred.md:62-63`). **The editor rework should either
  drop the knowledge-graph view from scope or force the decision.**
- **ADR 0001's human approval is recorded as pending** (`docs/decisions/0001-rust-edition-2024.md:44`).
  Nothing in this rework depends on it.
- **The AA question P2 exists to answer is still open** (ADR 0010: exit criterion 2 "still not
  met"). It does not constrain the editor's UI, but it does mean the viewport is not a settled
  visual target and the editor should not bake assumptions about final image quality.
