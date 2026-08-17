# Checks only a human at the keyboard can make

**Why this file exists.** Four green checks cover a lot and cannot cover this. `cargo xtask
image` drives the offscreen `Renderer`, which never constructs a `Ui` — so **no gate in this
project has ever seen a pixel of the editor**. `cargo xtask validate` opens a window and
proves the Vulkan calls are legal, not that the thing is usable. Everything below was built
and gated, and none of it has been looked at.

This is a running list, not a one-off. Tick items off when you have actually done them; add
to it whenever a stage ships something a gate cannot see.

---

## Pending — Stage 3's dock (`8df00a9`), the biggest outstanding block

The dock was built by a subagent, judged against a competing design, and verified by me
against the source and all four gates. **What could not be verified is anything requiring a
mouse.** The automated stand-in is `dock::tests::the_carve_leaves_the_viewport_and_nothing_else`,
which pins the carved rectangle's geometry — a fair substitute for *where the hole is*, and
no substitute at all for *what happens when you click in it*.

The risk is specific and worth understanding before testing. `DockArea` allocates the entire
root rect, which would leave `root_ui_available_rect` empty, which makes `is_pointer_over_egui`
answer true everywhere, which makes `run.rs`'s `Ui::wants_pointer` gate swallow **every**
viewport click. The fix — the "carve" — draws the dock into a child `Ui` and lays four
zero-frame panels to hand the viewport's rectangle back. If the carve is subtly wrong, the
symptom is *clicks not selecting*, not a crash and not a wrong picture.

- [ ] **Click an object at the far corner of the viewport.** Does the right thing select?
      Repeat in three different dock arrangements (default, splitter dragged far left,
      bottom node collapsed).
- [ ] **Click on a panel.** Does nothing behind it select? This is the failure the carve
      exists to prevent, in the opposite direction.
- [ ] **Scroll the wheel over the viewport.** Does the camera dolly, or does the tab scroll?
      (`scroll_bars` is `[false, false]` for viewport tabs, which should prevent the latter.)
- [ ] **Press Play.** Does the `Game` tab come to the front?
- [ ] **Drag a splitter to zero width.** Zero validation messages? The placement should clamp
      and the scene passes should skip below 8 px.
- [ ] **Tear a panel off and re-dock it.** Then **restart** — does the layout come back?
      (Layout persists to `$XDG_STATE_HOME/loom/layouts/default.json`.)
- [ ] **Drag the window edge** and watch the seam between the scene and the panel. A lagging
      seam means the `Ui::layout`/`record` split regressed — see ADR 0025.
- [ ] Does the inset scene align exactly with its rectangle at a **HiDPI scale factor**?

## Pending — Stage 3's theme

- [ ] **Does it read as sleek?** That judgement is the entire point of the theme step.
- [ ] Run the swatch probe and **sample three swatches before judging**, because a palette
      judged through a wrong encode is judged as a bad palette (ADR 0033). Sampled bytes
      should equal the table's hexes within ±2.
- [ ] Text weight: the sRGB fix moved blending from gamma to linear, which changes egui's
      glyph coverage. Does the type look right, or thin?

## Pending — Stage 1's inspector

- [ ] Set `Script.path` from the inspector on a node that has one. This was the single most
      limiting gap in the old editor.
- [ ] Pick a `Material` albedo with the colour swatch — **does the viewport follow?**
      (`Viewer::set_materials` is new and nothing but a human has seen it work.)
- [ ] Open `assets/test/prefab_room.loom`, change a field on an instance, see the override
      marker, click revert, watch it go back.
- [ ] Edit `WaterBody.waves` without touching a text editor.

## Pending — the gizmo drag performance fix

- [ ] Drag a gizmo on `assets/test/materials.loom`. **Smooth?** The reported stall was ~67 ms
      per frame; measured after the fix it should be ~0.9 ms of rebuild work.
      Note the launcher pins `loom-companion-docs`, so **rebuild release** first — the fix is
      not in a binary built before it.

---

## How to run the editor

The desktop launcher (`~/.local/bin/loom-editor`) pins the development checkout explicitly.
Rebuild before testing, or you will judge an old binary:

```
cd ~/loom/.claude/worktrees/loom-companion-docs
cargo build --release -p loom_cli -j 6
```

Then click the launcher, or:

```
target/release/loom run assets/test/materials.loom --edit
```
