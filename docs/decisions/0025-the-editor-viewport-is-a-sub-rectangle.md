# ADR 0025 — The editor viewport is a sub-rectangle of the swapchain

- **Date:** 2026-08-16
- **Status:** **accepted**
- **Decision touched:** the build-order row in `CLAUDE.md` — *headless offscreen
  render + PNG before the swapchain* — whose real content is that the offscreen
  path is the gate and the window has to agree with it pixel for pixel. Nothing
  in that row's text changes. What changes is that the window can now rasterise
  the scene somewhere other than the whole image, and the agreement is preserved
  by construction (`placement: None`) rather than by the two paths having no
  choice. Never-do #4 is honoured, not excepted: the new clear is a graph pass.
- **Plan:** `docs/design/editor/PLAN.md` §3 (this row) and Stage 2. It supersedes
  doc 05 §6.9's "hard dependency on the render-to-texture viewport (ADR I)" —
  there is no render-to-texture viewport, and `PLAN.md:968-972` says to read that
  dependency as a dependency on this ADR instead.
- **Implemented by:** `7140ffb` (Stage 2) and `b1d16e0` (Stage 3, the coordinate
  half).

## Context — the editor needs the scene to stop being the window

Every frame this engine has ever drawn filled its target. The forward pass, the
water and rain passes, the tonemap and the readback all took `self.width,
self.height` and covered them. The editor's dock puts panels on four sides, so
the scene has to occupy a rectangle in the middle, and there are exactly two
ways to do that.

## The decision

**The scene is rasterised into a sub-rectangle of the swapchain image, not into
a texture that egui samples.** `ViewportPlacement { x, y, width, height }`
(`crates/loom_render/src/renderer.rs:3095`) is `Option`al on both the offscreen
renderer and the viewer; `None` means fill the target and is the runtime's
placement, the offscreen path's, and every existing scene's.

### Why not render-to-texture

The texture path is the conventional answer and it is not free. It costs a
second colour image sized to the dock rect, a sampler, a descriptor set
registered with egui (`add_user_texture` over `create_vulkan_descriptor_set`), a
resize policy for what happens when the splitter moves, and a colour round-trip
through egui's own shader. That last item is the one that decides it: egui's
blit is a second place where the window and the offscreen path can disagree
about what a pixel is worth, and this project has already paid for that class of
defect three times — the viewer drawing at one sample while every AA number was
measured offscreen at 4×, the linear/sRGB confusions, and the clear value below
in this same change. A sub-rectangle costs a viewport, a scissor and a push
constant, and it leaves exactly one frame path.

The cost of choosing this way is that the scene image is not an object anyone
can sample, which is what §"What this forecloses" is about.

### The scene renders at the origin; only the tonemap moves it

The passes upstream of the tonemap draw at `(0, 0)` into window-sized images,
sized to the placement — `scene_extent` in `crates/loom_render/src/viewer.rs:1296`
is the placement's extent, and every `set_viewport` in the frame takes it. The
tonemap is what writes the destination sub-rect — `Tonemap::record`
(`crates/loom_render/src/tonemap.rs:147`) sets its viewport (`:198`) and scissor
(`:210`) to `placement.x, y, width, height`.

That split is the whole reason nothing reallocates when a splitter moves. If the
forward pass drew at the placement's origin instead, its attachments would be
correct only for one dock layout; drawing at the origin means a smaller
rectangle is *less* of the same image, and the images are only ever resized when
the window is.

It also means the projection has to come from the rectangle rather than the
image. `renderer.rs:1683` and `viewer.rs:1127` both take the aspect from
`self.placement`, falling back to the target's size. A projection built from the
window while the scene rasterises into a sub-rectangle stretches the scene by
exactly the ratio between the two — invisible when the dock rect happens to
share the window's shape, obvious the moment a panel moves.

### `TonemapPush` is `int2 origin; float exposure;`

`SV_Position` is in framebuffer coordinates, not viewport-relative ones. With an
offset destination viewport the fragment shader is handed coordinates the source
image does not have — the scene lives at the origin — so the tonemap subtracts
the origin to get back to the source texel
(`assets/shaders/tonemap.slang:79`).

The field order is not cosmetic. `int2` first and `float` second is twelve bytes
under either packing rule, so the Rust side and the shader cannot end up
disagreeing about where `exposure` starts. Both sides say twelve in as many
words: `crates/loom_render/src/tonemap.rs:94` (`.size(12)`) and `:189`
(`let mut push = [0u8; 12]`), against `assets/shaders/tonemap.slang:30`.

One further asymmetry worth keeping: the tonemap's **render area is the whole
target while its viewport is the sub-rectangle**
(`crates/loom_render/src/tonemap.rs:170-181`). `LOAD_OP` is `DONT_CARE`, and
that applies to the render area — so a render area matching the viewport would
leave the rest of the attachment formally undefined despite `chrome_clear`
having just written it.

### `Ui::draw` splits, and that is a bug fix, not a refactor

`Ui::draw` used to lay out and record in one call, from inside the `ui`
render-graph pass closure — which `RenderGraph::execute` runs *after* the
forward and tonemap closures have recorded. So egui's layout for frame N
happened after the scene for frame N was committed, and any rectangle read from
the dock to place the scene could only be frame N−1's.

That is invisible while the scene fills the window and glaring the moment it
does not: a splitter drag would show a stale scene rect against a live panel
edge on every frame, and `chrome_clear` would paint the gap a tidy near-black
that reads as intentional rather than as a bug. It is the plan's risk R1, and it
had to land before anything read a rectangle from the dock, because retrofitting
it afterwards means re-auditing every panel for "does this read state the frame
has not produced".

It is now `Ui::layout` (`crates/loom_render/src/ui.rs:244`) — `take_egui_input`,
`run_ui`, `handle_platform_output`, `tessellate` — called before the graph is
built, and `Ui::record` (`:284`) — `set_textures`, `cmd_draw`, `free_textures` —
still inside the `ui` pass, where the target is in `COLOR_ATTACHMENT_OPTIMAL`.
The rectangle rides on the opaque `UiFrame` that produced it (`:73`,
`scene_rect` at `:92`), so there is no way to read it from the wrong frame.
`scene_rect` also multiplies by that frame's `pixels_per_point`, because egui
lays out in logical points and a swapchain is sized in pixels; conflating them
puts a HiDPI viewport half a panel off.

`egui::Context` has no `available_rect` in 0.35 — the answer lives on the root
`Ui`, which `run_ui` owns and drops — so `layout` wraps the caller's build
closure and reads `available_rect_before_wrap` inside it rather than querying
the context afterwards.

### Two clamps, and the VUIDs they prevent

`ViewportPlacement::new` (`renderer.rs:3121`) clamps the origin into the
swapchain and the size to what is left, minimum 1×1. Both are load-bearing and
both are reachable by ordinary use, because a human can drag a splitter into the
window edge and egui will hand back a rectangle of zero or negative size:

- a zero-width viewport is **VUID-VkViewport-width-01770**;
- a scissor reaching past the attachment is **VUID-vkCmdSetScissor-x-00595**.

Clamping in the constructor rather than at the four call sites means there is
one place to get it right. `MIN_VIEWPORT = 8` (`renderer.rs:3108`) and
`is_degenerate` (`:3144`) are the separate, coarser question of when rendering a
whole frame into a sliver stops being worth doing at all. **They have no
consumer yet** — the clamp makes a sliver legal and the scene passes still run
for it. Wiring the skip is outstanding editor work, not part of this decision.

### `chrome_clear`, and why its colour is linear

The tonemap stopped covering the target, so the rest of the image is whatever
was last there — on a freshly acquired swapchain image, uninitialised memory,
and in a golden PNG, uninitialised memory presented as a rendered frame.
`clear_chrome` (`renderer.rs:4256`) is one `LOAD_OP_CLEAR` with no draw. It is a
graph pass (`renderer.rs:2402`) rather than a raw `vkCmdClearColorImage` because
the target has to be in `COLOR_ATTACHMENT_OPTIMAL` first and putting that
transition anywhere but the graph is never-do #4 — and it is added **only when a
placement is set**, so the barrier list for every existing scene is untouched.

Its clear value is the palette's `ground` (`#0E1013`) written as **linear**
floats, `[0.004_39, 0.005_18, 0.006_51, 1.0]` (`renderer.rs:4277`). The target
is `_SRGB`, and a clear value for an sRGB attachment is interpreted as linear
and encoded on write. Writing the hex bytes produced roughly **`#434A55`** — a
mid slate — where `#0E1013` was specified. It was found by rendering it and
sampling the PNG, and the barrier-list test now asserts the corner pixel is
`[0x0E, 0x10, 0x13]` (`crates/loom_render/src/lib.rs:561`) so it cannot come
back silently.

That same test is where the pass's graph membership is pinned: it asserts
`("chrome_clear", "loom.ldr_target")` appears among the transitions
(`lib.rs:548`) and that a placement adds **exactly one** pass rather than
rearranging the graph (`:550-554`).

### Coordinates: one mapping, in `View`

With the scene inset, every window pixel winit reports and every point egui
draws at is offset from the scene's own space. The conversion lives in
`gizmo::View` — `View::at` takes an origin
(`crates/loom_editor/src/gizmo.rs:77`), `to_viewport` subtracts it (`:94`) and
`to_window` adds it (`:100`) — so `pick_at_cursor`, `drag_gizmo`,
`press_in_viewport`, the per-frame handle recomputation and `agent_marks` all go
on speaking window pixels and all become correct at once. Converting at each
call site would have been five places to get right and five for a later reader
to miss one, and a missed one is a click that selects the wrong object, which
reads as a picking bug rather than as a coordinate bug.

`View` is built from `Viewer::last_placement` (`viewer.rs:878`) — the placement
the renderer actually rendered with, recorded after the layout — not from the
rectangle the editor intends. An inverse test alone cannot catch this class of
bug, because a `project` that forgot to add the origin and a `ray` that forgot
to subtract it are still exact inverses; the non-zero-origin test also asserts
the projection lands offset by exactly the origin against a centred view.

## The gate, and the evidence for "byte-identical"

**The window cannot be photographed by any of the four green checks.** Without a
gate, the whole placement path would ship on a human having looked at it once.
So `loom render --viewport x,y,w,h` (`crates/loom_cli/src/main.rs:569`, which
rejects a non-positive width or height with `bad_argument`) puts an existing
scene into a sub-rectangle of a larger canvas through the **offscreen** path,
and `viewport_rect` is a `GOLDEN` row (`xtask/src/main.rs:265`). Its scene is
`materials` deliberately: the entry is not about content, it is about where the
content lands — a wrong origin, a wrong aspect or a missing `chrome_clear` fails
on pixels rather than only on validation messages.

The byte-identity claim is evidence, not prose. `git show 7140ffb --
tests/references/MANIFEST.txt` is **one added line and no removed lines**:
`viewport_rect.png` appears and no existing reference hash moved. That is what
"`placement: None` is byte-identical" means, and it is what keeps `loom render`
and `loom run` in agreement and the golden gate a valid check on both.

The windowed half of green check 2 drives `loom run --edit --frames N`
(`xtask/src/main.rs:1053` and `:1106`), so from Stage 3 onward the dock, the placement,
`chrome_clear` and the clamps are all exercised under the validation layers on
every `cargo xtask validate`.

## What it costs and what it forecloses

The scene image is never a texture, so:

- **No render scale.** The scene cannot be rasterised at a different resolution
  from the rectangle it occupies; the two are the same numbers.
- **No scrolled or clipped viewport**, and no viewport floating beneath a
  translucent or overlapping panel. A tab that partially covers the scene covers
  it with egui's own draw, over the top, not with a composite of the scene.
- **No egui effect on the scene image** — no rounding, no drop shadow, no fade,
  no zoom animation on the viewport itself.
- **A windowed `--frames N` run renders whatever rectangle the dock's layout
  leaves**, rather than the whole window it used to. The gate's two windowed
  invocations pass `--edit`, so they exercise the inset; the plan's answer to
  a saved layout leaking into them (`--frames` ignoring persisted layout) is
  Stage 3 work and is not built.
- Every pass in the frame now carries a rectangle it did not carry before. The
  clamp is one place, but "which extent does this pass take" is a question a
  reader has to ask of each of them, and the answer is the placement's for
  everything upstream of the tonemap and the target's for everything after it —
  including rain and the UI, which draw into the resolved target.

## Reversal

The first of the foreclosed items that is genuinely wanted is the trigger to
adopt the texture path, and the reversal is deliberately cheap: `ViewportPlacement`
is `Option` at one seam on each renderer (`Viewer::set_placement`,
`viewer.rs:868`), so the texture path is a new colour image plus
`add_user_texture` over `create_vulkan_descriptor_set`, with every consumer of
the rectangle already routed through `last_placement` and `gizmo::View`. What
would have to be re-argued at that point is not the plumbing but the thing this
ADR is actually protecting — that the window and the offscreen path produce the
same pixels — and the golden manifest is the instrument that would answer it.
