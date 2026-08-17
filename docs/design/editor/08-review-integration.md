# Review — the integration lens

*Adversarial read of `00-survey-*.md` and `01`–`07` as a set, looking only for places where two
documents assume incompatible things or where a seam has no owner. Nothing here is about whether an
individual document is good; several of them are very good in isolation, which is exactly how a set
ends up unbuildable. Every claim about the tree was checked with a read-only command at `62f9ebe`
and the command or `file:line` is named. **No `cargo` command was run.***

---

## The one-paragraph verdict

**The seven documents do not describe one editor. They describe between two and four, and the
divergences are load-bearing rather than cosmetic.** Two documents give the same mouse gesture two
incompatible undo models (§C2). Two documents allocate the same 4 bytes of `ObjectData` (§C3). One
document's central mechanism spends a push-constant slot that does not exist, having cited a
different struct's size test (§C4). One document makes a green-check rule that another document's
green-check rule makes impossible (§H3), and a second such pair exists (§C5). Four documents claim
ADR 0022 (§C1). The project manifest has two incompatible schemas and three spellings (§C7), and
under one of them the shipped game does not contain its own scenes (§C6).

The common cause is visible and worth naming: **each document was written against the three surveys
and not against its siblings.** The surveys are excellent and are cited faithfully everywhere. The
siblings are cited *aspirationally* — 03 says its brush model "is shared with them and is written
here once", and 04 then writes a different one; 03 says a varying is worth adding because "UV
painting and decals will both want it", and 04 adds its own instead. Cross-references that assert
agreement without having read the other side are the specific failure mode of this set.

None of this is fatal. All of it is cheaper to fix now than after a commit.

---

## Contents

- **§C — Critical.** The design cannot be built as written. Seven items.
- **§H — High.** Two documents will produce two implementations, or a stated gate cannot pass.
- **§M — Medium.** Real conflicts with local fixes.
- **§S — Seams no document owns.** The list the set is missing.
- **§F — What to do**, as a small number of decisions rather than a long list of edits.

---

# §C — Critical

## C1. Four documents claim ADR 0022, and two claim 0023

Verified: `ls docs/decisions/` ends at `0021-a-reflected-hit-shades-with-the-materials-mean-albedo.md`.
0022 is genuinely next free. It is claimed by:

| Document | 0022 | 0023 |
| --- | --- | --- |
| `01-shell-and-docking.md` §10 | the viewport is a swapchain sub-rectangle | CMAA2 moves ahead of the UI pass |
| `02-project-hub.md` §11 | a project is a directory with a `loom.toml` | `[[asset]].path` is resolved, `id` is reserved |
| `04-painting-uv-and-decals.md` §4.3 | "*next free number at the time of writing is 0022*" | — |
| `05-authoring-tools.md` §13 | unnumbered ("the op vocabulary grows") | — |
| `06-build-and-ship.md` §8 | "ADR A" / "ADR B" | — |
| `07-documentation-and-ux.md` §12 | "ADR 00XX" | — |

04 at least flags the hazard. 01 and 02 do not, and both write their number into a decision
statement a reviewer would approve verbatim.

**Fix:** one allocation table in a `00-adr-allocation.md`, assigned before any ADR is written. On the
count above this rework needs **nine to eleven** ADRs (viewport rect, CMAA2 amendment, project
manifest, asset path resolution, stroke-list painting, splat-biases-slope, UV paint, decals, op
vocabulary, editor/runtime split, Windows cross-compile, UI dependencies, command table). That is a
large approval budget and the human should see it as one number, not discover it seven times.

## C2. Splat painting and UV painting give the same mouse gesture two different undo models

This is the sharpest conflict in the set, and it is the exact question the review brief asks about.

**03 §2** — a stroke is committed *during* the drag:

> Press → `SetField` through `apply_coalescing` with key `paint:{node}:splat:{gesture_epoch}`.
> Drag → append a point when the pointer has travelled `spacing * radius`; **re-issue the same
> `SetField`**. … "on the order of ten transactions a second".

**04 §4.1** — a stroke is committed once, on release, and the viewport lies in the meantime:

> "While the mouse is down, the tool paints into the CPU-side image and uploads the dirty rect.
> **Nothing is written to the scene.** The viewport is showing state the scene file does not yet
> contain — which is a thing this editor otherwise never does, and is the one deliberate exception."
> … "**No gesture coalescing is needed.**"

04 goes further and *rebuts* the survey's prediction that a `paint:{node}:{layer}:{epoch}` key would
be needed — a prediction 03 then implements.

They are not reconcilable by taste, because they differ in observable behaviour on the three cases
that matter:

| | 03 (splat, vertex) | 04 (UV paint) |
| --- | --- | --- |
| Agent writes mid-stroke | tail is abandoned, mask re-baked from reloaded text | **the whole stroke is lost**, by design |
| Ctrl+Z with the mouse still down | ends the gesture run; next frame starts a new entry | nothing to undo — no transaction exists yet |
| Escape mid-stroke | tool state cleared, partial stroke already in the file | tool state cleared, nothing in the file |
| Transaction log during a drag | ~10 rows/s coalesced into one | silent, then one row |
| Crash mid-stroke | partial stroke survives | stroke is gone |

A user who paints terrain substance and then paints a wall in the same session gets two different
answers to "did that land". 07 §8 then designs a History panel with a *"Move Crate (dragging)"* live
row that is correct for 03's model and shows nothing at all for 04's.

There is a further cost nobody priced: **04's diverging preview re-arms the `dirty`-flag trap** the
existing-editor survey lists as item 3 of the things the rewrite must not lose — *"`dirty` set only
when something actually moved; a no-op Ctrl+Z latched it true, the viewport stopped following the
file, and 'Keep mine' then wrote back the text as it was when the editor opened."* An in-progress
stroke is editor state that is not in the file for an unbounded period. 04 says the preview is
re-rasterised on rejection; it never says whether `dirty` is set, and if it is set on press it is
latched for the whole stroke.

**Fix:** pick one, in one place, and make it the *paint gesture contract* every painting tool
implements. 04's argument (one stroke, one transaction, no per-frame whole-file re-emit) is the
stronger one on cost, and 03's own §2 admits it has not measured the payload. But 04's "the viewport
may diverge from the file" exception needs to be an explicit clause of the paint ADR with a stated
`dirty` rule and a stated Escape/reload behaviour, not a sentence in a design doc. If 04's model
wins, 03 §2 is deleted and 03's `spacing`-based decimation becomes preview-only.

## C3. `ObjectData.material.y` is allocated twice, and nobody owns the layout budget

Verified at `crates/loom_render/src/renderer.rs:660-676`:

```rust
    /// Material index in `x`; the rest pads to the 16-byte alignment a
    /// std430 block needs for the member that follows it.
    material: [u32; 4],
```

- **03 §6:** "`material: [u32; 4]` — `x` is the material index today and `y`/`z`/`w` are stated
  padding. **Take `y` for the splat mask's bindless slot.**"
- **04 §1.3:** "`ObjectData.material` is `[u32; 4]` with only `.x` used … **Paint goes in `.y`.**"

Both cite the same comment. Both are correct that the slot is free; exactly one can have it.

04 §10.3 then reserves `.z`/`.w` for a per-object decal `[first, count]` range as its stated upgrade
path — so the whole `material` vector is now spoken for by three features from two documents. And 03
separately appends `splat: [f32; 4]`, taking `ObjectData` from 240 to 256 bytes, without 04 knowing.

**Fix:** one table, in whichever document survives as the GPU-plumbing owner, assigning every free
word of `ObjectData`, `EnvironmentData` and the scene push block across all four painting systems at
once. This is a ten-line table and it is the difference between two features and a silent
mis-decode — which `renderer.rs:602-603` already names as "garbage on screen with no diagnostic".

## C4. 03's vertex-colour design spends a push slot that does not exist, and cites the wrong struct

**03 §7, Fact 3:**

> "the push block has exactly one slot left. `size_of::<Push>()` is pinned at **120** bytes against
> Vulkan's guaranteed 128 (`crates/loom_render/src/rain.rs:717-718`) — so one more 8-byte device
> address fits, and it is the last one. (The doc comment at `renderer.rs:626` says '124 of its 128
> bytes'; the test says 120. **The test is the one that runs.**)"

**There are two unrelated `Push` structs and 03 has conflated them.** Verified:

- `crates/loom_render/src/rain.rs:78-95` — `pub(crate) struct Push` for the **rain compute pass**:
  `eye`, `wind`, `field`, `terrain`, `dims`, `control`, `drops`, `splashes`, `splash_args`,
  `terrain_heights`. This is the struct pinned at 120 by the test at `rain.rs:717`.
- `crates/loom_render/src/renderer.rs:608` — `pub(crate) struct Push` for the **scene pass**:
  `inv_view_proj`, `vertices`, `objects`, `materials`, `particles`, `grass`, `environment`,
  `object_offset`. Its own doc comment reads:

  > "**The block is at 124 of its 128 bytes with this**, which is why the wind parameters the vertex
  > shader also needs live in the environment buffer instead. **There is room for nothing else
  > here.**"

  and, at `:616`, the reason `environment` is placed where it is: *"putting it last would pad the
  block to exactly the 128 bytes Vulkan guarantees, with nothing spare."*

Also verified: **there is no size test for the scene `Push` at all** — `rg 'assert_eq!\(size_of::<Push>' crates/loom_render/src/renderer.rs`
returns nothing, despite the struct's doc comment claiming *"the sizes are asserted in a test"*. So
03's "the test is the one that runs" is doubly wrong: it is a different test, and the struct it
thinks it is measuring is unmeasured.

**Consequence.** `uint* vertexColors` in `scene.slang`'s `Push` is an 8-byte member needing 8-byte
alignment. At 124 bytes used, it lands at offset 128 and takes the block to 136 — **over the Vulkan
guarantee**. 03's central mechanism for vertex colour is unavailable as designed, and 03's
files-touched row (`crates/loom_render/src/rain.rs` — *"re-pin `size_of::<Push>()` 120 → 128, with a
comment that this is the last slot"*) would edit the rain compute pass for a feature that has nothing
to do with it.

04 §10.1 quotes the correct number (124) from the correct struct and correctly routes decals into
`EnvironmentData` for exactly this reason — "the fifth instance of the same pattern".

**Fix:** vertex colours take the same route decals do (a device address in `EnvironmentData`), or a
word of `ObjectData`. And whoever touches this adds the missing `assert_eq!(size_of::<Push>(), …)`
for the scene block, because a doc comment claiming a test that does not exist is how the next person
makes the same mistake.

## C5. "The shipped binary contains no egui" cannot be true — `hud.rs` is runtime and uses egui

**06 §6.6** makes this a green-check rule:

> "The shipped binary contains no egui. `cargo tree -p loom_cli --no-default-features -e normal` must
> not mention `egui`, `egui-winit` or `egui-ash-renderer`. **Add it to `scripts/check-deps.sh`.**"

**02 §6** already warned whoever writes that ADR:

> "**`hud.rs` draws the game's HUD with egui** (`crates/loom_cli/src/hud.rs:16`), so the shipped
> runtime links egui regardless, and 'stripping the editor' means not linking `loom_editor` — not
> making egui optional in `loom_render`. **That materially shrinks ADR F.**"

Verified. `crates/loom_cli/src/hud.rs:16` is `use loom_render::egui;`, and the file is the *game's*
HUD — `egui::Align2` anchors, `egui::FontId`, `align()` mapping the authored `Hud.anchor` strings.
`rg -ln egui crates/loom_cli/src/*.rs` returns exactly `hud.rs`, `gizmo.rs`, `run.rs`, `panels.rs`;
three of those are editor, `hud.rs` is not.

06 never mentions `hud.rs`. Its §1 makes egui optional in `loom_render` behind a non-default
`editor` feature and gates `Viewer::draw_with_ui` and the `ui` render-graph pass on it — which is
precisely the machinery the HUD needs. So a `--no-default-features` build either has no HUD (and
`Hud` becomes a component that silently does nothing in shipped games, the exact "key the parser
ignores" defect class ADR 0008 was written about) or the CI rule fails immediately.

**01 compounds it.** 01 §9's manifest change is `crates/loom_cli/Cargo.toml` gains
`egui = "=0.35.0"`, `egui_dock`, `egui-phosphor` — **as unconditional dependencies**, with the
justification that "that pattern exists for the *ash* rule and egui is not ash". Under 06's split,
`loom-play` lives in `loom_cli` and would link egui from `loom_cli`'s own manifest no matter what
`loom_render`'s feature says.

**Fix:** decide which of three things is true and write it into ADR F: (a) the HUD is re-implemented
without egui in the runtime, (b) egui stays unconditional and "stripping the editor" means not
linking `loom_editor` (02's position, and the cheap one), or (c) the shipped runtime has no HUD. Then
06 §6.6's rule becomes "no `loom_editor`, no `egui_dock`, no `egui-phosphor`" rather than "no egui",
and 01's direct egui pin has to be `optional` regardless.

## C6. Under 02's project layout, the shipped game does not contain its scenes

**02 §1** — what `loom new` creates:

```
KiteHollow/
  loom.toml
  scenes/
    main.loom               what the hub opens
  assets/
    meshes/ textures/ audio/ scripts/ prefabs/ input/
  builds/
```

**06 §3.4** — what ships:

> "**The whole asset tree, minus two things.** Copy `<root>/assets/**` preserving structure, and
> `loom.toml`, and nothing else."

`scenes/main.loom` is not under `assets/`. Every project created from any of 02's three templates
ships without its startup scene, and without the `prefabs/` and `scripts/` trees only if those are
inside `assets/` — which they are in 02's layout, so those survive and the scene does not.

06's tree diagram happens not to expose this because it uses the *engine repo* as its example
(`assets/games/proving_ground.loom`), which is the one project 02 explicitly declares has "an unusual
layout" and whose scenes *are* inside `assets/`.

06 §6.4 asserts "the startup scene is named in the manifest and exists" — but that check reads the
source tree, so it passes. Only §6.8's smoke run against the copied tree would catch it, and 06 §10
lists that run as unexecuted.

**Fix:** 06 copies `<root>/**` minus an exclusion list (`builds/`, `out/`, `target/`, `.git/`,
`.loom/`, `*.meta`), not `<root>/assets/**` plus the manifest. That is also more robust against 02's
stated rule that "nothing enforces this layout".

## C7. Two incompatible `loom.toml` schemas, three spellings, and a `deny_unknown_fields`

**02 §1** defines the manifest as exactly five fields and refuses more:

```toml
[project]
format = 1
id = "…"
name = "Kite Hollow"
main_scene = "scenes/main.loom"
[engine]
version = "0.0.0"
```

> "The struct derives `serde::Deserialize` with `#[serde(deny_unknown_fields)]`"
> … "**No build settings, no window size, no render settings.** None exists yet."

**06 §3.2** requires three keys that are not among them:

| Key 06 needs | Nearest thing 02 defines |
| --- | --- |
| `game.name` | `project.name` |
| `game.startup_scene` | `project.main_scene` |
| `build.targets` | *explicitly excluded* |

Under `deny_unknown_fields`, a manifest that satisfies 06 is rejected by 02's loader. 06 is aware it
is depending on an unwritten document ("The manifest format is not mine to define") but specifies
its keys in a different table with different names rather than asking for the ones that exist.

**07 §6 adds a third spelling** — its templates contain `project.toml`, not `loom.toml`.

And **06 §3.1 creates a second reader** (`crates/loom_cli/src/project.rs`, ~30 lines) while 02 §2
puts `Project`/`load`/`find_root` in `loom_scene::project` — after which 06's own §3.2 warns "**What
must not happen is a second manifest reader**".

**Fix:** 02 owns the schema, adds `[build] targets` (additive, and 02's own §9-of-the-format-spec
argument says adding a table is free), renames nothing, and 06 reads it from `loom_scene::project`.
`main_scene` is the name; `startup_scene` is deleted from 06.

---

# §H — High

## H1. Three homes for editor preferences, two for the dock layout, two file formats

| Document | Location | Format | Contents | Stated reasoning |
| --- | --- | --- | --- | --- |
| 01 §4 | `<project>/.loom/layout.json`, `<project>/.loom/layouts/<name>.json` | **JSON** | dock layout, named presets | "Per-project-per-machine is the only combination that is right", gitignored |
| 01 §4 | `~/.config/loom/editor.json` | **JSON** | theme scale, recents, last project | — |
| 02 §4 | `$XDG_STATE_HOME/loom/hub.toml` | **TOML** | recents, `new_project_dir` | "**State, not config**, because every byte of it is written by the program"; explicitly argues against `~/.config` |
| 07 §6 | `$XDG_CONFIG_HOME/loom/editor.toml` | **TOML** | recents, window geometry, **dock layout**, zoom, reduce-motion, high-contrast, onboarding flag | "settings are authored state, and authored state in this codebase is diffable text" |

07 puts the dock layout in a *per-user* file, which 01 spent a paragraph arguing "would be wrong the
moment two projects have different panel needs". 07 puts recents in `~/.config`, which 02 spent a
paragraph arguing is a category error. 02 says "**A project directory acquires no engine-written
files**" in its ADR; 01 writes `<project>/.loom/`.

Three documents, three answers, and all three assert theirs is the principled one.

**Fix:** 02's split is the coherent one (state in `$XDG_STATE_HOME`, and nothing in the project
directory), with 01's per-project layout keyed by project path *inside* that state file rather than
inside the project. Format: TOML for the hand-readable ones, JSON only for `DockState`, and 01's
argument for JSON there is sound and survives.

## H2. Where the editor code lives is answered three ways, and the build orders collide

- **01 §9:** `crates/loom_cli/src/editor/`, and explicitly **not** lifted now — "It is not lifted now
  because ADR F's split requires egui to become *optional* in `loom_render`, and `materials.rs` and
  `log.rs` are used by the headless render path". `gizmo.rs` **unchanged, stays where it is**.
- **05 §2:** `loom_editor/src/…`, `gizmo.rs` **"moved from `loom_cli/src/gizmo.rs`, extended"** — with
  a fallback sentence saying if ADR F does not land, read `crates/loom_cli/src/editor/` instead.
- **06 §1/§7:** creates `crates/loom_editor/`; `panels.rs` and `gizmo.rs` "go wholesale".
- **07 §13:** eight new modules in `crates/loom_editor/`.

The orders then collide. 01 §11 is a nine-step build order whose steps 1–5 put dock, theme and
viewport into `loom_cli/src/editor/`. 06 §9 is a five-step order whose **step 2** is "ADR A: the crate
split and the feature gate". Both are the first thing to build; if 06's step 2 runs first, 01's
steps 4–7 are written into a crate that is about to be split; if 01's runs first, 06 moves code that
was just written.

01's reason for deferring is also partly falsified by C5: if egui does not have to become optional in
`loom_render` (02's finding), the objection shrinks to `materials.rs`/`log.rs`, which stay in
`loom_cli` anyway.

**Fix:** one order, agreed. The lazy sequencing is 06's crate split *first* (it is mechanical, it is
checkable by `cargo tree`, and it is the thing every other document assumes), then 01's shell inside
it.

## H3. 07's command table forces `xtask → loom_editor`; 06 makes that a green-check failure

**07 §4** puts the command vocabulary in `loom_editor::command::COMMANDS` and **§12** makes
`cargo xtask docs --check` — which generates `commands.md` from that table — part of
`scripts/green.sh`. Its own §12 consequences say: "`xtask` gains a dependency on `loom_editor`, which
must be checked against `scripts/check-deps.sh`", and §14 lists it as unverified.

**06 §6.6** adds to the same script: "**nothing but `loom_cli` may depend on `loom_editor`** — the
same shape as the existing `loom_agent` rule, and for the same reason."

Both are green checks. They cannot both pass. Verified that the script's existing rules are of
exactly this shape (`scripts/check-deps.sh:19-28`, `loom_reflect`/`loom_scene` dependency walks).

There is a second cost even if the rule is relaxed: `cargo xtask docs` would then **build the whole
editor** on every `green.sh` run, against `LOOM-IMPLEMENTATION-ORDER.md:574`'s stop-and-fix trigger of
a one-minute warm build.

**Fix:** `COMMANDS` is plain data with no editor dependencies — put it in its own leaf module that
`xtask` can read without pulling egui, or generate `commands.md` from `loom_cli` (which `xtask`
already drives as a subprocess for `image`/`flythrough`) rather than by linking. The subprocess route
is the one the rest of this project already uses and it costs nothing.

## H4. 05 depends on a viewport mechanism 01 rejected, by name

**05 §6, change 9:**

> "**This is a hard dependency on the render-to-texture viewport (ADR I) and the authoring layer
> cannot be finished before it**; until then every tool is off by the panel widths."

**01 §0** rejects render-to-texture in its first section and spends §1 specifying the alternative:

> "The obvious implementation — render the scene into an offscreen image, wrap it in a descriptor
> set, hand it to egui as a `TextureId` … **The alternative is one push constant.**"

The *dependency* 05 names is real — tools need rect-relative coordinates, which is 01 §1.5's
`to_viewport`/`to_window` — but the mechanism is not, and an implementer reading 05 in isolation
would build the texture path 01 argues against at length, including the resize policy and the colour
round-trip 01 §12 flags as unverified.

05 also lists `loom_editor/src/gizmo.rs` as taking `View` from the viewport rect, which is correct
under 01's design too; only the sentence naming "render-to-texture" and "ADR I" is wrong.

**Fix:** one line in 05. But it is worth noting *why* it happened: 05 cites "ADR I" from the
constraints survey's provisional lettering, and 01 resolved that letter into a decision without 05
knowing. Provisional letters that later become real decisions need a back-reference.

## H5. Nobody owns the composite order in `fragmentMain`

Four features now write into the same shading path, from two documents, each specifying its own
insertion point relative to *one* neighbour:

| Feature | Doc | Where it says it goes | What it does |
| --- | --- | --- | --- |
| UV paint | 04 §1.3 | "immediately after the albedo-map block and **BEFORE the ground layer**" | `albedo = lerp(albedo, p.rgb, p.a)` |
| Splat | 03 §4 | *inside* the ground-layer block | `w = lerp(groundLayerWeight(…), m.r, m.g)` |
| Vertex colour | 03 §7 | "the fragment stage multiplies it into `albedo`" — **position unstated** | multiply |
| Decals | 04 §10.1 | "**after the ground layer**, before the wet block" | `albedo = lerp(albedo, t.rgb, a)` |

These do not commute. Paint-before-layer plus splat-biasing-the-layer means a painted mark on a
slope can be overpainted by splat-painted rock — two painting systems the user was told are
independent, fighting over one pixel, with no stated winner. 04 §12 admits its own placement is "a
taste call I could not settle without looking at a render"; 03 never raises the question.

And vertex colour's position is genuinely undefined: multiplied before paint it tints the base and
paint covers it; multiplied after decals it tints the decal too.

**Fix:** one ordered list, in one place, written as the shader comment it will become. My reading of
the four arguments gives: `base albedo → vertex tint → UV paint → ground layer (splat-biased) →
decals → wet/fog/lighting`. That is a decision, not an obvious truth, and it belongs in whichever
painting ADR lands first.

## H6. Two brush architectures, and 03 claims there is one

**03 §0** opens with:

> "the **brush model, the stroke schema and the transaction shape in §1–§3 are shared with them**
> and are written here once."

They are not. Every axis differs:

| | 03 (`loom_paint`) | 04 (`loom_asset::paint`) |
| --- | --- | --- |
| Crate | new `loom_paint` | module in `loom_asset` |
| Schema | **typed**, `#[derive(JsonSchema)]`, "strictly better than the precedent [`VoxelVolume.ops`] and worth not copying blindly" | **untyped** `Vec<serde_json::Value>`, "on purpose and by precedent", validated in one funnel |
| Radius units | **world metres** ("A screen-space radius cannot be serialised into a stroke that reproduces") | **UV units** ("so a scene's strokes survive a change of `resolution`") |
| Fields | `radius, hardness, strength, flow, spacing` | `radius, hardness, flow, color`, no `spacing` |
| Erase | **no mode** — "erasing is painting toward zero, and a separate mode would be a second spelling of the same arithmetic" | **`kind = "erase"`** is a first-class stroke kind |
| Kinds | one shape | `stroke` / `stamp` / `erase` |
| Points | world XZ (splat) / world XYZ (vertex) | UV `0..1` |
| Dabs | derived from `spacing` at bake time | not modelled |

Both are defensible. Both cannot be the brush the user holds. The toolbar has one `[`/`]` radius
control and it would mean metres on terrain and unit-square fractions on a wall — with no indication
which, since 03 §9's cursor readout prints `1.5 m · 60%`.

The two are also *justified against each other's precedent*: 03 says typed is "strictly better than
the precedent" of `VoxelVolume.ops`; 04 chooses that precedent deliberately and cites the four silent
failures the validation funnel exists to stop.

**Fix:** one `Stroke` type, typed (03's argument is stronger — the funnel 04 needs exists precisely
because untyped arrays are hard, and a stroke genuinely has one shape). Radius carried in the
stroke's own coordinate space with the space named on the component, so the UI can label it. `erase`
folded into `strength = 0` per 03. One crate — and it should be `loom_asset::paint`, because 03's own
stated dependency is `loom_asset` and nothing else, which is the definition of a module rather than a
crate.

## H7. Three mechanisms for one job: getting a CPU-rasterised texture to the GPU

- **03 §6:** `Viewer::set_material_texture(slot, &Texture)`, reusing `material::record`'s one-shot
  submit, *outside* the render loop. And, explicitly: "**The `Viewer` texture-update path is
  implementation under never-do #4, not an ADR** — it adds no pass and no hand-placed barrier as long
  as it stays a one-shot submit. **It becomes ADR territory the moment it is recorded into the
  frame's command buffer.**"
- **04 §3.2:** a `paint_upload` render-graph pass with `Access::TransferDst`, plus adding the paint
  image to `forward_uses` with `Access::ShaderRead` — "a genuine change to `forward_uses` … today no
  material texture is in the graph at all" — plus two new named transitions in the barrier-list test.
  That is exactly the thing 03 says makes it ADR territory.
- **04 §3.3:** a *third* path, `Viewer::set_materials`, with `device_wait_idle` + `reset_command_buffer`
  + descriptor headroom, for adding a paint layer mid-session.

So 03's "no ADR needed" is falsified by its sibling before either is built, and an implementation of
the set would grow three ways to upload a texture.

(04's use of `import_with_layout(…, SHADER_READ_ONLY_OPTIMAL)` is verified to exist —
`crates/loom_render_graph/src/lib.rs:411` — and the reasoning there is the best paragraph in either
document. It is the mechanism that should win.)

**Fix:** one path, in the graph, per 04 §3.2, and it needs the ADR 03 said it would need.

## H8. All four painting systems sit on a `Materials` update path that may not exist, and nobody owns fixing it

03 §6 raises it and §13 item 2 admits it is unverified:

> "As far as reading shows, **an inspector edit to `Material.roughness` in `loom run --edit` does not
> reach the GPU today** — that is a pre-existing gap, not one painting introduces, and painting
> cannot ship without closing it."

04 §3.3 designs headroom and `set_materials` for a related but different reason (descriptor array
growth). 05 and 07 both design inspector work that assumes material edits are live — 07 §7's "Fix"
button issues a `SetField` and expects the viewport to follow.

**This is a pre-existing defect that four documents depend on and none owns.** It is also cheap to
settle: open `loom run --edit`, drag a roughness slider, look. It should be settled *before* the
painting ADRs, because if material edits are already live the plumbing in 03 §6 and 04 §3.3 is
smaller than both documents believe.

## H9. The prefab load-path bug has two claimants and appears in no build order

Verified live: `crates/loom_cli/src/scene_view.rs:110` is `Scene::parse(text)`, not
`prefab_load::for_reading`.

- `00-survey-engine-surface.md` finding 3: "**`loom run --edit assets/test/prefab_room.loom` is that
  bug, live.**"
- 05 §11: "**Fix the load path before building any prefab UI.** … Any authoring tool built on top of
  an unresolved scene is authoring against a lie."
- 06 §6.1 makes `loom ship` go through `for_reading`, fixing a third reader while leaving the editor's.

01's nine-step build order never mentions it, and its step 5 moves the existing panel bodies into the
dock unchanged — so the Unity-shaped editor ships still not resolving prefabs, and every tool 05
builds on top of it inherits the lie.

**Fix:** it is a one-line change and it belongs in step 1 of whichever order runs first, with the
regression test ADR 0008 asks for.

## H10. Five documents each add to the golden gate; nobody states the total

Verified current: `xtask/src/main.rs:41` — `const SCENES: [&str; 43]`; `:253` — `const GOLDEN: [(&str, &str, &[&str]); 28]`.

| Document | Adds |
| --- | --- |
| 02 §9 | `empty` to `SCENES` **and `GOLDEN`** (explicitly "an extension of the stated rule"), `first_person` + `third_person` to `SCENES` |
| 03 §10 | `painted` to `SCENES` and `GOLDEN` |
| 04 §8, §11 | `paint_wall` **and** `decals` to `SCENES` and `GOLDEN` |
| 05 §15 | a sculpt-produced scene to `SCENES` |
| 06 §1, §7 | "one extra render in `image`: a golden scene through the **no-default-features binary**" |

That is +6 `SCENES`, +4 `GOLDEN`, and one entry that is not a scene at all but a *build-configuration
comparison* wearing the gate's clothes. 07 adds `cargo xtask docs --check` to `green.sh` on top.

Nobody prices the result, and `cargo xtask` gates are a cross-worktree singleton that serialise — so
gate time is the developer's iteration loop. 02's `empty` is also the only proposed GOLDEN entry that
admits it covers no new *rendering path*, which is the stated rule; its argument ("it is the one
scene whose appearance is itself the deliverable") is good but it is a rule change and should be
recorded as one.

---

# §M — Medium

**M1. Two documents each promise "zero references move" while both edit `fragmentMain`.**
01 §11 step 1's exit criterion is "`cargo xtask image` produces **zero** changed references", and §9
says a moved reference means "the change is wrong". 03 slice 2 makes the same promise. But 03 changes
`ObjectData` 240→256 and adds a `nointerpolation uint object` varying; 04 adds a `paint` varying and
a 16-iteration decal loop. Each argues byte-identity in isolation (branch-uniformity, `lerp(w,x,0)`
exactness). Nobody owns the *combined* claim, which is the one that will be measured.

**M2. `Camera.boom` changes `CameraView` for every scene and is unpinned.** 02 §9 argues no ADR and
no format bump because `boom = 0.0` reproduces today's behaviour "by construction" — while §12 item 4
admits the author did not read `active_camera`'s body and that "the sign convention is the thing most
likely to be backwards on the first attempt". A field on the *camera derivation path* that every
golden image and the editor's opening framing flow through deserves the golden gate run before the
templates depend on it. No document says who runs it.

**M3. The cwd-relative bindings bug is fixed four times with three different resolution orders.**
01 §7: `<project>/assets/input/*` → `<exe_dir>/assets/input/*` → compiled-in. 02 §8: `engine_assets()`
= `<exe dir>/assets` else cwd. 06 §3.3: `load_bindings(root)` where root is exe-dir when shipped,
project-dir when editing. 07 §11: `current_exe()`'s parent, cwd fallback. Only 01's project-first
order satisfies the shared claim that "a project can own its bindings"; 02's and 07's exe-dir-first
would make a project's own bindings unreachable in the editor.

**M4. One array-write op is proposed twice and ignored once.** 04 §4.4 proposes `AppendToArray` and
explicitly asks for it to be merged with any `AppendVoxelOps`; 05 §13 proposes `SpliceArray`, which
subsumes it and names four existing callers. Good. But **03 §2 builds splat and vertex painting on
whole-array `SetField`** and never mentions either, accepting the unreadable diff that 05 §10.2
proves is unacceptable for voxels. One editor would then have three array-write disciplines.

**M5. Two owners for the transaction label.** 05 §1: `Edit { label: String }`, "written by the tool,
per gesture, **never generated from the op list**". 07 §4: `Command { label: Option<&'static str> }`,
"The transaction label this command writes, with `{}` filled from the selection", and argues this is
why the table earns its keep. A tool-driven gesture and a palette-driven command would produce
different labels for the same action.

**M6. The Hub is in two binaries, and the agent CLI acquires an editor dependency.** 02 §6 argues
four ways for `loom edit` inside the `loom` binary. 07 §6 says "`loom-editor` with no argument opens
the Hub". 06 sets `[[bin]] name = "loom" … required-features = ["editor"]` — which means `loom
validate`, `loom render`, `loom sim` and `loom scene --tx`, i.e. the entire agent surface and
everything `cargo xtask` drives, cannot be built without egui. That inverts the split 06 exists to
create.

**M7. `primitives::NAMES` grows to 6 in one document and 7 in another.** 05 §4 adds `quad` and
changes the type to `[&str; 6]`, and enumerates the create menu without `box_atlas`. 04 §2.4 adds
`box_atlas` "and (`quad`, already owed)" and has the paint tool offer "swap to `box_atlas`?" — a mesh
the create menu cannot make.

**M8. The divergence banner now has four writers and no stated input contract.** 03 §9 (suppress the
brush preview while it is up), 04 §4.1 (a rejected commit discards the stroke), 05 §1 (`ToolState`
cleared by any external reload), 07 §7 (rewording plus a change count from `changes_from`). Nobody
says who suppresses *input* while it is raised, which is the thing that decides whether a stroke can
start against a scene the user has not chosen yet.

**M9. `loom_paint` vs `loom_asset::paint`.** Two homes for one rasteriser. 03's justification for a
crate — "CPU only, depends on `loom_asset` and nothing else" — is the definition of a module. Both
need the same reach: `loom_cli`'s `GroundGrid` (03's grass hook) and `loom validate` (04's funnel).

**M10. Two shipped-folder layouts, and 07's offline docs are not in 06's.** 06 §2 ships
`<name>`, `loom.toml`, `assets/`, `.loom-build.json`. 07 §11 ships `loom-editor[.exe]`, `<game>[.exe]`,
`assets/`, **`docs/`**, `projects/` — and 07 §2 requires the docs to be "shipped alongside the binary
so F1's external links work offline". 06's tree cannot satisfy that, and 07's tree ships the editor
into a release folder, which is the thing 06 exists to prevent.

**M11. Three partial "outside undo" lists; no union.** The constraints survey §4.J asked for *the*
list. 02 §10 (hub actions), 05 §14 (authoring, the fullest), 06 §5 (build), plus 01 §4 (layout) and
04 §4.1's implicit entry (an in-progress stroke). 07 §2 then designates `05-you-and-the-agent.md` as
the file that documents it — a file that has no source to document.

**M12. Three things now carry `--frames`, and no document says which one the gate drives.** 01 §4:
`--frames n` ignores the saved layout. 02 §6: `loom run` stays the drivable viewer, `loom edit` is the
application. 06 §7: `loom-play` gains `--render`/`--frames` for the smoke check. `xtask/src/main.rs`
opens five windows through one of them; whether it now opens ten is unstated.

**M13. 04's diverging preview and the `dirty` flag.** Covered in C2; recorded separately because it
is a distinct one-line contract (`dirty` is set on commit, not on press) that no document states.

**M14. Two token tables for one `theme.rs`, and ADR E receives two opposite icon answers.**

| Token | 01 §6.1 | 07 §10 |
| --- | --- | --- |
| panel | `bg_panel #16191E` | `bg_1 #16191E` ✓ same hex, different name |
| raised | `bg_raised #1E232A` | `bg_2 #1E222A` |
| accent | `#A78BFA` (one token) | `#7C5CFF` + `accent_text #A18FFF` (two) |
| agent | `#78C8FF` — "**unchanged**", matching `panels.rs:679` | `#34D3C0` — teal, a *change* to existing meaning |
| ok / warn / error | `#6FCF97` / `#E8B84B` / `#F0736D` | `#52C07A` / `#E0A33C` / `#F2555A` |

Both say colour is defined in exactly one place. 07 also changes the agent hue that 01 pinned as
unchanged — and the agent colour is the one hue in this editor with an existing meaning a user has
already learned. **Icons:** 01 §6.5 takes `egui-phosphor = "=0.13.0"` and rejects hand-drawn shapes
("writing an icon set is a week and looks it"); 07 §10 rejects an icon font ("a new binary asset
class, a licence question") and hand-draws ~14 in `icons.rs`. ADR E gets both submissions.

**M15. Fonts contradict too.** 01 §6.2: ship on egui's bundled fonts, add Inter **only if** the human
still reads it as default egui — "that sequencing is deliberate". 07 §10: "Ship Inter (Regular and
SemiBold)" as settled, and §13 lists the files under `crates/loom_render/Cargo.toml`, which is the
wrong crate for an asset only the editor uses.

**M16. Thumbnails shell out to a subcommand whose binary is about to grow a feature gate.** 02 §4
spawns `current_exe() render … --size 480x270`; 06 puts `render` in the `loom` binary behind
`required-features = ["editor"]` and adds a second `--render` to `loom-play`. Harmless in the editor,
but it is now ambiguous which binary the hub is invoking, and 02 §12 item 3 flags `--size 480x270` as
unverified.

**M17. `chrome_clear` and the barrier-list test across two build configurations.** 01 §1.9 adds
`chrome_clear` to the graph and requires the barrier-list test to name it. 06 §1 asserts the graph is
identical with and without the `editor` feature ("the forward pass, the tonemap, MSAA, the resolve
and CMAA2 are all unconditional"). If `chrome_clear` exists only when a placement does, the test must
be feature-aware or configuration-aware. Neither document says.

**M18. New panels are missing from the persisted tab enum.** 01 §2.2's `Tab` enum has twelve
variants and is `serde`-persisted. 07 introduces **Problems** and **History** panels (and replaces the
console's transaction column). Adding variants later invalidates every saved layout — 01 handles that
gracefully (warn, fall back to default), but it means every user's arrangement resets when 07 lands,
which is avoidable by agreeing the enum once.

**M19. 07 declares `ops.rs` untouched; 05 rewrites it.** 07 §13: "**Untouched, deliberately:**
`loom_scene/src/edit.rs` and `ops.rs` … If an implementation of this doc finds itself editing
`edit.rs`, something has gone wrong." 05 §13 adds `SpliceArray`, `Declare`, `SpawnNode.prefab` and the
`f32` round-trip fix to `ops.rs`. 07's statement is about *its own* changes and is technically
compatible, but read as a set it says the op vocabulary is frozen while a sibling grows it 22%.

**M20. Two unverified `loom_scene` facts carry two panels.** 07 §14 does not know whether
`Session::history()` is public (its History panel needs it plus undo/redo depths); 05 §14 finds that
`Transaction::dry_run` is honoured by the CLI and **not** by `Session` (`edit.rs:298-311` commits
regardless), which its preview-before-commit note depends on. Both are one `rg` away and neither was
run.

---

# §S — Seams no document owns

The list the set is missing. Each of these is a decision that at least two documents assume someone
else made.

1. **The `fragmentMain` composite order** across paint, splat, vertex colour and decals. (H5)
2. **The GPU byte budget** — every free word of `ObjectData`, `EnvironmentData` and the scene push
   block, allocated once across all four painting systems. (C3, C4)
3. **The paint gesture contract** — one undo model, one `dirty` rule, one Escape/reload/rejection
   behaviour, for all four painting tools. (C2, M8, M13)
4. **One `Stroke` schema and one brush**, with radius units named per component. (H6)
5. **The union "outside undo" list.** Three partial lists exist; the file that documents it (07's
   `05-you-and-the-agent.md`) has no source. (M11)
6. **`Materials` live update.** A pre-existing gap four documents build on and none owns. (H8)
7. **The prefab load-path fix** — an owner and a position in a build order. (H9)
8. **`loom.toml`'s schema, its reader, and its crate.** (C7)
9. **Editor preference and layout storage.** (H1)
10. **ADR numbering, and the total approval budget.** (C1)
11. **ADR E's contents** — icons, fonts, docking, gizmo — currently receiving contradictory
    submissions from 01, 05 and 07. (M14, M15; note 05 §6 also settles the *gizmo* half unilaterally
    by rejecting `transform-gizmo`, which `LOOM-IMPLEMENTATION-ORDER.md:434` named.)
12. **Total gate cost** — `SCENES`/`GOLDEN` growth, the second-configuration render, and
    `xtask docs --check`, against a serialised cross-worktree gate. (H10, H3)
13. **Which binary `cargo xtask validate` drives, and how many windows it opens.** (M12, M6)
14. **The shipped folder's contents** — scenes, docs, HUD, editor. (C5, C6, M10)
15. **What happens to a running Play session when the agent writes.** Every document inherits
    "the watcher is asleep while Play runs" (`run.rs:972-974`) and none of them says what a docked
    **Game** tab (01 §2.2), a paint tool, or a build subprocess does with it.
16. **Whether the four painting systems can coexist on one node.** A node with `SplatPaint`,
    `VertexPaint`, `PaintLayer` and a `Decal` overhead is legal under every schema proposed and is
    specified nowhere. `loom validate` needs a rule.

---

# §F — What to do

Six decisions close most of this. They are cheap because they are all "pick one of two already-written
answers", not new design.

1. **Allocate ADR numbers and count them.** One table, before any ADR text. (C1)
2. **One paint gesture contract and one `Stroke` type**, written into the painting ADR: 04's
   commit-on-release model, 03's typed schema, one crate (`loom_asset::paint`), one composite order,
   one GPU byte-budget table. This collapses C2, C3, C4, H5, H6, H7, M4, M9, M13 into a single
   document. (03 and 04 should merge or one should become a strict extension of the other.)
3. **Settle the egui question with `hud.rs` on the table.** It decides C5, and it shrinks or
   eliminates 06's feature-gate machinery, which in turn unblocks H2's ordering.
4. **02 owns `loom.toml`; 06 reads it.** Add `[build] targets`, delete `game.*`, delete the second
   reader, and copy the project root rather than `assets/`. (C6, C7, M10)
5. **Agree the crate layout and one build order**, with the prefab load-path fix and the
   `Materials` update check in step 1. (H2, H8, H9)
6. **Merge 01 §6 and 07 §10 into one theme and one ADR E submission**, keeping the existing agent
   hue. (M14, M15)

Two smaller things worth doing in the same pass because they are one line each and they are the kind
of error that survives: fix 05's "render-to-texture (ADR I)" reference (H4), and add the missing
`assert_eq!(size_of::<Push>(), …)` for the scene push block, whose doc comment already claims a test
that does not exist (C4).

---

## What I could not verify

No `cargo` command was run — the phase forbids it, and every dependency, feature-resolution and
compile claim in the set therefore remains unchecked by a compiler. Specifically unresolved here:

- **Whether `--no-default-features` actually keeps egui out**, which 06 §10 also lists. My finding in
  C5 is about `hud.rs` needing egui at runtime, which is independent of feature resolution and is
  verified by reading the file.
- **Whether the scene push block is 124 bytes.** The struct's own doc comment says so and I did not
  compute the layout field by field; what I did verify is that it is a *different struct* from the one
  03 cites, that the cited test measures the rain compute block, and that the scene block has no size
  test.
- **Whether `Materials` really has no public update path** (H8). 03 §13 raised it as unverified and I
  did not read `material.rs` end to end; I am escalating it as an unowned seam rather than confirming
  the defect.
- **egui_dock 0.20.1, egui-phosphor 0.13.0 and their egui-0.35 compatibility** — 01 §12 items 4 and 5
  flag these and I did not re-check crates.io.
- **Whether any of the "no reference moves" claims hold** (M1). That is a gate run, not a read.
- **Contrast ratios in either theme table.** Both documents computed their own by hand; I did not
  recompute either, and the conflict in M14 is about which table exists, not about the arithmetic.
