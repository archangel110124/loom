# The editor rework — the plan the build follows

*Synthesis of `00-survey-*`, the seven round-1 designs `01`–`07`, the four round-1 reviews `08-*`,
the four round-2 designs `09`–`12`, the two round-2 reviews `13-*`, the two round-3 designs
`14`–`15` and the round-3 review `16`. Where two documents disagree this document says which wins
and why; where a review's objection is correct the plan changed and says so; where a review is
wrong it says that too. **This file supersedes the build orders, stage numbers, ADR numbers, file
lists and conflicting decisions inside `01`–`16`.** Those documents remain the reasoning; this one
is the instruction.*

*Design phase — no `cargo` command was run. Every `file:line` below was read in this worktree at
`62f9ebe` with `rg`/`sed`/`grep`, and §6 lists what could not be checked without building.*

---

## 0. What round 2 changed, in one page

Round 1 produced ten stages and eleven ADRs. Then the user made seven decisions that round 1 could
not have known:

1. **The engine repo must remain a valid project** — the 43 gated scenes and both gates keep
   working — *and* standalone project folders must work. → §2.11, ADR 0036, Stage 5.
2. **"Paint textures" means terrain-layer painting** (sand/dirt/grass across a landscape), not
   Substance-style UV painting. → splat painting moves ahead of UV painting; Stage 7 before 10.
3. **Paint down actual 3D grass, and place vegetation in bulk** — Unity Detail, Unreal Foliage. →
   a whole new stage, Stage 8, ADRs 0039–0041.
4. **Worlds are built from geometry the user creates, and the AI must shape it too.** → sculpt
   (Stage 9) and the create/arrange toolkit (Stage 4) both keep the rule that every editor verb has
   a CLI equivalent that lands first.
5. **The agent is a first-class docked panel.** → Stage 6, ADRs 0034/0037/0038, and the one real
   defect it exposed: *an agent write silently destroys the human's undo stack today*.
6. **Bespoke visual identity.** → Stage 3, ADRs 0033/0035, and three cheap places where the "warp"
   metaphor becomes pixels rather than prose.
7. **Strangers will use it eventually.** → onboarding is not a stage, it is a clause of every
   stage: empty states, refusal messages that name the fix, and one-click repairs.

**Both round-2 reviews found the same structural failure and they are right:** four documents
written in parallel each claimed ADR 0033, nobody owned any combined number, and two documents
amended the same Stage-3-fixed `Tab` enum in incompatible directions. §3 allocates every number
once. §2.8 owns the gate counts. §2.6 owns the union list. §2.12 owns the state directory.

**One correction to round 1 that both reviews inherited without checking.** `PLAN.md` §3 said
*"Twelve ADRs — that is the approval budget"* and its table has **eleven** rows (0022–0032). Both
round-2 reviews therefore computed the new total as twenty-one. **It was twenty** — eleven from
round 1, nine from round 2, numbered 0022–0041. Round 3 adds one, so **it is twenty-one, 0022–0042**
(§0.1), plus ADR 0003 moving from `proposed` to `accepted`, which is a status change to an existing
file and not a new number.

---

## 0.1 What round 3 changed, in one page

**ADR 0003 has been accepted.** The knowledge graph — deferred since 2026-07-30 for want of a
project big enough to have a cross-file question — is being built. The ADR's own revisit condition
was *checked, not assumed*: it argued *"the project will not have 200 files at M9"*, and
`git ls-files assets docs crates | wc -l` is **288** today. **§3 records the status change; this
plan does not rewrite ADR files, and `docs/decisions/0003-knowledge-graph-deferred.md` still says
`proposed` until someone edits it.** That edit is the first commit of Stage 12.

Three documents arrived: `14` (the index), `15` (the editor surface) and `16` (the review). **The
review's first finding outranks all its technical ones and it is right: `14` and `15` are not two
views of one design.** They claim the same two ADR numbers with incompatible content, name two
different `Tab` variants, specify two incompatible stores, two node models, two CLI grammars, two
state directories and two opposite determinism postures — and `15` rejects `14`'s store *by name, on
four numbered grounds*, without knowing `14` exists. That is round 2's structural failure repeated
in the round whose whole job was to not repeat it. §2.16 arbitrates, in the shape §2 already uses,
and allocates **one** ADR rather than two.

**Four of the review's findings were re-verified here against the source and all four hold:**

1. `crates/loom_scene/src/components.rs:1036` — `ScatterExclude.field` is *"The node path of an
   earlier `Scatter` field"*, used at `forest.loom:143`. Doc 15 §0 states as *measured fact* that no
   component field anywhere holds a node path, and cuts scene nodes, the hierarchy surface and the
   whole intra-file half of its model on that basis. Its own stated reversal trigger had already
   fired.
2. `components.rs:964` — `Scatter.mesh` is a bare `String` holding *"an `[[asset]]` alias, or a
   primitive name"*. It is not an `AssetRef`, it has no extension, and in the schema it is
   byte-identical to `Name.value`. **Both** extractors are blind to it and **both** guard tests pass
   anyway.
3. `clippy.toml` sits at the workspace root and names `HashMap`, `HashSet`, `rand::thread_rng` and
   `Instant::now`; green check 1 is `--workspace`. Doc 14 §7's *"uses both freely, and that is
   correct, because clippy's ban is scoped to simulation code"* is a green-check-1 failure.
4. `git ls-files '*.png' | grep -v '^assets/'` is **28** reference PNGs under `tests/`, all inside
   ADR 0023's walk. Doc 14's `--orphans` reports every one of them, sorted by size, at the top of a
   list whose named consumer was a delete button.

**And the trap ADR 0003 named by name is present, wearing doc 14's hat rather than the
force-directed one.** Both documents cut the force-directed view and both are right to — doc 15
§4.1 is the strongest page in either. Doc 14 then spends the saved week on a SQLite schema, WAL,
`BEGIN IMMEDIATE`, a `user_version` policy, an mtime/size/hash ladder, an incremental
`DELETE`-by-owner protocol and a `--verify` harness whose only reason to exist is that protocol —
**every one of them a consequence of deciding there is a persistent store, taken before the one
measurement that says whether a store is needed.** That measurement is scheduled in the same
document, in the first commit of the stage. Stages 8 and 9 exist to forbid exactly that ordering,
and doc 14 quotes their discipline approvingly on the way past it.

**So the graph is built at roughly a fifth of what was proposed** — §2.16 — and the reasons are in
§5's new cut rows with a trigger each. **Two of the review's own conclusions are corrected**, both
in §2.16: its `Scatter.mesh` fixture does not exist, and its "not a stage, a slice" is overruled on
presentation grounds only.

---

## 0. ON HOLD — the VFX overhaul took priority

**Paused after Stage 3, 16 Aug 2026, by the human's decision.** Fire, smoke and water are
being overhauled to become a headline feature of the engine; that is engine-roadmap work
(P3 water and a fire rework) and is tracked by `../VFX-IMPLEMENTATION-REPORT.md` and
`../NIAGARA-AND-FIRE-RESEARCH.md`. This document resumes when that lands.

**Where it stopped.** Stages 0-2 complete. Stage 3 is substantially complete — viewport
coordinates, the applied theme, and `egui_dock` with the carve are all in and gated. It owes
four things, none structural: the Window menu, maximise-on-hover, `icons.rs`, and real bodies
for the four placeholder tabs (Problems, History, Prefabs, Agent). Stages 4-12 are untouched.

**Read `MANUAL-CHECKS.md` before resuming.** Everything Stages 1-3 shipped is gated and none
of it has been looked at by a human — no gate in this project has ever seen a pixel of the
editor. The dock in particular has a failure mode no gate can detect: if the carve is subtly
wrong, viewport clicks silently stop selecting.

---

## 1. The editor in one page

**What it is.** A Unity-shaped, docked, dark-themed editor for Loom, launched as `loom edit`
(a hub with no argument, a project or scene with one) and still reachable as `loom run --edit`.
It has a hierarchy, an inspector generated from the type registry, a scene viewport that is a
rectangle rather than the whole window, a project browser, a console, a transaction history, a
problems panel, **a docked agent panel**, transform gizmos, a create menu, snapping, voxel
sculpting, prefab authoring, **foliage painting**, and three of the four painting systems reachable
from a brush that behaves the same way in all of them.

**What it replaces.** Everything in `crates/loom_cli/src/{panels.rs, run.rs}` above the model
layer: the six-panel overlay, the flat `UiAction` list, the drag state machine, the layout (there
is none), the theme (there is none). Roughly 3,200 of the current 4,400 editor lines.

**What it keeps, untouched.** The parts that carry the project's guarantees, and all of them are
below the UI:

| Kept | Where | Why it must not move |
| --- | --- | --- |
| `SceneOp` → `Transaction` → `Session::apply` | `loom_scene/src/{ops,edit}.rs` | never-do #16. Every editor action is still ops. |
| `Session::apply_coalescing` + `gesture_epoch` | `edit.rs:282`, `run.rs:898` | one gesture, one Ctrl+Z. |
| Version tokens, the sidecar lock, atomic write | `edit.rs:37-136`, `ops.rs:196` | never-do #15. Reject and reload; never merge. |
| `transact` / `transact_as` — the one write path | `run.rs:1707-1756` | moves verbatim, comments included. |
| `TypeRegistry::describe` as the inspector's source | `loom_reflect/src/lib.rs:46` | a new component type still costs zero UI. |
| `SceneView` re-derivation and its keyed uploads | `scene_view.rs`, `run.rs:547-619` | the window is a live view of the file. |
| `gizmo.rs`'s single shared projection | `gizmo.rs:8-10`, test at `:211-224` | picking and the gizmo cannot drift apart. |
| The `.loom` format, `loom-mcp`, **and every existing CLI exit contract** | — | the agent's surface does not change. |
| The fourteen "must not lose" behaviours | `00-survey-existing.md` §14 | each was a bug once. |

That last row is now load-bearing rather than decorative, and it is why ADR 0038 changed shape
between doc 09 and this plan: the round-2 constraints review is right that making `loom scene --tx`
stop applying a class of transaction **by default** is a change to the CLI's contract, not an
addition to it, and every script that reads exit 0 as "applied" becomes silently wrong. §3's ADR
0038 gates on `LOOM_AGENT=1` instead. See H2 in §2.13.

**What it adds to the engine.** `SpliceArray` / `Declare` / `SpawnNode{prefab}` (nine ops →
eleven), `loom_scene::brush`, `loom_asset::paint`, `loom_scene::journal`, `SplatPaint`,
`PaintLayer`, `FoliagePaint`, `Decal`, `Scatter.remove`, `Scatter.align`, `Camera.boom`, `quad` and
`box_atlas` primitives, a `loom.toml` project manifest, `loom edit` / `loom new` / `loom ship` /
`loom docs` / `loom context` / `loom propose` / `loom foliage stats` / **`loom graph`**,
`loom render --eye --look`, a `loom-play` runtime binary, a `ViewportPlacement` in `loom_render`,
and **`loom_graph`** — a derived reverse index over the project's own text, and the one thing here
that answers a question no existing command can ask.

**The acceptance criterion, restated.** The brief is "sleek, good-looking, and **above all easy to
use**." Two round-1 reviews independently found that the design set spends its length furthest from
the user and is silent nearest to them — nobody designed the inspector, which is the surface a
person touches more than everything else combined, and which today renders a third of this engine's
authored surface as read-only text. **So the inspector is Stage 1, ahead of the docking work**, and
the docking work is deliberately not first even though it is the most specified.

---

## 2. The resolved architecture

### 2.1 Crates

```
loom_reflect ── loom_scene ──┬── loom_asset ──┬── loom_render ──┬── loom_editor
  (nothing)     (+project,   │   (+paint)     │   (egui, ash)   │   (all editor UI)
                 +journal,   │                │                 │        │
                 +brush)     ├── loom_physics ┴── …             └────────┴── loom_cli
                             │                                     │       ├─ bin loom      (default features)
                             └── loom_graph ──────────────────────┘       └─ bin loom-play (--no-default-features)
                                 (Stage 12; loom_scene only)
```

**`loom_editor` is created in Stage 1**, before any new UI is written, so nothing is written into
the wrong crate and then moved. Stage 1 moves `panels.rs` and `gizmo.rs` wholesale plus the
`UiAction` enum; `run.rs`'s winit/camera/play half stays in `loom_cli` until Stage 5, when
`loom-play` forces the runtime/editor line to actually be drawn. Splitting a 2,312-line file is the
expensive part of the split and it is deferred to the moment something needs it.

`loom_editor` imports no `ash` — it reaches egui through `loom_render`'s re-export
(`loom_render/src/lib.rs:64`), the pattern `panels.rs:17` already uses. `egui_dock` and any icon or
font dependency land in `loom_editor`'s manifest, **never in `loom_cli` or `loom_render`**. Doc 01
§2.1's plan to pin `egui` directly in `loom_cli` is overturned: doc 06 §6.6 would make that a
green-check failure on the day it lands.

**Two edges round 2 forced, both verified against `scripts/check-deps.sh:26-31`:**

- **`BrushParams` lives in `loom_scene::brush`, not `loom_asset::paint`.** Doc 10 §11 caught this
  and it is correct: `crates/loom_scene/Cargo.toml` lists `blake3`, `loom_reflect`, `schemars`,
  `serde`, `serde_json`, `toml_edit` and the check script fails the build on any other workspace
  edge. A `SplatStroke` component in `loom_scene` embedding a `BrushParams` from `loom_asset` is
  green check 1 failing on the day ADR 0027 lands as written. `BrushParams` is authored state —
  serialized, schema-validated, shown in an inspector — so `loom_scene` is where it belongs, and
  `loom_asset::paint` (the rasteriser) takes `loom_scene` as a dependency to read it. **This
  correction is applied to §2.3's S4 row and to ADR 0027's decision text, not left in doc 10**,
  because whoever implements Stage 7 has no reason to open a foliage document. The round-2
  constraints review's M8 is procedurally right about that and this paragraph is the fix.
- **`loom_asset` does not gain `loom_field`.** Verified: `crates/loom_asset/Cargo.toml` lists
  `blake3`, `gltf`, `png`, `serde`, `serde_json`, `uuid`. Doc 10 §2.3's edge break-up needs
  low-frequency noise; the baker takes `noise: &dyn Fn(f32, f32) -> f32` and `loom_cli` passes
  `loom_field::noise::value` — the same closure seam `loom_grass::tile` already uses
  (`lib.rs:315`). Zero new edges, and it is this design's own pattern used a third time.

### 2.2 How the editor is stripped for a runtime build — doc 06's ADR A is wrong and is replaced

**Verified, and it kills doc 06 §1 as written:**

- `crates/loom_cli/src/hud.rs:16` is `use loom_render::egui;`. The module builds `egui::Align2`,
  `egui::FontId`, `egui::Color32` and paints into `&mut egui::Ui` at `:137`. **The HUD is game
  content** — a scene component, drawn during Play, the thing `proving_ground.loom` demonstrates.
- `crates/loom_render/src/viewer.rs:922-924` — `pub fn draw(…) { self.draw_with_ui(objects, &[],
  camera, None, |_| {}) }`. **`draw` is a one-line wrapper.** Feature-gating out `draw_with_ui`
  does not remove a branch; it removes the only implementation of drawing a frame and obliges
  someone to write a second one — which is exactly the offscreen/window divergence ADR 0018 says
  this project has paid three defects for.

Doc 02 §6 found both of these and wrote them down for whoever wrote ADR F; doc 06 was written
without reading it. Three reviews independently confirm. **The resolution:**

> **egui stays an unconditional dependency of `loom_render`, and "stripping the editor" means not
> linking `loom_editor`.** No `editor` feature on `loom_render`. No `#[cfg(feature)]` on
> `draw_with_ui` or on the `ui` render-graph pass. The shipped binary links egui **because the HUD
> is egui**, and if binary size ever matters the fix is to stop drawing the HUD with egui, not to
> feature-gate the renderer.

Two new rules in `scripts/check-deps.sh`, in the shape of the existing `loom_agent` rule
(verified at `scripts/check-deps.sh:33-44`):

1. Nothing but `loom_cli` may depend on `loom_editor`.
2. `cargo tree -p loom_cli --no-default-features -e normal` must not mention `loom_editor`,
   `egui_dock`, or any icon/font crate.

`loom_cli` gets `[features] default = ["editor"]`, `editor = ["dep:loom_editor"]`. **The `loom`
binary carries no `required-features`** — doc 06's `required-features = ["editor"]` would have made
`loom validate`, `loom render`, `loom sim` and `loom scene --tx`, i.e. the entire agent surface and
everything `cargo xtask` drives, unbuildable without the editor. The `edit` subcommand and
`run --edit` are `#[cfg(feature = "editor")]` and print a one-line refusal otherwise.

**One consequence round 2 added and it belongs here, not in a theme document.** The sRGB encode fix
(ADR 0033) lands in `crates/loom_render/src/ui.rs`, which `loom-play` links, and the compensating
pre-warp must therefore live beside it — `loom_render::ui::tok`, a plain function — **not in
`loom_editor::theme`**. Doc 11 put `tok` in the editor and the round-2 constraints review's H7 is
right that this gives a shipped game's HUD the uncorrected half of a two-part fix, permanently,
with no code path that can reach the correction. This is not an editor palette in `loom_render`
(which ADR 0022 forbids and doc 11 §14 correctly rejects); it is a colour-space correction owned by
the module that sets the specialization constant. The editor's token table calls it.

### 2.3 The seam list — every unowned seam, with an owner

| # | Seam | Resolution | Owner | Stage |
| --- | --- | --- | --- | --- |
| S1 | `fragmentMain` composite order | `base albedo → UV paint → ground layer (splat-biased) → decals → wet/light/fog`. Written as the shader comment it becomes. Vertex colour is cut, so its undefined position evaporates. **Foliage adds nothing here — the foliage mask never reaches a shader** (§2.10). | ADR 0027 | 7 |
| S2 | GPU byte budget | `ObjectData.material = [material_index, splat_slot, paint_slot, decal_range]`; `ObjectData.splat: [f32;4]` appended (240→256); decals as a device address + count in `EnvironmentData`. The scene push block is **not touched** (§2.4). One doc comment on the Rust struct, mirrored in `scene.slang`. | ADR 0027 | 7 |
| S3 | Paint gesture contract | §2.5 below. One model for all four brush tools — splat, UV, sculpt and foliage. | ADR 0027 | 7 |
| S4 | One `Stroke` and one brush | **`loom_scene::brush::BrushParams { radius_m, hardness, strength, flow, spacing }`** (§2.1), embedded in every stroke type; the rasteriser is `loom_asset::paint`; **radius always world metres**; erase is `strength = 0`; typed, `JsonSchema`, no `serde_json::Value`. | ADR 0027 | 7 |
| S5 | The union "outside undo" list | §2.6 below. One table, and `docs/guide/05-you-and-the-agent.md` documents it. | ADR 0031 consequences | 4 |
| S6 | `Materials` live update | One path: the `paint_upload` render-graph pass (doc 04 §3.2). Doc 03 §6's one-shot submit is deleted. A minimal `Viewer::set_materials` rebuild lands earlier, in Stage 1, so the inspector's colour picker is real. | `loom_render` | 1, then 7 |
| S7 | The prefab load-path bug | **§2.14 — verified live, one line, Stage 0**, with the regression test ADR 0008 asks for. | Stage 0 | 0 |
| S8 | `loom.toml` schema, reader, crate | Doc 02's key names, `loom_scene::project`, one reader. **The struct carries `ship: Option<Ship>` and `agent: Option<Agent>` from the first version that ships** — `deny_unknown_fields` means a manifest carrying a table the struct lacks *fails to load*, and the engine repo's own checked-in manifest would not open in an editor built before Stage 6. Doc 06's second reader is deleted; doc 07's `project.toml` spelling is deleted. | ADR 0023 | 5 |
| S9 | Editor preference and layout storage | §2.12. Everything user-global under `$XDG_STATE_HOME/loom/`, **one path-keying helper**, nothing written into the project directory. | ADR 0023 | 3 |
| S10 | ADR numbering and total budget | §3. **Twenty-one ADRs, 0022–0042**, allocated here, before any is written; twenty from rounds 1–2 and **one** from round 3, against the two each round-3 document claimed. | this file | — |
| S11 | The visual language | §2.7, and doc 11 is its specification with the corrections §2.7 lists. | ADR 0030, 0033, 0035 | 0, 1, 3 |
| S12 | Total gate cost | `SCENES` 43 → **51**, `GOLDEN` 28 → **34**. No fifth green check. §2.8. | this file | — |
| S13 | Which binary the gate drives, and how many windows | Unchanged: `xtask validate` drives `loom run --edit --frames` and `loom run --edit --play --frames` — **five windows, same as today**. `loom-play` is smoke-tested by `loom ship`, not by the gate. **`--frames` forces scene-only mode** (§2.11). | Stage 5 | 5 |
| S14 | The shipped folder | The **whole project root** minus `builds/`, `out/`, `target/`, `.git/`, **any root entry whose name begins with `.`**, `*.meta`, `assets/shaders/`, plus `[ship] exclude` — not `assets/**`. No editor binary. No `docs/`. | ADR 0032 | 5, 11 |
| S15 | Play + a concurrent agent write | Unchanged from today: the watcher sleeps while Play runs (`run.rs:972-974`) and `stop_play` re-arms it. The docked Game tab inherits it. The paint, foliage and sculpt tools are inert during Play, with the rest of the editing keys (`run.rs:2042-2044`). A build subprocess touches no authored state. | Stage 3 | 3 |
| S16 | Can the painting systems coexist on one node? | Yes, and the composite order in S1 defines the result. `loom validate` warns (never errors) when a node carries both `SplatPaint` and `PaintLayer`, because the two answer the same question through different mechanisms. **`FoliagePaint` is not in that warning** — it answers a different question (where things are *placed*, not how the ground is *shaded*) and composes with splat multiplicatively through `Ground.rock` and `Ground.paint` with no ordering question. | ADR 0027, 0039 | 7, 8 |
| S17 | Where the agent panel is docked | §2.9. **A tab of the bottom node**, not a right-column tab (doc 09) and not a right-column split (doc 11). Both are overruled and §2.9 says why. | this file | 3 |
| S18 | Where the foliage palette lives | **No `Tab::Foliage`.** Tool-scoped UI, in the shape the sculpt brush already takes. §2.9. | this file | 3, 8 |
| S19 | Whose policy the agent obeys | §2.13 H1. `command`/`preamble` are **user** state; `approve`/`approve_above_nodes` may appear in `loom.toml` **only as a tightening**. A project can never loosen. | ADR 0038 | 6 |
| S20 | Which of `14` and `15` is the knowledge graph | **Neither as written** — §2.16 arbitrates clause by clause and names which document each clause came from. **One ADR (0042), not two.** | this file | 12 |
| S21 | Who resolves an alias to a path | **One function, shared with the loader.** `main.rs:1146-1170`'s ladder — split `#Object`, primitives first, then the declaring scene's `[[asset]]` table — moves down beside `Scene::asset_path` in `loom_scene` and is called from both `loom_cli` and `loom_graph`. An index owning a second copy is ADR 0006's divergence class in the crate whose only job is to be right about the first. | ADR 0042 | 12 |
| S22 | State versus cache under `$XDG_*` | **State holds what the user meant; cache holds what we can recompute.** §2.12. Layouts, prefs, the journal, proposals and transcripts are state; thumbnails and any graph persistence are cache. Doc 14 and doc 15 put the index in different ones and the rule settles it rather than the file extension. | ADR 0023 consequence | 12 |

### 2.4 The scene push block is not touched — doc 03 cited the wrong struct

Doc 03 §7 spends the last push-constant slot on a `uint* vertexColors`, citing
`rain.rs:717-718`'s `assert_eq!(size_of::<Push>(), 120)`. **Verified: those are two unrelated
structs.** `rain.rs:78` is the rain compute pass's `Push`; `renderer.rs:608` is the scene pass's,
and its own doc comment reads *"The block is at 124 of its 128 bytes with this … There is room for
nothing else here."* **Also verified: there is no size test for the scene `Push` at all**, despite
that struct's doc comment claiming *"the sizes are asserted in a test."*

Two consequences, both adopted:

- Vertex-colour painting is **cut** (§5), so nothing needs the slot.
- **Stage 0 adds the missing `assert_eq!(size_of::<renderer::Push>(), …)`.** A doc comment claiming
  a test that does not exist is how the next person makes the same mistake.

Anything that later needs scene-global data goes in `EnvironmentData`, which is where wind, the
camera position, the terrain height pointer and the wave set already are, and where the decal list
goes — the fifth instance of an established pattern.

### 2.5 The paint gesture contract (S3), resolving the sharpest conflict in the set

Doc 03 commits during the drag through `apply_coalescing`; doc 04 commits once on release and lets
the viewport diverge. They differ observably on five cases. **Doc 04's model wins** — it is less
write pressure, not more, and doc 03 §2 admits it never measured its own payload. But doc 04's
divergence has to be a clause of the ADR rather than a sentence in a design doc:

1. **One stroke is one `SetField` (or one `SpliceArray` append), in one `Transaction`, committed on
   mouse-up.** No gesture coalescing for paint. Label: `"Paint quay_wall: 1 stroke, 34 points"`.
2. **The in-progress preview is the only editor state permitted to diverge from the scene text.**
   It is CPU-side, uploaded as a dirty rect, and closed at mouse-up.
3. **`dirty` is set on commit, not on press.** Verified as necessary: `00-survey-existing.md` §14
   item 3 records that a latched `dirty` made the viewport stop following the file and made "Keep
   mine" write back stale text. An unbounded in-progress stroke re-arms exactly that trap.
4. **Escape cancels**: the preview is discarded and nothing is written.
5. **A stale-version rejection holds the stroke and raises the divergence banner with a third
   button — "Re-apply my stroke".** This overturns doc 04 §4.1's "the stroke is lost". The usability
   review is right: `CLAUDE.md` says *"Expect version-token rejections and handle them by re-reading
   and re-applying, not by forcing the write"* — re-applying an action the user just performed,
   against text they can see, with them choosing, is the prescribed handling. An auto-merge is
   silently reconciling two divergent states; this is neither silent nor a reconciliation.
   "Reload from disk" still discards the stroke, with a console line.
6. **Undo mid-stroke**: no transaction exists yet, so Ctrl+Z undoes the previous entry and the
   preview is discarded.

Doc 03 §2 is deleted wholesale. Its `spacing`-based decimation survives as a *preview* and *bake*
parameter, which is what it was always for.

**The correctness gate for the whole of painting** is one test:
`incremental_painting_equals_a_full_rasterisation` — paint 40 strokes incrementally, rasterise the
same 40 from scratch, assert byte-identical including every mip level. If it cannot be made to
pass, the preview drifts and the model changes. **Foliage inherits it rather than restating it**:
the foliage mask is a pure function of the same stroke list through the same dab walker, which is
what makes tile-level regeneration byte-identical (§2.10).

**One clause round 2 added, and it is a correctness clause rather than a nicety.** Doc 10 §2.1
promises *"a painter who erases grass gets no grass"* while doc 10 §4 makes `flow` accumulate
authority over repeated dabs — so one confident swipe at the default flow leaves authority 0.6,
`lerp(1, 0, 0.6) = 0.4`, and 40% of the grass survives. The round-2 usability review is right that
this reads as a broken eraser. **The `Clear` preset sets `flow = 1.0`** (feathering an erase is what
`radius` and `hardness` are for), the brush ring fills to show accumulated authority under the
cursor, and the promise is restated honestly: *erase is exactly zero at full authority, and the
ring shows when you have it.*

### 2.6 What is not a `SceneOp` — the union list (S5)

The constraints survey §4.J asked for one list. Here it is, merged from doc 02 §10, doc 05 §14,
doc 06 §5, doc 09 §8, doc 11 §3 and doc 12 §2, plus the anomalies nobody listed. **This list is
extended in place; a future document adds a row here rather than shadowing it.**

**Ephemeral, discarded on reload, never written:** selection; isolate/hidden sets (with a persistent
"3 objects hidden · Show all" bar, because hide without a trace is a way to lose work); camera, tool
mode, snap settings; the sculpt live preview and "preview to op N"; the marquee, gizmo hover and
drag-in-progress state; **the in-progress paint or foliage stroke** (§2.5); **the agent's `State`
and the current turn's streaming buffer**; **`agent_marks`' six-second decay**, which is recency and
not authorship (ADR 0035).

**User state, outside the project:** dock layout, theme scale, recents, zoom, high-contrast,
reduce-motion, thumbnails, **the agent command and preamble** (§2.13 H1), **the conversation
transcript**, **the proposal queue**, **the editor context file**, **the scene journal**. All under
`$XDG_STATE_HOME`/`$XDG_CACHE_HOME` — §2.12 is the one table.

**A second file, and the button must say so:** creating a prefab (`prefabs/<name>.loom` is written,
then the scene transaction — Ctrl+Z restores the scene and the file stays); creating a `.rhai`
script; copying an imported mesh into the project (the `Declare` naming it **is** an op);
**"Reload, saving my version to `<scene>.mine.loom`"** on the divergence banner. Each button carries
the sentence ADR 0008 established: *"Undo restores the scene; the file stays."*

**Not authored state at all:** creating or opening a project; removing a recents row; pressing
Build; editing `loom.toml`, **including its `[ship]` and `[agent]` tables** (git is its undo — the
same answer this project already gives for terrain recipes, and the panel shows **no** undo
affordance, not a greyed one); approving or rejecting a proposal *card* (the transaction it applies
is an ordinary undo entry, but the card's disappearance is not); **building or discarding the
reference index** (§2.16 — it is derived from files that are already the truth, and the graph
proposes but never writes).

**Outside the editor entirely, and the row exists so nobody adds it:** **deleting a file.** The
Project panel has Find references, Reveal and Copy path and **no Delete**, because every protection
this project has — the version token, `Session::apply`, the one write path, the divergence banner —
protects *scene text*, and none of it reaches `unlink`. A deleted file has no token, no transaction
and no undo, and giving it one means a trash mechanism, a restore path and a second undo domain,
which is never-do #16 arriving from outside the scene. Doc 15 §2.1 is right and this is where it
lands. The editor's contribution is to tell you what a deletion would cost *before* you go and do it
in a shell.

**Refused:** the in-window script buffer. Doc 05 §12 proposes one with focus-dependent Ctrl+Z; doc
02 §10 refuses the identical construction for `loom.toml` and gives the better reason — a second
undo stack whose reachability depends on which panel has focus is the letter of never-do #16. "Open
in your editor" (`xdg-open` / `$EDITOR`) is doc 05's own primary answer and needs no second stack.
If the buffer is ever wanted it is its own ADR.

### 2.7 The visual language (S11)

**Doc 11 is the specification.** It is the implementable form of round 1's merge ruling — doc 01
§6.1's palette and font sequencing, doc 07 §10's governing rule, type scale and hand-drawn icons —
with API shapes read out of `egui-0.35.0` rather than recalled. The ruling stands:

- **Governing rule: "the chrome is greyscale; every colour in the interface is data."** The
  strongest single idea in either theme document, and one a single developer can hold while adding
  the two hundredth widget.
- **The accent is violet at ~260°**, because warm red, green and blue are the three gizmo axes
  (`panels.rs:95-99`), cyan is the agent, and blue is the Z axis — so a blue selection highlight in
  a 3D viewport is a selection a user can misread as a depth handle.
- **The agent hue stays `#78C8FF`** — `panels.rs:679`'s existing value and a meaning a user has
  already learned. Doc 07 silently changed it to teal.
- **Icons are hand-drawn `egui::Painter` geometry.** No icon font, no SVG rasteriser, no new binary
  asset class.
- **Fonts: bundled first.** Inter only if the human still reads the result as default egui after
  the palette, spacing and radius land. Candidates, licences and provenance are pinned now
  (doc 11 §4) so taking the swap is a copy rather than a fresh decision. **egui's bundled set has
  exactly one weight**, so the type scale ships weightless and headings are differentiated by size
  and `text_strong` alone.
- **W/E/R become gizmo keys**, gated on `look` (`MouseRight`, held) being down. Ctrl+K opens the
  palette; Play moves to F5.

**Eight corrections to doc 11, six of them from the round-2 reviews and all adopted:**

1. **`tok` lives in `loom_render::ui`, not `loom_editor::theme`** — §2.2, and it is the difference
   between fixing the HUD and half-fixing it.
2. **`line` `#262C35` → `#2E3540`.** Doc 01's value is 1.25:1 against `surface`, below the
   threshold at which a 1 px rule is visible at all — which would delete the mechanism the whole
   surface strategy rests on.
3. **`hover` stops sharing a value with `line`**, so a hovered row is not exactly the colour of the
   rules around it.
4. **The focus ring is `accent` (6.47:1), never `line_strong` (1.97:1)**, which fails WCAG's 3:1
   for a meaningful non-text indicator.
5. **`raised` `#1E232A` → `#232830`** (~1.35:1). Doc 11 §15.4 admits 1.1:1 may collapse to one grey
   on an eight-bit panel and offers an opt-in toggle a stranger will never find. The strategy
   depends on the hairline, not on 1.1 specifically.
6. **`text_disabled` `#4C5561` → `#6B7484`** (2.29:1 → 5.4:1). ADR 0031 mandates showing
   unavailable commands rather than hiding them, so the command palette a stranger opens on their
   first day is *mostly disabled rows*. Doc 11's exemption argument is about correctness; this is
   about whether a person can read the thing they are told to read. "Disabled" is carried on the
   icon's alpha and a right-aligned reason in `text_weak`, never on the label's luminance.
7. **Viewport marks are a three-layer sandwich**: 3.0 px `chrome_casing` α200, a 1.5 px core, and
   the outermost 0.5 px of the casing at `chrome_core` α60. Doc 11's two-layer rule is right about
   snowfields and night scenes and fails in the middle — against a mid-grey overcast sky the casing
   is 2.1:1 and the violet core is 2.3:1. Still one helper (`overlay::stroked`), still no other way
   to draw.
8. **The icon set pins the *rules* and a budget, not the list.** Doc 11 fixes sixteen icons "so the
   set does not grow by improvisation" and the four parallel documents already need at least
   foliage, send, approve, reject, render and project — twenty-two. The four geometry rules (16 pt
   box, one 1.5 pt weight taken from `WidgetVisuals`, three primitives, 2 pt sub-grid) are what
   makes them a family and they are kept exactly. **Budget: ≤ 24, and the twenty-fifth is a
   conversation.**

**And one addition, because user decision 6 asked for bespoke and doc 11 delivered competent.**
The round-2 usability review is right: `#A78BFA` is Tailwind's `violet-400`, the ground ramp is
within a couple of ΔE of VS Code Dark Modern, and the one bespoke idea — naming the 2 px edge "the
warp" — appears nowhere in pixels. The hue argument is good and is not overturned. The metaphor is
spent in the three places it costs nothing and is seen constantly, all three reusing `icons.rs`'s
primitives with no new asset class:

- **A busy indicator that is a shuttle crossing a warp** — fixed vertical hairlines with one
  horizontal segment traversing them, ~20 lines of `Painter`. It is not a spinner, it is the same
  geometry vocabulary as the icons, and it is used by the agent's `Thinking`, the terrain bake,
  `loom ship` and the thumbnail subprocess. **Nothing else in the design defines a busy state**, and
  those are the moments a user's eye has nothing else to do.
- **Threads that cross rather than stack.** Doc 11 §1 says a selected node the agent just touched
  shows both threads stacked; making them *cross* — the agent thread down the row's left edge, the
  selection thread across its top-left corner over it — turns the motif into a behaviour instead of
  a name.
- **The hub headline** gets the lattice as its one piece of ownable art, drawn and not shipped.

**`srgb_framebuffer` is settled before any hex is tuned, and it is now a fact rather than a
suspicion.** Doc 11 §2 read the shaders: `ui.rs:88` sets `srgb_framebuffer: false`, which makes
`egui-ash-renderer 0.12.0`'s shader pair an identity on the vertex colour, and `viewer.rs:2101-2102`
then selects a `B8G8R8A8_SRGB` swapchain that encodes it a second time. A `#16191E` panel displays
as roughly `#535860`; a designed 14.6:1 arrives as 6.7:1. **Stage 0 flips it and measures.** ADR
0033.

### 2.8 The gates (S12)

Nobody owned the combined number and three documents each stated a different one. Derived here,
per stage, once:

| Stage | Scenes added to `SCENES` | Added to `GOLDEN` |
| --- | --- | --- |
| 2 | — | `viewport_rect` (an existing scene through `loom render --viewport`) |
| 5 | `empty`, `first_person` | `empty` |
| 7 | `decals`, `painted` | `decals`, `painted` |
| 8 | `foliage`, `foliage_mesh` | `foliage` |
| 9 | `sculpted` | — |
| 10 | `paint_wall` | `paint_wall` |

| | Today | After | Δ |
| --- | --- | --- | --- |
| `SCENES` | 43 | **51** | +8 |
| `GOLDEN` | 28 | **34** | +6 |
| Green checks | 4 | **4** | none |

Round 1's §2.8 said 48/32 and omitted `paint_wall` from both despite its own Stage 8 requiring it;
doc 09 restated 48/32 as unchanged; doc 10 moved it to 50/33 from that stale base. **51/34 is the
number.** Verified against `xtask/src/main.rs:41` (`const SCENES: [&str; 43]`) and `:253`
(`const GOLDEN: [(&str, &str, &[&str]); 28]`).

`empty` in `GOLDEN` covers no new rendering path and is admitted as an extension of the stated rule:
it is the one scene whose *appearance is itself the deliverable*, every user's first frame, and no
other gate can see it regress. Recorded as a rule change rather than slipped in.

`foliage` in `GOLDEN` is the second such extension and it is admitted for a sharper reason: it
covers no new *rendering* path — it draws through `grassVertexMain` like `meadow` — but it covers a
new *placement* path, and a golden image is the only gate that can see the mask fail to reach the
placement at all. That is exactly how `grass_blades` passing a flat constant `Ground` went unnoticed
for two slices.

`viewport_rect` answers the round-1 constraints review's §4(a): `loom render --viewport x,y,w,h`
renders an existing scene into a sub-rectangle of a larger canvas, so a wrong origin, a wrong aspect
or a missing `chrome_clear` fails on pixels rather than only on validation messages.

**The round-1 constraints review's §4(b) is wrong and needs no work.** It claims `cargo xtask
validate` drives `loom run --frames n` *"without `--edit`, so without a placement"*. Verified at
`xtask/src/main.rs:1023` and `:1077`: the invocations are `["run", scene, "--edit", "--frames", …]`
and `["run", scene, "--edit", "--play", "--frames", …]`. The windowed half of green check 2 already
drives the editor path, so from Stage 3 onward it exercises the dock, the placement, `chrome_clear`
and the zero-extent clamp under the validation layers automatically.

**No fifth green check.** Doc 07 §12 puts `cargo xtask docs --check` in `scripts/green.sh`, which
would also make `xtask` depend on `loom_editor` — colliding head-on with ADR 0022's boundary rule —
and would compile the whole editor on every green run, against `LOOM-IMPLEMENTATION-ORDER.md:574`'s
one-minute-warm stop-and-fix trigger. **Resolution:** the command table stays in `loom_editor`; the
generator is `loom docs [--check]` in `loom_cli`; `xtask docs` shells out to the `loom` binary it
already builds, the same way `xtask image` does. `xtask` gains no dependency. `--check` runs in CI
and by hand, not in `green.sh`, until its cost is measured.

**`cargo xtask flythrough` matters more than the still for two of the new stages**, for the reason
it always does. A painted foliage boundary is a curve in a density field and whether §2.10's edge
break-up stops it reading as a mown edge is a motion judgement no still frame makes. And
`cargo xtask shimmer` on `foliage` must not be worse than `meadow` **at the same density, colour
and lighting** — never compared across a change in any of those (ADR 0010).

### 2.9 Module layout inside `loom_editor`, and where the two new panels live

```
crates/loom_editor/src/
  lib.rs        Editor — owns DockState, Shell, theme, layout, shortcuts. One entry point.
  dock.rs       Tab, Shell (impl TabViewer), Window menu, maximise-on-hover
  theme.rs      the one token table (calling loom_render::ui::tok), applied via all_styles_mut
  icons.rs      the hand-drawn set — rules pinned, budget ≤ 24
  layout.rs     XDG state load/save, debounce, ignore-and-warn fallback
  viewport.rs   rect → ViewportPlacement, to_viewport / to_window, the input Response
  command.rs    Command, Needs, COMMANDS — data, not a trait
  palette.rs    fuzzy matcher over commands, component types, node paths, asset aliases
  help.rs       F1 popovers rendered from TypeRegistry::describe; explain(&FieldError)
  overlay.rs    stroked/chip/brackets/grid, gizmo handles, agent change-marks, busy indicator
  gizmo.rs      moved from loom_cli, extended (Stage 4)
  cursor.rs     the three-tier viewport raycast, and instance picking (Stage 8)
  snap.rs       grid / angle / increment / surface
  arrange.rs    a menu over loom_scene::place::resolve
  agent/        mod.rs, process.rs, panel.rs, proposal.rs, context.rs
  tools/        mod.rs (Tool, ToolEvent, ToolCtx, Outcome, Edit), select, transform,
                create, sculpt, paint, foliage, prefabize
  panels/       hierarchy, inspector, project, console, problems, history, transactions, prefabs
  hub.rs        the project hub, templates, loom new's UI half
```

**Doc 05 §1's `Outcome` enum is adopted verbatim and is the strongest structural idea in the set.**
A tool returns `None | Edit(Edit) | Select(Vec<String>) | View(ViewChange)` and holds no
`&mut Session`, no file handle and no `&mut SceneView`. Never-do #16 stops being discipline and
becomes a type: a tool that wanted to stash a mask or write a sidecar would have nowhere to put it.

`Tool` is an enum, not a trait (never-do #12). `Command` is data, not a trait.

**The `Tab` enum ships eleven variants, fixed in Stage 3:** `Scene`, `Game`, `Hierarchy`,
`Inspector`, `Project`, `Console`, `Problems`, `History`, `Transactions`, `Prefabs`, **`Agent`**.
`Environment`, `Terrain`, `Events`, `Profiler` and **`Foliage`** are cut (§5). Adding a variant
later invalidates every saved layout, which is why this list is decided once and why an
undesigned tab is worse than no tab.

**S17 — the Agent panel is a tab of the bottom node, and both round-2 designs are overruled.**
Doc 09 §5.1 puts it beside the Inspector as a tab; doc 11 §10 puts it as a vertical split of the
right column and argues, correctly, that a tab hides one of the two things user decision 5 asks you
to watch at once. The round-2 usability review answers both with arithmetic nobody else did: the
one surface that justifies the panel's existence is **the proposal card's diff**, and doc 11's
380 pt right column split 60/40 gives it roughly 330 px of height and 370 px of width with a 96 px
thumbnail competing for the same width. TOML lines wrap; a twelve-op diff does not fit. **The
bottom node is full-width, which a diff, a tool-call log and an inline render all need; the
conversation *is* a log and logs live there in every editor; and both the Inspector and the
viewport's `agent_marks` stay fully visible, which is more than either proposal offers.** The
bottom node's default height rises from 200 pt to 280 pt and `Agent` opens as its active tab —
that, not a column of its own, is what "first-class" costs.

**S18 — there is no `Tab::Foliage`.** Doc 10 §12.3 puts a species palette in `panels/`, which by
Stage 3's own rule would force a variant four stages before the feature exists. The round-2
constraints review's M3 is right and the cheaper branch is the correct one: **the Foliage palette
is tool-scoped UI**, shown while the foliage tool is active, in exactly the shape doc 05 gave the
sculpt brush. Species selection is a filtered view of the hierarchy's `Grass`/`Scatter` nodes, so
the hierarchy stays the one place things are named.

**S20 — and there is no `Tab::Graph` or `Tab::References` either. Eleven variants stand.** Doc 14
§0 spends a variant in Stage 3 with a real unconfigured body; doc 15 §3.1 amends this section's
"the enum is fixed once" rule to bind only against removal and reordering. **Both are overruled, and
the round-3 review's F16 is the reason: both documents admit, in their own "could not verify"
sections, that they did not read `egui_dock`'s `DockState` serialization — which is the single fact
the entire disagreement turns on.** The disagreement is also unnecessary. Doc 15 §3's argument for a
panel is sound — *a list you navigate from has to outlive the navigation* — and **the Problems panel
already is that list**: a tab of the bottom node, holding file-scoped rows you click to navigate,
surviving selection changes, landing in Stage 4. Reference results go there as a subject-keyed
section (§2.16). The enum is untouched, the layout-invalidation risk is zero, the `egui_dock`
question never has to be answered, and if someone then misses a dedicated References tab **that is
data** — which is precisely the standard doc 15 §10 sets for its own drawing slice and declines to
apply to its own tab.

### 2.10 Foliage: how a painted mask feeds a pure function without breaking purity

Doc 10's central idea is right and it is **not** ADR 0028's:

> **A painted foliage mask multiplies the placement rule. It never overrides it.**
> `coverage` and `viability` keep every term they have; painting scales the result.

ADR 0028's `lerp(rule, value, authority)` is correct for a **blend weight**, which has no
invariants. It is wrong for a **placement probability**, which has two that are documented, tested
and load-bearing: `slope_cutoff` means *grass stops entirely*, and `rock = 1.0` on a column with no
surface is the no-floating-blades path. A `lerp` lets a stroke restore grass past the cutoff, which
fails `grass_thins_on_a_slope_and_stops_on_rock` (`loom_grass/src/lib.rs:500`) and reopens the hole.

So `coverage` gains one factor — `(steepness * soil * lush * ground.paint / LUSH).clamp(0.0, 1.0)`
— where `ground.paint` is `lerp(1.0, value, authority)` and defaults to `1.0`. Four properties, and
the round-2 constraints review verified the first two against the source:

1. **An unpainted scene is bit-identical.** `x * 1.0` is exact in IEEE-754, and a fully-undone mask
   is `lerp(1, v, 0)` = exactly `1.0`. `meadow` and `grass_slope` do not move.
2. **Painting cannot defeat a hard rule.** `steepness` is zero past the cutoff and `soil` is zero
   on rock; zero times anything is zero. **The crater test passes unmodified.**
3. **Erase is absolute at full authority** — §2.5's `flow = 1.0` clause is what makes the brush able
   to reach it.
4. **Painting cannot exceed the density a gully already reaches.** `value` is clamped to `LUSH`
   (1.6), the headroom the candidate grid already carries.

**Property 4 is restated from doc 10's wording, because the round-2 constraints review's H4 is
right.** Doc 10 §2.1 says *"the `density` field in the inspector remains the truth about the
field's maximum"*. It is not, and never was: the candidate grid is `density × LUSH` per m²
(`lib.rs:322`) and ordinary ground accepts `1/LUSH` of it (`lib.rs:302`), so a gully already
reaches `1.6 × density` and painting merely makes that reachable across a field. **The budget meter
therefore computes `area × density × max(1.0, max_painted_value)`**, from the mask's actual maximum
rather than from the rule alone, or it under-reports by 60% in exactly the case a painter creates
deliberately.

**The factor lands inside `loom_scatter::viability`, not at its two call sites**, and that is not
stylistic. `habitable` is `viability(…) > 0.0` — a hard test, not a roll — so an erased region must
stop competing *like a cliff* rather than compete *like poor ground*, or the fringe of everything
you erase comes out thinned, which is the shaved-ring artifact arriving from a third direction.

**Neither crate gains a dependency.** `loom_grass::tile` already takes ground as
`&dyn Fn(f32, f32) -> Ground` (`lib.rs:315`) and `loom_scatter::region_on` takes the identical
shape; the CLI's `GroundGrid` gains a mask reference and one more field to fill. That is the same
seam ADR 0028 uses for `rock`, used a second time — and the second use is what turns a one-off into
a pattern.

**No shader is touched.** Not `grassVertexMain`, not `vertexMain`, not `fragmentMain`. The mask is
CPU-only, so it takes no bindless slot, no `paint_upload` pass, no barrier and no `ObjectData`
field.

**Two corrections to doc 10, both from the round-2 reviews, both adopted:**

- **The mask and the removal points are stored in the node's local XZ, not world XZ.** Doc 10 §3 and
  §6.2 both say world; §9 considers only sculpting and §13 only a future lateral terrain move. The
  case that exists today is simpler — **move the `Grass` node**. `grass_key` already includes the
  node's world translation (`main.rs:1633`), so the field regenerates in its new place while the
  painted path and the copse's missing tree stay where the node used to be. Local XZ is what the
  `half_extent` projection already uses; the transform happens at bake time, one matrix multiply in
  `GroundGrid`. **`SplatPaint` has the same defect and it is settled once for both.**
- **`reach_of` needs no new term.** Doc 10 §6.2 property 4 and its ADR both claim the removal list
  adds one. Verified at `loom_scatter/src/lib.rs:731-740`: `own(&Rules)` is
  `REACH * cell_size(spacing)` and the removal's reach is `spacing * 0.45`, which `REACH ≥ 2` cells
  already dominates. The dirty region a *stroke* needs is a bounding-box growth at the call site.
  Deleting the claim is the whole fix.

**The capacity ceiling is real, and slice 1 fixes it with arithmetic rather than with a mechanism.**
Verified: `MAX_BLADES = 262_144` (`renderer.rs:999`, `viewer.rs:436`), `GrassBlade` is 48 bytes
(`renderer.rs:582`), so **the largest grass field that fits at density 140 is about 43 × 43 m** and
a 256 m field is 3,500% of the buffer, truncating in generation order — a straight horizontal edge
across the landscape with an `"ok": true` render.

The round-2 usability review's §1.2 catches two consequences doc 10 wrote against its own table: the
auto-created field ("clamped to 128 m") is 8.7× to 36× over the ceiling, so *the very first stroke a
stranger makes produces a truncated field*; and the budget meter is per-field while the buffer is
global (`grass_blades` accumulates one `Vec` across every `Grass` node, `main.rs:1882-1890`;
`warn_if_grass_truncated` is called once on the total, `main.rs:695`), so six species each reading
17% truncate at the moment the meter says everything is fine.

Its fix is to pull doc 10 §7.6's CPU pre-cull into slice 1. **That collides head-on with the
constraints review's M2**, which is also right: `grassCullDraw` and the distance falloff are
hand-written Slang with no `loom_field::Expr` behind them and no agreement test, so a CPU
counterpart is a second implementation of a formula whose first implementation is in a shader —
the divergence ADR 0006 exists to prevent, with the direction of the port reversed. **Both findings
survive and the resolution is neither's:**

> **Slice 1 fixes the truncation with a budget-derived clamp and a scene-global meter, which need
> no shader knowledge at all.** The auto-created field's `half_extent` is computed from
> `MAX_BLADES`, the authored density and `LUSH`; the meter is one stacked bar for the whole scene
> with the per-field count as each segment's label; and `warn_if_grass_truncated` gains the one
> fact it lacks — *which* field was cut, which is always the last in generation order, so it is an
> `nth` and a name. **The CPU pre-cull stays in slice 3 (ADR 0041) and gains an explicit CPU/GPU
> agreement test in the shape of `fields.slang`'s**, with the ADR stating that the cull constants
> are a shared pair that move together.

The honest consequence: **slice 1 ships a 43 m ceiling, made visible and correct.** That is a
meadow you can walk across, it is a real deliverable, and the streaming work is paid for by someone
who has actually painted past it. Doc 10's own sequencing was right; only its clamp was wrong.

**One hole the reviews found that has no cheap workaround and must be built.** Doc 10 §6.2 and §6.3
— *"delete the tree in the doorway"* and *"drag one two metres"* — are the two verbs that make this
Unreal's Foliage mode rather than Unity's Detail brush, and **the engine cannot say which tree**.
Verified: `pick_at_cursor` (`run.rs:2002-2027`) ray-tests `SceneView::picks`, a
`BTreeMap<String, Bounds>` keyed by node path and built by `node_bounds` (`scene_view.rs:60`,
`:121`); scattered instances are produced separately by `scatter_objects` and appended to `objects`
(`:118-120`) with a transform and **no node path**, so a generated tree is invisible to the only
selection mechanism the editor has. Without it, `Scatter.remove` is a storage format whose only
author is the agent. **`SceneView` gains `instance_picks: Vec<(String, u32, Bounds)>`, filled by
`scatter_objects`, tested after `picks` so a real node always wins a tie; selecting an instance
shows a synthetic inspector — *"instance 412 of Pines · generated"* — with exactly two buttons,
Remove and Detach to node.** Marquee-select over instances is then free and multi-remove is one
`SpliceArray` with N points. ~40 lines, Stage 8 slice 2.

### 2.11 The project model: the engine repo is a project too

**The engine repository becomes a project by gaining one checked-in file — `loom.toml` at the root
— and nothing else changes.** No scene moves, no path is rewritten, and no code that resolves an
asset path is touched, so the 43 gated scenes and their references *cannot* move: not "should not",
cannot, because the resolver is the same function on the same bytes. Doc 12 checked this rather
than asserting it — across all 25 scenes carrying an `[[asset]]` block, **176 `path` values, 165
resolving scene-relative and zero resolving relative to the repository root**, with nine of the
remainder carrying a `#Object` selector that `main.rs:1155-1162` strips and two pointing at
directories that have never existed.

**`[[asset]]` resolution is unchanged and that is the deliverable.** A project-relative *fallback*
is rejected — it turns a broken reference into a working one at a distance, which is the failure
ADR 0024 rejects UUID sidecars to avoid. **`project://` is reserved as the spelling, not built**, so
that whoever wants it later does not invent `$/`, `//` or a bare leading `/` (which keeps meaning an
absolute filesystem path, as `base.join` and `prefab.rs:133` already implement).

**What the decision exposes is a real defect nobody had named: three different things call
themselves "assets" and only one resolves correctly outside this repo.** Scene-relative is correct
at all seven consumer sites. Cwd-relative is one site and a shipping bug — `load_bindings`
(`run.rs:2242-2251`) reads the literal `assets/input/default.toml` against the process working
directory, so a shipped game's rebinding silently does nothing. Engine-relative-pretending-to-be-
scene-relative is two sites — `sound.rs:57` and `main.rs:3238` load the weather bed as
`base.join("../audio/rain.wav")`, which resolves here only because every gated scene sits one
directory below `assets/`, and which in a project whose scene is `scenes/main.loom` lands on
`<project>/audio/rain.wav` and degrades to the synthesiser at info level. **ADR 0036 is that fix.**

**Two corrections to doc 12, both from the round-2 reviews, both adopted, and the second is the
more important:**

- **`engine_assets()` needs a third branch and the templates should not need it at all.** Doc 12
  §13.4 states the gap and ships without it: `cargo test`'s working directory is the crate
  directory, so a test calling `engine_assets()` finds no `assets/` from `crates/loom_cli/`. The
  branch is `exe dir → cwd → find_root(cwd) → compiled-in`, and `find_root` is a function the same
  stage already builds. **But the usability review's §1.3 is the sharper finding**: for an
  *installed* binary — `~/.cargo/bin/loom`, no sibling `assets/`, run from a home directory — the
  Templates rail is empty and `loom new` cannot create a project at all, which is the entry point of
  the whole project model failing in precisely the case user decision 7 is about. Doc 12's own
  argument supplies the answer: it rejects embedding `rain.wav` because it is 3 MB of WAV, and
  **templates are not 3 MB of WAV** — they are a `loom.toml`, a `.loom` scene and a `.rhai` script,
  the same category as `loom_input::DEFAULT_BINDINGS`, which this engine already compiles in.
  **Templates are compiled in (`include_str!`) and `loom new` writes them out**, which makes
  `loom new` have no filesystem precondition at all and leaves `engine_assets()` with only the two
  consumers that already have fallbacks. Doc 12's V6 is vacuous — it produces the same result before
  and after the change — and is replaced by V9 in §4 Stage 5.
- **`--frames` forces scene-only mode.** `find_root` walks up from `assets/test/…` and finds the
  repo's new `loom.toml`, so from Stage 5 all 43 gated windowed runs silently switch from scene-only
  to project mode: the Project panel populates, the state key becomes one shared project hash, and
  the hub's recents may acquire a row from a gate run. The consequences are probably benign; the
  comment doc 12 checks into the repository root (*"nothing here is read by `cargo xtask validate`"*)
  is not. One condition keeps the gate measuring what it measured.

**`[ship] exclude` must exclude dot-directories or it does not work at all.** Doc 12 §2 spots that
`.claude/worktrees/` holds full checkouts of this repository and adds the skip to
`project::scenes()`, then does not apply the same insight to shipping — so `loom ship` on the
repo-as-project copies `.claude/worktrees/<name>/crates/`, a second `Cargo.lock` and a second
complete `assets/` tree, **and doc 12's V8 passes anyway** because it tests root-relative names.
ADR 0032's fixed list gains *any root entry whose name begins with `.`* (subsuming its existing
`.git` entry) and V8 becomes recursive: no path anywhere in the output tree contains a `crates/` or
`target/` component. `/builds` joins `.gitignore` beside the existing `/target` and
`.claude/worktrees/`.

**`project::scenes()` skips `target`, `builds`, `out`, any dot-directory, and `*.mine.loom`** — the
last because §2.6's divergence-banner button writes one beside the scene and doc 12's glob would
list it in the hub forever with no way to tell it from a real scene.

### 2.12 The state directory (S9), owned once

Everything user-global lives under `$XDG_STATE_HOME/loom/` (thumbnails under `$XDG_CACHE_HOME`),
**nothing is written into a project directory**, and every path-derived key goes through **one
helper with one canonicalisation rule** — four documents each hashed a path their own way, and a
path that hashes two ways is a state directory that silently splits in half on the first symlinked
project.

| Path | Holds | Keyed by | Rotation |
| --- | --- | --- | --- |
| `prefs.toml` | recents, theme scale, zoom, high-contrast, reduce-motion, **agent `command` and `preamble`**, the user's `approve` floor | — | — |
| `layouts/<key>.json` | the `DockState` | project path | — |
| `journal/<key>.jsonl` | ADR 0034's transaction labels | **scene** path | 200 entries/file, and a file-count cap |
| `proposals/<key>/<token>.json` | ADR 0038's queue | project path, **or scene path in scene-only mode** | deleted on approve/reject |
| `context/<key>.json` | `loom context`'s selection/camera/version | project path | overwritten |
| `transcripts/<key>.jsonl` | the conversation | project path | last 200 turns |
| `$XDG_CACHE_HOME/loom/thumbs/` | hub thumbnails | project path | LRU |
| `$XDG_CACHE_HOME/loom/graph/<key>.*` | **the reference index, only if Stage 12's measurement says one is needed at all** (§2.16) | project path | deleted on any format change; never migrated |

**The rule that decides which of the two directories anything goes in, written once because two
round-3 documents put the same index in different ones (S22): state holds what the user meant;
cache holds what we can recompute.** A layout, a preference, a proposal and a transcript are
intentions and cannot be regenerated. A thumbnail and a derived index can be, from files that are
already the truth, so they are cache — and a `rm -rf $XDG_CACHE_HOME/loom` must lose nothing but
time. That is also the answer to *"the database must be gitignored"*: **it is never in the project
at all, which is stronger.** Doc 14 §4's sentence is the one to keep — *a file that is never in the
project cannot be committed by someone who has not read `.gitignore`; gitignoring is a request,
putting it in user state is a guarantee* — and the knock-on is that `.gitignore` gains no line.

TOML for the hand-readable file; JSON only for the `DockState` tree and the machine-written
records. **When `$XDG_STATE_HOME` is unwritable the editor warns once in the console and runs on
defaults; it never fails to open**, which is the same posture `layout.rs`'s ignore-and-warn fallback
already takes for a corrupt layout.

**The journal is opt-in, and that resolves the round-2 constraints review's M4.** Doc 09 calls
`journal::append` from `apply_to_file` — `loom_scene`'s single write path, which every CLI call,
every unit test and both gates go through — for data whose only consumer is the editor. That gives
green check 3 a side effect outside its temp directories and creates one journal file per test per
run, forever. **`append` runs only when `LOOM_JOURNAL=1`**, which the editor sets on itself and
which the agent's `loom scene --tx` calls inherit because the panel spawns them. `cargo test` and
`xtask` set nothing and write nothing.

**The actor is derived, not configured.** Doc 09 takes it from `$LOOM_ACTOR` defaulting to `"cli"`,
and nothing anywhere sets `$LOOM_ACTOR` — so the History row the whole journal exists to produce
would read `cli · Block out office: 14 nodes`. It is derived from `LOOM_AGENT=1`, which ADR 0038
already sets, with `$LOOM_ACTOR` as an override.

### 2.13 Where a round-2 review changed a decision

Recorded explicitly, because these are the places the plan moved rather than absorbed.

**H1 — the agent command does not live in the project manifest.** Doc 09 §4.2 puts an argv vector
in `loom.toml`, which is checked in, cloned, downloaded and shared. That is arbitrary code execution
one click from opening a project, for an audience the user has explicitly said will be strangers,
with the project root as the working directory. Worse, `approve = "none"` is a legal value in the
same file, so **the project can switch off the gate ADR 0038 exists to provide**. `command` and
`preamble` move to `prefs.toml`; `approve` and `approve_above_nodes` may appear in `loom.toml`
**only as a tightening** — the effective policy is the stricter of user and project, and a project
can never loosen. This is the one place in the rework where the lazy answer is refused: a trust
boundary is not a place to save a config file.

**H2 — ADR 0038 gates on `LOOM_AGENT=1`, not on the CLI's default.** §1 lists the CLI's contract as
untouchable and doc 09 changes it: `loom scene --tx` would stop applying a class of transaction and
exit 0 with `{"status":"proposed"}`, which every script reading exit 0 as "applied" gets wrong in a
way that looks like success. Verified good news that does *not* rescue it: nothing in `xtask`,
`scripts/` or `tests/` drives a destructive `--tx` today (the only `RemoveNode` construction outside
`ops.rs` is `run.rs:1892`, the editor's own Delete, which does not go through the CLI), so no green
check breaks — but the contract change is the problem, not the breakage. **The proposal path is
opt-in via `LOOM_AGENT=1`.** And the classifier changes shape: `SpliceArray { remove > 0 }` is not
destructive, it is how you edit an array element in place — retuning one sculpt stamp, replacing a
paint stroke, correcting a `Scatter.remove` point — so a gate on it fires on routine editing, which
*is* the blind-approve regression arriving through the mechanism built to prevent it. **Classify on
net loss: `RemoveNode`, `RemoveComponent`, and `SpliceArray` where `remove > insert`.** Finally,
under `LOOM_AGENT=1`, `--allow-destructive` **proposes rather than refuses**, so a script the human
deliberately started is not a dead end with no card to review.

**H5 — the undeclared-alias check cannot live in `Scene::parse`.** Doc 09 §7b's finding is real and
valuable: an undeclared mesh alias draws the box in total silence (`main.rs:1150`'s `?` returns
`None` with no log line; `index_for` returns 0), which is the shape a hallucinated asset path takes.
But the fix as specified needs `loom_asset::primitives::NAMES` and the aliases voxel volumes
generate in `loom_cli`, and `check-deps.sh:26-31` permits `loom_scene → loom_reflect` and nothing
else. **The check belongs where the alias set is actually known — beside `MeshLibrary`'s `wanted`
(`main.rs:1146`), as a `loom validate` and Problems-panel diagnostic**, which also makes it a
warning by construction, which doc 09's own escape hatch says it may have to be anyway.

**§1.5 — the approval loop gets a return path, or the flagship interaction stalls.** Trace it: the
human asks for six crates deleted; the CLI exits 0 with `proposed`; the agent's turn ends; the human
approves two minutes later; the agent is idle holding a pre-approval version token and its next
write is `stale_version`. Reject is worse — doc 09 §6.2 promises *"a one-line reason the agent can
read back"* through nothing. **`loom propose --wait <id>` blocks until approved, rejected or timed
out and prints the outcome plus the new version token as one JSON line**, joining
`loom_agent::TOOLS` as `propose_wait` beside the `editor_context` entry doc 09 §5.3 already adds.
The shipped preamble tells the agent to call it when a command returns `proposed`. One tool, one
sentence, and the turn stays open across the human's decision — which is what makes it a
conversation. **Also: the composer is live during `State::Thinking` and `State::Tool`**, and a
mid-turn line is sent as an ordinary `{"type":"user"}` — the wire is bidirectional and `ChildStdin`
is already held open, so *"no, not that one"* is a `writeln!` and Stop stops being the only
interrupt.

**§1.5 second half — the panel must not ship as an empty box.** The refusal to name a vendor CLI in
the engine's manifest schema is right and is kept. The consequence — a stranger's headline feature
is a config chore with four flags they have never heard of — is not forced. **The unconfigured panel
offers a Detect button** that probes `$PATH` for a short list of known agent CLIs, shows the exact
argv it found, and on the user's click writes it to `prefs.toml`. Zero detected → doc 09's
paragraph and copyable snippet. The vendor list is data in a config file the user can edit, which is
the same distinction doc 09 §4.2 already makes about the preamble.

**§1.2 vs M2 — resolved in §2.10, in neither review's favour.**

**M9 — `adopt_external`'s bookkeeping needs a test, not a redesign.** Doc 09's nine lines are not
never-do #15 (there is one state, taken whole; `accept_disk_version` at `edit.rs:366-385` is the
precedent), but the snippet does not show what `undo()` then does to `version` and `disk`, and a
save carrying the wrong token either force-writes over the agent's work or is rejected for the wrong
reason. **`undo_after_adopt_saves_against_the_disk_token` joins the test list**, because
`adopted_agent_transaction_undoes_in_one_step` never saves and therefore cannot cover it.

**§1.4 (instance picking), M7 (local XZ), H4 (the meter), and the erase-authority clause** are all
in §2.10.

**M6 and the theme probe.** Doc 11's probe validates `tok` on opaque fills — the easy half, and the
half the arithmetic already settles. Both reviews independently name the half that needs measuring:
with a `_SRGB` attachment the hardware blends in linear, so **every semi-transparent surface in the
design and egui's own glyph coverage change when the flag flips**, and light-on-dark text getting
thinner is the usual direction. **The probe renders the swatch strip, the same swatches at α128 over
a mid grey, and a paragraph of `Body` text at `text` / `text_weak` / `text_strong`, screenshotted
before and after the flip.** Acceptance is *swatches within ±2 bytes **and** text weight unchanged*.
If text thins, the answer is a `FontTweak` or accepting the double encode with the palette retuned,
and either is better known at Stage 0 than after the token table has been judged. **The probe is
fifteen lines of egui in `loom_cli`'s existing panel path at Stage 0** — doc 11 schedules it in
three different stages and in a crate that does not exist until Stage 1 — and it moves into
`loom_editor` with `theme.rs` at Stage 3.

**M10 — the UNORM rejection is refiled under its true reason.** Doc 11's ADR rejects a
`B8G8R8A8_UNORM` swapchain because it *"moves the scene's own tonemap output, which the golden
references pin"*, and §7 of the same document establishes that no golden reference ever sees the
swapchain — `xtask image` drives `loom render`, which constructs no `Ui`. The rejection stands; the
reason becomes *"it moves the encode into the tonemap for the window path only, creating a second
place the window and the offscreen path can disagree — ADR 0018's defect class."* A rejection filed
under a disprovable reason is one the next reader reverses.

**L2 / L4 / the count assertions.** Doc 12's V2 asserts "28 references, zero moved" for work that
lands at Stage 5, by which point `GOLDEN` is 30. **Verification steps assert `MANIFEST.txt` is
byte-unchanged and never name a count**, because an implementer who runs a step and sees a different
number concludes the step is stale rather than that it passed. Likewise V3 asserts the warning *set*
difference contains only `asset_file_missing`, which doc 12 §9 already says is the better check two
sentences before it asserts a number it admits it does not know.

**Where a review is wrong.** The round-1 constraints review's §4(b) (§2.8). And the round-2
usability review's §3 claim that foliage's dependency on Stage 9 is only a test dependency is
**accepted** — §9's mechanism is `grass_key` including every `VoxelVolume` (`main.rs:1641`), which
works today — which is why foliage is Stage 8 and sculpting is Stage 9. Its claim that doc 09's
central insight is "the single most valuable finding in round 2" is also accepted, and it is why
ADR 0034 is scheduled in Stage 4 rather than with the panel.

### 2.14 The prefab load-path bug (S7) — verified live today

**The bug.** `SceneView::build` (`crates/loom_cli/src/scene_view.rs:97`) and
`SceneView::build_cached` (`:105`) call `Scene::parse` directly at `:110` instead of
`prefab_load::for_reading`. Every other reader in the codebase resolves prefabs —
`main.rs:334`, `:565`, `:2535`, `:3111`, `:3222`, `:3308`, `:3903`, `:4013` all route through
`prefab_load::for_reading` or `for_reading_with_warnings`. `SceneView` does not.

**The blast radius is the entire windowed path.** `SceneView` has exactly two production callers and
they are both the window:

- `run.rs:2301`, inside `open_scene` — the entry point for **`loom run`, `loom run --edit`,
  `loom run --play` and `loom run --frames`**, i.e. read-only viewer and editor alike.
- `run.rs:508`, inside `show` — **the reload path the file watcher calls four times a second.**

So a scene using prefabs opens in the editor with its instances absent from `objects` (they draw
nothing), absent from `picks` (they cannot be clicked), and absent from `paths` (they are missing
from the hierarchy), **and it validates clean** — while `loom render`, `loom validate`,
`loom describe` and `loom sim` all draw and report the same file correctly. The two halves of the
engine disagree about what a scene contains. `assets/test/prefab_room.loom` is the reproduction:
`loom run --edit assets/test/prefab_room.loom` draws nothing where the room is.

It also reaches the gate. `xtask validate`'s five windowed invocations go through `open_scene`, so
any prefab-using scene added to `SCENES` is validated as an empty room — a full pass over content
that was never rendered, which is the same failure mode as a `GOLDEN` list missing `meadow`.

`loom explode` (`main.rs:3440`) parses directly for the same reason and is the third site.

**CLAUDE.md names this exact regression as the likeliest way to regress S4**, and the reason is
stated there too: *"a key it does not understand is a key it ignores"* — the instance arrives with
no components, draws nothing, and passes validation.

**The fix.** Both `SceneView::build` and `build_cached` route through `prefab_load::for_reading`
(already `pub(crate)` in `loom_cli`, so no visibility change); `explode` likewise, in the same
commit. **The regression test is the point of the commit**: open `assets/test/prefab_room.loom`
through `SceneView::build` and assert the instance's node paths appear in `paths` and its bounds in
`picks` — not merely that the call succeeds, because it succeeds today.

**Stage 0**, and it is the reason Stage 0 exists.

### 2.15 Two corrections to documents that call themselves authoritative

- **Doc 05 §6.9's "hard dependency on the render-to-texture viewport (ADR I)" is stale.** Doc 01
  §1 rejects render-to-texture by name and specifies a sub-rectangle instead. The *dependency* is
  real — tools need rect-relative coordinates — but the mechanism is not, and an implementer reading
  doc 05 alone would build the resize policy and colour round-trip doc 01 spent a section avoiding.
  Read it as a dependency on ADR 0025.
- **Both surveys mark `crates/loom_render/src/ui.rs` "KEEP AS-IS". It has to change** twice, and §6
  R2 explains the first. Verified: `ui.draw` is called at `viewer.rs:1613`, *inside* the `ui`
  render-graph pass closure, which `RenderGraph::execute` runs after the forward and tonemap
  closures have recorded. egui's layout for frame *N* therefore happens after the scene for frame
  *N* was recorded, so a `ViewportPlacement` read from egui can only ever be frame *N−1*'s
  rectangle. The second change is `srgb_framebuffer` and `tok` (§2.2, ADR 0033).

### 2.16 The knowledge graph (S20) — arbitrated clause by clause

**ADR 0003 is accepted, so this is being built. How much of it is built is a judgement, and the
judgement is: about a fifth of what the two round-3 documents propose.** The review's §5 is
substantially right and is adopted; where it is wrong, §2.16.7 says so.

#### 2.16.1 What it is, after arbitration

One crate, `loom_graph`, depending on `loom_scene` and nothing else. **No store, no incremental
path, no `--verify` harness, no watcher, no `mentions`, no `docs/` or `crates/` walk, no drawing, no
`Tab` variant, no impact modal, one ADR.** It walks the project, parses each `.loom` **unresolved**,
resolves aliases through the loader's own function, builds two `BTreeMap` adjacency maps in memory,
and answers three questions on a CLI subcommand wrapped as one MCP tool. A few hundred lines.

The clause-by-clause ruling, so an implementer never has to open `14`, `15` or `16` to know which
sentence won:

| Clause | Ruling | From |
| --- | --- | --- |
| Crate placement, `loom_graph → loom_scene` | adopted | both agree |
| **Scenes are read unresolved** | **adopted whole**, including its sentence as a module doc comment | **doc 14 §3.2** — the best argument in either document |
| Edge ownership by the file whose text asserted it | adopted as a concept; no `owner` *column*, because there is no table | doc 14 §2.2 |
| Node kinds: `file`, `node`, `type` | `file` and `node`; **`type` cut** (`describe_type` answers types from the registry, and an edge from every scene to `Transform` is 100% noise) | doc 14, trimmed |
| Store | **none.** Measure first; a single JSON file with `(mtime,len)` validation if the number demands it; SQLite only past doc 15's stated ceiling | **doc 15 §6.3**, review F12 |
| Freshness | **one `refresh()`, both consumers**, called by the CLI once per invocation and by the editor on the poll it already runs | **doc 14 §5.1**, review F8 |
| Determinism | `BTreeMap` only, no `HashMap`, no `thread_rng`; one `#[allow]` on the single timing call with its reason beside it, in the shape `run.rs:392` uses | **doc 15 §8**, review F5 |
| CLI grammar | **subject first** — `loom graph <subject> --impact` — because `main.rs:234` says every subcommand takes its subject as the first argument | **doc 14 §6.1** |
| `TOOLS` | one more row, `("graph_query", "loom graph")`, **named questions and never a SQL passthrough** | doc 15 §7 |
| Writes | **the graph proposes; it does not write** — doc 14 §7.4's wording verbatim | **doc 14**, review F15 |
| Surfaces | three inline, plus a Problems section. No tab, no modal, no picture | doc 15 §3 rows 1–4, review F16 |
| ADR count | **one, 0042** | review §4.11 |

#### 2.16.2 The model, and the two holes both extractors had

Two node kinds and five edge kinds. Ids are derived from the path and never stored, so re-indexing
a file twice produces identical rows and nothing in the index is a fact the files do not carry.

| Node kind | Id grammar |
| --- | --- |
| `file` | `file:<project-relative path>` |
| `node` | `node:<project-relative path>#<NodePath>` |

| Edge | src → dst | derived from |
| --- | --- | --- |
| `declares` | file → file | `Scene::assets()` / `Scene::prefabs()`, path joined scene-relative, `id` in meta |
| `instantiates` | node → file | `Node.prefab` alias → the declaration |
| `extends` | file → file | root `Node.extends` alias → the declaration |
| `references_asset` | node → file | the alias walk below |
| `references_node` | node → node | **F2's fix** — a string field whose value is a node path in the same file |

`contains`, `child_of` and `attaches` are cut: the hierarchy panel *is* the picture of `parent`, and
`describe_type` answers types without an index.

**The alias walk is the review's F3 rule and it is cheaper than either document's walker.** Doc 14
looks for `{ "asset": "<alias>" }` objects plus strings ending in a known extension; doc 15 looks for
`AssetRef`-typed fields via a `$ref` walk plus *"a small explicit rule"*. **`Scatter.mesh` is a bare
`String` with no extension whose schema is byte-identical to `Name.value`, so it defeats all four
mechanisms, and both documents' guard tests pass while it does.** The rule instead:

> **Any string field in any component whose value matches an `[[asset]]` key declared in that scene
> is a reference to that asset.**

No per-component knowledge, no schema inspection, no extension list; every future alias field is
covered for free; and the failure mode inverts from silent-missing to a spurious edge if someone
names a node after an alias. It is also not a new idea — it is what `MeshLibrary`'s `wanted` set
already does at `main.rs:1146-1170`, which is the same code S21 moves into `loom_scene` so the index
and the loader cannot disagree.

**`references_node` is the same trick pointed inward**, and it closes a gap this repository has
written down and left open. `assets/test/forest.loom:20`, checked in: *"the refusal happens when the
field is resolved, not in `loom validate`, which checks schemas and asset aliases but does not yet
follow `Scatter.exclude`."* Rename or delete a `Scatter` node and every `ScatterExclude` naming it
silently stops excluding. **That is the best single argument for building this at all**, neither
document used it, and doc 15's model structurally cannot represent it.

#### 2.16.3 Unresolved reading, which is the one place the graph must break §2.14's rule

`derive_scene` calls `Scene::parse` and stops. It does **not** call `prefab_load::for_reading`, and
§2.14 spends a section establishing that a reader which skips resolution is a live bug class — so
the exemption has to be earned, and doc 14 §3.2 earns it three times over:

1. **Resolution erases the edge the exit criterion needs.** After `prefab::resolve`, an instance
   node carries the prefab's components and *no `prefab` field*. The `instantiates` edge exists only
   in the unresolved text.
2. **Resolution breaks per-file ownership.** A resolved `prefab_room.loom` contains `lamp.loom`'s
   nodes, so editing `lamp.loom` would have to dirty every scene that instances it — the inverse of
   touch-one-file-reindex-one-file. It also produces edges that are *false as stated*.
3. **Two hops recovers everything resolution would have given**, labelled instead of flattened,
   which is strictly more information.

**The rule, written so it survives being quoted out of context, and carried as `derive_scene`'s
module doc comment:** *a consumer that renders, simulates, picks or measures must go through
`prefab_load::for_reading`; the index must not, because it is the one consumer whose subject is the
reference and not the result.*

#### 2.16.4 Where it must not reach

The brief asks for a guarantee, not a promise. Five mechanisms, ascending:

1. **`scripts/check-deps.sh` gains two stanzas, in the two shapes the file already has** — and the
   review's F14 is right that neither document's proposal enforced what it claimed. The leaf rule in
   the shape of the `loom_scene` one at `:26-31` (`loom_graph` may depend on `loom_reflect` and
   `loom_scene` and nothing else); the reverse containment rule in the shape of the `loom_agent` one
   at `:33-44` (only `loom_cli` and `loom_editor` may depend on `loom_graph`). Doc 14's
   `cargo tree -p loom_cli --no-default-features` grep is kept as well, because it catches a third
   thing — feature unification leaking into the runtime binary, R16.
2. **`loom_graph` may not import `loom_scene::{ops, edit}`.** It constructs no `SceneOp`, no
   `Transaction`, no `Session`. Its only filesystem write is its own cache, outside every project.
3. **The gates never construct one.** `xtask image` drives `loom render`; `xtask validate` drives
   `loom validate`, `loom render` and `loom run --edit --frames`, and §2.11 forces `--frames` into
   scene-only mode, which has no project and therefore no index. The honest statement is *linked but
   never constructed*, not *absent*, and a test asserts `main.rs` mentions `loom_graph` only inside
   the `graph` arm, in the shape of the existing `every_tool_wraps_a_real_subcommand` string test.
4. **`loom validate`'s output does not move.** The review's F4 caught doc 15 wiring the index into
   `alias_report` (`main.rs:483`) to aggregate `asset_file_missing` project-wide. That would make
   `loom validate <scene>` answer differently inside a project than outside one, make green check 2's
   warning set a function of what else is on disk, and reach the one command the gate drives — and
   doc 15's own `green_run_writes_no_index` test could not catch it, because scene-only mode builds
   no index. **The aggregation is a panel feature**: the Problems panel shows per-scene validate
   output beside `loom graph . --broken`. `main.rs:483` is not touched, and the panel gets the better
   answer anyway, because `--broken` covers all 52 scenes while `SCENES` covers 43.
5. **The strongest: information flows files → index and never index → files.** No component reads it,
   no shader is fed from it, no `SceneOp` is generated by it, no scene file is written by it. **The
   graph proposes; it does not write.** The enforceable statement, in these words: *no automatic path
   exists from the index to authored state, and the dependency rules make one a green-check-1
   failure; a human-mediated path exists by design — a person reading a Used-by list and then
   deciding — and its correctness rests on the version token, not on the index.* The rules constrain
   `loom_graph`, not `loom_editor`, so that seam is behavioural and saying so is cheaper than
   discovering it.

**On determinism.** `BTreeMap`/`BTreeSet` throughout, no `HashMap`, no `HashSet`, no `thread_rng`,
no wall clock inside a query — mtimes are read in the freshness check, which is I/O, never in a
result. This is not a courtesy: `clippy.toml` is at the workspace root and green check 1 is
`--workspace`, so doc 14 §7's exemption fails check 1 on the day it lands. The single elapsed-time
call carries one `#[allow(clippy::disallowed_methods)]` with its reason written beside it, exactly
as `run.rs:392` already does. Two runs over one tree print byte-identical JSON, which is what makes
`graph_query` output diffable.

#### 2.16.5 What ships, and what is cut with a trigger

Ships: `loom graph <subject> --used-by | --impact | --broken`, subject first, one JSON line, exit
0/1/2, every response carrying an `index` block naming the file count, the parse-error count and the
files whose edges are missing because they do not parse. **A neighbourhood computed while a file in
it is unparseable is reported as incomplete, never as complete-and-empty** — that is `CLAUDE.md`'s
named S4 regression shape (*"a key it does not understand is a key it ignores"*) arriving in a new
crate, and it is a test. `graph_query` joins `loom_agent::TOOLS` as one more row; `loom-mcp` still
shells out to the binary, so `loom_agent` gains no dependency and CLI-first is unchanged.

Cut, each with its trigger, and all of them in §5's table: SQLite and the whole consequence tree
(WAL, `user_version`, the mtime/hash ladder, `--verify`); `mentions` and the `docs/`+`crates/` walk;
`--orphans`; `--why`, `--stats`, `--split`, `--no-refresh`; the forward `--pack`; the drawing; the
`Tab` variant; the impact modal; `Q4`/`Q5`.

**Two of those deserve their reasoning here rather than in a table cell.**

**`--orphans` is cut, and it is the most dangerous wrong answer in either design.** ADR 0023's walk
sees `tests/`, which holds 28 reference PNGs — verified — so day one of doc 14's Q2 is 28 false
orphans, sorted by size, at the top of a list whose named consumer was a delete button. Worse, the
walk is a *filesystem* walk while both documents quote *`git ls-files`* counts: `assets/test/**/
*.actual.png` and `render.png` are gitignored, exist after any failed image gate or manual check,
and are orphan textures the 288 does not contain. The index and git disagree about what the project
holds. `--orphans` returns when the reverse index is trusted — i.e. after the alias rule has been in
use and the walk's scope is settled — and not before.

**The forward two-hop context pack is cut, and the design doc's own argument is why.** §2.7 argued
retrieval-beats-a-dump *at scale*; doc 15 §7 concedes the collapse in its own words — *"at 350 nodes
the whole index would fit in a context window. What the agent actually gets from it today is not
compression but **direction**."* Direction is the *reverse* relation, which is `--used-by` and
`--impact`, and those ship. The forward question — *what does this scene reference* — is answered by
reading the scene file, which the agent does anyway, in one tool call it already has. **The two-hop
mechanism ships; only the forward-facing surface is cut.** Trigger: a project where reading the
scene is no longer the cheaper answer, or an observed model wasting turns discovering dependencies.

#### 2.16.6 The surfaces — M12's "knowledge-graph view", scoped down and said so

M12 listed a knowledge-graph view as an editor panel, and the user has asked for it. **It ships as
three inline surfaces and one Problems section, not as a dedicated tab and not as a picture**, and
that is a deliberate scoping-down rather than an omission:

- **A "Used by" section in the inspector**, below the last component, collapsed when empty. Rows are
  `file · node · field`, grouped and sorted; clicking opens that scene at that node. Zero actions to
  reach it: it is there because you selected the thing. Doc 15's row 1, and its highest value per
  line.
- **A banner across the top of the scene view when the open file is a prefab** — *"3 scenes instance
  this prefab · Show"*. This is ADR 0003's exit criterion answered **at the moment the question is
  live**, which is when you open the prefab, not when you delete it. One query, one line.
  `prefab_room.loom`'s own header comment already says what the banner says, which is evidence both
  that this is the fact people need and that a comment is a bad place to keep it.
- **Reference results in the Problems panel**, subject-keyed, plus two new categories: *referenced
  file is missing* (project-wide, which per-scene validation structurally cannot give) and *nothing
  references this file* — the latter held until `--orphans` is trusted. Severity warning, never
  error. This is where doc 15's References tab goes (S20/F16).
- **Nothing in the hierarchy**, and now for a corrected reason: doc 15 said intra-scene references do
  not exist, which is false (`ScatterExclude`). They exist, they are indexed as `references_node`,
  and they still do not want a badge — a count on every row is decoration, and the Problems row for
  a *broken* one is the surface that matters.

**The force-directed drawing stays cut**, and doc 15 §4.1 is why: it is non-deterministic, so you
cannot compare a screenshot to a screenshot, cannot gate it and cannot describe it to someone else —
in a repository that built `cargo xtask shimmer` because things that move when they should not are
its recurring failure. At 288 files it is a hairball. Trigger for a dedicated `Tab::References` and
a deterministic two-column focus view: someone uses the inline surfaces for a week and misses it.

**The impact modal is cut and the impact block is advisory.** Doc 15 sells it as a check — *"the
agent cannot supply the impact set, which is the property that makes the card a check rather than a
restatement"* — and the review's F9 is right that it is decoration on top of one. The gate that
actually holds is `approving_a_stale_proposal_is_refused` on the version token, which is exact.
Adding a second refusal axis driven by a derived cache means **Approve can fail for a reason with no
representation in any file**, which is the opposite of never-do #15's posture. So: the impact
summary may be *shown*, labelled as derived, and **it can never refuse an Approve**; `loom propose
--list`'s output shape does not change, because §6.4's scene-only mode has no index and a CLI
contract that wobbles on whether a `loom.toml` is above the cwd is exactly what ADR 0038 built the
headless path to avoid. The modal itself fires on **three** prefab instances in the only project that
exists; until someone is actually surprised by a deletion, it is a sentence in the existing
confirmation.

#### 2.16.7 Where the round-3 review is wrong, and one number restated honestly

**Its `Scatter.mesh` fixture does not exist.** F3 says to add `scatter_mesh_alias_is_indexed` *"with
`forest.loom` as the fixture"* and describes `--orphans` reporting a Scatter-only mesh as
unreferenced. Verified: `grep -rh 'mesh = "' assets/` returns exactly two lines, `"box"` and
`"cylinder"`, both **primitives**. **No `Scatter.mesh` in this repository names an `[[asset]]`
alias**, so the hole is latent rather than live and the failure the review calls CRITICAL has not
happened. The fix is still adopted — it is cheaper than either walker and covers every future alias
field — but the test must **author its own fixture**, because the scene it needs does not exist, and
the finding is correctly ranked as *anticipating* a hole rather than reporting one.

**"A slice after Stage 6, not a stage" is overruled on presentation, not on substance.** The review
is right about the size; it is smaller than Stage 5's hub. But every other unit of work in this plan
has a number, an exit criterion, a green-check line and a thing for the human to look at, and hiding
this one inside Stage 6 makes it invisible in the only table anyone will read. **Stage 12, two
slices, explicitly sized as the smallest stage in the plan.**

**And ADR 0003's revisit condition, stated so it does not depend on a number the design contradicts.**
The review's F10 is right: 288 tracked files includes 92 `.rs` and 44 `.md` that this design declines
to index, and what it actually walks is `assets/` — **122 authored files**, below the ADR's own bar
computed the ADR's own way. The count was always the wrong proxy. The honest justification is the
reference web, and it is measured: **161 `[[asset]]` declarations, 330 `{ asset = … }` references,
176 `path` values, 52 scenes, 3 prefab instances and 2 `extends` across a repository of 288 tracked
files.** That is a real cross-file question whether or not the Rust files are counted beside it. §3
records both numbers and leans on the second.

#### 2.16.8 The exit criterion, on the files that exist

ADR 0003's criterion is *"what would break if I changed the desk prefab?"*, answered correctly on a
project with 200+ files. There is no desk prefab in this repository. **There is a lamp, and it is a
better demonstration**, because the chain is two hops long and one hop misses half of it:

```
assets/test/prefabs/lamp.loom          the prefab
  ← assets/test/prefab_room.loom       declares it as "lamp", instances it 3x  (depth 1)
      ← assets/test/prefab_night.loom  declares prefab_room as "day", extends it (depth 2)
```

`loom graph assets/test/prefabs/lamp.loom --impact` must name **both** files, with the edge that
reached each. The design doc's own §2.7 query is one hop over `edge.src` and returns only the first,
which is why §2.16.2's traversal steps from a thing to *the file that claimed it* and recurses.

**And it must produce that answer without resolving anything**, which is §2.16.3 demonstrated on
real files rather than argued: resolve `prefab_night.loom` and it becomes a flattened copy of
`prefab_room.loom`'s nodes with no `extends` key anywhere in it, and depth 2 vanishes.

---

## 3. The ADR set

Next free number is **0022** (re-verified against the real directory at the top of round 3:
`docs/decisions/` holds `0000-template` through
`0021-a-reflected-hit-shades-with-the-materials-mean-albedo.md`, twenty-two files, nothing above
0021 — so 0022–0042 are all free and nothing below collides). Round 1 allocated 0022–0032; round 2's
four documents then claimed 0033 four times, 0034 three times and 0035 twice, and cross-referenced
each other's numbers as though they were stable; **round 3's two documents then did it again, both
writing "ADR 0042" and "ADR 0043" with incompatible content and both editing this table to
"twenty-two".** Allocated here, once, before any is written.

**Ordering principle: by the stage that first implements the decision**, so `docs/decisions/` reads
in the order the rework happened. The round-2 constraints review proposed ordering by dependency and
the usability review by build order; build order wins because a reader scanning the directory is
following a build, and both proposed orderings are otherwise equally arbitrary.

**Twenty-one ADRs — 0022 through 0042 — and that is the approval budget the human should see as one
number.** Round 1's §3 claimed twelve while listing eleven, which is why both round-2 reviews
computed twenty-one. Round 3 adds **one**, not two: doc 15 §9's own closing paragraph argues that
the CLI and MCP shapes are clauses of the store decision rather than a separate approval, and it is
right — a second ADR there would be approving a spelling. The surfaces shrank to three inline
sections and a Problems row, which is not an architectural decision either. **ADR 0043 is not
allocated and stays free.**

| # | Title | Decision | Rejected |
| --- | --- | --- | --- |
| **0022** | The editor is a crate; the runtime links egui because the HUD does | All editor UI moves to `loom_editor`. egui stays **unconditional** in `loom_render` — no `editor` feature, no `#[cfg]` on `draw_with_ui` or the `ui` pass. "Stripping the editor" means not linking `loom_editor`, enforced by two `check-deps.sh` rules. `loom-play` is a second `loom_cli` binary built `--no-default-features`; the `loom` binary carries no `required-features`. | Doc 06's feature-gated `loom_render` (breaks `hud.rs`, and `Viewer::draw` is a one-line wrapper around the thing it gates out, so it would force a second frame implementation — ADR 0018's exact defect). A third `loom_runtime` crate (a rename of nine modules both binaries need). `required-features = ["editor"]` on `loom` (makes the whole agent CLI need the editor). |
| **0023** | A project is a directory with a `loom.toml`; editor state lives outside it | Five manifest fields (`project.format/id/name/main_scene`, `engine.version`) plus two optional tables (`[ship]`, `[agent]`), `deny_unknown_fields`, **both `Option`s present from the first version that ships**, one reader in `loom_scene::project`, no scene list and no asset list. Layout `loom.toml`/`scenes/`/`assets/`/`builds/` is a convention nothing enforces — **and the engine repository, with fifty scenes under `assets/` and no `scenes/` directory, is the case that rule exists to permit.** `scenes()` skips `target`, `builds`, `out`, any dot-directory and `*.mine.loom`. All editor state under `$XDG_STATE_HOME/loom/` through **one path-keying helper** (§2.12); a project directory acquires no engine-written files. `[engine] version` advisory; a strictly newer project is refused at the hub and offered read-only. | Resurrecting `loom_asset::meta`'s UUID sidecars (a good design for a problem this engine does not have). A declared scene list (a second source of truth that drifts on the first `cp`). Doc 01's `<project>/.loom/`. Doc 06's `game.*` keys and second reader. Doc 07's `project.toml`. A `[project] kind = "engine"` discriminator (one field whose only consumer is a branch `[ship] exclude` answers with data). Generating the repo's manifest on demand instead of checking it in (the hub would behave differently in a fresh clone than in a used one). |
| **0024** | An `[[asset]]`'s `path` is resolved from the declaring scene; `id` is reserved | `docs/format/README.md` §3 currently says `path` is *"a hint for humans and nothing else — never resolved"*; the implementation joins it onto the scene's directory (`main.rs:1150-1163`). The spec is amended to match. **Resolution is relative to the declaring scene file and to nothing else: no project-relative resolution, no fallback, no search path.** `project://` is *reserved* as the spelling for project-root resolution should it ever be wanted; a leading `/` keeps meaning an absolute filesystem path. `id` is written and preserved and resolves nothing. A mesh alias that is also a primitive name resolves to the primitive and its `path` is never consulted (`main.rs:1146`); `blockout.loom` depends on this. | Leaving the contradiction. Making `id` primary now. **A project-relative fallback** — provably inert in this repository today (176 paths audited, zero resolve from the root) and it would not stay inert: a reference that resolves for a reason the author cannot see is worse than one that breaks at the moment of the edit, which is ADR 0024's own argument applied to itself. Building `project://` now. Making a missing file an error rather than a warning (`office.loom` is in `SCENES` and would fail to load; `MeshLibrary`'s degrade-visibly rule is deliberate and older than this design). |
| **0025** | The editor viewport is a sub-rectangle of the swapchain | The forward pass renders at the origin of window-sized images, sized to the dock rect; the tonemap writes the destination sub-rect with the source origin as a push constant; a `chrome_clear` pass goes through the graph and is named in the barrier-list test. `placement: None` is byte-identical to today, which is what keeps `loom render` and `loom run` in agreement. `Ui::draw` splits into a layout half called before the graph is built and a record half inside the `ui` pass, because egui's layout currently runs inside that pass and the rect would otherwise be a frame stale. Forecloses render scale, a scrolled or clipped viewport, a floating viewport beneath a panel, and egui effects on the image; the first of those that is wanted is the trigger to adopt the texture path, whose mechanism (`add_user_texture` over `create_vulkan_descriptor_set`) is recorded so the reversal is cheap. | Render-to-texture (a new image, sampler, descriptor set, resize policy, and a colour round-trip through egui's shader — a second place the window and the offscreen path can disagree, which this project has paid three defects for). Leaving the one-frame lag unaddressed (a stale scene rect against a live panel edge for every frame of a splitter drag, made permanent-looking rather than alarming by `chrome_clear`). |
| **0026** | The op vocabulary grows: `SpliceArray`, `Declare`, `SpawnNode{prefab}` | Nine ops → eleven, plus one optional field. `SpliceArray { node, field, index, remove, insert }` preserves the array-of-tables spelling on disk. `Declare { kind, key, id, path }` writes `[[asset]]`/`[[prefab]]`. `SpawnNode.prefab` is mutually exclusive with `mesh`. **A splice against a prefab instance materialises the resolved array as an override and splices that** — the only reading that does not silently depend on the prefab's contents. Callers: the sculpt brush, `WaterBody.waves`, `Buoyancy.pontoons`, `Scatter.excludes`, **`Scatter.remove`**, **`FoliagePaint.strokes`**, **`SplatPaint.strokes`**, **`PaintLayer.strokes`**, the ground layer, mesh import, prefab creation, the prefab browser drop, the duplicate-an-instance fix. **A `SpliceArray` where `remove > insert` is a net deletion and is what ADR 0038 classifies as destructive** — recorded here so the classifier is discoverable from the op that triggers it. No `format` bump. | Three named ops for append/remove/replace. `AppendVoxelOps` and `AppendToArray` — both subsumed. A dotted field path — verified impossible: `SetField` splits the field name once and uses the remainder as a literal TOML key (`ops.rs:690`), so it would write a key named `ops.3.radius`. CLI-only prefab creation and mesh import. |
| **0027** | Painted surfaces are stroke lists, rasterised on load | One `loom_asset::paint` module over one typed **`loom_scene::brush::BrushParams { radius_m, hardness, strength, flow, spacing }`** (in `loom_scene`, not `loom_asset` — a component cannot embed a type from a crate `check-deps.sh` forbids `loom_scene` to depend on), one falloff, one dab walker, three stroke types (`SplatStroke`, `PaintStroke`, `FoliageStroke`). **Radius is always world metres**, projected into UV at bake time from the mesh's texel density. Erase is `strength = 0`, not a mode, and the `Clear` preset sets `flow = 1.0` so erase can actually reach authority 1. No raster is ever an authored artifact. One stroke is one transaction on mouse-up (§2.5), and the in-progress preview is the one editor state permitted to diverge from the file. Texture upload is one `paint_upload` render-graph pass with `Access::TransferDst`, `import_with_layout(SHADER_READ_ONLY_OPTIMAL)` and the image declared in `forward_uses`; two new named transitions in the barrier-list test. The `fragmentMain` composite order and the `ObjectData` field map are fixed here (S1, S2). **Stroke coordinates are stored in the receiving node's local space** (§2.10), so a stroke survives moving the node it is painted on. | A PNG per stroke with an undo exemption. A binary tile-delta journal, and a checkpoint image plus journal. GPU-authoritative painting with readback (two implementations of one formula — ADR 0006's whole point). A new `loom_paint` crate (its stated dependency is `loom_asset` and nothing else, which is the definition of a module). `Vec<serde_json::Value>` strokes (a stroke has one shape; `VoxelVolume.ops` is untyped because it is a union of five, and its `invalid_voxel_op` funnel is the scar tissue that proves the cost). Doc 03's per-drag `apply_coalescing` commits. Doc 03's one-shot `Viewer::set_material_texture` (a hand-placed barrier on a live image at ten strokes a second — never-do #4). |
| **0028** | The splat mask biases the slope rule; it does not replace it | The mask carries a painted **value** and a painted **authority**; the shader computes `w = lerp(groundLayerWeight(…), value, authority)`. Untouched texels keep authority 0 and the live rule — including its low-frequency wander — is evaluated every frame. `loom_grass::Ground.rock` is fed from the same mask through the existing `&dyn Fn` closure, so painting rock also removes grass and the two boundaries cannot disagree. **Scope clause: this `lerp` form applies to shading weights only.** A painted mask over a *placement probability* composes multiplicatively — ADR 0039 — because a placement rule carries hard guarantees a blend weight does not. Grass is consequently touched by two masks, `rock` and `paint`, both multiplicative, so they compose with no ordering question. | A mask that simply *is* the blend weight (it would bake the rule's output into a raster, after which `GroundLayer.slope` does nothing on painted terrain — never-do #11 with a different noun). A triplanar mask (three samples on a *control* texture to gain authority over vertical faces the slope rule already handles correctly). |
| **0029** | Decals are box projectors evaluated in the forward fragment shader | A `Decal` component makes its node a unit-box projector along local −Y, oriented and sized by the node's own transform, evaluated inside `fragmentMain` from a list in `EnvironmentData`. No pass, no pipeline, no barrier, no G-buffer, no change to ADR 0018's order. Lit, fogged, HDR and MSAA'd. Capped at 16, unculled. Not in the TLAS, so invisible in reflections and casting no shadow — following ADR 0019's rule rather than excepting it. | A deferred screen-space pass (needs a G-buffer this 4× MSAA forward renderer cannot have; lands after the resolve with no AA and after the tonemap unlit). Geometry decals (z-fighting, no conformity to voxel surfaces, and they would become `Object`s and cast the ray-traced shadow of their own quad). Baking decals into a `PaintLayer` (fails on voxel meshes, on multi-node receivers, and on anything the human wants to move). |
| **0030** | Editor UI dependencies, icons and fonts | `egui_dock = "=0.20.1"` in `loom_editor` only, pinned, verified against `egui-ash-renderer 0.12.0`'s egui-0.35 line by `cargo add --dry-run` before it lands. Icons are hand-drawn `egui::Painter` shapes — **no icon font, no SVG rasteriser, no new binary asset class** — pinned by four geometry rules (16 pt box with 1 pt inset, one 1.5 pt weight taken from the current `WidgetVisuals`, three primitives, every endpoint on a 2 pt sub-grid) and a **budget of ≤ 24**, not by a fixed list. Fonts: ship on egui's bundled fonts, which have **exactly one weight**, so the type scale's SemiBold column is inert in slice one and headings are differentiated by size and `text_strong` alone; Inter + JetBrains Mono (both SIL OFL 1.1) only if the human still reads it as default egui after the palette, spacing and radius land, vendored to `assets/fonts/` with `OFL.txt` and a `SOURCES.txt` recording release tag, URL and sha256 per file. **Fonts are `loom_editor`-only, so a shipped game's HUD keeps egui's bundled fonts, deliberately.** `accesskit` stays off, with the reason recorded rather than the gap discovered. The hand-written `gizmo.rs` is kept and extended. | `egui-phosphor` (its stroke weight will not match the hand-drawn gizmo handles in the same window, and it costs a dependency and a licence entry for one screenful of geometry). `transform-gizmo`, which `LOOM-IMPLEMENTATION-ORDER.md:434` named — it brings its own camera math, and `gizmo.rs`'s load-bearing property is that picking and the gizmo project through *one* `View` with a test asserting `project` and `ray` are inverses. Shipping Inter unconditionally. A hand-rolled dock tree. Pinning a fixed icon list (four parallel documents already need twenty-two). |
| **0031** | Every editor action is a row in one command table | `loom_editor::command::COMMANDS`, `&'static [Command]` of plain data. The palette, menus, toolbar, tooltips, F1 help, displayed keybindings and `docs/guide/04-reference/commands.md` are all views of it — including Send, Stop, Approve, Reject, Attach view, Restart agent, and every foliage and paint verb. Keybindings resolve through `loom_input::ActionMap`, so the documented key and the key that fires are one lookup. Commands with unmet preconditions are shown greyed **with the reason**, never hidden — and `text_disabled` is legible (§2.7) because a stranger's first palette is mostly disabled rows. The generator is `loom docs [--check]` in `loom_cli`; `xtask docs` shells out to the `loom` binary. `xtask` gains no dependency on `loom_editor`, and `--check` does not join `green.sh`. The union "outside undo" list (§2.6) is a consequence recorded here. | Doc 07's `xtask → loom_editor` dependency. A fifth green check before the cost is measured. Hiding unavailable commands. |
| **0032** | Windows is cross-compiled for the GNU ABI, and "supported" means what Wine proved | `x86_64-pc-windows-gnu`, linked with `x86_64-w64-mingw32-gcc` (verified present: `mingw64-gcc-16.1.1-1.fc44`). Shaders need no work — `build.rs` runs `slangc` on the host and SPIR-V is `include_bytes!`d. `ash` dlopens `"vulkan-1.dll"`, and `loom ship` proves it by asserting the name is absent from the linked import table. Verification is sequenced **first**, before any Build UI. **"Windows supported" means: the build links, starts, and renders a reference-matching frame under Wine on the development machine. It has never been run on Windows, and the documentation says so in those words.** `loom ship` copies the whole project root minus a fixed exclusion list that includes **any root entry whose name begins with `.`** (subsuming `.git`, and closing the `.claude/worktrees/` hole that would otherwise ship a second copy of the workspace), plus the project's own `[ship] exclude`. The output-tree assertion is **recursive**: no path anywhere contains a `crates/` or `target/` component. | `-msvc` (needs the Windows SDK import libraries and CRT on a Linux box). Asking the user to build on Windows. Reachability-pruned asset copying. Running the golden gate on the second target (a Wine render is a build check, not a platform check). Copying `assets/**` rather than the project root — verified fatal: under ADR 0023's layout `scenes/` sits beside `assets/`, so every project created from a template would ship without its startup scene. A root-relative-names-only exclusion test (it passes while the tree contains the thing it excludes). |
| **0033** | UI colour is authored in display space and encoded exactly once | `crates/loom_render/src/ui.rs:88` becomes `srgb_framebuffer: true`, because the swapchain is `B8G8R8A8_SRGB` (`viewer.rs:2101`) and `egui-ash-renderer 0.12.0`'s shader pair is an identity on the vertex colour when the constant is `false` — so the hardware encode is a second one and the UI currently displays every colour lifted (a `#16191E` panel arrives as `#535860`; a designed 14.6:1 arrives as 6.7:1). Token tables are written in the colour intended **on the screen**, and **`loom_render::ui::tok`** — beside the module that owns the specialization constant, *not* in `loom_editor` — pre-warps each channel by the residual between the shader's gamma-2.2 and the hardware's piecewise sRGB, so a hex equals a pixel within ±2 bytes. **`loom-play`'s HUD reaches the same function**, which is the whole reason it lives there. The Stage 0 probe proves it, and proves it on **blends and text**, not only on opaque fills: swatches at α128 over mid grey and a paragraph at three text tiers, before and after, with acceptance *within ±2 bytes **and** text weight unchanged*. No golden image can see any of this — `xtask image` drives the offscreen `Renderer`, which never constructs a `Ui`. | Leaving it and tuning a palette to compensate (every ratio would be fiction and the palette would break the day someone fixes the encode). A `B8G8R8A8_UNORM` swapchain — **not** because golden references pin the swapchain (they do not; §7 of doc 11 disproves its own ADR's stated reason) but because it moves the encode into the tonemap on the window path only, creating a second place the window and the offscreen path can disagree. Doing the correction by hand in the token hexes (invisible and unreviewable in a table of 25 constants). Putting `tok` in `loom_editor` (gives a shipped game's HUD the uncorrected half of a two-part fix, permanently). |
| **0034** | The scene journal, and an external write becomes an undo entry | `loom_scene::journal` appends one JSON line per applied transaction — label, resulting version token, ops, actor — to `$XDG_STATE_HOME/loom/journal/<key>.jsonl`, **only when `LOOM_JOURNAL=1`**, which the editor sets on itself and which the agent's CLI calls inherit, so `cargo test` and both gates write nothing. The actor is derived from `LOOM_AGENT=1` with `$LOOM_ACTOR` as an override. `Session::adopt_external(text, label)` takes a new disk version **as an ordinary undo entry** — nine lines, `commit` with the op application removed — instead of clearing the stack. The editor adopts when it is clean and not mid-gesture and the journal explains the change; otherwise it falls back to today's `reload`. **The journal is a disposable cache and never a source of truth**: every entry is validated against the file's actual token before use, it is capped in entries *and* in file count, and deleting it loses labels and nothing else. This fixes a live defect — `Session::reload` clears undo, redo and gesture (`edit.rs:395-406`), so *"a twelve-op agent transaction undoes in one Ctrl+Z"* is false today for every transaction arriving through the file watcher. | Storing intermediate texts so a burst of agent writes becomes N undo entries (a second copy of the scene's history in a cache directory). Putting the journal in the project directory (a non-diffable engine-written file in a git repository gets committed, then trusted, then becomes a source of truth). Writing it unconditionally from `apply_to_file` (green check 3 acquires a side effect on `$XDG_STATE_HOME` and one file per test per run, forever). Auto-merging on a dirty session — never-do #15; the divergence banner keeps exactly two destructive choices and gains one that loses nothing, *"Reload, saving my version to `<scene>.mine.loom`"*, which carries ADR 0008's sentence and which `project::scenes()` skips. |
| **0035** | The editor colours recency, not authorship | The `agent` colour marks a *recent write the editor did not cause*, decayed over `CHANGE_FADE` seconds and held only in session memory. It never marks ownership. Human-authored content gets no colour at all. `Transaction` gains **no** author field and the `.loom` format gains **no** provenance key, because a scene file describes a scene and not who typed it. The History panel's agent rows and the viewport's agent marks are the only provenance surfaces, and `docs/guide/05-you-and-the-agent.md` says in words that the blue means "just now", not "theirs". | A persistent agent tint on nodes the agent created (needs authorship in the file — a `format` bump, a migration, and a field every hand-edit invalidates). Authorship in the transaction log only (survives a session but not a restart, so the same node is tinted or not depending on when you opened the editor — worse than either honest answer). Inferring authorship from git blame (an authored scene is often not in git, and a squashed commit erases it). |
| **0036** | Engine-owned assets resolve from the executable; templates are compiled in | Three things belong to the *engine* and not to any project: the default input bindings, the weather recording, and the project templates. **Templates are compiled into the binary** (`include_str!`, kilobytes of text, the same category as `loom_input::DEFAULT_BINDINGS`) and `loom new` writes them out, so the Templates rail and project creation work on an installed binary with no filesystem precondition. The other two resolve through `loom_scene::project::engine_assets()` — **exe dir → cwd → `find_root(cwd)` → compiled-in/synthesised** — with the open project's root consulted first so a project may own its copy. This is **not** a search path for `[[asset]]` entries (ADR 0024). It replaces two constructions that are wrong outside this repository: `load_bindings`' cwd-relative literal (`run.rs:2242`), under which a shipped game's rebinding silently does nothing, and `base.join("../audio/rain.wav")` (`sound.rs:57`, `main.rs:3238`), which addresses an engine-owned file as though the scene owned it and works here only because every gated scene happens to sit one directory below `assets/`. In this repository every lookup lands on the byte-identical file it lands on today, and `loom render` — which drives `xtask image` and 43 of `xtask validate`'s invocations — constructs no `Sound` at all. | A general search path for all asset paths (turns a broken reference into a working one at a distance). An `[engine_assets]` key in `loom.toml` (right exactly once, wrong after the first `loom ship`). Embedding `rain.wav` (3 MB of WAV in every build to avoid one lookup). **The two-branch definition doc 12 shipped** — `cargo test`'s working directory is the crate directory, so a test finds no `assets/` from `crates/loom_cli/`, and doc 12's own V6 is vacuous because it produces the same result before and after the change. |
| **0037** | The agent is a subprocess, and the panel is not the write path | `loom_editor::agent::Process` spawns a user-configured command as a child with the project root as its working directory, piped stdio, one JSON object per line each way, read by one `std::thread` into an `mpsc` channel drained in the egui frame. **No LLM client, no HTTP, no async runtime, no `loom_agent` dependency, no terminal emulator, no new crate dependency at all.** The agent mutates the scene only through the `loom` CLI and `loom-mcp`, exactly as it does with no editor running. Five keys are understood and **every unrecognised line renders as raw text rather than vanishing**. Stderr is captured always and shown on exit. The panel spawns nothing under `--frames`. The command and preamble are **user** state in `prefs.toml`, never in the shared project manifest (§2.13 H1), and the unconfigured panel offers a **Detect** button that probes `$PATH`, shows the exact argv, and writes it on the user's click. The composer stays live during `Thinking` and `Tool`, so a mid-turn correction is an ordinary `{"type":"user"}` line rather than a SIGTERM. | An in-process LLM client (`reqwest`/`tokio`/SSE/API keys/a tool-call loop inside the frame loop, and it makes Loom an agent harness). An MCP client in the editor (the same thing plus a protocol, talking to itself). A pty terminal emulator (two dependencies, and the approval card, the op rows and the inline render have nowhere to live). **The panel as the write path** — the agent would then behave differently with the editor open than without it, a second code path tested by nobody, in the write path. A socket or daemon, for the same reason. A `loom_agent_ui` crate (its stated dependency is `loom_editor` and nothing else). Shipping a vendor's argv in the engine's manifest schema. |
| **0038** | The destructive scope is enforced, and a gated transaction becomes a proposal | Transactions are classified on **net loss**: `RemoveNode`, `RemoveComponent`, and `SpliceArray` where `remove > insert`. One touching more than `approve_above_nodes` distinct nodes is **bulk** — a separate axis, because two hundred `SetTransform`s destroy an afternoon and every one is reversible in principle. **The gate is opt-in via `LOOM_AGENT=1`**, which the panel sets on the process it spawns, so `loom scene --tx` keeps its exit contract for every existing script, MCP caller and CI job. Under the gate a classified transaction is neither applied nor refused: it is written to `$XDG_STATE_HOME/loom/proposals/…` and the command reports `{"status":"proposed","id":…}`. The panel shows one card with the diff `apply_with(dry_run)` already computes (`ops.rs:124`) and Approve/Reject; `loom propose --list|--approve|--reject` is the headless equivalent. **Approve applies through the editor's own `Session`, so it is the human's transaction and one Ctrl+Z**, and it re-checks the version and refuses if the scene moved — which also makes two editors racing on one proposal safe for free. **`loom propose --wait <id>` blocks and prints the outcome plus the new token**, joining `loom_agent::TOOLS` as `propose_wait`, because without it the agent's turn ends at the proposal and its next write is `stale_version`. Under `LOOM_AGENT=1`, `--allow-destructive` **proposes rather than refuses**, so a deliberately-started script is not a dead end. A project's `loom.toml` may **tighten** the policy and can never loosen it. **The honest limit, documented in these words:** an agent that unsets `LOOM_AGENT` is out of policy and the policy cannot stop it — the same posture the `rhai` sandbox takes. | A plain refusal (the agent asks in conversation, the human says yes, the agent re-runs with the bypass flag — the gate is then advisory). Default-on for every CLI caller (changes the contract §1 lists as untouchable, and exit 0 stops meaning "applied"). Classifying `SpliceArray { remove > 0 }` as destructive (that is how you edit an array element in place — retuning a sculpt stamp, replacing a paint stroke — so the gate would fire on routine editing, which is the blind-approve regression arriving through the mechanism built to prevent it). Timed approval grants (a grant that outlives its intent is blind-approve with a clock). A project-supplied `approve = "none"`. |
| **0039** | A painted foliage mask multiplies the placement rule; it never overrides it | A `FoliagePaint` component carries a stroke list per field, rasterised on load into a CPU-only mask of value and authority, **stored in the node's local XZ** so it survives moving the node. `loom_grass::Ground` and `loom_scatter::Ground` each gain `paint: f32`, default `1.0`, supplied through the closure that already exists — **neither crate gains a dependency**, and the baker takes noise as a closure so `loom_asset` gains no `loom_field` edge either. `coverage` and `viability` multiply by it, and the factor goes *inside* `viability` so an erased region stops competing like a cliff rather than competing like poor ground. Untouched ground is exactly `1.0` and bit-identical; erased ground at full authority is exactly `0.0`; and because both rules are products containing a slope and a rock term, **no stroke can place foliage where the rule forbids it** — `slope_cutoff` keeps its documented meaning and the crater test passes unmodified. `value` is clamped to `LUSH` (1.6), so painting borrows the existing gully headroom — and **the budget meter multiplies by the mask's actual maximum**, because `area × density` under-reports a Grow-painted field by 60%. The mask is never uploaded: no bindless slot, no `paint_upload` pass, no barrier, no `ObjectData` field, **no shader change of any kind**. Stroke edges are broken up by modulating the dab radius with frozen low-frequency noise at bake time. | ADR 0028's `lerp` form (it lets a stroke override a hard cutoff, failing an existing test and reopening the floating-blades hole). A mask that *is* the coverage (never-do #11 with a different noun). The factor applied at `habitable`/`kept` separately (an erased region would thin its own fringe — the shaved-ring artifact from a third direction). A GPU-side mask. Perturbing the *sampled gain* to break up the edge (it breaks either erase-exactness or unpainted-identity). **World-XZ storage** (moving the `Grass` node regenerates the field in its new place and leaves the painted path behind). |
| **0040** | A species is a node; a hand-placed instance is a node; a removed instance is a point | Foliage is three tiers and the instances are never written. The rule is `Grass`/`Scatter`; the mask is `FoliagePaint.strokes`; the exceptions are hand-placed instances, which are ordinary scene nodes spawned by `SpawnNode`, and deletions, which are `Scatter.remove: Vec<[f32;2]>` **node-local** XZ points killing every instance within `spacing * 0.45` — provably at most one, from the crate's guaranteed minimum separation. Dragging a generated instance is `SpliceArray` into `remove` plus `SpawnNode`, **one transaction, one Ctrl+Z**. A species is a node rather than an array entry, so it inherits naming, selection, `Material`, prefab overrides and undo, and the Foliage palette is a filtered view of the hierarchy rather than a second model — **and it is tool-scoped UI, not a dock tab.** `SceneView` gains `instance_picks` so a generated instance can actually be clicked; without it `Scatter.remove`'s only author is the agent. `Scatter` gains `align: f32`, default `0.0`, byte-identical today, slerping toward the surface normal `GroundGrid` already computes and throws away. Grass gets no `remove` list and no align control — erasing a blade is meaningless and real grass is gravitropic. | A species array inside one node (a second hierarchy nobody can select in, needing its own naming, reorder op and override semantics). Removals as cell indices (renumber when `spacing` changes, so every deletion silently points at a different tree). Baked instance arrays (never-do #11's shape, and the diff goes dark). An erase brush as the only deletion mechanism (cannot remove one tree from a copse). A `Tab::Foliage` (Stage 3 fixes the enum four stages before the feature exists). Claiming the removal list adds a term to `reach_of` (verified: `REACH` cells already dominate `spacing * 0.45`). |
| **0041** | Grass generation is camera-centred, and the CPU pre-applies the shader's cull | *(Deferred to Stage 8 slice 3; drafted so the slice has somewhere to land.)* Blades are generated only for tiles within `GRASS_FAR + margin` of the camera, and each candidate is tested against the same cull the vertex shader applies, at a conservative distance one tile short so the CPU's survivors are provably a subset of the GPU's and no blade can pop. `MAX_BLADES` rises to 524,288. No shader changes. The ring regenerates on a camera tile crossing — the crater path with a moving crater, correct because a tile is a pure function of its coordinates. **The cull must be either expressed as a `loom_field` expression so the Slang is generated, or covered by an explicit CPU/GPU agreement test in the shape of `fields.slang`'s, and the ADR states that the cull constants are a shared pair that move together.** Only the *hash* is frozen ABI today; `grassCullDraw` and the falloff are hand-written Slang with no generator, so a straight CPU port is the divergence ADR 0006 exists to prevent with the direction reversed. | Doing it in slice 1 to fix the first-stroke truncation (a budget-derived clamp and a scene-global meter fix that with arithmetic and no divergence risk — §2.10). A placement compute pass with `vkCmdDrawIndirect` (the deferral's own trigger was scale, not cost, and it would additionally require hand-porting Voronoi clumping into Slang). Trusting the conservative-distance argument without a test (it holds for the current falloff and does not survive a one-line shader tune no gate can see). |

| **0042** | The knowledge graph is a derived in-memory index over unresolved scene text | A `loom_graph` crate depending on `loom_scene` alone, consumed by `loom_cli` (under the existing `editor` feature) and `loom_editor` and by nothing else — **two `check-deps.sh` stanzas in the two shapes the file already has**, a leaf rule and a reverse containment rule, plus the `--no-default-features` grep. Two node kinds (`file`, `node`) with **derived, path-based ids**, five edge kinds (`declares`, `instantiates`, `extends`, `references_asset`, `references_node`), each owned by the file whose text asserted it. **Scenes are indexed *unresolved*** — resolution erases the `instantiates` edge the exit criterion needs, breaks per-file ownership, and is recovered at two hops anyway; the rule is a module doc comment so it survives being quoted out of context. **An alias is resolved by one function shared with the loader** (`main.rs:1146-1170`'s ladder, moved beside `Scene::asset_path`), and **any string field whose value matches an `[[asset]]` key declared in that scene is a reference** — which needs no per-component knowledge and is the only rule that sees `Scatter.mesh`. **No store until a measurement demands one**, and the measurement is the stage's first commit: in memory, two `BTreeMap`s; then a single JSON file under `$XDG_CACHE_HOME` if the cold build is slow enough to matter; SQLite only past ≈3,000 files, where the swap is one file because the query functions are the only surface. **No file watcher**: one `refresh()` used by both consumers, on the poll the editor already runs. **`BTreeMap` only** — `clippy.toml` is workspace-root and green check 1 is `--workspace`. `loom graph <subject> --used-by\|--impact\|--broken`, subject first, exit 0/1/2, an `index` block on every response naming the files whose edges are missing because they do not parse. `graph_query` is one more `TOOLS` row wrapping it, **named questions, never SQL**. **The graph proposes; it does not write** — no automatic path from the index to authored state exists, and the dependency rules make one a green-check-1 failure; the human-mediated path's correctness rests on the version token, not on the index. The editor gains no file-deletion verb. | Doc 14's SQLite, `rusqlite` `bundled`, WAL, `busy_timeout`, `user_version`, the mtime/hash ladder, the `DELETE`-by-owner protocol and `--verify` — **the entire tree of consequences of a store decision taken before the measurement that justifies it**, which is the ordering Stages 8 and 9 exist to forbid, and which additionally puts a C toolchain requirement inside Stage 0's `cargo check --target x86_64-pc-windows-gnu` (`loom_cli`'s default features include `editor`). A uuid primary key (a fact the files do not carry, which makes the DB a source of truth). A `component_type`/`type` node kind and `attaches` edges (`describe_type` answers types from the registry; an edge from every scene to `Transform` is 100% noise). `contains`/`child_of` (364 `parent` keys, and the hierarchy panel already *is* that picture). `notify` (fires mid-write, drops on queue overflow, and would give the editor and the CLI two different freshness mechanisms — ADR 0037's objection). `mentions`, `derive_doc` and walking `docs/`+`crates/` (458 backticked tokens resolved by ambiguous basename, and it would put design documents in the impact answer, where a document that *refers* to a file is not a thing that *breaks*). Hardcoded (component, field) tables and extension-suffix matching (both blind to `Scatter.mesh`, and both guard tests pass while they are). Parsing `.rhai` and `.rs` bodies for `reads_component`/`emits`/`listens` and a `system` node kind (two real parsers, for edges the exit criterion does not use, and this engine has no `system` as a data object). A `.loom-cache/` in the project (ADR 0023 forbids engine-written files there; gitignoring is a request where absence is a guarantee). A SQL passthrough tool (couples the agent to a schema that must stay free to change, and turns the failure mode from "no such file" into "syntax error"). Doc 15's aggregation of `asset_file_missing` inside `alias_report` — it would make `loom validate` answer differently inside a project, and reach the one command the gate drives. |

**Deferred, written when built:** *CMAA2 moves ahead of the UI pass when the viewport is docked* —
an amendment to ADR 0018, separable because CMAA2 is opt-in and off by default (`LOOM_CMAA2`,
`viewer.rs:75`). It takes the next free number at that time.

**Not ADRs:** the theme itself (doc 11 is its specification), the shortcut table, the default
layout, the templates and base scene (they are `.loom` files or compiled-in text), end-user
documentation, `/builds` in `.gitignore`, the `#Object` fix in `alias_report`, `Camera.boom` (an
additive optional field with a default that reproduces today's behaviour — but it touches
`World::active_camera`, which every render and the editor's opening framing flow through, so it runs
the golden gate in its own commit), and the `SetTransform` `f32` fix (a defect against a format rule
the spec already states).

**Amended, not created: ADR 0003 moves from `proposed` to `accepted`, and this plan does not make
that edit.** `docs/decisions/0003-knowledge-graph-deferred.md` still reads `Status: proposed —
needs a human decision`; changing it is a one-file commit and it is **the first commit of Stage
12**, so the directory and the plan never disagree about whether the graph is being built.

The status line becomes **`accepted`**, and the file records four things:

1. **The outcome is option 2 in substance with the timing moved.** The ADR offered defer to M9.5,
   fold in as designed, or cut permanently. The graph is built — the indexer, `graph_query`, and
   M12's view scoped to inline surfaces — but *after* the editor rework rather than before M9, which
   is neither of the first two exactly and is closest to the second.
2. **Its own revisit condition was met, and it was checked rather than assumed.** The ADR argued
   *"the design doc's own exit criterion assumes 200+ files. The project will not have 200 files at
   M9."* Measured today: `git ls-files assets docs crates | wc -l` = **288**. §2.16.7 states the
   sharper number honestly beside it — the walk covers 122 authored files under `assets/`, and the
   justification that actually holds is the reference web (161 `[[asset]]` declarations, 330
   `{ asset = … }` references, 176 `path` values, 52 scenes, 3 prefab instances, 2 `extends`), not
   the file count.
3. **Its consequence 3 is discharged rather than struck.** The ADR says M12's knowledge-graph view
   clause "must be struck too — a view over a nonexistent index is not a feature" *if the index is
   never built*. The index is being built, so the clause stands — **scoped**, per §2.16.6, to three
   inline surfaces and a Problems section rather than a dedicated tab or a picture, and §5 records
   the trigger that would bring the rest back.
4. **Its consequence 1 is now wrong in the way that matters least.** *"The tool set ships as nine,
   not ten"* — verified, `loom_agent::TOOLS` holds **eight** entries today (`lib.rs:24-40`), Stage 6
   adds `editor_context` and `propose_wait`, and `graph_query` is one more. The design doc's "ten
   always-loaded tools" was never a budget anyone held to and is not one now.

Nothing else in this plan depends on the answer, which is exactly what made deferring it cheap and
is why accepting it costs one stage rather than a migration.

---

## 4. Staged implementation order, with checkpoints

**Thirteen stages, 0–12.** Round 2 proposed a "Stage 5A" and a "Stage 7½" to avoid renumbering, and
two decimals threaded through a plan that opens *"ten stages"* is worse for the person following it
than one mapping table is. Round 3 adds Stage 12 on the end, which needs no renumbering at all. Old
→ new, so citations in `01`–`16` stay resolvable (docs `14`–`16` were written against the new
numbering and cite Stage 12 directly):

| Old | New | | Old | New |
| --- | --- | --- | --- | --- |
| 0–5 | 0–5 (unchanged) | | 7 ("sculpt") | **9** |
| 5A ("agent") | **6** | | 7½ / 6½ ("foliage") | **8** |
| 6 ("material/decals/splat") | **7** | | 8 ("UV paint") | **10** |
| | | | 9 ("Windows/docs") | **11** |

**Foliage moves ahead of sculpting**, against doc 10's own placement. Its stated dependency on
sculpting is *"§9's 'sculpt under painted grass and it follows' criterion"*, which is an acceptance
test, not a compile dependency — the mechanism is `grass_key` including every `VoxelVolume`
(`main.rs:1641`), which works today. User decision 3 names foliage explicitly, and the earlier it
lands the more sessions it gets before Stage 11's documentation freezes what it is. The criterion
becomes a checkpoint item in Stage 9 instead.

Each stage ends at a point where the editor runs, the four green checks pass, and there is a
specific thing for the human to look at. **Stages 0–3 are strictly sequential**; 5 can be slotted
anywhere after 3; 6 needs 3, 4 and 5; 7–10 are sequential among themselves; **12 needs 1, 4 and 5
and slots anywhere after 5, ideally before 11 so the guide covers it in the same pass**; 11 is
otherwise last. **Number is not order** — that has been true since Stage 5 and Stage 12 is written
last in this section because it is numbered last, not because it is built last.

Stage 12 is deliberately *not* given a dependency on Stage 6. Doc 15 put the impact block in the
agent's proposal card and derived a dependency from it; §2.16.6 cuts that block down to advisory and
the dependency with it, which is what lets the graph land at any point after the project model
exists.

---

### Stage 0 — Probes and one-line fixes · half a day

**No new features. This stage exists because nine cheap facts can invalidate later stages, two of
them are live bugs, and one more is three doc comments that say the opposite of the code.**

Built:

1. **`prefab_load::for_reading` in `SceneView::build` and `build_cached`** (§2.14) — the S4
   regression, verified live, with the test that asserts the instance's paths appear in `paths` and
   its bounds in `picks`. `loom explode` (`main.rs:3440`) in the same commit.
2. **`SetTransform` emits `f32` shortest-round-trip** (`ops.rs:680`, verified the only `f64::from`
   in the file), reusing the trick `prefab.rs:186` already has. **Its own commit**, with the
   one-time numeric churn stated in the message. Grid snapping's defaults depend on this.
3. **`assert_eq!(size_of::<renderer::Push>(), …)`** — the test that struct's doc comment already
   claims exists (§2.4).
4. **The `#Object` fix in `alias_report`** (`main.rs:483` joins without stripping the selector that
   `MeshLibrary` strips forty lines earlier), with a regression test asserting `props.loom`
   validates with zero `asset_file_missing` warnings.
5. **`op_index: Option<usize>` on `TransactionError`**, set at the two `apply_one` call sites
   (`ops.rs:226`, `:229`). Additive to a *result* payload, so no `format` bump. Every later stage's
   error messages get better and the cost never drops.
6. **`ui.rs:88 → srgb_framebuffer: true` and `loom_render::ui::tok`** (ADR 0033), behind the probe:
   `loom run --edit --theme-probe`, fifteen lines of egui in `loom_cli`'s existing panel path,
   rendering the swatch strip, the same swatches at α128 over mid grey, and a paragraph at three
   text tiers. Screenshot before and after; accept on *±2 bytes **and** text weight unchanged*.
7. **Windows V0/V1**: `rustup target add x86_64-pc-windows-gnu`, `cargo tree --target … -e normal`,
   `cargo check --target …`. Metadata and type-checking, no codegen.
8. **Does an inspector `Material` edit reach the GPU?** Open `loom run --edit`, drag a roughness
   slider, look. Verified prerequisite: `Viewer`'s entire public mutation surface is `set_grass`,
   `set_rain*`, `set_terrain`, `set_meshes` — there is no `set_materials`.
9. **Three scratch benchmarks that gate Stages 7, 8 and 10**: a CPU brush stamp at the proposed 512²
   dirty-rect clamp; a `PARTIALLY_BOUND` descriptor array with unwritten slots under the validation
   layers; and **ten lines of the configured agent CLI's actual stdout**, captured by hand, because
   doc 09 §10.3 admits its wire example is written from memory and the whole of Stage 6 is built on
   it.
10. **Three doc comments that contradict the code, corrected in one commit** — the same class as ADR
    0024's amendment to `docs/format/README.md` §3, and found the same way. `Script.path`
    (`components.rs:1676`) and `GameRules.path` (`:1684`) both say *"Project-relative path to a
    `.rhai` file"* and both are joined onto the **scene's** directory (`play.rs:1090`;
    `proving_ground.loom:89` writes `"../scripts/fps.rhai"`). And `Scene::asset_path`
    (`scene.rs:161-165`) says *"nothing resolves a reference through it at runtime"* while
    `main.rs:1150-1163` does exactly that — which is the contradiction ADR 0024 already resolved in
    the format spec and never propagated to the source. Stage 12's extractor reads all three; a
    comment that lies about a resolution base is how the index acquires a second, wrong resolver.

Runnable at the end: `loom run --edit assets/test/prefab_room.loom` **draws the room**.

Green checks: **all four.** Check 4 must show zero moved references except where the `f32` fix
rewrote a scene, which must be numerically identical. The `ui.rs` flip cannot move a reference —
`loom render` constructs no `Ui` — and if it does, something is wrong that is worth stopping for.

Human looks at: the prefab room in the viewport, where there was nothing. The two theme-probe
screenshots side by side — did the text get thinner? The printed `cargo tree --target` output — does
`alsa-sys` or `x11-dl` appear? A before/after screenshot of a roughness drag. The `f32` churn diff
on one scene, read line by line.

---

### Stage 1 — `loom_editor`, the inspector, and the token module · the largest user-visible win

**This is ahead of the docking work deliberately.** Two round-1 reviews independently found the
inspector is the single largest omission in the design set and the highest value per line, and doc
01's own theme experiment establishes the right shape: fix the thing over the *old* layout first,
and the editor is measurably better before a single tab moves.

Built:

- **`crates/loom_editor/`**, with `panels.rs`, `gizmo.rs` and `UiAction` moved wholesale.
  `run.rs` stays in `loom_cli` for now. Both `check-deps.sh` rules land here.
- **The `toml_edit` spike, first**: can `SpliceArray` write `[[node.components.VoxelVolume.ops]]`
  nested inside an `[[node]]` array-of-tables, preserving whichever spelling is on disk? Does
  `Scene::parse` accept both spellings? **If this fails, stop and redesign before anything is built
  on it** — sculpting, foliage, prefab-instance duplication and four inspector fields sit on it.
- **`SpliceArray`, `Declare`, `SpawnNode{prefab}`** (ADR 0026), including the prefab-instance
  semantics (materialise the resolved array as an override, then splice) and its test.
- **`theme.rs` as a token module only** — the constants, calling `loom_render::ui::tok`, no
  `apply`. Half a day, and it stops Stage 3 having to re-read every line of the largest new surface
  in the rework for `Color32::from_rgb` literals. Doc 11 §13 is right about this split.
- **`Viewer::set_materials`** — a rebuild written as a sibling of `set_meshes`, copying its
  `device_wait_idle`-then-`reset_command_buffer` discipline and its "build the new before destroying
  the old" structure (`viewer.rs:841-876`). The minimum that makes a colour picker real.

  > **Correction, found while reading the source rather than while writing the code: a rebuild is
  > not unconditionally safe, and for a colour picker it is not needed at all.**
  >
  > `Materials::new` sizes its descriptor set layout from the texture count —
  > `descriptor_count(slots)` at `material.rs:160-166`, `slots = textures.len().max(1)`. Two
  > descriptor set layouts are pipeline-compatible only when *identically defined*, and a different
  > `descriptor_count` is not identically defined. So a rebuild that changes the **texture set**
  > invalidates every pipeline built against the old layout, and binding the new set is a validation
  > error rather than a wrong pixel. The plan's one-line "rebuild as a sibling of `set_meshes`" does
  > not cover that, and `set_meshes` is not a precedent for it: meshes live in buffers reached by
  > device address, which have no descriptor and therefore no compatibility rule.
  >
  > **So the operation splits, and the split follows what the inspector actually does:**
  >
  > 1. **A value changed, the texture set did not** — every colour swatch, every scalar, i.e. all of
  >    Stage 1. This needs no descriptor work and no rebuild: rewrite the material buffer's contents
  >    and nothing else. Cheapest correct thing, and it cannot desynchronise a pipeline.
  > 2. **The texture set changed** — assigning an `albedo_map`, which is Stage 4's asset picker. This
  >    needs the full rebuild *plus* pipeline recreation, and it should be built when the feature
  >    that needs it is, with the validation layers watching.
  >
  > Stage 1 builds (1) only, and the name should say so. Building (2) speculatively is exactly the
  > shape of thing this plan cuts elsewhere.
- **The inspector**: a recursive schema walk following `$ref` through `$defs` with
  `loom_reflect`'s existing `resolve` (never a second walker); string editing; enum dropdowns via
  the `oneOf`+`const` spelling `loom_reflect/src/lib.rs:233-258` already parses; `[f32;3]` colour
  pickers; an `AssetRef` picker; array-of-object rows over `SpliceArray`; **prefab override display
  and per-field revert** (`RevertOverrides` has existed since S4 and the editor has never issued
  it); multi-selection editing; component headers from the schema root description; a fixed 96 px
  label column; the constraint range as the tooltip's last line.

Runnable: `loom run --edit`. Every string, enum, colour, asset reference and object array in this
engine is editable. Prefab overrides are visible and revertable.

Green checks: **all four.** New tests: `SpliceArray` preserves the array-of-tables spelling;
append-then-delete round-trips byte-identical; a splice against a prefab instance materialises the
override; the existing `edit.rs:457` twelve-op test and `:498`/`:514` gesture tests still pass.

Human looks at: set `Script.path` from the inspector on a node that has one — the single most
limiting gap in the current editor. Pick a `Material` albedo with the colour swatch and watch the
viewport follow. Open `prefab_room.loom`, change a field on an instance, see the override marker,
click revert, watch it go back. Edit `WaterBody.waves` without touching a text editor.

---

### Stage 2 — The frame's shape · the trunk, and the riskiest stage

Built:

- **`Ui::draw` splits into `layout()` and `record()`** (§2.15, §6 R1). `layout()` —
  `take_egui_input` → `run_ui` → `handle_platform_output` → `tessellate` — is called *before*
  `draw_with_ui` builds the graph; `record()` — `set_textures` → `cmd_draw` → `free_textures` —
  stays inside the `ui` pass. ~80 lines, and it must be here, because retrofitting it after the
  dock exists means re-auditing every panel for "does this read state the frame has not produced".
- **`ViewportPlacement`**, threaded into the forward, water and rain viewports/scissors/render
  areas; the tonemap writing the destination sub-rect; `TonemapPush` reordered to
  `int2 origin; float exposure;` (12 bytes under either packing rule).
- **`chrome_clear`**, through the graph, named in the barrier-list test.
- **`ViewportPlacement::new` clamps to 1×1 and to the swapchain**; the editor skips the scene passes
  entirely under 8 px in either axis.
- **`loom render --viewport x,y,w,h`** and the `viewport_rect` GOLDEN entry.

Runnable: `loom run --edit` draws the scene into a hardcoded 200 px inset with the existing panels
over it.

Green checks: **all four.** Check 4 must show **zero** moved references for every existing scene,
plus the one new `viewport_rect` reference. A moved reference means the change is wrong, not that
the reference needs blessing.

Human looks at: **drag the window edge and watch the seam between the scene and the panel.** That
observation is why this stage exists — a stale rect against a live panel edge is the one respect in
which render-to-rect is strictly worse than render-to-texture, and the `Ui` split is the fix. Then:
does the inset scene align exactly with its rectangle at a HiDPI scale factor? Drag a splitter to
zero width — zero validation messages?

---

### Stage 3 — Coordinates, theme, dock, identity

Built, in this order:

- **`to_viewport` / `to_window`**, and every consumer moved onto them: `pick_at_cursor`,
  `drag_gizmo`, `press_in_viewport`, handle recomputation, `agent_marks`, `gizmo_overlay`,
  `agent_overlay`, the fly camera's aspect, `FlyCamera::at`'s framing, and **the HUD's
  `available_rect_before_wrap` anchoring, which must now anchor to the Game tab and not the
  window**. Overlays move off `LayerId::background()` to a foreground layer scoped to the tab.
  `gizmo.rs`'s inverse test is extended to a non-zero origin.
- **The theme** — `apply`, `Spacing`, `text_styles`, `icons.rs`, panel composition, empty states,
  motion, high contrast, `--theme-probe` moving into `loom_editor`, and the font-swap checkpoint.
  Applied over the *old* panels first: doc 01's step-4 experiment, kept because it is the cheapest
  possible test of whether the palette reads as sleek and it is reversible in one file.
- **The three bespoke spends** (§2.7): the shuttle busy indicator, crossing threads, the hub
  lattice. Cheap, reusing `icons.rs`'s primitives, and they are what user decision 6 actually asked
  for.
- **`egui_dock`**, the `Tab` enum, the default layout, layout persistence in XDG state, the Window
  menu, maximise-on-hover, `--frames` ignoring the saved layout **and forcing scene-only mode**
  (§2.11).

**The `Tab` enum is fixed once, here: eleven variants ending in `Agent`** (§2.9). `Environment`,
`Terrain`, `Events`, `Profiler` and `Foliage` are cut — a tab variant with an empty body is worse
than no tab, and adding one later invalidates every saved layout. `Agent`'s body until Stage 6 is
its real body when unconfigured: one paragraph, a Detect button and a copyable snippet.

**The default layout takes Unity's four regions but one bottom node, not two.** Doc 01 §3 argues
copying Unity buys the only free familiarity available and then adds a full-width Project strip
Unity does not have — 180 pt of console plus 160 pt of project under a 28 px menu, a 36 px toolbar
and a 22 px status bar leaves a 42% viewport in an editor whose subject is a 3D scene. `Project`
becomes a tab of the bottom node with `Console`, `Problems`, `History`, `Transactions` and `Agent`;
the bottom node is 280 pt (§2.9).

Runnable: `loom run --edit` is Unity-shaped, docked, themed, with every existing panel plus the
Stage 1 inspector in a tab, and the layout survives a restart.

Green checks: **all four.** From here the windowed half of check 2 exercises the docked path
automatically (§2.8) — verified, the gate already passes `--edit`.

Human looks at: **does it read as sleek?** That judgement is what the theme step exists for and no
gate substitutes — **and run `--theme-probe` and sample three swatches before judging**, because a
palette judged through a wrong encode will be judged as a bad palette. Then: click an object at the
far corner of the viewport in three different dock arrangements — does the right thing select? Tear
a panel off and re-dock it. Restart and confirm the layout came back. Press Play in a docked Game
tab and move the mouse to the panel edge (R11).

---

### Stage 4 — Tools, commands, history, and the journal

Built:

- `Outcome`/`Edit`/`Tool`/`ToolEvent`/`ToolCtx`; `cursor::under_cursor`'s three tiers (mesh AABB →
  voxel SDF sphere-march → ground plane → focus plane).
- Create menu with `quad` added to `primitives::NAMES`; **Create → Terrain** issuing `SpawnNode` +
  a `VoxelVolume` with one `terrain` op, so the sculpt and foliage brushes have a target in the
  scene every new project opens to.
- `snap.rs` (absolute, not delta), `arrange.rs` over `place::resolve` (~80 lines, the best reuse in
  the set), marquee select, group/ungroup with compensating transforms through
  `SceneView::parent_inverse`, Shift-drag duplicate under the drag's own gesture key.
- The nine gizmo improvements: plane handles, screen-space translate, local/world basis, arcball
  rotation replacing the 45°-per-unit gearing, uniform-scale centre handle, live numeric readout,
  a multi-selection gizmo issuing one `SetTransform` per node in one transaction under one gesture
  key, median/individual pivot, rect-relative coordinates.
- **Viewport chrome** (doc 11 §7): `overlay::stroked`'s three-layer sandwich, `chip`, selection
  corner brackets, the bounded tool-scoped grid, the gizmo restyle, **and routing the existing agent
  overlay through `stroked`** — `panels.rs:680` paints a bare 1.5 px stroke that vanishes on a
  bright render, which is a live defect and the proof the rule matters.
- Prefabs: instancing by drag, revert, unpack, create-from-selection (two steps, said out loud),
  and `loom prefab create --from --out` **before** the button, because property 2 is not satisfied
  by an editor-only verb.
- `COMMANDS`, the palette (Ctrl+K), F1 help rendered from the schema, `loom docs [--check]`.
- **Problems** (validation errors with `explain(&FieldError)`, `loom_physics::sanity`'s findings
  live as you author, and **the undeclared-alias diagnostic** — §2.13 H5) and **History**.
- **`loom_scene::journal` and `Session::adopt_external`** (ADR 0034), because they change what
  History shows.

**History is not optional polish, and round 2 completed the finding.** Doc 07 §8 is the only place
in this repo that noticed that *every agent write reaching a clean editor silently destroys the
human's saved undo history* — `poll_file` → `Session::reload` → `edit.rs:395-403` clears undo, redo
and gesture, for a correct reason. Doc 09 noticed the clearing is usually *avoidable*. So:
**the common case is now an ordinary, undoable, agent-tinted row**, and the rule drawn where the
agent wrote — *steps above this cannot be undone* — becomes the **fallback**, drawn only on rows
where the adopt was refused (dirty session, or a journal that cannot explain the change). The
adopt is deferred while a gesture key is live, because `apply_coalescing`'s contract says an
ordinary apply ends the run and adopting mid-drag would cut the human's drag in half.

Runnable: block out a scene entirely with the mouse. Every action has a palette row, a keybinding
and a written transaction label. An agent write while the editor is clean lands as one undoable row.

Green checks: **all four.** Plus the **hand-and-agent parity check**, which is M12's exit criterion:
create-a-cube, drop-on-surface and distribute performed in the editor and by `loom scene --tx` /
`loom place --op`, then `diff`ed. Identical by construction — align, drop and array go through
`place::resolve` precisely so this holds by construction rather than by luck. Plus a gesture test
per new gesture key, in the shape of `edit.rs:498`. Plus:
`journal_round_trips_and_validates_against_the_file`, `adopt_decision_table` (the five rows as
data), `adopted_agent_transaction_undoes_in_one_step` (the twelve-op test at `edit.rs:457` re-run
through `adopt_external`), and **`undo_after_adopt_saves_against_the_disk_token`** (§2.13 M9).

Human looks at: **does the gizmo feel attached rather than geared?** (the arcball is the change).
The three parity diffs, side by side. Rotate six props about a shared pivot and undo it once.
**Let the agent write the scene while the editor is open, then press Ctrl+Z — does its transaction
come back out in one step?** That is the flagship promise, and until this stage it was false.

---

### Stage 5 — Hub, projects, templates, and a Linux ship · slottable any time after Stage 3

Built: `loom_scene::project` with `find_root`, `scenes()` and `engine_assets()`; **the repo's own
checked-in `loom.toml`** with `main_scene = "assets/games/proving_ground.loom"` and its `[ship]
exclude`; **compiled-in templates** (ADR 0036); `loom edit` (hub with no argument, project or scene
with one), with `loom run --edit` forwarding to the same entry in scene-only mode; `loom new`; the
hub UI (recents, thumbnails by subprocess `loom render`, a directory browser over `read_dir` rather
than a new dialog dependency); the bindings and weather-bed lookups moving onto `engine_assets()`;
`loom-play`; the runtime/editor split of `run.rs`; `loom ship` for the host with its assertions;
`/builds` in `.gitignore`.

**`Camera.boom` lands here, in its own commit, with the golden gate run**, because it changes
`World::active_camera`, which every render and the editor's opening framing flow through, and
doc 02 §12.4 admits the sign convention is inferred from a doc comment. **`third_person` is held
until `boom` is proven** — shipping a template whose header comment apologises for it is worse than
shipping two.

**The highest-value affordance in the entire design set lands here, and it is twenty lines.** The
round-1 usability review traced the default first-run path and found it runs into a failure the
design itself calls unacceptable: a user picks "Empty" (because that is what "empty project" means
everywhere), the onboarding tells them to press Play, and the scene has a `Camera` and no
`CharacterController`, so the console prints *"no player rig — flying instead"*. **Fix:** templates
are named by outcome, not implementation — *"Walk around (first person)"*, *"Follow a character
(third person)"*, *"Blank scene"*, in that order, defaulting to the first; and Play on a rig-less
scene raises a viewport banner — *"No player in this scene — flying camera instead"* — with an
**Add Player** button that spawns `CharacterController` + child `Camera` + `Script` in one
transaction over ops that all exist.

Runnable: `loom edit` opens a hub, creates a project, opens a base scene, and `loom ship` produces
a folder you can double-click. The engine repo appears in recents like any other project.

Green checks: **all four**, plus `loom ship`'s assertions, plus these, which are the byte-identity
proof the repo-as-project decision owes:

- **V1** — `git status --porcelain` after the change lists `loom.toml`, `.gitignore`, `docs/`, the
  new `loom_scene` module and the edited `loom_cli` files. **No `.loom` file and nothing under
  `tests/references/`.** A scene in that list means the design was not followed.
- **V2** — `cargo xtask image`: **`tests/references/MANIFEST.txt` byte-unchanged.** No count is
  asserted (§2.13). A moved reference means something reads `loom.toml` that should not —
  investigate, never `--bless`.
- **V3** — `cargo xtask validate`: 43 scenes, zero Vulkan messages, and the warning **set**
  difference contains only `asset_file_missing`.
- **V4** — `project_root_paths_do_not_resolve`: a temp project whose scene declares a
  project-relative texture path must fall back and warn. **This test is the design** — without it
  someone reasonably adds the fallback.
- **V5** — `engine_repo_is_a_project`: `project::load(repo_root())` succeeds and `scenes()` returns
  a set containing every entry of `SCENES`, **with unique paths** (which is what fails if the
  dot-directory skip is dropped, because `.claude/worktrees/` would double it).
- **V6** — cwd independence: `loom new /tmp/p --template first_person`, then from `/tmp` render
  `/tmp/p/scenes/main.loom` and require a non-empty PNG and a non-empty alias report.
- **V7** — the weather bed both ways: `"recording"` in the repo, `"synthesised"` and *said once* in
  `/tmp/p`.
- **V8** — `loom ship` on the repo: **recursively** assert no path in the output tree contains a
  `crates/` or `target/` component, while `assets/` and `loom.toml` are present.
- **V9** — the installed case: copy the binary alone to an empty directory, `cd` elsewhere, run
  `loom new /tmp/q --template first_person`, and require it to succeed. This is the one V6 cannot
  see and the one every stranger is in.
- The shipped-tree smoke run: launch the shipped executable *from its own directory* with
  `--render` and require a non-empty PNG.

`empty` and `first_person` join `SCENES`; `empty` joins `GOLDEN`.

Human looks at: create a project from the hub and press Play — does a character move? Run the
shipped folder from `/tmp`. Read the `.loom-build.json` report. Confirm `cargo tree -p loom_cli
--no-default-features` mentions no `loom_editor`. Open the engine repo from the hub and confirm
fifty scenes with real meshes, textures, scripts, prefabs and terrain recipes all still load.

---

### Stage 6 — The agent panel

Built: `loom_editor::agent::{process, panel, proposal, context}` (ADR 0037); the conversation with
compacted tool rows and coloured event rows; the proposal card over `Applied.diff` (ADR 0038);
`loom propose --list|--approve|--reject|--wait`; `loom context` and `editor_context` in
`loom_agent::TOOLS`; `loom render --eye x,y,z --look x,y,z` and the "Attach view" chip; the
`LOOM_AGENT` / `LOOM_JOURNAL` markers; the Detect button and the unconfigured empty state; the
live composer during `Thinking`/`Tool`.

**Nothing here is in the write path.** The agent writes through `loom scene --tx` exactly as it does
with no editor open, which is what keeps the editor's presence from being a variable in the agent's
behaviour. **The one exception is Approve**, because there the actor is the human and the
transaction it applies is the human's — which is precisely what makes an approved twelve-op deletion
one Ctrl+Z.

**A landed-transaction row is a link into History, not a copy of it.** History remains the
authoritative list of everything that happened to the scene; the Agent panel shows the subset that
happened during this conversation, in conversational context. Clicking a row selects the History
entry and frames the affected nodes. Two panels, one truth.

Runnable: type "make the crates smaller", watch the transaction land in the viewport and the row
appear in History; ask it to delete six nodes, read the card, approve it, undo it in one keystroke.

Green checks: **all four.** No rendering path, no component, no scene — `SCENES` and `GOLDEN` are
unchanged. New tests: `destructive_classifier` (one case per op kind, plus `SpliceArray` at
`remove == insert`, `remove < insert` and `remove > insert`);
`approving_a_stale_proposal_is_refused` (never-do #15 as a test rather than a promise);
`an_unknown_line_from_the_agent_renders_rather_than_vanishing`; and a test that the panel spawns
nothing under `--frames`.

**Exit criterion**, in the shape M12's was: *ask the agent to delete six nodes; the transaction
arrives as one card with a readable diff; approving it lands one History entry; one Ctrl+Z restores
all six; the scene file after the undo is byte-identical to the scene file before the approve* —
**and the agent, still in the same turn, sees the outcome and continues.** That last clause is the
one doc 09's criterion omitted and it is the one that distinguishes a conversation from a
fire-and-forget.

Human looks at: does the card's diff fit and read (S17's whole argument)? Ask for something
destructive and reject it — does the agent hear the reason and adapt? Kill the agent process
mid-turn — is the scene untouched and does the stderr say why? Type a correction mid-turn.

---

### Stage 7 — The material path, decals, splat painting · cheapest surfaces first

Built, in this order:

1. **The `paint_upload` render-graph pass** (S6/ADR 0027): a persistent staging buffer, one
   `vkCmdCopyBufferToImage` with a region per mip level, `Access::TransferDst`,
   `import_with_layout(SHADER_READ_ONLY_OPTIMAL)` (verified to exist,
   `loom_render_graph/src/lib.rs:411`), the paint image declared in `forward_uses`, two new named
   transitions in the barrier-list test, and `PAINT_HEADROOM` on the descriptor array.
2. **Decals** — the cheapest of the painting systems, the only one that needs no brush, and the one
   that works on voxel terrain. `Decal` component, one loop in `fragmentMain`, a device address and
   count in `EnvironmentData`, `decals.loom` covering a mesh, voxel terrain, a grazing angle and two
   overlapping decals (blend order is array order, and a silent reorder is otherwise invisible).
3. **`loom_scene::brush` + `loom_asset::paint`** — `BrushParams`, the falloff, the dab walker, the
   two bakers, `stamp_incremental`, and the
   `incremental_painting_equals_a_full_rasterisation` test that is the correctness gate of all
   painting, foliage included.
4. **Splat painting** — `SplatPaint` with **node-local stroke coordinates**, the authority channel
   (ADR 0028), the `ObjectData` field map, the `Ground.rock` grass hook, `painted.loom`.

Runnable: paint rock onto a hillside and watch grass retreat from it; stamp a scorch mark on voxel
terrain that no UV brush could ever reach.

Green checks: **all four.** Every *existing* reference must be unmoved — that is the check that the
branches are per-object uniform and that `lerp(w, x, 0.0)` is exact. `cargo xtask shimmer` must read
**0.000** on both new scenes with a static camera; anything above zero means something samples with
a per-frame-varying coordinate.

Human looks at: the painted boundary on a hillside — does it wander, or does it draw the shaved ring
`groundLayerWeight`'s noise exists to prevent? The decal at a grazing angle. Two overlapping decals
in the authored order. Move a painted node and confirm the paint moves with it. And a stroke's diff
in `git diff` — is it readable?

---

### Stage 8 — Foliage · user decision 3

**Gated on two measurements, taken before any UI is drawn**, in exactly the discipline Stage 9
already establishes: `grass_blades` wall time per tile at density 140, and `Session::apply` on
`proving_ground.loom` with a 200-point stroke in the array. The known number is the one that hurts:
`build_cached` calls `scatter_objects` unconditionally (`scene_view.rs:118`) and its own comment
records **103 ms on `forest.loom`** — and a paint stroke *is* a file change, so today every foliage
commit re-places every scatter field in the scene. A brush whose mouse-up costs 100 ms feels broken
and no preview hides it.

**Slice 1 — the mask and the grass brush.** `FoliagePaint` with node-local strokes, the foliage
baker (taking noise as a closure), `Ground.paint` and the one factor in `coverage`, the tool, the
tool-scoped palette, the ragged-edge break-up, the refusal banner, the **budget-derived auto-field
clamp** and the **scene-global stacked budget meter** (§2.10), `warn_if_grass_truncated` naming the
field it cut, `foliage.loom`. Runnable: paint a meadow into existence with the mouse and erase a
path through it.

**Slice 2 — mesh foliage.** `Ground.paint` in `viability`, `Scatter.remove` (node-local),
`Scatter.align`, **`SceneView::instance_picks` and the synthetic instance inspector** (§2.10),
place-one, detach-and-move as one transaction, `scatter_key` in the shape of `grass_key`,
`reach_of`-sized dirty regions — the function has existed, tested and documented, with **no caller
anywhere in the workspace** since it was written — and `foliage_mesh.loom`. Runnable: paint a copse,
click the tree in the doorway and delete it, drag another two metres, undo each in one keystroke.

**Slice 3 — streaming (ADR 0041), gated on its agreement test.** Runnable: a 256 m painted
landscape.

**The refusal banner offers a local fix, not a global one.** Doc 10 §4's *"[Raise to 0.55]"*
changes `slope_cutoff` on the whole field — every other slope boundary in the meadow moves, and if
the scene is in `GOLDEN` its reference moves with them, for a complaint whose intent was *grass on
this bank*. The primary button is **[Paint soil here]**, which switches to Stage 7's splat brush
with rock authority inverted — the composition path ADR 0028 already establishes — and the
`slope_cutoff` raise is a secondary text link. Same thirty lines, correct scope, and it teaches the
two-brush model on the one occasion the user is guaranteed to be curious.

Also built: `loom foliage stats <scene>` (per field: blades or instances, the budget, and the
fraction of the field's area with any authority — how an agent *verifies* a paint landed), and a
`loom validate` warning when `FoliagePaint` sits on a node carrying neither `Grass` nor `Scatter`.

Green checks: **all four.** `foliage` and `foliage_mesh` join `SCENES`; `foliage` joins `GOLDEN`.
**Every existing reference must be unmoved**, which is the check that `x * lerp(1, v, 0)` is exactly
`x`. New tests: `painting_a_patch_leaves_blades_outside_it_untouched` (the crater test with a mask),
`an_erased_region_does_not_thin_its_own_fringe` (the `habitable`-vs-`kept` ordering, asserted rather
than assumed), and a removal-radius test asserting at most one instance per point.

Human looks at: **`cargo xtask flythrough` on `foliage`** — does the painted boundary read as ragged
or as mown? That is a motion judgement and no still makes it. Then: paint a meadow in two minutes
and time it. Erase across a steep bank and read the banner. Move the `Grass` node and confirm the
painted path goes with it. Click a generated tree.

---

### Stage 9 — Voxel sculpting

**Gated on a measurement, taken before any UI is drawn:** does `bake(ops)` equal
`bake(ops[..1])`-then-`edit`-the-rest, bit-for-bit? And what does a stamp cost on a realistic
volume? If the first fails, the live preview is wrong and falls back to a re-bake on stroke release
by design rather than by discovery.

Built: the brush emitting sphere/capsule/box ops in the volume's own space (with the unit test that
sculpts on a translated, yawed volume and asserts the op's `center` puts material under the cursor);
`smooth` and `flatten` as op kinds in `loom_voxel`; the stroke-grouped op-list panel; Preview-to-here.

**Two changes to doc 05's design, both from the round-1 usability review, both correct.** *Group by
stroke, not by stamp*: every op gets an optional `stroke` integer written by the brush, so the panel
shows *"Carve path · 10 stamps"* rather than four hundred rows, and "delete stroke" is one
`SpliceArray` over the run. *Ship `smooth` and `flatten` with the sculpt UI, not after it*: doc 05
§10.6 defers them "until a terrain author asks for them twice", and they are the second and third
tools every sculptor reaches for.

Runnable: sculpt a hillside with the mouse; the op list reads as forty strokes.

Green checks: **all four.** A sculpt-produced scene joins `SCENES`.

Human looks at: is the list comprehensible at forty strokes? Delete stroke 7 of 40 — does the result
match what "Preview to here" said it would? Carve a roof and confirm rain comes through on the next
frame. **And sculpt under a painted meadow: does the grass follow the new height on mouse-up, and
does the message fire when a painted bank becomes too steep to hold grass?** (Stage 8's §9
criterion, deferred to here because this is when it becomes testable.)

---

### Stage 10 — UV texture painting

Last of the painting systems and the most expensive, and it is last on purpose: it does not work on
voxel terrain (any auto-unwrap moves under every carve, and destructible terrain is locked), so the
most expensive of the four does not apply to the thing people will most want to paint, while the
cheap ones do. User decision 2 confirms the ordering from the other direction.

Built: `PaintLayer` with **typed** strokes in node-local coordinates (ADR 0027 — doc 04's untyped
array is overturned); the degenerate-UV refusal in `loom validate` and in the tool, using
`loom_asset::packed::bounds`' existing `uv_extent`; `box_atlas` as a **conversion target, not a
create-menu entry** (doc 05's "no second name for one mesh" rule, honoured); **doc 04 §12's
`paint_key` preview handoff in v1 rather than as a contingency** — hand the already-correct preview
image into the rebuilt `MaterialLibrary` instead of re-rasterising, which the
incremental-equals-full test is what makes safe; `loom paint bake` as the one-way escape hatch;
`paint_wall.loom` authoring `uv_scale = [8, 4]` on purpose, so a regression that tiles the paint
eight times fails the image gate.

Green checks: **all four**, plus shimmer 0.000 on `paint_wall`.

Human looks at: does the paint tile eight times (it must not)? Does the surface twitch at mouse-up?
Undo a stroke on a thousand-stroke layer — how long is the hitch?

---

### Stage 11 — Windows, and the documentation

Built: V2–V5 (leaf-first cross-compilation, link, `objdump` the import table, Wine smoke run, Wine
pixel-compare against a golden reference); `loom ship --target`; the Build modal as a subprocess
runner streaming `loom ship`'s JSON lines, using the shuttle busy indicator; the guide — four prose
files, two generated, `docs/guide/03-the-interface.md` (what each colour means, written for someone
who has never seen the tool) and **`docs/guide/05-you-and-the-agent.md`**, which is the most
important file in the set because the co-authoring model is the genuinely novel thing here and a
user who does not understand version tokens will experience the editor as flaky rather than as
careful. It must say in words that the blue means *just now*, not *theirs* (ADR 0035), and that a
regenerated foliage field reapplies its rule to every instance by construction — the single best
answer this design has to an Unreal user's expectations, which no design document thought to state.

Green checks: **all four**, plus the ship assertions, plus the Wine pixel-compare at ADR 0005's
tolerance (reported as a *build* check, never as a platform check, and skipped honestly when Wine
is absent).

Human looks at: `wine out/…/game.exe --frames 1`. The printed import table — no `vulkan-1.dll`, and
every non-system DLL copied and listed. Read `01-first-hour.md` cold and try to follow it.

---

### Stage 12 — The knowledge graph · the smallest stage in the plan · slots any time after Stage 5

**Needs Stage 1** (the inspector, which "Used by" is a section of), **Stage 4** (the Problems panel
and `COMMANDS`) and **Stage 5** (`loom_scene::project` — without ADR 0023 there is no project to
index). **Not Stage 6** (§4's opener). Slot it before Stage 11 so the guide covers it in the same
pass. §2.16 is the architecture; this is the order.

**First commit: ADR 0003's status line**, `proposed` → `accepted`, with §3's four recorded facts.
The plan does not make that edit and the directory should not disagree with the plan for longer than
one commit.

**Second commit is a measurement and nothing else, in exactly the discipline Stages 8 and 9 use.**
Time `Scene::parse` over all 52 `.loom` files (394,509 bytes) and print it, with the per-file
breakdown. **This is the number the store decision rests on and neither round-3 document measured
it** — doc 14 estimated 150–450 ms and admitted in the same document that the parse term "could be
wrong by an order of magnitude", then specified a database, a WAL policy, an incremental protocol
and a verify harness on top of it. The branch, decided here rather than under pressure:

| Cold build | What is built | What is not |
| --- | --- | --- |
| under ~50 ms | in memory, per invocation, nothing persisted | everything below |
| ~50–250 ms | + one JSON file under `$XDG_CACHE_HOME`, `(mtime, len)` validated per file | a database, `--verify` |
| over ~250 ms | the JSON cache first; **SQLite only if that is also not enough**, and it is one file to swap because the query functions are the only surface | — |

An incremental `DELETE`-by-owner protocol and the `--verify` harness that exists to prove it are
**only ever built if the third row is reached**, because a whole-file rewrite has nothing to drift
from and needs no proof that it does not.

**Slice 12.1 — the index and the CLI.** `crates/loom_graph/` (`loom_scene` only, two
`check-deps.sh` stanzas); `project::scenes()` generalised to `walk()` with `scenes()` becoming a
filter over it — **one function, two callers, no second answer to "what is in this project"**;
`Scene::parse` unresolved with §2.16.3's rule as the module doc comment; the two node kinds and five
edge kinds; **the shared alias resolver (S21)**, moved beside `Scene::asset_path` and called from
both `main.rs` and `loom_graph`; `loom graph <subject> --used-by | --impact | --broken` with the
`index` block on every response; `("graph_query", "loom graph")` in `loom_agent::TOOLS`.

**Slice 12.2 — the two surfaces that pay for it, plus Problems.** The inspector's **Used by**
section; the **prefab banner** on the scene view; reference results and the two new categories in
the **Problems panel** (§2.16.6) — *in the panel, never in `alias_report`* (§2.16.4 item 4); four
`COMMANDS` rows with their unavailability reasons for the no-project case.

**Runnable at the end of 12.1:**

```
$ loom graph assets/test/prefabs/lamp.loom --impact
{"subject":"assets/test/prefabs/lamp.loom","hops":4,"truncated":false,
 "impact":[{"file":"assets/test/prefab_room.loom","depth":1,"via":"declares,instantiates"},
           {"file":"assets/test/prefab_night.loom","depth":2,"via":"extends"}],
 "index":{"files":…,"errors":0,"incomplete":[]}}
```

**Runnable at the end of 12.2:** open `assets/test/prefabs/lamp.loom` in the editor and the banner
reads *"1 scene instances this prefab · Show"*; select the `Shade` node and the inspector's Used by
section names `prefab_room.loom`'s three instances.

**Exit criterion — ADR 0003's own, on the files that exist.** *"What would break if I changed the
desk prefab?"* There is no desk; there is `assets/test/prefabs/lamp.loom`, and it is the better
demonstration because the chain is two hops: `prefab_room.loom` declares and instances it three
times, and `prefab_night.loom` declares `prefab_room.loom` as `"day"` and **extends** it.
`--impact` must name **both**, each with the edge that reached it, **and must do it without
resolving anything** — resolve `prefab_night.loom` and it becomes a flattened copy of
`prefab_room.loom`'s nodes with no `extends` key anywhere in it, and depth 2 disappears. The design
doc's own §2.7 query is one hop over `edge.src` and returns only the first file; that is why
§2.16.2's traversal steps from a thing to the file that claimed it, and recurses.

Green checks: **all four, unchanged.** **`SCENES` stays 51 and `GOLDEN` stays 34** — no rendering
path, no component, no scene, no shader, no `ObjectData` field, no descriptor. The same position
Stage 6 is in. New tests:

| Test | What it stops |
| --- | --- |
| `impact_of_lamp_names_prefab_room_and_prefab_night` | the exit criterion, as a test rather than a demo — and one hop failing it |
| `scatter_exclude_node_path_is_indexed` | the `references_node` hole doc 15's model could not represent; `forest.loom:143` is the fixture and it exists |
| `scatter_mesh_alias_is_indexed` | the bare-`String` alias hole no schema walk can see. **Authors its own fixture** — verified, both `Scatter.mesh` values in this repo are primitives (§2.16.7), so `forest.loom` cannot be it |
| `an_unparseable_file_reports_error_not_emptiness` | a half-written file reading as "references nothing" — `CLAUDE.md`'s named S4 regression shape in a new crate |
| `impact_terminates_on_a_prefab_cycle` | the index must be total on files the loader would reject; that is half of what it is for |
| `impact_reports_truncation_at_the_hop_limit` | a silently short answer |
| `queries_are_sorted_and_two_runs_agree` | diffability of `graph_query` output, and the `BTreeMap` rule |
| `the_index_is_not_opened_by_render_or_sim` | §2.16.4 item 3 — a string test over `main.rs`, in the shape of `every_tool_wraps_a_real_subcommand` |
| `validate_output_is_unchanged_inside_a_project` | §2.16.4 item 4 — the same scene validated from a project root and from outside it produces the same warning set |
| `an_excluded_directory_is_never_indexed` | `.claude/worktrees/` holding whole checkouts and tripling the project |

Human looks at: **open `prefab_night.loom` and ask what breaks if you change the lamp.** Does the
answer include `prefab_night.loom`, two hops away? Then: click `assets/textures/` in the Project
panel and read a Used-by list against `rg` for the same alias — **does the index find the ones `rg`
cannot, which is every reference that goes through an `[[asset]]` alias rather than a literal
path?** Then delete a `Scatter` node that a `ScatterExclude` names and watch the Problems panel say
so, which nothing in this engine does today (`forest.loom:20` records the gap in a checked-in
comment). And read the second commit's timing output before believing any of §2.16's store
reasoning.

**The honest check on whether this was worth building**, applied at the end of the stage rather than
argued now: the agent has `rg`, and `rg` answers *"which files mention this literal"* in
milliseconds with no crate. Three things it provably cannot do are the whole product — **resolve an
alias** (136 texture aliases and 193 mesh aliases whose paths live in a different block of the same
file, so going from a `.png` back to the nodes that use it is two greps and a manual join, every
time), **reverse a relation**, and **cover all 52 scenes** where `xtask validate` walks 43. If after
a week those three are not what the graph is being used for, the stage over-built and §5's cut rows
are where the rest was already parked.

---

## 5. The cut list

| Cut | Why | Trigger to pull it back |
| --- | --- | --- |
| **Vertex-colour painting** | Doc 03 nominates it first to drop and admits splat covers most of it. It also spends the last scene push-constant slot on evidence that cites the wrong struct (§2.4), forces a private mesh copy per painted node, and creates a silent-no-op failure through `mesh_key` that doc 03 §13.7 admits it did not trace. | Someone needs colour variation across instances of one imported mesh that splat cannot reach. It then goes in `EnvironmentData`, not the push block. |
| **Multiple viewports, camera picture-in-picture** | Two forward passes by default, and a read-then-write hazard on the scene image that doc 01 §12.3 flags as unread. No stated need. | A second monitor becomes a working requirement, or camera framing becomes a daily task. |
| **The CMAA2 reorder** | Opt-in and off by default (`LOOM_CMAA2`), so the editor is correct without it. | Someone turns CMAA2 on in the editor and the chrome looks filtered. Then it is an ADR amending 0018. |
| **`Environment`, `Terrain`, `Events`, `Profiler`, `Foliage` tabs** | None is designed, and the foliage palette is better as tool-scoped UI in the shape the sculpt brush takes. A tab enum variant with an empty body is worse than no tab, and the enum is fixed in Stage 3. | Each gets a design document. `Events` is the strongest candidate — `EventLog` is a deterministic replay and has no viewer anywhere. |
| **The four-step onboarding task strip** | Its four steps teach the wrong four things (no camera control, and the user's first Play is the rig-less failure). The base scene's own comments already teach more, for free. | Replaced by the "Add Player" banner and the empty states, which are strictly better. Revisit only if a real user gets lost. |
| **`cargo xtask docs --check` in `green.sh`** | Would make `xtask` depend on `loom_editor` (colliding with ADR 0022) and compile the editor on every green run. The generator stays; the gate does not. | The compile cost is measured and is small, or the generator drifts once. |
| **The in-window script buffer** | A focus-dependent Ctrl+Z is the one place in the design where the key means two things. Doc 02 §10 refuses the identical construction for `loom.toml`. | Never, without its own ADR. "Open in your editor" is the answer. |
| **A light theme** | The editor's main content is a lit 3D scene whose average luminance the engine controls and the chrome does not; a light chrome around `cave` makes the viewport read as a hole. The real need underneath is contrast, met by zoom plus a five-token high-contrast swap. | A user who needs it and for whom high contrast is not enough. It is a third `const` block behind the same `tok`, which is why the token table exists. |
| **`accesskit` / screen-reader support** | `egui-winit` is pinned `default-features = false`, egui's per-widget labelling is partial, and the result would read as support without being it. | An actual user who needs it. |
| **`loom_asset::meta` (UUID sidecars, content hashes, manifest)** | Dead code with no caller. Path-relative resolution fails loudly at the point of authorship, and the repo's 176 asset paths resolve today with no identity layer. | An asset registry is genuinely needed — which is also when ADR 0024's `id`-becomes-primary migration happens. |
| **`project://` project-root resolution** | Reserved, not built. Every case it solves today is solved better by the editor's asset picker computing a relative path (~15 lines of `std::path`) and by the agent doing what it already does in 165 places. | Someone hits a case the picker cannot express. The spelling is already decided so nobody invents a third. |
| **Reachability-pruned asset shipping** | Needs the asset graph `meta.rs` would hold. Whole-tree copy is fine at any size a single developer produces. | A ship is too large to upload. |
| ~~**The knowledge-graph view**~~ — **this row's trigger fired.** ADR 0003 is accepted (§3), the index is Stage 12, and M12's view clause is discharged rather than struck. It is **scoped down** to three inline surfaces and a Problems section (§2.16.6); the rows below are what remains cut of it. | — | — |
| **The knowledge graph's SQLite store** — and with it `rusqlite`, `bundled`'s C toolchain requirement, WAL, `busy_timeout`, `BEGIN IMMEDIATE`, the `user_version` policy, the mtime/size/hash ladder, the incremental `DELETE`-by-owner protocol, the unreferenced-node sweep, and the `--verify` harness whose only reason to exist is that protocol | Every one of them is a consequence of deciding there is a persistent incremental store, and doc 14 takes that decision **before** the one measurement that would justify it — the ordering Stages 8 and 9 exist to forbid. `bundled` also lands inside Stage 0's `cargo check --target x86_64-pc-windows-gnu`, which runs at `loom_cli`'s default features, i.e. with `editor` on. | Stage 12's second commit measures the cold build and it exceeds ~250 ms **and** a single `(mtime, len)`-validated JSON file is also not enough. The swap is one file, because the query functions are the only surface. |
| **`mentions`, `derive_doc`, and walking `docs/` and `crates/`** | 458 backticked path tokens across `docs/`, resolved by *unique basename* against 92 `.rs` files whose names collide (`lib.rs`, `main.rs`, `ops.rs`); doc 14 §11 admits it does not know whether the ambiguous count is 5 or 200. It is the only edge kind either document calls lossy, it is the sole reason `docs/` must be walked, editing `PLAN.md` would dirty the graph — and it puts design documents in the impact answer, where **a document that refers to a file is not a thing that breaks when the file changes**. | Someone asks twice which document describes a file. `rg` answers that today, better. |
| **`loom graph --orphans`** | The single most dangerous wrong answer in either design. ADR 0023's walk sees `tests/`, which holds **28 reference PNGs** (verified) — day one is 28 false orphans sorted by size at the top of a list whose named consumer was a delete button. And the walk is a *filesystem* walk while both documents quote *`git ls-files`* counts: `*.actual.png` and `render.png` are gitignored, exist after any failed gate, and are orphan textures the 288 does not contain. | The reverse index is trusted — i.e. the alias rule has been in use, the walk's scope is settled, and there is a scoping rule better than "ADR 0023's list plus whatever the index needs", which would be a second answer to "what is in this project". |
| **The forward two-hop context pack** (`--pack`), `--why`, `--stats`, `--split`, `--no-refresh`, the pack's prose rendering, split-identity and orphaned-override queries | The design doc's argument for the pack was retrieval-beats-a-dump *at scale*, and doc 15 §7 concedes the collapse in its own words: at 350 nodes what the agent gets is **direction, not compression** — and direction is the *reverse* relation, which ships. The forward question is answered by reading the scene file in one tool call the agent already has. The two-hop *mechanism* ships inside `--impact`; only the forward surface is cut. Orphaned overrides is a second opinion on something `loom validate` already reports. | Each on first use. The pack: a project where reading the scene is no longer the cheaper answer, or an observed model wasting turns on discovery. |
| **The graph drawing — force-directed or otherwise — and `Tab::References` / `Tab::Graph`** | Doc 15 §4.1 is the strongest page in either round-3 document: a force-directed layout is non-deterministic, so you cannot compare a screenshot to a screenshot, gate it, or describe it to anyone — in a repository that built `cargo xtask shimmer` because things that move when they should not are its recurring failure. At 288 files it is a hairball. The tab is cut separately (§2.9 S20): both documents' proposals rest on an `egui_dock` `DockState` fact **neither of them read**, and the Problems panel is already the list that outlives navigation. | Someone uses the inline surfaces for a week and misses a dedicated results tab. Then it is one enum row and a deterministic two-column focus view — sorted rows at a fixed pitch, no simulation — and the `egui_dock` question gets answered by writing a layout, adding a variant and reopening. |
| **The impact modal as a widget, and any impact-driven refusal** | It fires on **3** prefab instances in the only project that exists. And the "check" it advertises is decoration on top of a real one: the gate that holds is `approving_a_stale_proposal_is_refused` on the version token, which is exact. A second refusal axis driven by a derived cache means **Approve can fail for a reason with no representation in any file** — the opposite of never-do #15's posture — and doc 15's `loom propose --list` summary would make a CLI output shape depend on whether a `loom.toml` sits above the cwd, which is the headless-path wobble ADR 0038 exists to prevent. | Someone is actually surprised by a deletion. Until then it is a sentence in the existing confirmation, and the impact summary is advisory, labelled as derived, and can never block. |
| **Parsing `.rhai` and `.rs` bodies** — `reads_component` / `writes_component` / `emits` / `listens` and a `system` node kind | Two real parsers for edges the exit criterion does not use, and this engine has no `system` as a data object — systems are Rust functions. A script is a leaf: reachable *from* scenes, pointing at nothing. No script in the repo uses `import`. A regex over a scripting language is a lie that reports confidently. | A script gains an `import`, or *"which scripts write `detonate`"* is asked twice. The host-variable whitelist is already in `loom_script`; the cost is taking that dependency. |
| **File deletion in the editor** | §2.6's new row. Every protection this project has protects scene *text*; none reaches `unlink`. A deleted file has no version token, no transaction and no undo, and giving it one means a trash mechanism, a restore path and a second undo domain — never-do #16 from outside the scene. | Never, without an ADR that answers where the undo lives. Find references, Reveal and Copy path are the context menu. |
| **`third_person` template** | Depends on `Camera.boom`, and the fallback "is playable and it pitches wrong". | `Camera.boom` lands and passes the golden gate (Stage 5). |
| **A general voxel-op compactor** | General CSG simplification is a research problem and a wrong one silently changes the terrain. | A `loom voxel compact` CLI dropping *provably* redundant ops (a containment test, not a solver). |
| **`egui-phosphor` / an icon font / Inter shipped unconditionally** | §2.7. Zero dependencies and zero binary assets in slice one is the cheapest thing to reverse. | The human reads the themed editor as default egui anyway. |
| **A third "detail mesh" path** (batched, not in the TLAS — Unity's detail meshes) and **octahedral impostors** | A fourth way to put geometry on screen. The measured numbers say `Grass` and `Scatter` cover the range: a 256 m field at 8 m spacing is 326 instances against `MAX_OBJECTS = 4096`. | A scene wants more than ~4,000 small meshes *and* the frame telemetry shows the TLAS rebuild dominating. That telemetry does not exist yet and is Stage 8's first measurement. |
| **Per-instance foliage transform, lasso select, select-all-of-type, multi-species one-stroke painting** | Stage 8 ships Remove and Detach-to-node, which are the two verbs that make the storage model reachable. The rest is palette work on a model that already supports it. | Someone paints a forest and asks for the same thing twice. Multi-species is the cheapest: the same `Outcome::Edit` carrying N `SpliceArray`s in one transaction. |
| **A size-jitter channel in the foliage mask** | `Scatter.scale` is `[min, max]` and already does it. A second channel doubles the raster and adds an encoding question. | Someone paints two size-populations of one species by duplicating the field twice. |
| **A grass `remove` list, and a grass align-to-normal control** | Erasing an individual blade is meaningless — the mask is the eraser. And real grass is gravitropic: blades on a hillside stand up. `blade.tilt` already carries the variation that matters, and a slope-align term would need the normal in the 48-byte payload for an effect that is wrong. | Neither. These are what the two systems *are*. |
| **Timed approval grants, an in-editor LLM client, an MCP client in the editor, a pty terminal, a socket or daemon so agent writes route through the editor** | ADR 0037 and 0038. Each makes the agent's behaviour depend on whether a window is open, or puts a network dependency inside the crate `xtask validate` drives. | Never, without an ADR that answers the second-write-path objection. |
| **Authorship in the scene file** | ADR 0035. A `.loom` file describes a scene, and a provenance field goes wrong on the first `git merge`, the first `cp` and the first hand edit. | Never. |
| **A "merge" button on the divergence banner** | Never-do #15, named here so nobody rediscovers it as a good idea. §2.6's `.mine.loom` button is the non-lossy alternative. | Never. |

---

## 6. The risk register

Ranked by how much collapses if the assumption is wrong. **Every experiment above the line is
scheduled in Stage 0 or Stage 1.**

**Re-ranked in round 3, without renumbering, because six stage bodies cite these numbers by name and
a citation that silently retargets is worse than a list that reads out of order.** The numbers are
labels; the ranking is this sentence. **The top band is unchanged — R1, R2, R3 still gate everything
after them.** The graph's three new rows slot in as follows: **R25 sits immediately below R7** (both
are "a system reports a clean pass over content it never looked at", which is this repository's
twice-shipped failure and its most expensive one); **R26 sits with R19 and R7** as a
measure-before-you-build gate, and it is *cheaper* than either; **R27 sits just above R22** as an
ordinary correctness risk with a known fix. Nothing below R20 moved.

| # | Risk | Cheapest experiment that settles it | Scheduled |
| --- | --- | --- | --- |
| **R1** | **The dock rectangle is one frame behind the scene rectangle**, and no design document mentions it. **Verified:** `ui.draw` is called at `viewer.rs:1613`, inside the `ui` graph pass closure, which `RenderGraph::execute` runs after the forward and tonemap closures recorded. So a splitter drag shows a stale scene rect against a live panel edge every frame, `chrome_clear` renders it as a benign-looking permanent black band, and `--frames 1` may legitimately render no scene — which is the invocation the gate makes. This is ADR 0025's blind spot and everything from Stage 3 on sits on it. | Split `Ui::draw` into layout-then-record (~80 lines) and drag a splitter. If the seam still lags, the fallback is the texture path, so this must be answered before Stage 3. | **Stage 2, first commit** |
| **R2** | **`toml_edit` may not be able to splice a nested array-of-tables**, and `Scene::parse` may not accept both spellings. `SpliceArray` is the prerequisite for sculpting, foliage, paint strokes, prefab-instance duplication and four inspector fields. The fallback — whole-array `SetField` — is verified to collapse `[[node.components.VoxelVolume.ops]]` into one inline array (`ops.rs:1039-1052`), rewriting a 4,000-character line per stroke and taking `git diff` dark for the one system whose authored form exists to be diffable. | Write the op against `assets/test/cave.loom`, splice three ops in, and read the file. **If it fails, stop and redesign before anything is built on it.** | **Stage 1, first commit** |
| **R3** | **The UI encodes its colours twice, and fixing it moves text antialiasing.** Verified from source, not suspected: `ui.rs:88` is `srgb_framebuffer: false`, which makes the shader pair an identity, against a `B8G8R8A8_SRGB` swapchain that encodes again. Every contrast number in every theme document is currently fiction, and **every judgement the human makes in Stages 1–3 is made through whichever encode is in place**. The flip also moves blending from gamma to linear, which changes every α-composited surface and egui's glyph coverage. | The probe: swatch strip, α128 swatches over mid grey, and a paragraph at three text tiers, before and after. Accept on *±2 bytes **and** text weight unchanged*. Fifteen lines in `loom_cli`'s existing panel path — **not** in `loom_editor`, which does not exist yet. | **Stage 0** |
| **R4** | **The agent CLI's actual wire format is written from memory.** Doc 09 §10.3 says so. The whole of Stage 6 — five recognised keys, the state machine, the tool rows, the `proposed` handoff — is built on a JSONL schema nobody has looked at, and the example flags will land in a documentation file a stranger copies. | Run the configured command by hand, capture ten lines of stdout, and diff them against §4.2's five keys. Minutes, and it also validates that the raw-line fallback is doing real work rather than decorative work. | **Stage 0** |
| **R5** | **Windows cross-compilation is impossible or expensive.** A whole deliverable and no compiler has looked at it. Predicted first failure is `blake3`'s `*_windows_gnu.S` under mingw; second is `winevulkan` not supporting dynamic rendering + descriptor indexing + BDA + 4× MSAA, which would leave "Windows supported" with no evidence. *(The feasibility review demotes this on good evidence — no `cfg(unix)` anywhere in `crates/`, `cpal`'s ALSA path correctly gated, `File::lock` is std. It stays high because it is the only item whose failure deletes a deliverable the user asked for, and the probe is an afternoon.)* | `rustup target add x86_64-pc-windows-gnu`, then `cargo tree --target … -e normal` (seconds), then `cargo check --target …` (no codegen). | **Stage 0** |
| **R6** | **An inspector `Material` edit may never reach the GPU today.** **Verified prerequisite:** `Viewer`'s public mutation surface is `set_grass`/`set_rain*`/`set_terrain`/`set_meshes` and nothing else. Four painting systems, the colour picker, the texture slot and the Problems panel's Fix button all assume material edits are live. | Open `loom run --edit`, drag a roughness slider, look. Ten minutes, and the answer changes the size of the material path by a factor of two in either direction. | **Stage 0** |
| **R7** | **Every foliage stroke commit re-places every scatter field and rebakes all grass.** Measured, in the tree, by the code's own comment: **103 ms on `forest.loom`** (`scene_view.rs`, and `build_cached` calls `scatter_objects` unconditionally at `:118`). A paint stroke is a file change. A brush whose mouse-up costs 100 ms feels broken and no preview hides it, and doc 10's "call it 4 ms" for grass is arithmetic on two unmeasured terms. | `grass_blades` wall time per tile at density 140, and `Session::apply` on `proving_ground.loom` with a 200-point stroke — **before any foliage UI is drawn**, in exactly the discipline Stage 9 already uses. `scatter_key` and `reach_of` dirty regions are the fixes and both mirror existing functions. | **Stage 8, before any UI** |
| **R8** | **The grass buffer holds a 43 m field and the design invites a 256 m one.** Verified: `MAX_BLADES = 262_144` (`renderer.rs:999`), 48-byte blades (`renderer.rs:582`). Truncation is z-major, so the failure is a straight horizontal edge across the landscape with an `"ok": true` render — and doc 10's own auto-created field was 8.7× to 36× over. | Arithmetic, then two guards: a unit test asserting the auto-clamp is under `MAX_BLADES` for any density, and the scene-global meter. Both are cheap; the ceiling itself is a human judgement at Stage 8's checkpoint. | **Stage 8, slice 1** |
| **R9** | **`incremental_painting_equals_a_full_rasterisation` may not pass bit-exactly** including every mip level. If it does not, the surface twitches at every mouse-up, the preview model changes, and foliage inherits the same drift because its mask is baked by the same walker. | The test itself. It is cheap, it runs in `cargo test`, and it is the only thing standing between this design and a preview that drifts. | **Stage 7** |
| **R10** | **The UV-paint GPU path's cost model is arithmetic, not measurement.** The re-raster on commit is ~30 ms by doc 04's own estimate, paid on every mouse-up **and every Ctrl+Z**. | A 30-line scratch benchmark: stamp at the clamp limit, and rasterise 1,000 strokes at 1024². If the full raster is over ~30 ms the `paint_key` preview handoff is required in v1 rather than optional (it is, in this plan). | **Stage 0** |
| **R11** | **Pointer capture during Play in a docked Game tab is unsolved.** `CursorGrabMode::Confined` confines to the window, not a sub-rect, so the flagship "press Play and walk around" flow has an unresolved input behaviour in exactly the layout this rework introduces. `Locked` covers the common case; the fallback is what fires on the machines that need it. | Press Play in a docked Game tab and move the mouse to the panel edge. | **Stage 3** |
| **R12** | **`PARTIALLY_BOUND` with unwritten descriptors may not be validation-clean.** The spec permits it, `material.rs:141-145` does set the flag, and nobody has run the validation layers against an over-sized array on this driver. Green check 2 is zero messages. | Size an existing scene's descriptor array with 16 spare slots and run `cargo xtask validate`. Fallback is `VARIABLE_DESCRIPTOR_COUNT` or the full `set_materials` rebuild, both strictly more work. | **Stage 0** |
| **R13** | **`loom propose --wait` assumes a blocking tool call keeps an agent's turn open.** True of every tool-calling loop the reviews know of, and unverified against the tool that will actually be configured. If it is false, the approval loop stalls exactly as §2.13 describes and the fix is a polled `--list` in the preamble instead. | Configure the agent, issue a destructive request, and watch whether the turn survives the block. It is the same session that settles R4. | **Stage 6** |
| **R14** | **`SpliceArray` against a prefab instance was undefined**, and sculpting or painting a prefab-instanced terrain chunk is a plausible first user action. | Decided in ADR 0026 before the op ships — a splice materialises the resolved array as an override and splices that, the only reading that does not silently depend on the prefab's contents — then tested. | **Stage 1** |
| **R15** | **`egui_dock 0.20.1` and its egui-0.35 compatibility are unverified**, and doc 11 §15.5 additionally assumes its `Style` exposes the active-tab thread, the `surface`-filled active tab and the `ground` gutter. Neither it nor `egui-phosphor` is in this machine's registry. | `cargo add --dry-run egui_dock@0.20.1`, first thing in Stage 3's dock commit. If the tab strip cannot be styled, the fallback is drawing it inside the tab body, which costs the thread its position rather than its meaning. | **Stage 3** |
| **R16** | **`--no-default-features` may not actually keep `loom_editor` out**, under `resolver = "3"` with feature unification across the workspace. | `cargo tree -p loom_cli --no-default-features -e normal`. It is a `check-deps.sh` rule precisely so a regression is caught by whoever caused it. | **Stage 5** |
| **R17** | **`engine_assets()` and `find_root` change what 43 gated runs do.** `find_root` walks up from `assets/test/…` into the repo's new `loom.toml`, switching every windowed gate invocation from scene-only to project mode. | `--frames` forces scene-only mode (one condition), and V2 asserts `MANIFEST.txt` is byte-unchanged. If a reference moves, something reads the manifest that should not. | **Stage 5** |
| **R18** | **`DONT_CARE` with a sub-rect `render_area`** is correct by spec — contents outside `render_area` are preserved — but it is the kind of thing where an IHV fast path differs. | Look at frame one of a hardcoded inset. The same observation Stage 2 already makes. | **Stage 2** |
| **R19** | **Incremental `Volume::edit` may not equal a full `bake`.** If it does not, the sculpt brush is a click-and-wait whose cost is linear in an unbounded op list. | The test doc 05 §10.5 specifies: `bake(ops)` vs `bake(ops[..1])`-then-`edit`, bit-identical fields, plus a stamp timing on a realistic volume. | **Stage 9, before any UI** |
| **R20** | **The `SetTransform` `f32` fix produces one-time numeric churn** across every scene the editor rewrites. Numerically identical, visually a diff, landing in the same window as the golden gate. | Land it in its own commit with the churn stated, and re-read one scene's diff by hand. | **Stage 0** |
| **R21** | **Nobody owns the *combined* "no reference moves" claim.** Four documents now each promise zero moved references — the viewport rect, splat, foliage and UV paint — and each argues byte-identity in isolation (branch uniformity, `lerp(w,x,0)` exactness, `x * 1.0` exactness). | `cargo xtask image` after each of Stages 2, 7, 8 and 10, read as a set rather than per-feature. Any moved reference is a bug in the branch, not a bless. | **2, 7, 8, 10** |
| **R22** | **`instance_picks` may cost something at 5,000 instances.** It is a linear ray-box loop the size of `scatter_objects`' output, and `pick_at_cursor` already does one over `picks` — assumed to be the same cost class, measured by nobody. | Time a pick on `forest.loom` with the loop in. If it is visible, the fix is the same bounds hierarchy picking would want anyway. | **Stage 8, slice 2** |
| **R23** | **egui frame cost of a long conversation.** A thousand-message transcript in an immediate-mode panel is the obvious pressure point, and `LOOM-IMPLEMENTATION-ORDER.md:571` already names row virtualisation as the response. The design assumes retaining 200 turns is enough, which is a guess. | Paste 500 turns in and watch the frame time. Virtualise if it moves. | **Stage 6** |
| **R24** | **Whether any of this reads as sleek, whether the palette reads as *Loom* rather than as a competent dark theme, whether the arcball feels attached, whether sixteen hand-drawn icons look like a family, whether a painted boundary reads as ragged or mown, and whether a forty-stroke sculpt list is comprehensible.** No gate substitutes and no user has been observed. **Round 3 adds one to the same session:** whether anyone ever looks at a Used-by section or reads the prefab banner, which is the only evidence that Stage 12 was worth its week. | A session with the human at the end of Stages 3, 4, 6, 8, 9 and 12. Stated as unautomatable rather than assumed away. **At Stage 3 the session begins with `--theme-probe`**, or the human judges a palette that is not the one in the document. | **3, 4, 6, 8, 9, 12** |
| **R25** | **The reference extractor silently stops seeing a reference kind, and the index confidently reports "nothing uses this" about a file eleven nodes use.** This is not hypothetical and it is not a modesty ranking: it is `CLAUDE.md`'s twice-shipped failure — `meadow` missing from `GOLDEN`, `grass_blades` passing a flat `Ground` — arriving in the one subsystem whose entire product is knowing what points at what. **Verified live, and it defeated every mechanism both round-3 documents proposed**: `Scatter.mesh` is a bare `String` holding an `[[asset]]` alias (`components.rs:964`) whose schema is byte-identical to `Name.value`, so it is invisible to an `{asset:…}` object walk, to an extension-suffix rule, to a `$ref`-typed-field walk, and to both documents' guard tests. `ScatterExclude.field` holds a node path (`components.rs:1036`) and doc 15 cut a third of its model on a measurement asserting no such field exists. | **The alias-set rule** (§2.16.2): any string field whose value matches an `[[asset]]` key declared in that scene is a reference. It needs no schema inspection, no extension list and no per-component knowledge, and its failure mode inverts to a *spurious* edge. Plus two named tests with real fixtures — and `scatter_mesh_alias_is_indexed` must **author** its fixture, because verified: both `Scatter.mesh` values in this repo are primitives, so the hole is latent and no existing scene exercises it. | **Stage 12, slice 1** |
| **R26** | **The knowledge graph's whole storage architecture rests on an unmeasured parse cost.** Doc 14 estimates a cold rebuild at 150–450 ms, admits in the same document that the parse term "could be wrong by an order of magnitude", and specifies a database, a WAL policy, an incremental protocol and a verify harness on top of it; doc 15 estimates "tens of milliseconds" from the same unmeasured constant and specifies none of them. **Neither ran it, and neither knows whether `Scene::parse` validates every component against its schema on every parse** — which is the term the estimate hinges on. | Time `Scene::parse` over all 52 `.loom` files and print the breakdown. It is the stage's second commit, before any query surface or UI exists, and §4's three-row branch table says in advance what each answer buys. Cheapest experiment in the register and it can still delete a database. | **Stage 12, before any query surface** |
| **R27** | **The index re-implements the loader's alias resolver and the two drift.** Alias→path is: split `#Object`, try `primitives::build` **first**, then the declaring scene's `[[asset]]` table, then `base.join` (`main.rs:1146-1170`) — and ADR 0024 pins the primitive precedence because `blockout.loom` depends on it. Doc 14 re-derives all of it inside `loom_graph` *and adds a constraint the loader does not have* (the joined path must stay inside the project root), which would drop the edge for `base.join("../audio/rain.wav")` — a path ADR 0036 exists because of. The index would then describe a slightly different program than the one that runs: ADR 0006's divergence class, in the crate whose only job is to be correct about the first implementation. | S21: move the ladder down beside `Scene::asset_path` in `loom_scene`, once, and call it from both `main.rs` and `loom_graph`. It is a better home for it than `main.rs` regardless. If the move is too large for the stage, the index labels its resolved edges approximate in the JSON and the ADR says so — but it never owns a second copy silently. | **Stage 12, slice 1** |

**What could not be checked at all in this phase:** no `cargo` command was run, so every dependency,
feature-resolution, compile-time, GPU-cost and validation claim in the entire design set — including
this document's — is unchecked by a compiler. The twenty-seven rows above are the ones that carry
weight. The three claims most worth a second pair of eyes are the two `check-deps.sh` findings
(`BrushParams` and the undeclared-alias check, §2.1 and §2.13 H5), which are `cargo tree`'s to
settle and would each fail green check 1 on the day the ADR they correct landed as written; and the
double-encode chain (§2.7), which was read from `egui-ash-renderer 0.12.0`'s GLSL sources rather
than disassembled from the `.spv` files that are actually `include_bytes!`d.

**Round 3 adds four to that list, and two of them cost nothing to leave open.** *(a)* The cold parse
cost — R26, and the whole point of scheduling it as a commit rather than settling it here. *(b)*
Whether `Scene::parse` validates every component against its schema on every parse, which is the
term (a) hinges on; both round-3 documents leaned on it and neither traced it. *(c)* Whether adding
a `Tab` variant invalidates a saved `DockState` — both documents asserted opposite consequences and
both admitted they had not read `egui_dock`'s serialization. **§2.9's S20 makes the question moot
rather than answering it**, which is why it costs nothing: the enum is not touched. *(d)* Whether
`loom_reflect::resolve` can identify an `AssetRef`-typed field generically by `$ref`. §2.16.2's
alias-set rule does not need it, so this degraded from load-bearing to an optimisation — which is
the second thing that costs nothing, and it is the shape of the whole round-3 merge: the cheapest
mechanism was also the only one that saw `Scatter.mesh`.

**What round 3 *did* check, cheaply, and what it overturned:** `ScatterExclude.field`'s doc comment
and its use at `forest.loom:143`; `Scatter.mesh`'s type and its two values in this repo (both
primitives — the review's CRITICAL is latent, not live); `clippy.toml`'s location and contents;
`git ls-files assets docs crates` = 288 and `assets` = 122 authored; 28 reference PNGs outside
`assets/`; `loom_agent::TOOLS` = 8 entries; `components::registry()` = **24** types, not the 26 doc
14 states as verified; `docs/decisions/` ending at 0021; the three stale doc comments in Stage 0
item 10; and the lamp→room→night prefab chain the exit criterion runs on. Every one was a `grep` or
a `wc`, which is the standard the rest of this document was held to and the reason two documents'
central measurements did not survive contact with it.
