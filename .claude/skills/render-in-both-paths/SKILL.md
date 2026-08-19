---
name: render-in-both-paths
description: Use for any change that adds, moves or fixes something drawn in Loom — a pipeline, a vertex count, a buffer upload, an environment field, a sort, a sample count — before calling it done. The engine has two renderers and no gate can see the second one.
---

# Two renderers, one of which no gate photographs

`loom_render` has two types with two copies of the draw wiring:

- `crates/loom_render/src/renderer.rs` (~5,100 lines) — headless, offscreen.
  **The only path any gate ever photographs.**
- `crates/loom_render/src/viewer.rs` (~2,800 lines) — the window the human
  actually judges in.

**This is the single most-repeated defect in the repository, and the commits
say so in their own subject lines.**

| Commit | What shipped one-sided |
| --- | --- |
| `f937b15` | *"loom run draws the cinematic tier — **the both-paths defect, third time**"*. Play mode stepped the solver and drew none of it; the flagship feature of the whole water rebuild was invisible in the window. *"The defect is two floors deep and either floor alone reproduces it."* |
| `310f650` | *"the nappe rides in the window's water draw too — **same defect, fourth**"*. `renderer.rs` counted `WATER_VERTS + NAPPE_VERTS`, `viewer.rs` counted `WATER_VERTS`. One token. `cascade.loom` was a canyon with no waterfall. |
| `b01b0ee` | `Renderer::set_ripples` shipped before `Viewer` had it — the wake was felt by buoyancy and by `--assert` while the surface drew flat. |
| `a1baa3c` | *"the viewer has no grass path, and why no gate could see it"*. |
| `a3c02e8` | The window built every pipeline at `TYPE_1` **for two whole phases**, so every AA number in the project was measured on a path the human never looked at. |
| `8c2bcb6` | `environment.eye` assigned below the upload in the viewer only (the window drew a frame behind); `environment.viewport` never written at all. |
| `a6c491b` | The particle sort was transcribed into `viewer.rs` under a comment saying *"identical to the offscreen path"*. *"I fixed `renderer.rs` first and measured no change at all."* 36.6 → 140.3 fps once the duplicate was deleted. |

`a82e34d` states the rule the project reached: *"Splitting only the offscreen
one would be the offscreen/viewer divergence that has already cost this project
three defects, **and no golden image can see it — the gate only ever renders
headless**."*

## What must be mirrored

Anything on this list that you touch in one file has a counterpart in the other:

- pipelines and their **rasterisation sample count** (`Msaa`, `renderer.rs`)
- per-effect buffers and their `set_*` uploads (`set_ripples` and its siblings)
- every `environment.*` field, and **where** it is assigned relative to the
  upload
- vertex-count constants feeding the forward-pass split
  (`WATER_VERTS + NAPPE_VERTS`)
- particle assembly, culling and sort order

## Fix it by extracting, not by mirroring

In preference order:

1. **One shared function both callers use.** `renderer::sort_particles` is the
   template — `viewer.rs:1421` now calls it. So are
   `renderer::draw_water_and_particles` and `Msaa::new`.
2. Only if that is impossible, mirror the edit — and then say in the commit
   message that you did.

**A comment saying "identical to the offscreen path" is the marker of a bug in
waiting.** Grep for that phrasing when hunting.

## Deliberate asymmetries — do not "fix" these

- **Rain and the editor UI are single-sample by design.** They draw into the
  resolved target, after the MSAA resolve.
- Golden references are offscreen renders; the viewer resolves into the
  swapchain format, not `COLOR_FORMAT`.
- The `Viewer` is deliberately uninstrumented for `LOOM_GPU_TIMING`.

## The check, because no gate covers it

```bash
./target/release/loom run <scene> --play          # and look
./target/release/loom run <scene> --frames 300    # per-frame cpu/draw
```

Then the cross-path agreement test this project actually used: **a countable
quantity matched at a fixed tick between the window and
`loom render --sim N`.** `f937b15` matched marched triangles — 14 → 19,328;
51 → 20,518; 72 → 19,828.

For the motion half, use the `watch-loom` skill. For registering the offscreen
half in the gate, `loom-gate-a-rendering-path`.

## Note

This skill is wrong only if `Renderer` and `Viewer` are merged into one type.
Nothing proposes that, and the split is structural (swapchain + winit versus
offscreen readback). Even then the core claim survives: **the gate never
photographs a window.**
