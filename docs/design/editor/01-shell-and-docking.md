# Design — the editor shell: docking, layout, chrome, theme

*Phase: design only. Nothing here has been compiled. Every version number, API signature and
struct field named below was read out of the vendored crate source or crates.io today and is
cited; every claim that was **not** verifiable without building is in §12 and nowhere else.*

Reads on: `00-survey-existing.md` (what is there), `00-survey-engine-surface.md` (what must be
reachable), `00-survey-constraints.md` (what is forbidden). It does not repeat them.

---

## 0. The one decision that matters

Everything else in this document is layout and paint. **The structural change is that the scene
stops being the window and becomes a rectangle, and the cheapest correct way to do that is to move
the rectangle, not the pixels.**

The obvious implementation — render the scene into an offscreen image, wrap it in a descriptor
set, hand it to egui as a `TextureId`, draw it as an `egui::Image` inside the dock tab — is what
every Unity-like editor does and it is what the task brief assumes. It is also, in *this* codebase,
a new image, a new sampler, a new descriptor set, a resize policy, a second place where the window
and the offscreen path can disagree about colour, and a gamma round-trip through egui's fragment
shader that I cannot predict without measuring it.

**The alternative is one push constant.** The scene already renders into window-sized images and
the tonemap already writes the swapchain with a full-screen triangle. Set the forward pass's
viewport to the dock rect's *size*, set the tonemap's destination viewport to the dock rect's
*position*, and tell the tonemap shader where its source begins. The scene lands in the rectangle.
egui draws its chrome around it and its overlays on top of it, exactly as it does today. No new
image, no sampling, no colour conversion, and — the part that matters most here — **`loom run` and
`loom render` keep writing byte-identical pixels through byte-identical passes**, which is the
invariant ADR 0018 says this project has already paid three defects to learn.

§1 specifies it. §1.7 names the four things it cannot do and the exact trigger that would force
the upgrade to a real texture, so the decision is reversible rather than load-bearing.

---

## 1. The viewport is a rect, not a texture

### 1.1 What the frame does today

`Viewer::draw_with_ui` (`crates/loom_render/src/viewer.rs:936`) builds a render graph whose passes
are, in order:

```
forward → [water resolve] → rain → tonemap → ui → [cmaa2_edges, cmaa2] → present
 1245                       1502   1573      1590        1624              1654
```

With CMAA2 off — which is the default, gated on `LOOM_CMAA2` at `viewer.rs:75` — `post_id` **is**
the swapchain image (`viewer.rs:1184-1188`), so the tonemap writes the swapchain directly and the
`ui` pass draws over it with `AttachmentLoadOp::LOAD` (`renderer.rs:3848`). The tonemap's fragment
shader is a texel fetch at the window coordinate:

```slang
float3 hdr = scene.Load(int3(int2(pos.xy), 0)).rgb * push.exposure;   // assets/shaders/tonemap.slang:67
```

and its push block is one float (`TonemapPush { float exposure; }`, `tonemap.slang:19-22`).

The panels are drawn over all of it, which is why `panels.rs:706-710` can say "window pixels map to
egui points by the one scale factor, with no offset" — and why a camera framed on a scene frames it
behind the inspector too.

### 1.2 The change, in full

Add one type to `loom_render`:

```rust
/// Where the scene lands in the window. `None` is the whole window, which is
/// what `loom run` without `--edit` and every headless path pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportPlacement {
    /// Physical pixels, top-left of the dock rect in the swapchain image.
    pub origin: [i32; 2],
    /// Physical pixels. Clamped to at least 1x1 by the constructor.
    pub extent: vk::Extent2D,
}
```

`Viewer::draw_with_ui` takes `placement: Option<ViewportPlacement>` and threads it into four
places:

**The forward pass renders at the origin of the HDR image, sized to the rect.** Every
`cmd_set_viewport` / `cmd_set_scissor` / `RenderingInfo::render_area` in the forward, water and
rain passes uses `(0, 0, placement.extent)` instead of `self.extent`. The HDR scene image, the
depth image and the MSAA pair stay **window-sized and are never reallocated** — they are simply
larger than what is drawn into. The camera's aspect ratio comes from `placement.extent`.

**The tonemap writes the destination sub-rect.** `Tonemap::record` grows two parameters:

```rust
pub(crate) unsafe fn record(
    &self, device: &ash::Device, cmd: vk::CommandBuffer, destination: vk::ImageView,
    exposure: f32,
    origin: [i32; 2],          // NEW — where in the destination
    extent: vk::Extent2D,      // was (width, height)
)
```

`render_area`, viewport and scissor all become `origin + extent`. `load_op` stays `DONT_CARE`: the
full-screen triangle covers the whole viewport, and a `DONT_CARE` scoped to a sub-rect leaves the
pixels outside it alone (that is what `render_area` means).

**The tonemap shader learns where its source starts.** The push block gains a member and the fetch
gains a subtraction:

```slang
struct TonemapPush {
    float exposure;
    int2  origin;      // the destination sub-rect's top-left, in pixels
};
...
float3 hdr = scene.Load(int3(int2(pos.xy) - push.origin, 0)).rgb * push.exposure;
```

`SV_Position` is a *window* coordinate, so subtracting the destination origin recovers the
scene-image coordinate, which is what makes the scene image renderable at its own origin. Entry
points `tonemapVertexMain` / `tonemapFragmentMain` are unchanged in name and signature; only the
push block moves, and the push-constant range in `create_pipeline` (`tonemap.rs:242`) grows from
4 to 12 bytes.

**One new graph pass clears the chrome.** Outside the tonemap's sub-rect the swapchain image holds
whatever that image held two or three frames ago. egui's panels are opaque and cover almost all of
it, but drop shadows under floating windows are alpha-blended and would composite over stale
frames. So:

```rust
graph.pass("chrome_clear", &[(target, Access::ColorWrite)], move |d, cmd| {
    // A rendering block with LOAD_OP_CLEAR and no draws. The transition IS the
    // work, same as the `present` pass.
});
```

It runs **first**, before `forward`. Measured cost is not yet known (§12), but a full-screen clear
at 2560x1440 on a 4090 is bandwidth-bound at roughly 15 GB/s of writes, i.e. tens of microseconds;
it is in the same class as the `present` pass. It is skipped entirely when `placement` is `None`.

### 1.3 The property that makes this safe

**When `placement` is `None`, every one of those four changes evaluates to exactly what the code
does today** — origin `(0,0)`, extent `self.extent`, `push.origin` zero, no clear pass. So
`loom render`, `loom run` without `--edit`, `cargo xtask image`, `cargo xtask flythrough` and
`cargo xtask shimmer` are untouched, and the golden references cannot move.

That is not a convenience. It is the answer to ADR 0018's consequences paragraph — *"the forward
pass wrote a different destination depending on an environment variable — in the window, which is
where the human judges everything, and that class of offscreen/window divergence has cost this
project three defects."* A viewport that is a rect differs from a full window by an integer offset
and nothing else. A viewport that is a texture differs by an image, a format, a sampler and a
transfer function.

### 1.4 The zero-extent bug, named before it is written

**A dock splitter dragged fully closed produces a zero-width tab body, and a zero-extent
`render_area` is a Vulkan validation error on every affected pass.** `ViewportPlacement::new`
clamps to a minimum of 1x1 and to the swapchain bounds, and the editor additionally skips the
scene passes entirely — `placement` becomes a `None`-like "no scene this frame" — when the rect is
under 8 px in either axis. This is cheap to get right now and is an unbounded source of
validation noise if it is discovered later, because it fires only on a gesture no test performs.

The same clamp handles HiDPI rounding: the rect arrives from egui in *points*, is multiplied by
`Context::pixels_per_point()`, and `min` and `max` are rounded independently before subtraction, so
the extent is never negative and never a half pixel.

### 1.5 Coordinates: picking, gizmos, overlays

This is the part §9 of the existing-editor survey warns about, and it is a two-line change made in
one place.

`gizmo::View::new(&camera, w, h)` (`gizmo.rs:8-10`) is the single projection shared by picking, the
gizmo handles and the agent change-marks — with a test asserting `project` and `ray` are inverses
(`gizmo.rs:211-224`). It is constructed today from the **swapchain** extent (`run.rs:1007-1009`).
It is now constructed from `placement.extent`, and every screen-space consumer offsets by the
rect's origin:

```rust
// One helper, one place. Physical pixels in, viewport-local pixels out.
fn to_viewport(&self, window_px: Vec2) -> Option<Vec2>   // None when outside the rect
fn to_window(&self, viewport_px: Vec2) -> Vec2           // for the egui overlay painter
```

Everything that today reads a cursor position and feeds `View` goes through `to_viewport`;
everything that paints into egui's background layer (`panels.rs:701-739`, `agent_overlay` at
`:663-695`) goes through `to_window` and divides by `pixels_per_point` as it already does. **The
overlays move from `LayerId::background()` to a foreground layer scoped to the viewport tab**,
because the scene is now drawn *before* egui rather than under it, and a background-layer stroke
would be painted before the dock's own frames — invisible in the general case and wrong in the
overlapping one.

### 1.6 Input routing

Today the viewport sees a mouse press when `Ui::wants_pointer()` is false (`run.rs:889`). With a
docked viewport that test is both wrong and unnecessary: the viewport is a widget, so it can be
asked directly.

```rust
// inside the Viewport tab's `TabViewer::ui`
let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
```

`response.hovered()`, `response.dragged_by(PointerButton::Secondary)` (fly-look),
`response.interact_pointer_pos()` (picking and gizmo grabs) replace the raw-event path. `rect` is
the source of `ViewportPlacement`. **Three rules from §14 of the survey survive verbatim and must
be re-stated in code comments where they move:**

- keys are read **once per redraw**, not per event (`run.rs:940-950`) — unchanged, keys still come
  from `loom_input::ActionMap`, not from egui;
- **Tab is un-consumed unless a text field has focus** (`Ui::wants_text_input`, `ui.rs:164-173`) —
  unchanged, and now more important, because a dock has far more focusable widgets than six panels
  did;
- **editing keys are inert during Play** (`run.rs:2042-2044`) — unchanged.

Pointer capture for first-person play (`capture_pointer`, `run.rs:1361-1378`) now also needs to
*confine to the viewport rect* rather than the window when Play runs in a docked Game tab. winit's
`CursorGrabMode::Confined` confines to the window and not to a sub-rect, so the honest behaviour is
`Locked` (which is what play already tries first) and the fallback is unchanged. Noted rather than
solved; it is a play-mode concern, not a shell one.

### 1.7 What this cannot do, and the trigger to change the decision

Four things, stated so the choice is reversible:

**Render scale.** Drawing the scene at 50% and upscaling into the tab needs a filtered read, which
needs a texture. Not requested; would be requested the day a 4K display makes the editor viewport
expensive.

**A viewport inside a scroll area, or clipped by a parent.** egui's clip rectangles are honoured by
egui's painter and not by the render graph, so the scene would spill. **The rule is: the viewport
tab never scrolls** — `TabViewer::scroll_bars` returns `[false, false]` for it (§2.3), which is a
one-line enforcement of the constraint rather than a hope.

**A viewport in a *floating* dock window that sits below another panel in z-order.** The scene is
always drawn before egui, so anything egui draws inside the rect lands on top. A floating panel
*above* the viewport is therefore correct; a floating viewport *above* a docked panel is not —
where they overlap, the panel would show through. `TabViewer::allowed_in_windows` returns `false`
for the viewport tabs, which forbids exactly that case and nothing else.

**Any egui effect applied to the image** — rounded corner clipping, a fade while a tab is being
dragged, a thumbnail of the viewport in a tab preview.

**The trigger:** the first of those four that someone actually wants. The upgrade path is
contained — the tonemap writes an owned `R8G8B8A8_SRGB` image instead of the swapchain sub-rect,
`egui_ash_renderer::renderer::vulkan::{create_vulkan_descriptor_set_layout,
create_vulkan_descriptor_pool, create_vulkan_descriptor_set}` build the set (all three are `pub`,
verified in the vendored source at `renderer/vulkan.rs:24, 249, 267`),
`Renderer::add_user_texture(set) -> TextureId` registers it (`renderer.rs:408`), and the tab draws
`egui::Image::new(egui::load::SizedTexture::new(id, size))`. The descriptor set bakes
`SHADER_READ_ONLY_OPTIMAL` into its `DescriptorImageInfo` (`renderer/vulkan.rs:288`), so the graph
must leave the image in that layout — a `pass_with` edge, not a hand-placed barrier (never-do #4).
Everything needed is already in the crate; the work is the resize policy and the colour question in
§12.

### 1.8 CMAA2 moves, and that is a deliberate amendment to ADR 0018

ADR 0018 fixed the chain as forward → tonemap → **UI** → CMAA2 → present, with the reasoning that
CMAA2 is conservative enough to leave egui's own anti-aliased text almost alone
(`viewer.rs:1172-1182`).

**With a docked viewport that reasoning inverts.** CMAA2 is a display-referred edge filter over the
whole frame; a frame that is 40% panel chrome is not a frame it should see. So when `placement` is
`Some`, **CMAA2 runs before the `ui` pass and over the viewport sub-rect only**, using the same
origin push constant as the tonemap. When `placement` is `None` the order is exactly as it is
today.

CMAA2 is opt-in and off by default (`LOOM_CMAA2`, `viewer.rs:75-81`), so this can land a slice
after the rest and the editor is correct without it. It is called out here because it is a change
to a decision an ADR recorded, and §10 drafts the amendment.

### 1.9 Render-graph consequences

The barrier-list test in `loom_render_graph/src/lib.rs` — the one CLAUDE.md's current-phase block
says "names all four transitions", and ADR 0018's consequences say names all eleven — **gains the
`chrome_clear` pass's transition and must name it**. That test is how never-do #4's ownership stays
visible rather than assumed, and a new pass that is not in it is a pass whose barriers nobody
checked.

`plan_full` already orders read-after-write and write-after-read across passes; the clear writes
`target`, the tonemap writes `target`, and the UI writes `target`, which is a chain of writes to
one image and the simplest case it handles.

### 1.10 What it costs

| | render-to-rect (chosen) | render-to-texture (rejected) |
| --- | --- | --- |
| New GPU memory | **0** | one `R8G8B8A8_SRGB` image, 8.3 MB at 1920x1080 |
| New passes | 1 clear (full-screen, no draws) | 1 clear + 1 textured quad inside the UI pass |
| Extra sampling | **none** | one full-screen texture read per frame |
| Colour conversions | **none** | one, through egui's `LINEARtoSRGB` (§12) |
| Image resize policy | **none** — images stay window-sized | required, and it is the classic validation-noise source |
| Divergence from `loom render` | **none — same passes, same formats, integer offset** | one image and one transfer function |
| Forecloses | render scale, scrolled viewport, floating viewport under a panel, egui effects on the image | nothing |

The forward pass also gets *cheaper*, because it now rasterises the dock rect rather than the whole
window — at a typical Unity-like layout that is roughly 55–65% of the pixels.

---

## 2. Docking

### 2.1 `egui_dock` 0.20.1, pinned, and the reason is the version table rather than the line count

**Take `egui_dock = "=0.20.1"`.** Verified today on crates.io: 0.20.1 was published 2026-06-28,
its normal dependencies are `egui ^0.35`, `duplicate ^2.0`, `paste ^1.0`, `thiserror ^2.0.18`, plus
an optional `serde ^1`; it is MIT and not yanked. **0.21.x moved to egui ^0.36 and is therefore not
available to this project**, because `egui-ash-renderer 0.12.0` — the crate that actually decides
which egui the renderer can talk to — is pinned to egui 0.35 by its own compatibility table
(vendored at `egui-ash-renderer-0.12.0/src/lib.rs:29-31`).

That last fact is the whole justification. The standing objection to a docking crate is that it
couples your egui upgrade cadence to a third party's. **It does not, here, because the cadence is
already owned by `egui-ash-renderer`** — 0.13.0 landed 2026-08-12, four days ago, and moving to it
is a separate decision this shell does not need. `egui_dock` adds no upgrade friction that is not
already in the tree.

New transitive crates: `duplicate` and `paste` only. `thiserror 2.0.19` is already in `Cargo.lock`
(line 2751), `serde 1.0.229` is already pinned across seven crates. That is **two new proc-macro
crates** against a 365-package lock file.

**Rejected: a hand-rolled dock tree.** A `Split { axis, frac, a, b } | Tabs { Vec<Tab>, active }`
enum with rect-recursive layout and splitter dragging is perhaps 250 lines and I would write it
without hesitating. Tab drag-and-drop is not: drop zones on four edges plus the centre of every
leaf, a drag preview, undock-to-window, and the reparent-and-collapse-empty-node tree surgery
afterwards. That is the other 500+ lines, it is fiddly rather than hard, and it is the part users
notice when it is wrong. egui_dock is 81.6 KB of source doing exactly it, and never-do #12's
one-implementation-trait objection does not apply to a dependency.

`egui_dock` lands in `crates/loom_cli/Cargo.toml`, with `egui = "=0.35.0"` added there directly
alongside it. The existing pattern routes egui through `loom_render`'s re-export
(`loom_render/src/lib.rs:64`) so the CLI needs no `ash`; that pattern exists for the *ash* rule and
egui is not ash. With three egui-family crates in play a direct pin is clearer, and identical `=`
pins guarantee cargo unifies them into one `egui`.

### 2.2 The tab vocabulary

```rust
// crates/loom_cli/src/editor/dock.rs
#[derive(Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    Scene,            // the 3D viewport — §1
    Game,             // play-mode view, same mechanism, different camera
    Hierarchy,
    Inspector,
    Project,          // the asset/file browser
    Console,          // engine log + physical-sanity warnings
    Transactions,     // the session's labelled history — the agent's activity feed
    Prefabs,
    Environment,      // sun/sky/fog/cloud/wind, the scene-wide authoring surface
    Terrain,          // recipe editor + `loom terrain` metrics readout
    Events,           // the deterministic event-log timeline
    Profiler,
}
```

`DockState<Tab>` is the layout. `Tab` is `Copy`-cheap, hashable and serde-able, which is what makes
§4's persistence one line.

`Shell` implements `egui_dock::TabViewer` with `type Tab = Tab`. Every method signature below was
read off `docs.rs/egui_dock/0.20.1`:

| Hook | What we return, and why |
| --- | --- |
| `title(&mut self, tab) -> WidgetText` | icon glyph + name, styled per §6.5 |
| `ui(&mut self, ui, tab)` | dispatch to the panel module; for `Scene`/`Game`, allocate the rect and record it (§1.6) |
| `clear_background(&self, tab) -> bool` | **`false` for `Scene` and `Game`** — this is the hook that makes §1 possible at all; `true` for everything else |
| `scroll_bars(&self, tab) -> [bool; 2]` | `[false, false]` for `Scene`/`Game` (§1.7), default elsewhere |
| `allowed_in_windows(&self, tab) -> bool` | **`false` for `Scene` and `Game`** (§1.7), `true` elsewhere |
| `is_closeable(&self, tab) -> bool` | `false` for `Scene`, `Hierarchy`, `Inspector` — the three you cannot get back without the Window menu |
| `on_rect_changed(&mut self, tab)` | mark the layout dirty for §4's debounced save |
| `context_menu(&mut self, ui, tab, path)` | per-panel menu; also "Split right / Split down" |
| `id(&mut self, tab) -> Id` | derived from the variant, so two `Scene` tabs coexist (§5) |

`DockArea::show_inside(ui, &mut shell)` is called from the central region, below the menu bar and
the toolbar and above the status bar, which are ordinary `TopBottomPanel`s.

### 2.3 What egui_dock does not give us

It gives tabs, splits, drag-to-dock, undock-to-window and serde. It does **not** give a Window menu
to re-open a closed tab, named layout presets, or maximise-on-hover. All three are ours and all
three are small: a `Vec<Tab>` of what is not currently in the `DockState`, a directory of layout
files (§4), and §5.

---

## 3. The default layout

```
┌──────────────────────────────────────────────────────────────────────────┐
│ File  Edit  Create  Scene  Window  Help                     ● unsaved    │  menu bar   28 px
├──────────────────────────────────────────────────────────────────────────┤
│ [⊕][↔][⟳][⤢]  │ local▾ │ ⊞snap 0.25 │   ▶  ⏸  ⏭  ⏹   │  ↶ ↷  💾        │  toolbar    36 px
├────────────────┬───────────────────────────────────────┬─────────────────┤
│ Hierarchy      │ Scene | Game                          │ Inspector       │
│                │                                       │                 │
│  ▾ world       │                                       │  Transform      │
│    ▸ terrain   │        ← the ViewportPlacement →      │  MeshRenderer   │
│      shed      │                                       │  Material       │
│      lamp      │                                       │                 │
│                │                                       │                 │
│  260 pt        ├───────────────────────────────────────┤  320 pt         │
│                │ Console | Transactions | Events       │                 │
│                │                                  180pt│                 │
├────────────────┴───────────────────────────────────────┴─────────────────┤
│ Project (assets, prefabs, scripts, scenes)                        160 pt │
├──────────────────────────────────────────────────────────────────────────┤
│ ● 4.1 ms   142 nodes   38 draws   ⌁ agent idle           assets/x.loom   │  status     22 px
└──────────────────────────────────────────────────────────────────────────┘
```

**Hierarchy left, Inspector right, viewport centre, Console under the viewport, Project along the
bottom.** That is Unity's arrangement, and the reason to copy it exactly is that it is the one
layout a user arriving at this editor already knows. Deviating buys nothing and costs the only free
familiarity available.

Three choices inside it are not Unity's and are deliberate:

**Console and Transactions are tabs of one dock node, not two panels.** The transaction log is the
agent's activity feed, and the entire premise of this editor is that a human and an agent are
editing the same file at once. Putting it one click from the console — and next to Events, the
deterministic replay — makes "what just changed and who did it" a single place rather than three.

**Project spans the full width along the bottom** rather than sharing the console's node, because
it will hold thumbnails and folders and it is the panel that most wants horizontal room.

**Game is a tab beside Scene, not a separate window.** Two viewports are supported (§5) but the
default is one, because two live viewports are two forward passes and the default should not cost
that.

The **status bar** is one line and carries the four numbers `run.rs` prints today into the toolbar
(`fps · nodes · draws`) plus the agent-activity indicator and the current file. Moving them out of
the toolbar frees the toolbar for tools, which is what a toolbar is for.

---

## 4. Layout persistence, per project

**A layout is user state, not authored state, and that distinction decides where it lives.**
Property 1 of `CLAUDE.md` binds *authored* artifacts to diffable schema-validated text; a dock
split fraction is not authored, nobody reviews it in a diff, and item J of the constraints survey
says the exemption has to be written down. This is that writing.

```
<project>/.loom/layout.json          the current layout, saved on change (debounced 2 s)
<project>/.loom/layouts/<name>.json  named presets: Default, Wide, Sculpt, …
~/.config/loom/editor.json           preferences that are not per-project: theme scale,
                                     recents list, last project
```

**JSON, not TOML, and only here.** `DockState<Tab>` serialises through egui_dock's optional `serde`
feature into a deeply nested tagged-enum tree; TOML represents that badly and `toml_edit`'s
format-preserving DOM exists to protect *authored* files, which this is not. `serde_json 1.0.151`
is already pinned in seven crates. Scene text stays TOML; nothing about that changes.

`.loom/` is written to the project's `.gitignore` on project creation. A layout that follows a
project into version control would make every teammate's window jump; a layout that follows the
*user* would be wrong the moment two projects have different panel needs. Per-project-per-machine
is the only combination that is right, and an ignored directory is how you spell it.

**Loading a layout is fallible and must not be fatal.** A `DockState` whose `Tab` enum lost a
variant, a truncated file, a hand-edit — all produce a deserialisation error, and the response is
one console warning and the built-in default. An editor that refuses to open because its window
arrangement did not parse is an editor you cannot recover.

**`--frames n` ignores the saved layout entirely and uses the default.** `cargo xtask validate`
drives five windows through create-draw-teardown (`xtask/src/main.rs:1024, :1077`) and a saved
layout with a collapsed splitter would make the windowed half of green check 2 depend on the
developer's last drag. §1.4's clamp is the safety net; this is the actual fix.

---

## 5. Multiple viewports, and maximise-on-hover

**Multiple viewports fall out of `ViewportPlacement` being a parameter rather than a mode.** The
shell collects a `Vec<(Tab, ViewportPlacement, Camera)>` while egui builds the frame, and
`draw_with_ui` grows from taking one placement to taking a slice. Each entry is a forward pass and
a tonemap into a different destination rect, sharing the window-sized HDR, depth and MSAA images —
which the render graph must serialise, because tonemap *n* reads the scene image that forward *n+1*
then writes. That is a write-after-read hazard `plan_full` already handles for images.

**Ship one viewport in the first slice and the slice for `n` after it.** The mechanism is
identical; the risk is entirely in the graph's ordering of a repeated read-then-write on one image,
which is the one thing here I would want a barrier-list test to state before trusting it (§12).

**Maximise-on-hover is not a dock feature and is four lines of shell state.** Unity binds
Shift+Space; keep that. On press, if the pointer is over a tab body, stash the current `DockState`
and replace it with a single-leaf tree holding just that tab; on press again, restore the stash.
No layout is written to disk while maximised, so a crash mid-maximise loses nothing. It works for
the viewport for free, because the viewport is a rect and a maximised rect is just a bigger one.

**Camera picture-in-picture** — the "look through the selected `Camera`" affordance the engine
survey names as missing — is a third placement inside the Scene tab's rect, drawn in its lower
right. Same mechanism, no new machinery, and worth noting because it is the case that would
otherwise argue for a texture.

---

## 6. The theme

Default egui is instantly recognisable, and the request is that this not be. The distance is
covered by a palette, a spacing scale, a corner radius, one accent hue and one font decision — all
applied in **one file**, `crates/loom_cli/src/editor/theme.rs`, through
`egui::Context::all_styles_mut` (verified at `egui-0.35.0/src/context.rs:2145`). No panel sets its
own colours; a panel that needs a colour reads a token.

### 6.1 Palette

Dark only. A light theme is a second palette to maintain and a second set of contrast numbers to
check, for an editor whose main content is a lit 3D scene that a light chrome fights. If it is
wanted later it is a second token table and nothing else — which is the point of having a token
table.

| Token | Hex | Where |
| --- | --- | --- |
| `bg_deepest` | `#0E1013` | behind the dock, letterbox bars, viewport when no scene |
| `bg_panel` | `#16191E` | `Visuals::panel_fill` — every panel body |
| `bg_raised` | `#1E232A` | tab bar, toolbar, menu bar, table headers, `window_fill` |
| `bg_sunken` | `#0F1216` | `extreme_bg_color` — text fields, code, the console body |
| `bg_hover` | `#262C35` | `widgets.hovered.weak_bg_fill` |
| `bg_active` | `#2F3742` | `widgets.active.weak_bg_fill`, open menus |
| `line` | `#262C35` | 1 px separators, `widgets.noninteractive.bg_stroke` |
| `line_strong` | `#3A424E` | focused field border, splitter while dragged |
| `text_strong` | `#E6EAF0` | headings, the selected row, values being edited |
| `text` | `#C3CAD4` | body |
| `text_weak` | `#7C8794` | labels, units, secondary counts |
| `text_disabled` | `#4C5561` | `add_enabled(false)` |
| `accent` | `#A78BFA` | selection, focus ring, active tool, links, sliders' filled track |
| `accent_deep` | `#6E5BC4` | selection fill behind a row (at ~35% alpha) |
| `ok` | `#6FCF97` | validation passed, assertion held |
| `warn` | `#E8B84B` | physical-sanity warnings, "overrides differ" |
| `error` | `#F0736D` | parse failure, rejected transaction |
| `agent` | `#78C8FF` | **unchanged** — `Color32::from_rgb(120,200,255)`, `panels.rs:679` |
| `axis_x/y/z` | `#E2544F` `#7CC860` `#5494E8` | **unchanged** — `AXIS_COLORS`, `panels.rs:95-99` |

**The accent is violet on purpose.** Every hue in this editor that already means something is warm
red, green, blue or cyan — the three gizmo axes and the agent-change marks. Violet at roughly 260°
is the furthest unclaimed hue from all four, so a selection highlight can never be misread as an
axis or as somebody else's edit. Blue, the default choice, is the one hue that *would* be.

Contrast against `bg_panel`, computed rather than eyeballed (WCAG relative luminance):

```
text_strong 14.6   text 10.7   text_weak 4.8   accent 6.5
ok 9.3   warn 9.6   error 6.2   agent 9.7
axis X 4.7   axis Y 8.6   axis Z 5.7      text_disabled 2.3 (exempt)
```

Everything that carries meaning clears 4.5:1 — `text_weak` at 4.82 and `axis_x` at 4.71 are the two
that only just do, and neither may be darkened without re-checking.

**Surface separation is carried by strokes, not by fill.** `bg_raised` against `bg_panel` is 1.12:1,
which is invisible on its own and deliberately so: a 1 px `line` at the boundary reads as a crisp
edge where a luminance step reads as a smudge, and it is what makes a dense editor look drawn
rather than shaded.

### 6.2 Typography

egui 0.35's `Style::text_styles` is a `BTreeMap<TextStyle, FontId>` (`style.rs:288`). The scale:

| Style | Size | Use |
| --- | --- | --- |
| `Heading` | 14.0 | panel titles, inspector component headers |
| `Body` | 13.0 | everything |
| `Button` | 13.0 | |
| `Small` | 11.0 | units, counts, status bar, tooltips |
| `Monospace` | 12.0 | console, transaction labels, numeric fields, paths |

**Numeric fields use the monospace family.** A column of `DragValue`s in a proportional font
jitters horizontally as digits change under a drag, and a transform inspector is nine of them.
This is the single highest-value typography decision in the document and it costs one line.

**Ship on egui's bundled fonts first.** They are Ubuntu-Light and Hack, they are already embedded,
and the palette plus spacing plus radius does most of the "not default egui" work. **If the human
still reads it as default egui after §6.1–6.4 land, add Inter (SIL OFL 1.1) for UI and JetBrains
Mono (OFL 1.1) for monospace** — two `.ttf` files, roughly 350 KB, registered through
`Context::set_fonts` (`context.rs:2038`). That sequencing is deliberate: a font swap is a new
binary asset class and a licence entry, which ADR E has to cover, and it should be spent only if
the cheaper change did not work.

### 6.3 Spacing and shape

A 4 px base unit, used at 4 / 8 / 12 / 16 / 24. Concretely, against `Style::spacing`
(field names verified at `style.rs`, `Spacing`):

```
item_spacing      (6, 4)        button_padding   (8, 4)
window_margin     8 all round   menu_margin      6
indent            14            interact_size.y  22      icon_width 14
scroll.bar_width  8   (thin, and only over the content)
```

`interact_size.y` of 22 against egui's default 18 is the one change that most makes it read as an
application rather than a debug overlay: rows get room, and a 22 px row at 13 px text is the
density Unity and Blender both land on.

Corner radius — `CornerRadius` in 0.35, not `Rounding` (`style.rs:1302`) — is **4 for widgets,
6 for windows and menus, 0 for panels and tab bars**. Panels with rounded corners in a dock leave
gaps that show the layer beneath; the crispness is the point.

Shadows: `window_shadow` only, soft and near-black; `popup_shadow` the same at half the spread. No
shadow on docked panels — a docked panel has no elevation to express.

### 6.4 The tab bar

The one place worth spending detail, because it is the surface a docking editor is judged on. The
active tab is `bg_panel` (continuous with the body below it, so the tab and its content read as one
sheet), inactive tabs are `bg_raised` with `text_weak`, and the active tab carries a **2 px
`accent` rule along its top edge**. Hover lifts an inactive tab to `bg_hover` and its text to
`text`. No close buttons on inactive tabs — they appear on hover only, because twelve always-visible
✕ glyphs is visual noise proportional to the number of panels.

### 6.5 Icons

**`egui-phosphor = "=0.13.0"`**, verified today on crates.io: published 2026-07-22, one normal
dependency (`egui ^0.35`, default features off), wrapping the Phosphor icon set. Take the
`regular` weight only. It registers an extra font family and exposes icons as `&'static str`
constants, so an icon is a character in a `WidgetText` and needs no image loading, no atlas of
ours, and no per-icon texture.

**Rejected: hand-drawn `egui::Shape` paths** (writing an icon set is a week and looks it) and **a
PNG atlas** (a new binary asset class for something a font does better at every DPI).

**Icons never appear without a label** except in the toolbar's tool group and the tab bar, where
they are three or four glyphs the user learns once. An icon-only inspector is a memory test.

The licence question — Phosphor is MIT, the crate wrapping it is MIT/Apache — is ADR E's to record
alongside the fonts, because "sleek" quietly introduced a binary asset class and that should be a
decision rather than a commit.

---

## 7. Keyboard shortcuts

**Every binding stays in `assets/input/default.toml` through `loom_input::ActionMap`**
(`run.rs:2242-2251`), in named contexts, with the compiled-in copy as the fallback. The shell adds
one context, `shell`, alongside the existing `fly` / `edit` / `play`. Nothing is hardcoded in the
UI, which is what makes the rebinding UI later a panel rather than a refactor.

**`W`/`E`/`R` are not gizmo modes and never will be, because they fly the camera.** That is an
inherited constraint (`assets/input/default.toml:32-56`), it is why the existing editor uses
`1`/`2`/`3`, and every proposal to "just use the standard keys" breaks the fly camera. Written here
so it is refused once.

| Key | Action | Context |
| --- | --- | --- |
| `1` `2` `3` `4` | Move / Rotate / Scale / Transform gizmo | edit |
| `F` | Frame selection, or the scene when nothing is selected | shell |
| `Tab` / `` ` `` | Next / previous node — **un-consumed unless a text field has focus** | shell |
| `F2` | Rename selected | edit |
| `Delete` | Delete selection (deepest-first, one transaction) | edit |
| `Ctrl+D` | Duplicate (one transaction) | edit |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo — the **session's** stack, never the editor's (never-do #16) | edit |
| `Ctrl+S` | Save; on `SaveRejected::Stale` raises the divergence banner, never forces | edit |
| `Ctrl+K` | Command palette | shell |
| `Ctrl+F` | Filter the focused panel (hierarchy, project, console) | shell |
| `Shift+Space` | Maximise / restore the hovered tab | shell |
| `Ctrl+1…9` | Focus the *n*th tab in the current node | shell |
| `Ctrl+P` / `Ctrl+Shift+P` / `Ctrl+.` / `Ctrl+Shift+.` | Play / Pause / Step / Stop | shell |
| `Esc` | Release pointer capture; else clear selection; **never closes the window while captured** | play/shell |
| `IJKL` `UO` | Nudge (one `SetTransform` per node, one transaction) | edit |

`Ctrl+K` for the palette rather than `Ctrl+Shift+P`, because `Ctrl+Shift+P` is Pause and a transport
key that sometimes opens a text box is worse than an unfamiliar palette key.

**The bindings file must stop being resolved against the process working directory.** `run.rs:2243`
loads `assets/input/default.toml` relative to cwd, which the engine-surface survey lists as a
shipping blocker (item 11): `exe + assets/` only works when launched from the right folder, and a
project cannot own its bindings. The shell resolves in order — `<project>/assets/input/*.toml`,
then `<exe_dir>/assets/input/*.toml`, then the compiled-in copy — and this is small enough to land
in the shell's first slice rather than waiting for the Hub.

---

## 8. Window chrome

**Keep winit's native decorations.** A custom-drawn title bar on X11 means implementing drag,
double-click-to-maximise, edge resize, snapping and the window menu by hand, per window manager,
for a cosmetic gain — and this box runs a cut-down KDE Plasma on X11 with dwm as the styled
session, i.e. two window managers with different conventions. The menu bar sits inside the client
area as an ordinary `TopBottomPanel` and reads as part of the application.

`LOOM_WINDOW_AT=x,y` keeps working (`run.rs:776-781`); the validation gate opens five windows and
scatters them without it.

The window title is `<scene name> — <project name> — Loom`, with a leading `●` when there are
unsaved edits. That is where the existing toolbar's "● unsaved" indicator goes, freeing toolbar
width and putting the state where a user with three Loom windows open can see it from the taskbar.

---

## 9. Files and modules

**All new editor code lands in `crates/loom_cli/src/editor/`, as a module tree with one entry
point and no reverse dependencies**, so that lifting it into a `loom_editor` crate later is a
`git mv` plus a manifest rather than an untangling. It is not lifted now because ADR F's split
requires egui to become *optional* in `loom_render`, and `materials.rs` and `log.rs` are used by
the headless render path in `main.rs` (`:573`, `:1138`) and cannot simply move with the UI.

```
crates/loom_cli/src/editor/
  mod.rs          Editor — owns DockState, Shell, theme, layout, shortcuts. The entry point.
  dock.rs         Tab, Shell (impl TabViewer), the Window menu, maximise-on-hover
  theme.rs        the token table of §6 and the single `all_styles_mut` application
  layout.rs       load/save/named presets, the debounce, the ignore-and-warn fallback
  viewport.rs     rect → ViewportPlacement, to_viewport/to_window, the input Response
  shortcuts.rs    the `shell` context over loom_input::ActionMap
  overlay.rs      gizmo handles + agent change-marks, moved off the background layer
  panels/         hierarchy.rs inspector.rs project.rs console.rs transactions.rs …
```

Touched outside it:

| File | Change |
| --- | --- |
| `crates/loom_render/src/viewer.rs` | `ViewportPlacement`; `draw_with_ui` takes it; forward/water/rain viewports; the `chrome_clear` pass; CMAA2's conditional reorder |
| `crates/loom_render/src/tonemap.rs` | `record` takes `origin`/`extent`; push range 4 → 12 bytes |
| `assets/shaders/tonemap.slang` | `int2 origin` in `TonemapPush`; the subtraction in `tonemapFragmentMain` |
| `crates/loom_render_graph/src/lib.rs` | the barrier-list test names `chrome_clear`'s transition |
| `crates/loom_cli/src/run.rs` | **split** — the winit `ApplicationHandler`, fly camera, watcher and play driver stay; every `UiAction` and every panel call moves out. `transact`/`transact_as` (`:1707-1756`) move **verbatim**, comments included |
| `crates/loom_cli/src/gizmo.rs` | unchanged; `View` is constructed from the rect extent instead of the swapchain's |
| `crates/loom_cli/src/panels.rs` | deleted |
| `crates/loom_cli/Cargo.toml` | `egui = "=0.35.0"`, `egui_dock = "=0.20.1"`, `egui-phosphor = "=0.13.0"`, `egui_dock` with `serde` |
| `assets/input/default.toml` | the `shell` context of §7 |
| `xtask/src/main.rs` | nothing, if §1.3 holds. If a golden reference moves, §1.3 does not hold and the change is wrong |

**The shell issues no `SceneOp`s of its own.** Docking, theming, maximising and layout saving are
outside the scene text and are the documented exemptions §4 names. Every mutation still funnels
through `transact` / `transact_as` into `Session::apply` / `apply_coalescing`, which is the whole
of never-do #16's machinery and the one thing in the old editor that must survive untouched.

---

## 10. ADRs this needs

**ADR 0022 — the viewport is a sub-rectangle of the swapchain.** *This is constraints-survey item
I and it is the one that must be approved before code is written.*

> **Decision.** The editor's 3D viewport is rendered by setting the forward pass's viewport to the
> dock rectangle's size and the tonemap's destination viewport to its position, with the source
> origin passed as a push constant, rather than by rendering to an offscreen image sampled as an
> egui texture. `ViewportPlacement` is `None` for every non-editor path, and when it is `None` the
> frame is byte-identical to the frame this project renders today — same passes, same formats, same
> destinations — which is what keeps `loom render` and `loom run` in agreement and the golden gate
> authoritative. The cost is that the viewport cannot be scaled, scrolled, clipped, floated beneath
> another panel, or given an egui effect; the first of those that is wanted is the trigger to adopt
> the texture path, whose mechanism (`add_user_texture` over a `create_vulkan_descriptor_set`) is
> recorded here so the reversal is cheap. `chrome_clear` is added to the render graph and to its
> barrier-list test, because a barrier outside the graph is never-do #4.

**ADR 0023 — CMAA2 moves ahead of the UI pass when the viewport is docked.** An amendment to
ADR 0018 rather than a new decision, and separable from 0022 because CMAA2 is off by default.

> **Decision.** ADR 0018 placed the UI pass before CMAA2 so that egui's text was filtered along with
> the scene. With a docked viewport the frame is largely panel chrome, and a display-referred edge
> filter over chrome is wrong; so when a `ViewportPlacement` is present, CMAA2 runs before the UI
> pass and over the viewport sub-rectangle only, using the same origin push constant as the
> tonemap. With no placement the order is unchanged. The chain is therefore
> `forward → tonemap → [cmaa2] → ui → present` in the editor and
> `forward → tonemap → ui → [cmaa2] → present` everywhere else.

**ADR E (per the constraints survey) — new UI dependencies.** The shell's contribution to it:
`egui_dock = "=0.20.1"` (MIT, egui ^0.35, adds `duplicate` and `paste`) and
`egui-phosphor = "=0.13.0"` (egui ^0.35, one dependency, embeds the MIT Phosphor icon font), both
in `loom_cli`, with `egui = "=0.35.0"` pinned there directly. The decision statement should record
that **`egui-ash-renderer 0.12.0` is what pins egui at 0.35** and that both new crates track that
line, so neither adds upgrade coupling. It should also record the font/icon licence answer —
Phosphor MIT now, and SIL OFL 1.1 for Inter and JetBrains Mono if §6.2's second step is taken.

**ADR H (per the constraints survey) — projects.** Not this document's decision, but the shell
depends on one clause of it: **that a project has a root directory**, because §4's layout files,
§7's binding resolution and the Project panel's tree all hang off it. If projects are deferred, the
project root falls back to the scene file's parent directory and everything above still works.

**Not an ADR:** the theme, the shortcut table, the default layout, and moving the editor into
`crates/loom_cli/src/editor/`. No `// STABLE` markers exist anywhere in `crates/` (verified by the
constraints survey), so never-do #13 does not fire on replacing `panels.rs`.

---

## 11. Build order

Small commits, each producing something that runs, which is the style rule and also the only way a
Vulkan viewport change is debuggable.

1. **`ViewportPlacement` with a hardcoded inset.** No dock, no theme. `loom run --edit` draws the
   scene into a rectangle 200 px in from every edge with the existing six panels over it. If the
   scene is in the wrong place, or `cargo xtask validate` reports one message, it is visible here
   and nowhere later. Exit criterion: `cargo xtask image` produces **zero** changed references.
2. **`chrome_clear` + the barrier-list test names it.**
3. **Picking, gizmos and overlays in viewport coordinates** — `to_viewport`/`to_window`, the
   `gizmo.rs:211-224` inverse test extended to a non-zero origin.
4. **The theme.** `theme.rs` alone, over the *old* panels. It is the cheapest possible test of
   whether §6 reads as sleek, and it is reversible in one file.
5. **egui_dock, the `Tab` enum, and the default layout**, with the existing panel bodies moved
   into `TabViewer::ui` unchanged. The editor is now Unity-shaped and no panel has been rewritten.
6. **Layout persistence, the Window menu, maximise-on-hover.**
7. **The shortcut table and the bindings-path fix.**
8. **CMAA2 reorder** (opt-in, so last).
9. **The second viewport**, and the barrier-list test for the repeated scene-image read-then-write.

Steps 1–3 are the risky ones and they are first. Steps 4–7 are UI and cannot break a render. That
ordering is the point.

---

## 12. What I could not verify

Written plainly, because an unmarked guess is worse than an admitted gap. Nothing below was built,
run or measured — this phase forbids `cargo build`.

**1. egui's gamma handling through `egui-ash-renderer`, and therefore what the *texture* path would
look like.** The vendored fragment shader (`egui-ash-renderer-0.12.0/src/shaders/shader.frag`) has
a specialization constant `SRGB_FRAMEBUFFER`; with it `false` — which is what `ui.rs:88` sets — the
shader applies `pow(color, 1/2.2)` itself. The swapchain is chosen as `B8G8R8A8_SRGB`
(`viewer.rs:2098-2106`) and `Ui::new` is handed that format (`run.rs:799`), so the hardware encodes
a second time. By my reading that is a double encode; by the evidence that nobody has complained
about the panels looking washed out, my reading is probably missing something. **I did not resolve
it and I did not need to, because the chosen design routes no scene pixels through that shader.**
It is recorded because it is exactly the trap the research doc names
(`loom-pcg-and-editor.md:157`) and it is the first thing to measure if the texture path is ever
adopted. The experiment is one screenshot: render `assets/test/materials.loom` through
`loom render` and through a docked texture viewport and difference them with `loom compare`.

**2. The cost of the `chrome_clear` pass.** I reasoned it to tens of microseconds from the
project's own measured 13.5–14.0 GB/s readback bandwidth, which is a PCIe number and not a VRAM
number, so the estimate is conservative by an order of magnitude and still negligible. It is a
`LOOM_GPU_TIMING=1` line on the first commit.

**3. Whether `plan_full` orders a read-then-write on one image across two passes correctly.** The
graph owns buffer barriers as of ADR 0017 and the barrier-list test names image transitions, so I
believe it does — but multiple viewports are the first thing in this codebase to read the scene
image and then write it again in the same command buffer, and I have not read `plan_full`. That is
why §11 puts the second viewport last and asks for a barrier-list test.

**4. `egui_dock` 0.20.1's exact API surface beyond `TabViewer`.** I read every method of that trait
off docs.rs and quoted the ones I rely on. I did **not** read `DockArea`'s builder methods,
`DockState`'s tree-surgery API, or the `style` module, so the details of how the tab bar is
restyled (§6.4) may be a `Style` struct rather than egui's `Visuals`, and "2 px accent rule on the
active tab" may need a custom `tab_style_override` rather than a global setting. The shape holds;
the exact call may not.

**5. Whether `crates.io`'s dependency listing for `egui_dock 0.20.1` is complete.** The API
response labelled `egui ^0.35` inconsistently between two endpoints (once "normal", once "dev");
`docs.rs/crate/egui_dock/0.20.1` lists it as a normal dependency at `^0.35`, which is what I used.
One `cargo add --dry-run` settles it and I did not run one.

**6. Whether the fly camera survives being driven by an egui `Response` rather than raw winit
events.** Right-mouse-held look is `Response::dragged_by(PointerButton::Secondary)`, which should be
equivalent, but egui's drag threshold and the existing per-redraw key latching are two different
event models meeting, and the failure mode is a camera that stutters on the first few pixels of a
look. Cheap to test, not tested.

**7. Every number in §6.1 that is not a contrast ratio.** The ratios were computed. The hexes are
chosen, not measured against a display, and "sleek" is a judgement the human makes by looking at
step 4 of §11 — which is why step 4 exists as its own commit and touches one file.
