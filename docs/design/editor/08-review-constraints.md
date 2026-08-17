# Review — the constraints lens

*Adversarial review of `01`–`07` against the binding rules in `CLAUDE.md` and
`00-survey-constraints.md`. Design phase; nothing was built. Every rule quoted is quoted from the
document that binds it, and every design quote is from the document under review.*

**Verdict in one line: no document proposes an editor undo stack, no document proposes an
auto-merge, no document floats a dependency version, and no document puts `ash` outside
`loom_render` — the four rules most likely to be broken are held. What is broken is that the seven
documents were written in parallel and contradict each other on eleven shared resources, three of
which are single 32-bit fields or single files that cannot hold two designs at once.**

The failures below are ordered worst-first. "Worst" is measured by how late the defect would be
found: a design that cannot compile is cheaper than a design that compiles, ships, and is never
looked at by a gate.

---

## 1. BLOCKER — `06` makes egui optional in `loom_render`, but the *runtime* uses egui

`06-build-and-ship.md` §1:

> ```toml
> # crates/loom_render/Cargo.toml
> [features]
> default = []                                              # nobody gets egui unless they ask
> editor  = ["dep:egui", "dep:egui-ash-renderer", "dep:egui-winit"]
> ```
> `loom_cli` gains a second binary, `loom-play`, built with `--no-default-features`; that binary
> is what ships.

and, in the same section, the list of what the shipped binary needs:

> The shipped binary needs `play.rs`, **`hud.rs`**, `scene_view.rs`, `materials.rs` …

Verified in the tree: `crates/loom_cli/src/hud.rs:16` is `use loom_render::egui;`, and the module
draws the game's HUD with `egui::Align2`, `egui::FontId`, `egui::Ui`. Its own header states the
design intent: *"It also costs no new pass: the editor's UI layer already draws into the swapchain
image the scene wrote."* The HUD **is** the editor's UI layer, at runtime.

So `cargo build --no-default-features -p loom_cli` does not produce a stripped runtime; it produces
a compile error in `hud.rs`. And §6.6's proposed green-check rule —

> `cargo tree -p loom_cli --no-default-features -e normal` must not mention `egui`

— is unsatisfiable for as long as `Hud` is a scene component drawn by egui.

`02-project-hub.md` §6 already found this and wrote it down for whoever writes ADR F:

> **One finding for whoever writes it:** `hud.rs` uses egui at *runtime*, so the shipped runtime
> links egui regardless and the split is about not linking `loom_editor`, not about making egui
> optional in `loom_render`.

`06` was written without it.

**Minimum fix.** Drop the `loom_render` feature entirely. Keep egui unconditional there; make the
boundary `loom_editor`, enforced by one `check-deps.sh` rule ("nothing but `loom_cli` depends on
`loom_editor`", the same shape as the existing `loom_agent` rule) plus a second rule that
`loom-play`'s tree contains no `egui_dock`/`egui-phosphor`/`loom_editor`. This is smaller than
what `06` proposes, removes the `#[cfg(feature = "editor")]` on `viewer.rs:936` and the `ui` pass
that ADR 0018's divergence warning makes risky, and it survives contact with `hud.rs`. If egui
genuinely must leave the runtime, that is a separate decision about whether `Hud` stops being an
egui overlay — and it is not in scope of any of these seven documents.

---

## 2. BLOCKER — `03` and `04` both allocate `ObjectData.material.y`

`03-painting-splat-and-vertex.md` §6:

> `material: [u32; 4]` — `x` is the material index today and `y`/`z`/`w` are stated padding
> (`renderer.rs:673-675`). **Take `y` for the splat mask's bindless slot**, `NO_TEXTURE` when
> unpainted.

`04-painting-uv-and-decals.md` §1.3:

> `ObjectData.material` is `[u32; 4]` with **only `.x` used** … Paint goes in `.y`.

Verified at `crates/loom_render/src/renderer.rs:675`: `material: [u32; 4]`, doc comment *"Material
index in `x`; the rest pads to the 16-byte alignment a std430 block needs."* One field, two owners.
`04` §10.3 then compounds it by assuming `.y` is already spent by paint and reserving `.z`/`.w` for
a future decal cull — so under `04`'s own arithmetic, splat painting has nowhere to go.

Whichever lands second silently overwrites the first's bindless index. The failure mode is a
painted mask sampled as a paint layer, or `NO_TEXTURE` written over a live slot — a wrong texture,
not a crash, and invisible to `cargo xtask image` unless both features are in one golden scene.

**Minimum fix.** One document owns the `ObjectData` field map and the other cites it. The obvious
allocation is `material = [material_index, splat_slot, paint_slot, decal_range]`, written as a
doc comment on the Rust struct and mirrored in `assets/shaders/scene.slang:56-57` — which is the
existing discipline for that struct (*"two declarations are one layout described twice and a
mismatch is silent"*, `03` §6). Note that this exhausts `material`, so `03`'s appended
`splat: [f32; 4]` (240 → 256 bytes) is still needed and must be agreed once, not twice.

---

## 3. BLOCKER — the shipped game contains no scenes

`02-project-hub.md` §1 defines the project layout:

> ```
> KiteHollow/
>   loom.toml
>   scenes/
>     main.loom               what the hub opens
>   assets/
> ```
> **`scenes/` sits beside `assets/` rather than inside it**

`06-build-and-ship.md` §3.4 defines what ships:

> **The whole asset tree, minus two things.** Copy `<root>/assets/**` preserving structure, and
> `loom.toml`, and nothing else.

Under `02`'s layout that copies zero `.loom` files. `06`'s own example output tree
(`assets/games/proving_ground.loom`) only works for the *engine repo's* unusual layout, which `02`
§1 explicitly declares a special case ("The repo is a project with an unusual layout").

The same two sections disagree about the manifest itself. `06` §3.2 requires `game.name`,
`game.startup_scene` and `build.targets`; `02`'s ADR draft specifies `project.name`,
`project.main_scene`, `project.id`, `project.format`, `engine.version` and states:

> **No build settings, no window size, no render settings.** … inventing it now is a table of
> values nothing reads.

And `06` §3.2 warns against exactly what `06` §7 then does:

> **What must not happen is a second manifest reader**: the ship step and the runtime must
> deserialize through the same type

— while its files table adds `crates/loom_cli/src/project.rs` **new**, `~30 lines: project_root,
manifest read`, against `02`'s `loom_scene::project::load`.

**Minimum fix.** One manifest type in `loom_scene::project`, with `02`'s key names; `06` reads it
and adds nothing. `loom ship` copies the whole project root minus `builds/`, `target/`, `.git/` and
any editor state directory — not `assets/**`. `build.targets` becomes a `[build]` table added
additively when the Build panel is built, or the target list is a CLI flag and a hub preference,
which is what it actually is.

---

## 4. HIGH — the docked viewport is a rendering path no gate will ever render

`CLAUDE.md`, definition of green:

> **Adding a rendering path means adding a scene to `GOLDEN`**, or the gate reports a full pass
> without ever having looked at it; grass shipped two slices before anyone noticed.

`01-shell-and-docking.md` §1.3 makes the gate's blindness a *feature*:

> **When `placement` is `None`, every one of those four changes evaluates to exactly what the code
> does today** … So `loom render`, `loom run` without `--edit`, `cargo xtask image`,
> `cargo xtask flythrough` and `cargo xtask shimmer` are untouched, and the golden references
> cannot move.

and §9's files table:

> | `xtask/src/main.rs` | nothing, if §1.3 holds. |

The safety argument is right and the conclusion is wrong. A sub-rect forward pass, a tonemap
writing a destination sub-rect with an origin push constant, a `chrome_clear` pass, and a
conditionally-reordered CMAA2 are four new pieces of Vulkan that **execute only when `placement`
is `Some`**, which is a state no green check enters. `cargo xtask validate`'s windowed half drives
`loom run --frames n` (`xtask/src/main.rs:1024`, `:1077`) — without `--edit`, so without a
placement. The zero-validation-message gate would report a clean pass over a viewport path it never
ran.

This is the same defect as the shimmer instrument that framed a scene not containing grass, and
`01` §11's exit criterion for step 1 ("`cargo xtask image` produces **zero** changed references")
proves only that the `None` path is unchanged.

**Minimum fix.** Two things, both cheap. (a) Add a `--viewport x,y,w,h` flag to `loom render` that
sets a `ViewportPlacement`, and one `GOLDEN` entry rendering an existing scene through a non-zero
placement into a larger canvas; the reference is the existing scene's image with a known offset, so
a wrong origin or a wrong aspect fails immediately. (b) Add `--edit` to at least one of the five
windows `cargo xtask validate` drives, so the docked path, the `chrome_clear` transition and the
zero-extent clamp of §1.4 are executed under the validation layers. Without (b), §1.4's own warning
stands unaddressed: *"it fires only on a gesture no test performs."*

The same argument applies to `02` §7's hub state, which creates an `Instance`, `Device`, surface,
swapchain and `Ui` with **no `Viewer`** — a window lifecycle that no gate drives and that
`run.rs:294-335`'s hard-won teardown order was never tested against.

---

## 5. HIGH — one brush, two rasterisers, two schemas, two transaction models

`03` claims a shared foundation in its own header:

> the **brush model, the stroke schema and the transaction shape in §1–§3 are shared with them**
> and are written here once

`04` shares none of it. Concretely, four divergences:

| | `03` | `04` |
| --- | --- | --- |
| Rasteriser lives in | **new crate `loom_paint`** (§10) | `crates/loom_asset/src/paint.rs` (§8) |
| Stroke schema | typed, `#[derive(JsonSchema)]`; *"strictly better than the precedent and worth not copying blindly"* (§1) | `strokes: Vec<serde_json::Value>`; *"untyped JSON **on purpose and by precedent**"* (§1.1) |
| `radius` units | **world metres** (§1) | **UV units** (§1.1) |
| Transaction | per-drag `SetField` through `apply_coalescing`, key `paint:{node}:splat:{epoch}` (§2) | **one** `SetField` on mouse-up, *"no gesture coalescing is needed"* (§4.1) |

The rule this collides with is the one ADR 0006 exists to enforce, quoted in `CLAUDE.md`:

> a field is one expression tree, `Expr::eval` walks it on the CPU and `build.rs` emits … from it,
> **so the two cannot implement different formulas**

and `04` §3.1 states the principle itself while violating it across the document boundary:

> **a second implementation of the same formula is a divergence waiting to happen**

Two `smoothstep(1.0, hardness, dist/radius)` accumulators in two crates is exactly that, with the
added wrinkle that one is in metres and one in UV.

**Minimum fix.** One module, one `Brush`, one falloff, one dab walker, one set of round-trip tests.
`loom_asset::paint` is the right home (both bakers produce a `loom_asset::Texture`, and `03`'s
`loom_paint` would depend on `loom_asset` and nothing else anyway — a crate for one file, against a
one-minute-warm-build stop-and-fix trigger, `LOOM-IMPLEMENTATION-ORDER.md:574`). Take `03`'s typed
schema (see §10 below). Keep the two coordinate spaces but make them a field on the stroke, not a
convention two documents remember differently. Pick one transaction model: `04`'s mouse-up commit is
strictly simpler and `03`'s coalescing costs nothing if unused, so `04`'s wins — but then `03` §2's
whole subsection is wrong and must be deleted rather than left as a second answer.

Same defect, smaller: **two texture-update mechanisms**. `03` §6 proposes
`Viewer::set_material_texture` as a one-shot submit outside the graph; `04` §3.2 proposes a
`paint_upload` graph pass with `Access::TransferDst` plus `forward_uses` declaring the image. See
§9 below — `04`'s is the only one that satisfies never-do #4.

---

## 6. HIGH — `01` adds unconditional egui to `loom_cli`; `06` forbids it

`01` §2.1:

> `egui_dock` lands in `crates/loom_cli/Cargo.toml`, with `egui = "=0.35.0"` added there directly
> alongside it.

`01` §9's files table repeats it:

> | `crates/loom_cli/Cargo.toml` | `egui = "=0.35.0"`, `egui_dock = "=0.20.1"`, `egui-phosphor = "=0.13.0"` |

`06` §6.6:

> **The shipped binary contains no egui.** `cargo tree -p loom_cli --no-default-features -e normal`
> must not mention `egui`, `egui-winit` or `egui-ash-renderer`. Add it to `scripts/check-deps.sh`

Three unconditional dependencies in `loom_cli` make that check fail on the day it is added.
`01` also puts all new editor code in `crates/loom_cli/src/editor/` while `05`, `06` and `07` all
write paths under `crates/loom_editor/` — so the documents also disagree about which crate exists.

**Minimum fix.** `egui_dock` and `egui-phosphor` (if kept at all — see §13) go in `loom_editor`, not
`loom_cli`; `loom_cli` keeps reaching egui through `loom_render`'s re-export for `hud.rs` only.
`01` §9's *"lifting it into a `loom_editor` crate later is a `git mv` plus a manifest"* is the right
instinct and the wrong sequencing: the manifest entries are what makes it not a `git mv`. Decide the
crate once, in ADR F, before `01` step 5 adds a dependency to the wrong manifest.

---

## 7. HIGH — ADR numbers 0022 and 0023 are allocated three times

`docs/decisions/` currently ends at `0021-a-reflected-hit-shades-with-the-materials-mean-albedo.md`,
so the next free number is genuinely 0022. Three documents claim it:

- `01` §10: *"**ADR 0022 — the viewport is a sub-rectangle of the swapchain**"* and
  *"**ADR 0023 — CMAA2 moves ahead of the UI pass**"*
- `02` §11: *"### ADR 0022 — A project is a directory with a `loom.toml`"* and
  *"### ADR 0023 — An `[[asset]]`'s `path` is resolved; `id` is reserved"*
- `04` §4.3: *"Next free number at the time of writing is 0022; the exact number depends on the
  sibling editor design docs, which also owe ADRs."*

ADR 0002 fixes the precedence chain as `CLAUDE.md` → `docs/decisions/` (**newest applicable**) →
brief. Two different ADR 0023s make "newest applicable" undecidable, and `CLAUDE.md`'s
locked-decisions table says changing a locked decision "requires an ADR in `docs/decisions/`" — a
number that identifies two decisions is not an ADR.

**Minimum fix.** Allocate the block once, in writing, before any ADR is drafted. Proposed, and it
costs nothing to change as long as it is written down first: 0022 project manifest (`02`), 0023
asset `path` resolution (`02`), 0024 viewport sub-rect (`01`), 0025 CMAA2 reorder (`01`), 0026 UI
dependencies (`01`+`07`), 0027 stroke-list painting (`03`+`04`, merged — see §5), 0028 decals
(`04`), 0029 op-vocabulary growth (`05`, see §8), 0030 editor/runtime crate split (`06`), 0031
Windows cross-compile (`06`), 0032 the command table (`07`).

---

## 8. HIGH — three names for one new `SceneOp`, in three documents

`00-survey-engine-surface.md`:

> The lazy fix is an **`AppendVoxelOps` op** — one new `SceneOp`, order-preserving, appending to a
> named array.

`04` §4.4:

> an **`AppendToArray { node, field, values }`** op … **If the terrain design doc proposes
> `AppendVoxelOps`, these should be one op and not two.**

`05` §13:

> ```rust
> SpliceArray { node, field, index: Option<usize>, remove: usize, insert: Vec<Value> }
> ```

`CLAUDE.md`'s Agent API row and never-do #16 make the op vocabulary the write surface of the whole
engine; `05` is right that nine → eleven is *"a 22% growth in the write vocabulary"* and should not
be waved through. It is also the only one of the three that answers delete, edit-in-place and
reorder, and the only one with a named list of existing callers (`WaterBody.waves`,
`Buoyancy.pontoons`, `Scatter.excludes`, `GroundLayer`).

**Minimum fix.** `SpliceArray` wins; `04` §4.4 and the survey's `AppendVoxelOps` are struck and
cite it. One ADR (`05` already drafts it) covering `SpliceArray`, `Declare` and
`SpawnNode { prefab }` together. Two open items in that ADR that no document has answered and that
must be answered before it is approved: `05` §16.1 — whether `Scene::parse` accepts `ops` written
as an inline array as well as `[[…ops]]`, since `SpliceArray` must preserve whichever spelling is on
disk; and §16.5 — whether `toml_edit` can write a nested array-of-tables inside an `[[node]]`
array-of-tables entry.

---

## 9. MEDIUM-HIGH — `03`'s texture update places barriers outside the render graph

`CLAUDE.md` never-do #4: *"Never place a barrier outside the render graph."* ADR 0017 extended it to
buffers, and `00-survey-constraints.md` §2.8 restates it: *"A scene-image → sample-in-egui
dependency is a graph edge, not a hand-placed barrier."*

`03` §6:

> The narrow fix: `Viewer::set_material_texture(slot, &loom_asset::Texture)`, reusing
> `material::record`'s one-shot submit — which is explicitly framed as *"initialisation work that
> must finish before the first frame, not per-frame work the graph schedules"* …
> **If it is ever recorded into the frame's own command buffer it becomes a render-graph pass with
> a declared `Access::TransferWrite`, never a hand-placed barrier (never-do #4).**

The escape clause is doing too much work. `material::record`'s exemption is that it runs *before the
first frame*, when no image is in flight and no layout is live. A stroke transaction runs between
frames of a live editing session, against an image the previous frame's `forward` pass sampled and
the next frame's will sample again — which needs `SHADER_READ_ONLY_OPTIMAL → TRANSFER_DST →
SHADER_READ_ONLY_OPTIMAL`, i.e. two barriers, placed by hand, at ten strokes a second, outside the
graph. That is the letter of never-do #4, not an edge case of it.

`04` §3.2 does it correctly and even names the trap the graph would otherwise introduce:

> **The paint image is imported with a known layout, not as UNDEFINED** … `import_with_layout(name,
> image, SHADER_READ_ONLY_OPTIMAL)` already exists for exactly this
> (`loom_render_graph/src/lib.rs:410-425`)

**Minimum fix.** Delete `03` §6's one-shot path. Both painting systems use `04`'s `paint_upload`
graph pass, `import_with_layout`, and the `forward_uses` declaration; the barrier-list test in
`loom_render_graph/src/lib.rs` names the two new transitions. That test is how never-do #4's
ownership stays visible rather than assumed, and it is also the thing `01` §1.9 must extend for
`chrome_clear` — one commit, three transitions, one place.

---

## 10. MEDIUM-HIGH — `04`'s `PaintLayer.strokes` is not schema-validated on load

`CLAUDE.md`, property 1: *"Everything authored is **diffable text**, schema-validated on load."*

`04` §1.1:

> ```rust
> /// **This doc string is the whole schema for a stroke**, exactly as
> /// `VoxelVolume::ops`' is — `strokes` is `Vec<serde_json::Value>`, so a
> /// generated JSON Schema says only "array".
> pub strokes: Vec<serde_json::Value>,
> ```

`04` is honest about the cost and names the precedent's own scar tissue (*"`invalid_voxel_op` exists
because layer 1 never looks inside the array"*, and four silent failures the funnel was created to
stop). But it then relies on a validation funnel *"called by `loom validate` and by every
rasterisation"* — which is not "on load", and `00-survey-constraints.md` §2.4 records the exact
failure class: a key the parser does not understand is a key it *ignores*.

`03` §1 gets this right and says why:

> Typed, not `serde_json::Value`. `VoxelVolume.ops` is untyped because it is a *union* of five
> shapes behind a `kind` discriminator … A stroke list has one shape, so it gets
> `#[derive(JsonSchema)]` and validates through the registry like everything else. **That is
> strictly better than the precedent and worth not copying blindly.**

`04`'s strokes are a union of three shapes (`stroke`, `stamp`, `erase`), which is a weaker case than
`03`'s but still a `#[serde(tag = "kind")]` enum, which schemars renders as `oneOf` and which
`loom_reflect` already resolves (`lib.rs:233-258` handles the `oneOf` + `const` spelling).

There is a second, downstream cost `07` §3 already identified:

> **`VoxelVolume.ops` is untyped JSON** … The generator cannot do better than print that comment.

An untyped `strokes` is a hole in the generated component reference too.

**Minimum fix.** Typed tagged enum for both painting systems. `03`'s reasoning is already written;
`04` adopts it. `04`'s `erase` variant disappears under `03`'s rule that erasing is `strength = 0`.

---

## 11. MEDIUM — three locations and two formats for editor state, one of which is forbidden

`01` §4:

> ```
> <project>/.loom/layout.json          the current layout, saved on change (debounced 2 s)
> <project>/.loom/layouts/<name>.json  named presets
> ~/.config/loom/editor.json           preferences that are not per-project
> ```
> **JSON, not TOML, and only here.**

`02` §0 and its ADR draft:

> There is no asset database, no import step, no project-scoped registry and **no `.loom/` dropping
> in the user's folder.**
> … *"A project directory acquires no engine-written files."*

`07` §6:

> Prefs live at `$XDG_CONFIG_HOME/loom/editor.toml` … and carry: recents, window geometry, **dock
> layout**, zoom factor, reduce-motion, high-contrast, and the "seen the strip" flag.

Three answers for the dock layout, two for the recents list (`02` puts it in
`$XDG_STATE_HOME/loom/hub.toml`, `07` in `$XDG_CONFIG_HOME/loom/editor.toml`), and two formats.
`01`'s per-project argument is the strongest of the three and `02`'s prohibition is the flattest —
they cannot both be in ADR H.

**Minimum fix.** ADR H decides once. The reasoning that survives scrutiny: layout is
per-project-per-machine (`01` is right that a user-global layout is wrong the moment two projects
have different panel needs), recents and preferences are user-global, and both are user state
outside property 1. So `<project>/.loom/` exists and `02`'s ADR sentence is amended to *"a project
directory acquires no engine-written files outside `.loom/`, which is gitignored"*. Format is a
coin-flip; `01`'s reason for JSON (`DockState<Tab>` is a deeply nested tagged-enum tree) is
concrete and `07`'s reason for TOML ("authored state in this codebase is diffable text") misapplies
property 1 to state nobody authors.

---

## 12. MEDIUM — two theme token tables, both claiming to be the one `theme.rs`

`01` §6:

> all applied in **one file**, `crates/loom_cli/src/editor/theme.rs` … No panel sets its own
> colours; a panel that needs a colour reads a token.

`07` §13 lists `theme.rs` in `crates/loom_editor/` holding "the §10 tokens".

The tables differ. Same role, different values:

| Role | `01` §6.1 | `07` §10 |
| --- | --- | --- |
| panel surface | `bg_panel` `#16191E` | `bg_1` `#16191E` ✓ |
| raised | `bg_raised` `#1E232A` | `bg_2` `#1E222A` ✗ |
| sunken | `bg_sunken` `#0F1216` | `sunken` `#0A0C0F` ✗ |
| accent | `#A78BFA` | `#7C5CFF` ✗ (and `07` splits it into `accent`/`accent_text`) |
| agent | `#78C8FF` *"**unchanged** — `panels.rs:679`"* | `#34D3C0` ✗ |
| error / warn / ok | `#F0736D` `#E8B84B` `#6FCF97` | `#F2555A` `#E0A33C` `#52C07A` ✗ |

`01` also asserts the agent colour is unchanged from `panels.rs:679`; `07` changes it. Both derive
the same argument for a violet accent, from different hues, and both present computed contrast
ratios as evidence.

**Minimum fix.** One table, in one document, cited by the other. Merge before the theme is written,
because `01` §11 step 4 is *"`theme.rs` alone, over the *old* panels … reversible in one file"* —
which is the right experiment and is worthless if two tables exist.

While merging: **`07`'s contrast column has arithmetic errors.** Recomputed by hand from sRGB
relative luminance against `bg_1 #16191E` (L = 0.00940): `fg_1 #A7AFBD` is **8.0:1**, not the 6.9
claimed; `fg_2 #6B7280` is **3.65:1**, not 3.0. Both errors are conservative, so nothing is
under-contrast, but §14's *"I calculated them by hand … and did not run a checker"* is confirmed and
`fg_2`'s "sitting exactly on a threshold" caveat is wrong in the safe direction. `01`'s spot-checked
values (`text_weak #7C8794` → 4.83, claimed 4.8) are right.

---

## 13. MEDIUM — `01` pins an icon font; `07` rejects icon fonts outright

`01` §6.5:

> **`egui-phosphor = "=0.13.0"`** … Take the `regular` weight only.
> **Rejected: hand-drawn `egui::Shape` paths** (writing an icon set is a week and looks it)

`07` §10:

> **Draw them, do not ship them.** … One `icons.rs` of about 120 lines against `egui::Painter`
> covers it.
> Rejected: an icon font, because it is a new binary asset class, a licence question, and a glyph
> lookup table, and **its stroke weight will not match the gizmo handles that are already
> hand-drawn lines in the same window**.

Each rejects the other's choice, and each rejection contains an argument the other does not
address — `07`'s stroke-weight point is the better one and `01` never sees it; `01`'s "a week and
looks it" is the better cost estimate and `07` claims 120 lines for fourteen icons.

The same split runs through fonts. `01` §6.2: *"**Ship on egui's bundled fonts first** … **If** the
human still reads it as default egui after §6.1–6.4 land, add Inter."* `07` §10: *"**Ship Inter
(Regular and SemiBold, SIL OFL 1.1)**."* And `07` §13 puts the font files in
`crates/loom_render/Cargo.toml` — an editor asset in the crate `06` is trying to strip the editor
out of.

`00-survey-constraints.md` §4.E makes this ADR E's decision, and ADR E cannot record two answers.

**Minimum fix.** ADR E takes `01`'s sequencing (bundled fonts first, measure, then Inter) and
`07`'s icon answer (painter geometry, no new dependency, no new binary asset class) — that
combination adds zero binary assets and zero dependencies in slice one, which is also the cheapest
thing to reverse. Fonts, if adopted, live in `assets/fonts/` and are loaded by the editor crate,
never by `loom_render`.

---

## 14. MEDIUM — `07` makes `xtask` depend on `loom_editor`; `06` forbids it

`07` §12's ADR draft:

> `xtask` gains a dependency on `loom_editor`, which must be checked against
> `scripts/check-deps.sh`.

`06` §6.6:

> A second rule with it: **nothing but `loom_cli` may depend on `loom_editor`** — the same shape as
> the existing `loom_agent` rule, and for the same reason.

`07` §14 flags its own uncertainty and adds the sharper objection:

> It also means `cargo xtask docs` builds the editor, which is a compile-time cost on a project
> whose resequencing trigger is a one-minute warm build.

`LOOM-IMPLEMENTATION-ORDER.md:574` makes that trigger a stop-and-fix condition, and
`00-survey-constraints.md` §2.6 quotes brief §7.6: *"the agent's loop is `cargo check`"*.

**Minimum fix.** Generate both reference files through the CLI, not through xtask's dependency
graph: `loom docs --check` in `loom_cli`, which already links `loom_scene::registry()` for the
component table and would link the command table wherever it lives. `xtask docs` shells out to the
`loom` binary it already builds (`xtask/src/main.rs:470` does exactly this for `image`). CLI-first
is a locked decision and this is the case it was written for.

Related and worth deciding rather than drifting: `07` §3 puts `cargo xtask docs --check` into
`scripts/green.sh`, making a **fifth** green check against `CLAUDE.md`'s "**Definition of green —
all four, every time**". That is fine, and it is a `CLAUDE.md` edit in the same commit, not a
silent addition.

---

## 15. MEDIUM — the "outside undo" exemption list exists three times and is incomplete

`00-survey-constraints.md` §4.J asked for one list:

> The design phase should produce that list; this survey establishes that the list must exist.

It produced three partial ones — `02` §10 (five hub rows), `05` §14 (seven authoring rows),
`06` §5 (two build rows) — and one anomaly nobody listed. `04` §4.1:

> While the mouse is down, the tool paints into the CPU-side image and uploads the dirty rect.
> Nothing is written to the scene. **The viewport is showing state the scene file does not yet
> contain — which is a thing this editor otherwise never does, and is the one deliberate
> exception.**

and `05` §12's in-window script buffer:

> a script file is not scene text, so **Ctrl+Z in that buffer must be egui's text undo** and must
> never reach `Session::undo`.

Against never-do #16 — *"Never give the editor its own undo stack"* — a focus-dependent Ctrl+Z that
sometimes drives egui's text-undo ring and sometimes drives `Session::undo` is the letter of the
rule, not the spirit of it. `05` sees the hazard and mitigates with focus suppression; `02` §10
refuses the identical construction for `loom.toml` and gives the better reason:

> It would be symmetrical and it is precisely what never-do #16 forbids — a second undo stack in
> the editor, with its own semantics, that Ctrl+Z either does or does not reach depending on which
> panel has focus.

**Minimum fix.** One table, in `07` (which owns the user-facing model and already writes
`05-you-and-the-agent.md`), consolidating all three lists plus `04`'s preview divergence, plus
`05`'s isolate/hidden view state, plus `01`'s dock layout. And apply `02` §10's ruling to `05` §12:
the in-window script buffer is dropped in v1 — "Open in your editor" is `05`'s own primary answer
and needs no second undo stack. If the buffer is wanted later it is an ADR, because it is the one
place in the design where Ctrl+Z means two things.

---

## 16. MEDIUM — `03` spends the last push-constant slot, against the documented overflow rule

`CLAUDE.md`, P2 slice 4: *"Wind and the camera position both live in the environment buffer because
the push block is at its 128-byte guarantee."* Verified: `crates/loom_render/src/rain.rs:717-718`
asserts `size_of::<Push>() == 120` and `<= 128`.

`03` §7:

> **Fact 3: the push block has exactly one slot left.** … so one more 8-byte device address fits,
> and it is the last one.
> **Take the push slot; if a later feature needs one, this pointer is the one that moves**

`04` §10.1 reads the same block the other way and follows the established rule:

> The decal list lives in the **environment buffer** and not the push block, because
> `renderer.rs:626-628` records that the push block is at **124 of the 128 bytes Vulkan
> guarantees** … a `decals` device address plus a `decal_count` is the fifth instance of the same
> pattern.

Five prior features went into the environment buffer for this reason; the sixth taking the last push
byte, for a per-node feature carried as a scene-global pointer, is a decision that should be argued
rather than taken because it fits. `03` §13.1 concedes it is unverified:

> Whether Slang accepts a `uint*` in the push block alongside the six pointers already there, and
> whether `size_of::<Push>()` at 128 hits a driver limit rather than the guaranteed minimum.

**Minimum fix.** `vertexColors` goes in the environment buffer with wind, the camera, the terrain
height pointer, the wave set and (per `04`) the decal list. It is read once per vertex, so one extra
indirection is amortised — `03` makes that argument itself, as the reason this pointer is the one
that would move later. Move it now and the push block keeps its four bytes of margin.

---

## 17. MEDIUM — `01`'s tonemap push range of 12 bytes assumes a packing rule

`01` §1.2:

> ```slang
> struct TonemapPush {
>     float exposure;
>     int2  origin;
> };
> ```
> the push-constant range in `create_pipeline` (`tonemap.rs:242`) grows from 4 to 12 bytes.

`CLAUDE.md` never-do #5: *"**Never write `ash` calls from memory.** … The API has churned across
versions and recalled shapes are confidently wrong."* The same caution applies here, one layer up:
12 bytes is the answer under HLSL constant-buffer packing, where a `float` at offset 0 and an `int2`
at offset 4 share a 16-byte row. Under the scalar/std430-style layout Slang emits for Vulkan push
constants, `int2` carries 8-byte alignment, `origin` lands at offset 8, and the block is 16 bytes.
A range declared as 12 against a shader expecting 16 is a validation error at best and a garbage
origin at worst — and a garbage origin means the scene draws in the wrong place, which is the one
symptom `01` §11 step 1 is designed to catch, so it would be caught. It should still not be written
down as a fact.

**Minimum fix.** Reorder to `int2 origin; float exposure;` (12 bytes under either rule, with
`origin` naturally aligned), or read the reflected offsets out of the compiled SPIR-V rather than
asserting them. One line either way.

---

## 18. LOW-MEDIUM — the documents disagree about which crate the editor is in

`01` §9: *"All new editor code lands in `crates/loom_cli/src/editor/`"*, with `gizmo.rs` staying in
`loom_cli` unchanged. `05` §2: `loom_editor/src/gizmo.rs` — *"**moved from `loom_cli/src/gizmo.rs`,
extended**"*. `06` §7: *"`panels.rs` and `gizmo.rs` go wholesale to `loom_editor`."* `07` §13: eight
new modules under `crates/loom_editor/`.

`01` gives a real reason for deferring (*"ADR F's split requires egui to become optional in
`loom_render`"*) which §1 of this review shows is not required. With that premise removed, the
deferral loses its justification and three of four documents already assume the crate.

`05` §6.9 also names the viewport dependency as *"a hard dependency on the render-to-texture
viewport (ADR I)"* — a design `01` explicitly rejected in favour of a sub-rect. The dependency is
real either way; the name is stale and will send a reader to the wrong ADR.

**Minimum fix.** ADR F lands `loom_editor` first, per §1's simplified form. `01` §9's file tree
becomes `crates/loom_editor/src/`. `05` §6.9 cites the viewport ADR by its allocated number (§7).

---

## 19. LOW — `04` proposes a primitive `05` does not know about

`05` §4 fixes the create menu at six entries: *"**Cube**, **Sphere**, **Capsule**, **Plane**,
**Quad**, **Cylinder**"*, and *"**No `cube` alias.** … A second name for one mesh is a second
answer."*

`04` §2.4 adds a seventh:

> **Ship a second primitive, `box_atlas`, whose six faces occupy a 3×2 grid of the unit square**

which is precisely a second name for one mesh, differing only in UVs, and which the paint tool
offers to swap to. `04`'s reasoning for not re-unwrapping `box` is sound (verified at
`crates/loom_asset/src/primitives.rs:66-72` and the test at `:273-281`); the naming collides with
`05`'s stated rule.

**Minimum fix.** `box_atlas` exists in `primitives::NAMES` but not in the create menu — it is a
conversion target, not a thing you create. One line in `05` §4 saying so.

---

## 20. LOW — unverified external facts that a design depends on

Not violations, but each is a place a design would have to be redrawn if the fact is wrong, and none
can be checked in this phase. Recording them so they are checked before code, not after:

- **`egui_dock = "=0.20.1"` and `egui-phosphor = "=0.13.0"`.** Neither is in this machine's cargo
  registry (checked: `~/.cargo/registry/` contains neither). `01` §12.5 already admits the
  crates.io dependency listing was inconsistent between two endpoints. One `cargo add --dry-run`
  settles both, and it is the first thing to run in `01` step 5.
- **`02` §12.3** — whether `loom render --size 480x270` accepts that size. `GOLDEN_SIZE` is
  `"320x200"` (`xtask/src/main.rs:458`), so small sizes work; the aspect is unverified.
- **`06` V0** — `cargo tree --target x86_64-pc-windows-gnu`. `06` §10 correctly notes this is a
  metadata operation that would have been safe and that the instruction was categorical. It is the
  single cheapest de-risking action in the whole rework and it should be step one of the build phase.
- **`04` §12** — the `PARTIALLY_BOUND` descriptor-headroom trick, and whether changing a set
  layout's descriptor count forces a pipeline rebuild in this codebase's exact construction.
- **`05` §16.4** — whether `Volume::edit` applied stamp-by-stamp and `Volume::bake` over the whole
  op list agree bit-for-bit. `05` correctly makes this a gate on its own design: *"If that test
  cannot be made to pass, the preview must fall back to a full re-bake on stroke release."*

---

## What is right, and should not be lost in the rework of the rework

Recorded because an adversarial review that lists only failures misrepresents the set.

- **Never-do #16 is held everywhere.** `05` §1's `Outcome` enum makes it structural rather than
  disciplinary (*"A tool has no `&mut SceneView`, no file handle and no `&mut Session`"*), `07` §8's
  History panel calls `undo()` N times rather than jumping, and every painting design routes through
  `SetField`. This was the rule most likely to be broken and it was not.
- **Never-do #15 is held everywhere.** Every document that touches a rejection discards rather than
  replays: `03` §2, `04` §4.1 (*"Losing at most one stroke is the correct price"*), `05` §12 for
  script files.
- **`LOOM-IMPLEMENTATION-ORDER.md:455-457` ("never bitmaps") is satisfied rather than exempted** by
  all four painting systems. That was the constraint survey's §4.A/B/C and it looked like it would
  need an exemption; it did not.
- **`03` §4's authority channel** is the strongest single idea in the set: it is what stops a
  painted mask freezing `groundLayerWeight`'s low-frequency wander into a raster, which is never-do
  #11 with a different noun.
- **`06` §4.5's V0–V6 sequencing** — prove the cross-compile before drawing the UI for it — is the
  right shape, and V6's refusal to say "Windows supported" without a Windows machine is the honesty
  the operating rules ask for.
- **`01` §1's rect-not-texture viewport** avoids the colour-space and resize-validation traps that
  ADR 0018's consequences paragraph warns about, and §1.7 names the four things it forecloses plus
  the trigger to reverse. That is a reversible decision documented as one.
