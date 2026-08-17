# Review — feasibility and scale

*Adversarial review of `docs/design/editor/00-*` through `07-*`, written by the engineer who
would have to build it. Design phase: no `cargo` command was run. Every claim about the tree
below was checked with `rg`/`sed`/`rpm`/`which` in this worktree at `62f9ebe`, and the check is
named. Where I could not check, I say so.*

**The verdict in one paragraph.** The engineering *reasoning* in this set is unusually good —
the render-to-rect argument, the stroke-list-not-bitmap argument, the "the preview is the only
thing allowed to diverge" argument and the Windows evidence are all better than I expected, and
several are things I would not have thought of. What the set does not have is a **plan**. There
is no combined build order, no total, and each document sized its own work against the assumption
that it was the only work happening. Seven of the twelve tabs the default layout declares have no
design document at all — including the Inspector, which is the panel a user spends most of their
day in and the one with the largest measured gap. Four documents claim ADR number 0022. Two claim
the same `ObjectData` field. Two specify contradictory theme palettes and contradictory icon
strategies. One (`06`) proposes a runtime/editor split that **cannot work as written**, and its
sibling (`02`) says so without either of them noticing. My estimate for the set as specified is
**19,000–25,000 lines and 9–14 months** for one developer with an agent, against a codebase that
is 62,008 lines today (`wc -l crates/*/src/*.rs`). §9 cuts that to a usable editor in roughly a
third of it.

---

## 1. The critical path, which no document states

Read across the seven documents, the dependency structure is much more sequential than the
per-document build orders imply. The spine:

```
    prefab_load fix ──┐                                      (1 line, do it today)
    SetTransform f32 ─┤                                      (3 lines, blocks snapping)
                      │
    ViewportPlacement ─────► coordinate remap ─────► egui_dock shell ─────► every panel
      (doc 01 §1)             (to_viewport)           (doc 01 §2-3)          and every tool
           ▲                                                                      ▲
           │                                                                      │
      the one-frame lag (§3.1) — unsolved, unnamed                                │
                                                                                  │
    op vocabulary (SpliceArray/Declare/SpawnNode{prefab}) ─────────────────────────┤
      (doc 05 §13) — independent of the viewport entirely                          │
                                                                                  │
    Viewer material/texture update path ──────────────────────────────────────────┤
      (doc 03 §6 / doc 04 §3.3 — two designs, no owner)                            │
                                                                                  │
    crate split + loom-play + ship ───────────────────────────────────────────────┘
      (doc 06 — independent; ADR A is broken, §3.2)
```

**Everything visual is gated on one change**, and it is the least verifiable one. Doc 05 §6.9
states it outright: *"This is a hard dependency on the render-to-texture viewport (ADR I) and the
authoring layer cannot be finished before it; until then every tool is off by the panel widths."*
So the shell is not "phase one of several parallel tracks" — it is the trunk.

**What is genuinely parallel, and should start immediately because none of it waits on the trunk:**

| Track | Depends on | Why it can start now |
| --- | --- | --- |
| Windows V0–V3 (doc 06 §4.5) | nothing | `cargo tree --target` is metadata. It is the only thing that could invalidate a whole deliverable, and it costs an afternoon. **Do it first.** |
| Op vocabulary + `SetTransform` f32 (doc 05 §7, §13) | nothing | Pure `loom_scene`. Unblocks sculpt, prefab instancing, and four array fields the inspector cannot edit. |
| `prefab_load::for_reading` in `scene_view.rs:110` | nothing | One line. `loom run --edit assets/test/prefab_room.loom` is a live bug today. |
| `Viewer` material update path | nothing | Renderer-only. Prerequisite for all four painting systems *and* for a bug that already ships (§4.3). |
| Hub / `loom new` / templates (doc 02) | nothing structural | The hub is a full-window egui state with no `Viewer`. It does not need the dock. |
| Theme tokens (doc 01 step 4) | nothing | Explicitly designed to land over the *old* panels. Good call. |
| Decals (doc 04 Part II) | material update path | One loop in `fragmentMain`. No pass, no `SceneOp`, no barrier. |

**What is strictly sequential and must not be parallelised:** `ViewportPlacement` → coordinate
remap → dock → panels → tools. Attempting the tool layer before the coordinate remap produces
tools that are correct in the old layout and wrong in the new one, which is a whole rewrite of
their input path.

---

## 2. Size estimate per subsystem

Estimates are new-and-changed lines including tests, calibrated against this repo: the entire
current editor is ~4,400 lines (`run.rs` 2312 + `panels.rs` 898 + `gizmo.rs` 280 + `hud.rs` 496
+ `scene_view.rs` 390 + `materials.rs` 429, verified by `wc -l`), and P2 grass took nine slices
to reach "blades render and bend". Calendar is one developer with an agent, at this project's
demonstrated slice cadence.

| Subsystem | Doc | Doc's own estimate | Mine | Calendar | Confidence |
| --- | --- | --- | --- | --- | --- |
| `ViewportPlacement` + coords + `chrome_clear` | 01 §1 | "two-line change made in one place" | **700–1,100** | 2–3 wk | low — see §3.1 |
| egui_dock shell, tabs, layout persistence, Window menu, maximise | 01 §2–5 | none given | **900–1,400** | 2–3 wk | medium |
| Theme + icons | 01 §6 / 07 §10 | "one file" | **500–900** (two conflicting specs, §6) | 1–2 wk | medium |
| **The seven undesigned panels** (§5) | — | **none** | **4,000–6,000** | 6–10 wk | low |
| Hub, `loom.toml`, `loom new`, templates, thumbnails, XDG | 02 | "roughly 200 lines" for `project.rs` | **1,200–1,800** total | 2–3 wk | medium |
| Op vocabulary + `SetTransform` f32 | 05 §13 | none given | **600–900** | 1.5–2.5 wk | medium — §4.4 |
| Tools: cursor, select/marquee, gizmo ×9, snap, arrange, create, prefabize, script | 05 §3–12 | "a few hundred lines against 280" for the gizmo | **2,500–3,500** | 5–7 wk | medium |
| Voxel sculpt + op-list panel | 05 §10 | none given | **800–1,200** | 2–4 wk | low — §4.5 |
| Splat painting (`loom_paint`, shader, `ObjectData`, grass hook, golden) | 03 §1–6 | none given | **1,400–1,900** | 3–5 wk | medium |
| Vertex-colour painting | 03 §7 | "separable, drop it if the schedule bites" | **700–1,000** | 2–3 wk | **cut, §9** |
| UV texture painting | 04 Part I | none given | **1,800–2,500** | 4–6 wk | low — §4.2 |
| Decals | 04 Part II | none given | **500–700** | 1.5–2 wk | **high — best value in the set** |
| Crate split, `loom-play`, `loom ship`, cross-compile | 06 | `ship.rs` ~400, `play.rs` ~120, `project.rs` ~30 | **1,100–1,600**, plus an unbounded tail if ADR A is redesigned (§3.2) | 2–4 wk | low |
| Docs: palette, F1, Problems, History, generator, 8 prose files | 07 | "~30 lines" for the matcher | **1,800–2,600** + prose | 4–6 wk | medium |
| **Total** | | **never stated anywhere** | **19,000–25,000** | **9–14 months** | |

That is 30–40% growth of the whole engine, in the one area where the four green checks are least
able to tell you it is wrong: `cargo xtask image` sees nothing an editor does, `shimmer` and
`flythrough` see nothing, `cargo test` sees the ops layer and not the UI. **The only automated
signal over most of this work is `cargo xtask validate`'s windowed half** — verified: it drives
`loom run --edit --frames n` at `xtask/src/main.rs:1024` and `--edit --play --frames n` at
`:1077`. That is a much thinner gate than this project is used to, and it is worth saying out
loud before starting rather than discovering at slice six.

---

## 3. The three things most likely to derail the build

### 3.1 The scene's rectangle is one frame behind the panel's rectangle, and no document mentions it

**Verified.** `Ui::draw` (`crates/loom_render/src/ui.rs:117-151`) is where egui's entire layout
happens: `take_egui_input` → `context.run_ui(input, build)` → `tessellate` → `cmd_draw`. And
`Ui::draw` is called from *inside the `ui` render-graph pass closure*
(`crates/loom_render/src/viewer.rs:1590-1619`), which `RenderGraph::execute`
(`loom_render_graph/src/lib.rs:625`) runs **after** the forward and tonemap closures have already
recorded.

So the dock rectangle for frame *N* does not exist until after the forward pass for frame *N* has
been recorded. `ViewportPlacement` for frame *N* can only be the rect egui computed in frame
*N−1*.

Consequences the design does not cover:

- **A splitter drag shows a stale scene rect against a live panel edge, every frame of the drag.**
  Doc 01 adds `chrome_clear` (§1.2) so the mismatch renders as a black band rather than as
  garbage, which makes it benign-looking and permanent rather than alarming and fixed. Splitter
  dragging is a continuous gesture users perform constantly.
- **Window resize has the same artifact**, and this is the case the research doc already warned
  about from the other direction (`loom-pcg-and-editor.md:157`).
- **Frame 1 has no placement at all.** Combined with doc 01 §1.4's rule that the editor skips the
  scene passes entirely when the rect is under 8 px, `loom run --edit --frames 1` may legitimately
  render **no scene**. That is the exact invocation `cargo xtask validate` makes.
- Maximise-on-hover (doc 01 §5) and tab-drag both flash.

**The fix is not hard but it is not in any files-touched table.** Split `Ui::draw` into a layout
half (`take_egui_input` / `run_ui` / `handle_platform_output` / `tessellate`) called *before*
`draw_with_ui` builds the graph, and a record half (`set_textures` / `cmd_draw`) that stays in the
pass. That changes `ui.rs` — which both surveys mark **KEEP AS-IS** — and changes `run.rs`'s call
shape, because the panel-build closure then runs before the frame is drawn rather than during it.
It is maybe 80 lines. It must be in slice 1, because retrofitting it after the dock exists means
re-auditing every panel for "does this read state the frame has not produced yet".

**Why this is derailer #1 and not a footnote.** Doc 01 §0 stakes the whole shell on render-to-rect
being *cheaper and safer* than render-to-texture. The lag is the one respect in which it is
strictly worse — a texture is written before egui samples it and therefore cannot lag — and it is
the argument's blind spot. If the lag turns out to be visible enough to be unacceptable and the
`ui.rs` split does not close it, the fallback is the texture path, which is the foundation
everything else in the set sits on. Doc 01 §1.7 is right that the reversal is contained; it is
wrong that it is cheap once eight other documents have been built on top.

### 3.2 ADR A (doc 06) breaks the HUD and rewrites `Viewer::draw`, and doc 02 already said so

Doc 06 §1 proposes making `egui`, `egui-winit` and `egui-ash-renderer` optional in `loom_render`
behind a non-default `editor` feature, and `#[cfg(feature = "editor")]`-gating `Viewer::draw_with_ui`
and the `ui` render-graph pass.

**Two verified facts kill it as written.**

1. **`hud.rs` is egui, and the HUD is game content, not editor chrome.** `crates/loom_cli/src/hud.rs:16`
   is `use loom_render::egui;` and the module builds `egui::Align2`, `egui::Color32`,
   `egui::FontId` and paints into an `&mut egui::Ui` (`hud.rs:137`). The shipped runtime draws the
   `Hud` component. A shipped build with egui gated out has no HUD, which means `GameRules`
   win/lose text, scores and prompts do not render — the thing `assets/games/proving_ground.loom`
   exists to demonstrate.

2. **`Viewer::draw` is not a second path; it is a one-line wrapper.** `viewer.rs:922-924`:
   `pub fn draw(...) { self.draw_with_ui(objects, &[], camera, None, |_| {}) }`. Gating out
   `draw_with_ui` therefore does not remove a branch — it removes the **only implementation of
   drawing a frame** and obliges someone to write a second one. That is precisely the
   offscreen/window divergence ADR 0018 says this project has paid three defects for, introduced
   by the very ADR whose §1 promises not to introduce it.

**Doc 02 §6 point 3 states the correct answer and doc 06 does not cite it:** *"`hud.rs` draws the
game's HUD with egui, so the shipped runtime links egui regardless, and 'stripping the editor'
means not linking `loom_editor` — not making egui optional in `loom_render`. That materially
shrinks ADR F."* Doc 02 is right. Two documents in the same set reached opposite conclusions about
the central mechanism of the ship target, and neither notices.

The salvage is real and smaller: keep egui unconditional in `loom_render`, create `loom_editor`,
and let the boundary be *"nothing but `loom_cli` depends on `loom_editor`"* — the same shape as the
existing `loom_agent` rule in `scripts/check-deps.sh`, and mechanically checkable. Doc 06 §6.6's
check then becomes `cargo tree` must not mention `loom_editor` rather than must not mention `egui`,
which is a weaker guarantee for binary size and an identical one for the thing that matters. Say so
in the ADR: **the shipped binary links egui because the HUD is egui**, and if binary size ever
matters, the way to fix it is to stop drawing the HUD with egui, not to feature-gate the renderer.

### 3.3 The `Viewer` has no way to update a material or a texture, all four painting systems need one, and nobody owns it

**Verified.** `Viewer`'s entire public mutation surface is `set_grass`, `set_rain`, `set_rain_tick`,
`set_rain_field`, `set_terrain`, `set_meshes` (`grep -n "pub fn set_" crates/loom_render/src/viewer.rs`).
There is **no `set_materials` and no texture-update entry point**. Doc 03 §13.2 draws the
consequence and flags it as unverified: *"an inspector edit to `Material.roughness` in
`loom run --edit` does not reach the GPU today."* If that is right, it is a shipped bug in the
current editor that nothing in the design set is scheduled to fix, and it is the prerequisite for:

- splat painting (doc 03 §6),
- UV texture painting (doc 04 §3.2–3.3),
- decals (texture loading, doc 04 §11),
- the inspector's colour picker and texture slots (undesigned, §5),

i.e. everything the user asked for that touches a surface.

**And the two documents that need it specify it differently.**

- Doc 03 §6 proposes `Viewer::set_material_texture(slot, &Texture)` reusing `material::record`'s
  one-shot submit, at *"roughly ten times a second"* during a stroke, and calls it cheap.
  `crates/loom_render/src/material.rs:432-470` shows what `record` actually is: allocate a command
  buffer, submit, **`wait_for_fences(&[fence], true, u64::MAX)`**, destroy, free. That is a full
  GPU sync on the render thread. Ten of those a second during a brush drag is a stutter, and the
  function's own doc comment says it is *"initialisation work that must finish before the first
  frame, not per-frame work the graph schedules"* — doc 03 quotes that sentence and then does the
  thing it forbids.
- Doc 04 §3.2 designs it correctly: a persistent staging buffer, a `paint_upload` graph pass with
  `Access::TransferDst`, `import_with_layout(…, SHADER_READ_ONLY_OPTIMAL)` (verified to exist,
  `loom_render_graph/src/lib.rs:411`), and the paint image declared in `forward_uses`. Plus
  `PAINT_HEADROOM` on the descriptor array and a `set_materials` written as a sibling of
  `set_meshes` for the rebuild case.

Doc 04's is the right design and doc 03's is not, and they are in different documents building
different features that both land in `MaterialLibrary`. **One document must own this and both must
use it, before either painting system starts.** Sizing it honestly: staging buffer + graph pass +
`Access` plumbing + descriptor headroom + `set_materials` + the `device_wait_idle`-then-reset
discipline `set_meshes` documents at `viewer.rs:841-857` is **400–700 lines in `loom_render`**, and
it is the single most load-bearing unowned item in the set.

---

## 4. Unverified assumptions that carry real weight

Ordered by how much collapses if the assumption is wrong.

### 4.1 Windows cross-compile — the evidence is better than the docs claim, and the risk is misranked

I checked doc 06's environmental claims and **they all hold**:

```
which x86_64-w64-mingw32-gcc  → /usr/bin/x86_64-w64-mingw32-gcc
rpm -q mingw64-gcc            → mingw64-gcc-16.1.1-1.fc44.x86_64
which wine                    → /usr/bin/wine
~/.wine/drive_c/windows/system32/vulkan-1.dll   → present
Cargo.lock                    → windows_x86_64_gnu, windows_x86_64_gnullvm, windows_x86_64_msvc
```

And two things doc 06 did not check that strengthen its case further:

- **There is no platform-specific code in this workspace at all.**
  `rg "cfg\(unix\)|cfg\(target_os|std::os::unix|MetadataExt|PermissionsExt|fs2|fd-lock|libc::" crates/ xtask/`
  returns **nothing**. Even the file locking is portable: `lock_scene`
  (`crates/loom_scene/src/edit.rs:113-136`) uses `std::fs::OpenOptions` and `File::lock()`, which
  is std and works on Windows.
- **`cpal`'s ALSA path is correctly gated.** `cpal-0.18.1/Cargo.toml:188-191` puts `alsa` and
  `alsa-sys` behind `cfg(any(target_os = "linux", …))`; the Windows branch is `windows` /
  `windows-core` / `audio_thread_priority` at `:252-289`, all pure Rust. Doc 06 marked this "high,
  unproven"; it is now high and read.

**So I would demote Windows from "largest technical risk" (doc 06's framing) to third or fourth.**
The residual unknowns are `blake3`'s `*_windows_gnu.S` under mingw (doc 06 names it as the
predicted failure and its `pure` fallback is semantics-preserving, which is correct — BLAKE3's
output is spec-defined, so a `VersionToken` is the same bytes either way) and whether winevulkan
supports dynamic rendering + descriptor indexing + BDA + 4× MSAA. The second is the one that could
cost real time, because if Wine cannot run it, V4 and V5 evaporate and "Windows supported" has no
evidence behind it at all — at which point doc 06 §4.5's V6 sentence is the whole deliverable and
the honest thing is to say the target builds and has never been executed.

One thing doc 06 misses: `rustup target list --installed` shows **only** `x86_64-unknown-linux-gnu`.
Adding the target needs a `rustup target add` and a network fetch, and `rust-toolchain.toml` pins
`channel = "1.97.1"` — so the Windows std must exist for that exact pinned toolchain. Trivial, but
it is a step that is not in doc 06 §4.3's two-file change list.

### 4.2 The UV-paint GPU path — the mechanism is sound; the *cost model* is arithmetic

Doc 04 §3 is the strongest technical section in the set and I could not fault its mechanism:
CPU rasteriser (correct — a GPU brush plus a CPU load-time rasteriser is exactly the divergence
ADR 0006 exists to prevent, and `Expr` genuinely cannot express a stamp accumulator), dirty-rect
`vkCmdCopyBufferToImage` in a graph pass, `import_with_layout` so the image is not `UNDEFINED`,
`PARTIALLY_BOUND` headroom instead of a pipeline-layout rebuild. All four are correct against the
source I read.

What is *not* verified, in doc 04's own words and mine:

- **Every timing number is texel-counts times an assumed nanoseconds-per-texel.** "Tens of
  microseconds for a 250² rect" and the 512² clamp that follows from it are arithmetic. If a stamp
  at the clamp limit is over a millisecond, the brush is not interactive and the clamp is wrong.
- **The re-raster on commit is ~30 ms by the doc's own estimate** (§4.1), paid on every mouse-up
  *and every Ctrl+Z*. Doc 04 §12 admits a fast scribble may not survive it. The mitigation it
  names — hand the already-correct preview image into the rebuilt `MaterialLibrary` keyed by
  `paint_key` — is a real design change, not a tuning knob, and it should be in v1 rather than
  behind an "if".
- **`PARTIALLY_BOUND` with unwritten descriptors was not run under the validation layers.**
  `material.rs:141-145` does set the flag, so the reasoning is sound, but green check 2 is
  zero-messages and this is exactly the class of thing that produces one.
- **The `incremental_painting_equals_a_full_rasterisation` test is the correctness gate of the
  whole feature and it does not exist yet.** If it cannot be made to pass bit-exactly including
  every mip level, the surface twitches at every mouse-up and Part I needs a different preview
  model.

And the structural objection doc 04 states honestly and then does not weigh: **UV painting does not
work on voxel terrain** (§2.2, correctly reasoned — any auto-unwrap moves under every carve, and
destructible terrain is locked). Voxel terrain is this engine's headline authoring surface. So the
most expensive of the four painting systems is the one that does not apply to the thing people
will most want to paint, and the two cheap ones (splat, decals) both do.

### 4.3 "An inspector material edit does not reach the GPU today"

Doc 03 §13.2 flags this as inferred, not run. I traced it the same way and reached the same place:
no `set_materials`, everything in `material.rs` is `pub(crate)`, `Materials::new` uploads in the
constructor. **I could not falsify it and neither could doc 03.** It is a ten-minute check with a
built binary and it should be the first thing anyone does when the build ban lifts, because the
answer changes the size of §3.3 by a factor of two in either direction.

### 4.4 `SpliceArray` through `toml_edit`, nested inside an array-of-tables

Doc 05 §16.5 names this as *"the one place the op could turn out to be harder than it reads"*, and
it is right to. `SpliceArray` has to write `[[node.components.VoxelVolume.ops]]` — an
array-of-tables nested inside `[[node]]`, itself an array-of-tables — while preserving the existing
spelling. Doc 05 §16.1 additionally does not know whether `Scene::parse` accepts the inline-array
spelling as well as the array-of-tables one.

This matters more than its size suggests, because **`SpliceArray` is the prerequisite for voxel
sculpting, prefab-instance duplication, and the four array-of-object fields the inspector cannot
edit** (`WaterBody.waves`, `Buoyancy.pontoons`, `Scatter.excludes`, the ground layer). If it turns
out that `toml_edit` cannot do the surgery cleanly, the fallback is `SetField` on the whole array,
which doc 05 §10.2 correctly shows collapses the op list into one 4,000-character line and takes
`git diff` — a named verification channel (`LOOM-BUILD-BRIEF.md:164`) — dark for the one system
whose authored form exists to be diffable.

Verified in support of doc 05's argument: `f64::from` at `crates/loom_scene/src/ops.rs:680` is
real, and it is the only occurrence in the file, so the fix is genuinely three lines.

### 4.5 Incremental sculpt preview == full bake

Doc 05 §10.5 proposes the test and §16.4 admits it did not read `Volume::bake` and `Volume::edit`
closely enough to predict the answer. This is the same class of risk as 4.2's raster test: if it
fails, the live sculpt brush must fall back to a full re-bake on stroke release, whose cost is
linear in the op count and unbounded over a session. That turns sculpting from a brush into a
click-and-wait. **Take the measurement before designing the UI**, which doc 05 §16.3 also says.

### 4.6 The theme's contrast tables are computed on colours the display does not show

Not flagged anywhere, and cheap to verify:

- `crates/loom_render/src/ui.rs:88` sets `srgb_framebuffer: false`, which makes egui's own fragment
  shader apply the sRGB encode.
- `crates/loom_render/src/viewer.rs:2102` selects a `B8G8R8A8_SRGB` swapchain, so the **hardware
  encodes a second time**.

Doc 01 §12.1 spotted the double-encode, said *"by my reading that is a double encode; by the
evidence that nobody has complained about the panels looking washed out, my reading is probably
missing something"*, and moved on — without noticing that it invalidates **its own §6.1 contrast
table**, and doc 07 §10's as well. Every WCAG ratio in both documents is computed on the nominal
hex; a double sRGB encode lifts mid-tones substantially, so `bg_panel #16191E` does not display as
`#16191E` and `text_weak` at a claimed 4.82:1 is not 4.82:1 on screen.

Nobody complained because there is no design language yet. A deliberate dark palette will notice
immediately. **Settle `srgb_framebuffer` before tuning a palette**, or the palette gets tuned to
compensate for a bug and then breaks the day the bug is fixed. It is a one-line change with a
whole-UI blast radius, so it belongs in slice 1 beside the theme, not after it.

---

## 5. The scope hole: seven of twelve tabs have no design

Doc 01 §2.2 declares the tab vocabulary. Cross-referencing it against the design set:

| Tab | Designed in | Status |
| --- | --- | --- |
| `Scene` / `Game` | 01 §1 | ✅ |
| `Console` | 07 §5, §7 (as Problems) | ✅ |
| `Transactions` | 07 §8 (as History) | ✅ |
| `Hierarchy` | — | ❌ only a list of what today's lacks (`00-survey-existing.md` §9) |
| **`Inspector`** | — | ❌ **nothing** |
| **`Project`** (asset browser, import) | — | ❌ **nothing**, despite doc 05's `Declare` op existing chiefly to serve it |
| `Prefabs` | 05 §11 mentions a browser | ❌ not designed |
| `Environment` | — | ❌ nothing |
| `Terrain` (recipe editor + `loom terrain` metrics) | — | ❌ nothing; doc 05 §10 is voxel sculpt, which is a different thing from `loom_terrain`'s `Recipe`/`Layer` `.toml` |
| `Events` (deterministic event-log timeline) | — | ❌ nothing |
| `Profiler` | — | ❌ nothing |

**The Inspector is the largest single omission in the set.** `00-survey-existing.md` §5 enumerates
seven concrete gaps (no string editing, no enums, no asset picker, no colour picker, no override
affordance, no multi-edit, no collapse/search/copy) and `00-survey-engine-surface.md` finding 2
quantifies it: *"roughly a third of the authored surface of this engine is display-only in the
editor today"*, all of it behind one `match` arm at `panels.rs:877-895`. Every other document
routes work to it and none of them writes it:

- doc 05 §6: *"A `BoxCollider` half-extent gizmo and a light-range sphere … belong with the
  inspector design, not this one."*
- doc 05 §11: *"The display of override state belongs to the inspector design."*
- doc 04 §7: the `resolution` field's *"doc comment, which is also the tooltip"* — a tooltip the
  inspector must render.
- doc 07 §5: F1 popovers "over an inspector field", component headers from the schema root.
- doc 03: colour and texture slots for `SplatPaint`, `Material`, `Decal`.

That is at minimum a recursive schema walker (the current one is one level deep — `scene.rs:638`
says so explicitly), a widget table keyed on schema kind, enum handling through the `oneOf`+`const`
spelling `loom_reflect/src/lib.rs:233-258` already parses, an asset picker, a colour picker, an
override channel, multi-selection editing, and array-of-object editing on top of `SpliceArray`.
**I size the inspector alone at 1,200–2,000 lines and 3–5 weeks**, and it is the panel that
delivers the most user-visible value per line in the entire rework. It needs a document before any
of the four painting systems gets one.

**Asset import is the second hole.** Doc 05 §13 justifies the new `Declare` op partly on
*"mesh import, which is the asset panel's whole reason to exist"* — but no document designs drag-a-
file-in, the copy-into-project step, alias naming, or thumbnailing. Doc 02 §5 explicitly refuses to
resurrect `loom_asset::meta` (correctly, in my view), which makes import *simpler*, not designed.

---

## 6. Cross-document collisions

These are not stylistic. Each is two documents specifying incompatible things in the same field,
file, or number, and each will be discovered as a merge conflict or a silent bug.

| # | Collision | Where | Consequence |
| --- | --- | --- | --- |
| 1 | **Four documents claim ADR 0022.** Highest existing is `0021-a-reflected-hit-shades-with-the-materials-mean-albedo.md` (verified, `ls docs/decisions/`). Doc 01 §10 assigns 0022=viewport, 0023=CMAA2. Doc 02 §11 assigns 0022=project manifest, 0023=asset path. Docs 04 and 05 both say "next free is 0022". | 01, 02, 04, 05 | Two ADRs with one number is a broken record, and ADR 0002 makes precedence depend on the number. Allocate a block now. |
| 2 | **`ObjectData.material.y` claimed twice.** Doc 03 §6: *"Take `y` for the splat mask's bindless slot."* Doc 04 §1.3: *"Paint goes in `.y`."* Verified there is exactly one such field: `material: [u32; 4]` at `renderer.rs:676`, `.x` used, `.yzw` padding. | 03, 04 | Whichever ships second silently reads the other's texture index. `.z`/`.w` are also spoken for by doc 04 §10.3's decal range. Four u32s, five claimants. |
| 3 | **Doc 03's push-constant evidence cites the wrong struct.** §7 Fact 3 cites `rain.rs:717-718`'s `assert_eq!(size_of::<Push>(), 120)` and reconciles it against `renderer.rs:626`'s "124 of its 128 bytes" as though they described one thing. They are two different `Push` types — the rain compute block and the scene block. §10's files-touched table then says to *"re-pin `size_of::<Push>()` 120 → 128"* **in `rain.rs`**, for a change to the scene shader's push block. | 03 | The conclusion (one 8-byte slot remains in the scene `Push`) happens to survive the arithmetic — 64+6×8+4, padded, is 120, and a seventh pointer makes 128 exactly — but the cited evidence proves nothing about it. Verify `size_of::<renderer::Push>()` before spending the last slot. |
| 4 | **Doc 01 rejects render-to-texture; doc 05 depends on it by name.** Doc 05 §6.9: *"This is a hard dependency on the render-to-texture viewport (ADR I)."* | 01, 05 | Probably only stale naming, but doc 05's tool layer is written against a viewport whose failure modes it has not read. |
| 5 | **egui optional in `loom_render`: doc 02 says impossible, doc 06 does it.** §3.2 above. | 02, 06 | Ship target's central mechanism. |
| 6 | **Two theme token tables, different values.** Doc 01 §6.1 and doc 07 §10 both define the palette. `bg_raised` `#1E232A` vs `bg_2` `#1E222A`; `accent` `#A78BFA` vs `#7C5CFF`; `error` `#F0736D` vs `#F2555A`; `ok` `#6FCF97` vs `#52C07A`. Doc 01 keeps `agent` at the existing `#78C8FF` and calls it *"unchanged"*; doc 07 changes it to `#34D3C0` while its table lists the axis colours as "existing, unchanged". | 01, 07 | Two `theme.rs` specs. Both compute contrast ratios; both are computed on the wrong colours (§4.6). |
| 7 | **Icons: font vs hand-drawn.** Doc 01 §6.5 adopts `egui-phosphor = "=0.13.0"` and rejects hand-drawn shapes as *"a week and looks it"*. Doc 07 §10 rejects an icon font as *"a new binary asset class, a licence question, and a glyph lookup table"* and specifies ~14 hand-drawn painter icons in ~120 lines. | 01, 07 | Both feed ADR E with opposite recommendations. Neither is wrong; the set must pick one. |
| 8 | **Fonts.** Doc 01 §6.2: ship on egui's bundled fonts, add Inter only if the human still reads it as default egui. Doc 07 §10: ship Inter Regular + SemiBold. | 01, 07 | Doc 01's sequencing is better (a font is a new binary asset class and a licence entry); doc 07 treats it as settled. |
| 9 | **Where user state lives — three answers.** Doc 01 §4: `<project>/.loom/layout.json` + `~/.config/loom/editor.json`. Doc 02 §4: `$XDG_STATE_HOME/loom/hub.toml` + `$XDG_CACHE_HOME/loom/thumbs/`. Doc 07 §6: `$XDG_CONFIG_HOME/loom/editor.toml` carrying *recents, window geometry, dock layout, zoom, reduce-motion, high-contrast, onboarding flag*. | 01, 02, 07 | Recents and dock layout are each specified in two places, in two formats (JSON vs TOML). |
| 10 | **Doc 01 writes into the project; doc 02's ADR forbids it.** Doc 01 §4 creates `<project>/.loom/layout.json` and gitignores it. Doc 02's ADR text: *"A project directory acquires no engine-written files."* | 01, 02 | Direct contradiction inside a proposed ADR's decision statement. |
| 11 | **Three entry points.** `loom run --edit` (doc 01, doc 05, and `xtask/src/main.rs:1024`, verified). `loom edit` (doc 02 §6). `loom-editor` binary (doc 07 §6: *"`loom-editor` with no argument opens the Hub"*, and §11's shipped folder lists a `loom-editor[.exe]`). | 01, 02, 06, 07 | Doc 02's reasoning for one binary is the strongest; doc 07 contradicts it in two places. |
| 12 | **Two shipped-folder layouts.** Doc 06 §2: renamed exe + `loom.toml` + `assets/` + `.loom-build.json`. Doc 07 §11: `loom-editor[.exe]` + game exe + `assets/` + `docs/` + `projects/`. | 06, 07 | Doc 07's layout ships the editor inside the game folder, which is the opposite of decision 3. |
| 13 | **Three manifest names and schemas.** `loom.toml` with `[project] main_scene` (doc 02). `game.name` / `game.startup_scene` / `build.targets` (doc 06 §3.2). `project.toml` (doc 07 §6). | 02, 06, 07 | Doc 06 §3.2 at least says "not mine to define" and names its three keys — good discipline. Doc 07 just uses a different filename. |
| 14 | **Typed vs untyped stroke arrays, decided oppositely.** Doc 03 §1: `SplatStroke` is typed with `JsonSchema` and argues the `VoxelVolume.ops` untyped precedent is *"strictly better than the precedent and worth not copying blindly"*. Doc 04 §1.1: `PaintLayer.strokes` is `Vec<serde_json::Value>` *"untyped JSON on purpose and by precedent"*. | 03, 04 | Two sibling painting systems, opposite schema decisions, same week. Doc 03's argument is the better one; doc 04's costs it a hand-written `parse_strokes` validation funnel. |

None of these is fatal on its own. Collectively they say the seven documents were written in
parallel without a reconciliation pass, and **a reconciliation pass is cheaper now than after any
of it is code.**

---

## 7. Pieces materially larger than their document admits

Beyond the estimates in §2, these are the specific under-statements worth naming.

**"A two-line change made in one place"** (doc 01 §1.5, the coordinate remap). It is two functions
*plus* every consumer: `pick_at_cursor` (`run.rs:2002-2030`), `drag_gizmo` (`:1935-1994`),
`press_in_viewport` (`:1914-1932`), handle recomputation (`:1015-1025`), `agent_marks` (`:422-466`),
`gizmo_overlay` (`panels.rs:701-739`), `agent_overlay` (`:663-695`), the HUD's
`available_rect_before_wrap` anchoring (`hud.rs:136-161`, which must now anchor to the Game tab and
not the window), the fly camera's aspect, and `FlyCamera::at`'s framing. Plus moving the overlays
off `LayerId::background()`, which doc 01 correctly identifies and does not size.

**The gizmo's nine improvements** (doc 05 §6, *"a few hundred lines against 280 existing"*). Plane
handles, screen-space translate, local/world basis, arcball rotation replacing the 45°-per-unit
gearing, a uniform-scale centre handle, live numeric readout, a multi-selection gizmo issuing one
`SetTransform` per node under one gesture key, median-vs-individual pivot, and rect-relative
coordinates. The multi-selection case alone needs each node's transform taken about a shared pivot
and back through `SceneView::parent_inverse` — the function that exists *because* a gizmo under a
rotated parent moves the node the wrong way. **600–900 lines with the tests this module already
sets a standard for.**

**Templates as "a recursive copy plus three edits"** (doc 02 §8). Step 4 is *"rewrite `[scene] id`
and every `[[prefab]] id` in every copied `.loom` file"* via `toml_edit`, which doc 02 §12.8 admits
it has not confirmed is ergonomic. Plus three real template projects that must load, resolve, bake
and validate clean, plus `empty` entering `GOLDEN` (a new reference PNG and a `MANIFEST.txt` line),
plus `Camera.boom` — an engine change doc 02 §9 flags honestly as *"the one place a template drove
an engine change"*, with §12.4 admitting the sign convention is inferred from a doc comment rather
than read from the arithmetic.

**`cargo xtask docs --check` in `scripts/green.sh`** (doc 07 §3, §12). The ADR consequence says
*"`xtask` gains a dependency on `loom_editor`"* — so running the second green check now compiles
the whole editor, on a project whose stated resequencing trigger is *"compile times exceeding
roughly one minute warm are a stop-and-fix condition"* (`LOOM-IMPLEMENTATION-ORDER.md:574`). Doc 07
§14 spots this and does not resolve it. **Keep the command table; keep the generator; do not put
`--check` in `green.sh`** until the compile cost is measured.

**`loom ship` at "~400 lines"** (doc 06 §7). Eight assertions, a cargo JSON message-stream parser,
a recursive tree copy with symlink resolution and two exclusion rules, an objdump import-table
reader with DLL copying, a Wine smoke run with honest skipping, the `.loom-build.json` overwrite
marker, and a JSON report. **700–1,000**, and that is before the Build modal.

---

## 8. Smaller findings, each verified

- **`GOLDEN` grows by at least four** (`painted`, `paint_wall`, `decals`, `empty`), from 28
  (`xtask/src/main.rs:253`, verified) — and `SCENES` from 43 (`:41`) by more. Each is a reference
  PNG, a `MANIFEST.txt` line, and a permanent addition to check 4's runtime. Fine, and correct per
  the `GOLDEN` rule, but nobody totals it.
- **Doc 01 §1.2's `DONT_CARE` with a sub-rect `render_area`.** Correct by spec — contents outside
  `render_area` are preserved — but it is exactly the kind of thing where an IHV's fast path
  differs. Worth an explicit check on frame one rather than an assumption.
- **`egui_dock 0.20.1` and `egui-phosphor 0.13.0` are not in the local registry** (`ls
  ~/.cargo/registry/src/*/egui_dock*` → nothing), so doc 01's version and dependency claims come
  from crates.io metadata and are unverified against actual source. Doc 01 §12.4–12.5 says so.
  One `cargo add --dry-run` settles it.
- **`loom_cli` has no direct `egui` dependency today** (`crates/loom_cli/Cargo.toml`, verified) —
  it reaches egui through `loom_render`'s re-export. Doc 01 §2.1 proposes adding `egui = "=0.35.0"`
  directly. Fine, and the identical `=` pin does unify, but it is a new pin to keep in step with
  `egui-ash-renderer`'s compatibility table by hand.
- **The `SetTransform` f32 fix produces one-time churn.** Every scene the editor rewrites after the
  fix will show numeric diffs where widened values were previously stored. Numerically identical,
  visually a diff. Land it in its own commit with that stated, before the snap UI (doc 05 §7 is
  right about the ordering and does not mention the churn).
- **Doc 05's `SpliceArray` on a prefab instance is undefined.** ADR 0008 routes `SetField` on an
  instance into `[node.overrides]`; doc 05 §11 relies on that routing being invisible to tools.
  What a *splice* means against an override — replace the whole array, or splice into the resolved
  one? — is not answered anywhere, and sculpting a prefab-instanced terrain chunk is a plausible
  first user action.
- **Doc 03 §7's `paint:<node>` private mesh copies interact with `mesh_key`.** The doc names the
  trap (*"`key()` must fold in the paint hash or the viewport will not follow a stroke"*) and §13.7
  admits it did not walk every caller of `MeshLibrary::key`. This is the "ships and silently does
  nothing" failure class, and it argues on its own for cutting vertex painting.
- **Doc 07 §9 cuts screen-reader support explicitly and states the reason.** That is the right
  call and the right way to make it; noted so it is not re-litigated.

---

## 9. What to cut, and what to build in what order

The goal is *usable soonest without painting into a corner*. The corner-painting risks are exactly
two: the viewport mechanism (§3.1) and the op vocabulary (§4.4). Everything else is additive.

### Stage 0 — a week, all parallel, none of it blocked on anything

1. Windows **V0/V1** (`cargo tree --target`, `cargo check --target`). Cheapest thing in the set
   that could invalidate a deliverable.
2. `prefab_load::for_reading` in `scene_view.rs:110`. One line, live bug.
3. `SetTransform` f32 shortest-round-trip (`ops.rs:680`), with the test doc 05 §15 specifies.
4. Settle `srgb_framebuffer` (§4.6) — *before* anyone picks a hex.
5. Verify whether a `Material` edit reaches the GPU (§4.3).
6. **Reconcile §6's fourteen collisions.** Allocate the ADR block. Pick one theme, one icon
   strategy, one entry point, one manifest, one state location, one `ObjectData` field map.

### Stage 1 — the shell, sequential, the trunk

7. `Ui::draw` split into layout-then-record (§3.1). **Before** `ViewportPlacement`, not after.
8. `ViewportPlacement` with a hardcoded inset, over the *existing* panels. Doc 01 §11 step 1 is
   exactly right and its exit criterion — `cargo xtask image` produces zero changed references —
   is the right gate.
9. `chrome_clear` + the barrier-list test names it.
10. Coordinate remap: picking, gizmos, overlays, HUD anchoring.
11. Theme (one table), over the old panels.
12. `egui_dock` + the tab enum + default layout, existing panel bodies moved in unchanged.
13. Layout persistence, Window menu, `--frames` ignores the saved layout.

**Cut from stage 1:** multiple viewports and camera picture-in-picture (doc 01 §5) — two forward
passes, an unverified read-then-write hazard on the scene image the doc itself flags in §12.3, and
no stated need. CMAA2 reordering (doc 01 §1.8) — opt-in, off by default, defer with its ADR.

### Stage 2 — the panels that do not exist yet, in value order

14. **Write the Inspector design document.** Then build it: recursive schema walk, string edit,
    enum dropdown, colour picker, asset picker, override display and per-field revert
    (`RevertOverrides` already exists and has never been issued), multi-edit. **This is the single
    highest-value item in the whole rework** and it currently has no owner.
15. Hierarchy: collapse, filter, scroll-to-selection, icons, rule-node collapsing.
16. Project browser + mesh import (needs `Declare`).
17. Console/Problems, History (docs 07 §7–8 are ready to build).

### Stage 3 — authoring, parallel with stage 2 after the trunk lands

18. Op vocabulary: `SpliceArray`, `Declare`, `SpawnNode { prefab }`. **Prove the `toml_edit`
    array-of-tables surgery first** (§4.4); if it does not work, stop and redesign before the
    sculpt UI exists.
19. Create menu + `quad` + snap + `place::resolve` wrapper (`arrange.rs` — genuinely ~80 lines and
    the best reuse in the set).
20. Gizmo improvements. Keep `gizmo.rs`; doc 05 §6's reason for rejecting `transform-gizmo` — one
    shared projection with an inverse test — is correct and overrides Phase 7's E3.
21. Prefabs in the editor (instancing, revert, unpack, create).
22. Hub + `loom new` + templates. Independent; slot it wherever there is a gap.
23. Voxel sculpt — **after** the preview-equals-bake measurement (§4.5).

### Stage 4 — surfaces, in cost order, which is the reverse of the documents' order

24. The `Viewer` material/texture update path (§3.3). **One design, doc 04 §3.2's.**
25. **Decals.** Cheapest of the four, works on voxel terrain, no new `SceneOp`, no new pass, one
    loop in `fragmentMain`, and it covers the bullet-hole/scorch/graffiti case that UV painting
    cannot reach on terrain. `VSOutput` is used only by `vertexMain`/`fragmentMain` (verified,
    `grep -n VSOutput assets/shaders/scene.slang` → 4 hits), so the varying additions are contained.
26. **Splat painting.** Second cheapest, and doc 03 §4's authority-channel design is the best idea
    in the set — it is the difference between biasing a live procedural rule and freezing it into a
    bitmap. Include the `Ground.rock` grass hook (§4 of that doc); it is small and it stops the two
    boundaries disagreeing.
27. **UV texture painting.** Last, and only after 24–26 have proved the material path. Take doc 04
    §12's `paint_key` preview-handoff into v1 rather than leaving it as a contingency.

### Cut outright

- **Vertex-colour painting** (doc 03 §7). Doc 03 §11 already nominates it as the first thing to
  drop and §4 admits splat covers most of what it does. It additionally spends the last
  push-constant slot on evidence that cites the wrong struct (§6 item 3), forces per-node vertex
  duplication, and creates a silent-no-op failure mode through `mesh_key`. Four costs, one
  marginal capability.
- **Multiple viewports, camera PiP** (doc 01 §5).
- **The task strip / onboarding flow** (doc 07 §6). Nice, unnecessary, and the base scene's own
  comments already do most of the teaching for free.
- **`cargo xtask docs --check` in `green.sh`** (doc 07 §12) — keep the generator, drop the gate.
- **`Environment`, `Terrain`, `Events`, `Profiler` tabs** from the default layout until designed.
  A tab enum variant with an empty body is worse than no tab.

That leaves roughly **8,000–11,000 lines and 4–6 months** to an editor that is Unity-shaped,
themed, has a working inspector, creates primitives, snaps, sculpts, instances prefabs, ships a
Linux build, and paints two of the four requested surfaces — with UV paint, vertex paint and the
remaining panels as clearly-separable follow-ons that the architecture already admits.

---

## 10. What I checked, and what I did not

**Checked, with the command:** ADR count and highest number (`ls docs/decisions/`); `hud.rs`'s egui
use and `Viewer::draw`'s body (`rg`, `sed`); `Ui::draw`'s position inside the `ui` graph pass
(`sed` on `ui.rs`, `viewer.rs`, `loom_render_graph/src/lib.rs`); `Viewer`'s public `set_*` surface
(`grep`); `material::record`'s fence wait (`sed`); `Access` variants and `import_with_layout`
(`sed`, `grep`); `SceneOp`'s nine variants and `PlaceOp`'s four (`grep`); `f64::from` in `ops.rs`
(`grep`, one occurrence); `renderer::Push` and `ObjectData` field layout (`sed`); `VSOutput`'s
consumers (`grep`, 4 hits); `SCENES`/`GOLDEN` counts (`grep`); mingw, wine, winevulkan, rustup
targets, `Cargo.lock`'s windows crates (`which`, `rpm -q`, `ls`, `grep`); the absence of any
platform-specific code in `crates/` and `xtask/` (`rg`, zero hits); `lock_scene`'s use of std
`File::lock` (`sed`); `cpal 0.18.1`'s cfg boundaries (vendored `Cargo.toml`); `loom_cli`'s and
`loom_render`'s dependency lists; total workspace size (`wc -l`, 62,008).

**Not checked, because it needs a build:** everything in §4 marked unverified — whether a
`Material` edit reaches the GPU, whether `PARTIALLY_BOUND` with unwritten slots is validation-clean,
whether `toml_edit` can splice nested arrays-of-tables, whether incremental `Volume::edit` equals
`bake`, whether incremental raster equals full raster, whether anything cross-compiles, whether the
double sRGB encode is what I think it is, and every timing number in every document including mine.

**Not checked, because it needs a person:** whether the arcball feels attached, whether a
forty-op sculpt list is comprehensible, and whether any of this reads as sleek. Doc 05 §15 is right
to say so and no gate substitutes.
