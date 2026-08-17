# ADR 0022 — The editor is a crate; the runtime links egui because the HUD does

- **Date:** 2026-08-16
- **Status:** **accepted**
- **Decision touched:** none locked. It adds a workspace dependency rule of the
  same kind as the three CLAUDE.md already lists, and `scripts/check-deps.sh`
  enforces it beside them.
- **Replaces:** the editor-stripping design in editor companion doc 06 §1
  ("ADR A"), which feature-gated egui out of `loom_render`. That design does not
  survive contact with two lines of shipped code; §"Why egui is not gated"
  quotes both.

## Context — "make the editor removable" was assumed to mean "make egui removable"

The editor needs to be a thing a shipped game does not carry. Everything else
about the editor rework — the dock, the inspector, the theme, twenty more ADRs —
assumes there is a line somewhere with editor UI on one side and runtime on the
other, and that the line can be drawn *without splitting a frame loop in half*.

The obvious reading is that the line is a Cargo feature on `loom_render`: no
`editor` feature, no egui, no UI pass. Two facts in the tree kill it.

## Decision

**The editor is a crate — `loom_editor` — and "stripping the editor" means not
linking that crate. egui stays an unconditional dependency of `loom_render`.**

- `crates/loom_editor/` holds the editor UI and nothing else: `panels.rs`,
  `dock.rs`, `gizmo.rs`, `theme.rs`, `console.rs`, ~3,270 lines. It depends on
  `loom_scene`, `loom_reflect` and `loom_render` only — never on `loom_cli`,
  which depends on *it*.
- It imports no `ash` and pins no `egui`. It reaches egui through
  `loom_render`'s re-export (`crates/loom_render/src/lib.rs:67-68`), which is
  the pattern `panels.rs:17` already used before the crate existed. A second
  direct pin is how two egui versions get linked, and that failure reads as a
  type mismatch on `egui::Context` rather than as a version conflict.
- `egui_dock = { version = "=0.20.1", features = ["serde"] }` lands **here**,
  in `loom_editor`'s manifest, never in `loom_cli` or `loom_render`, and it is
  pinned to 0.20.1 rather than the latest because 0.21.x moved to egui 0.36
  against this workspace's 0.35 — the same two-versions failure arriving through
  a transitive edge instead of a direct one.
- `loom_cli` gets `[features] default = ["editor"]`, `editor = ["dep:loom_editor"]`
  (`crates/loom_cli/Cargo.toml:17-19`) and an `optional = true` dependency edge
  (`:26`). `loom-play` is this same crate built `--no-default-features`.

The crate boundary is doing structural work beyond removability, and
`lib.rs`'s header says so: every panel is a pure function of borrowed state,
returning `UiAction`s that `loom_cli` turns into `loom_scene::ops::Transaction`s
through the same path the agent uses. A UI crate that *cannot reach* the apply
path is never-do #16 — no second undo stack — made structural rather than
remembered.

## Why egui is not feature-gated in `loom_render`

**1. The HUD is game content.** `crates/loom_cli/src/hud.rs:16` is
`use loom_render::egui;`; the module builds `egui::Align2`, `egui::FontId`,
`egui::Color32` and paints into a `&mut egui::Ui` at `:137`. A `Hud` is a scene
component, drawn during Play, demonstrated by `assets/games/proving_ground.loom`.
A `loom-play` that cannot draw a HUD is not a runtime build of this engine; it
is a different engine. So the shipped binary links egui *because the HUD is
egui*, and if binary size ever becomes a real number, the fix is to stop drawing
the HUD with egui — not to gate the renderer.

**2. `Viewer::draw` is a one-line wrapper around the thing the gate would
remove.** `crates/loom_render/src/viewer.rs:1041-1042`:

```rust
pub fn draw(&mut self, objects: &[Object], camera: &Camera) -> Result<(), RenderError> {
    self.draw_with_ui(objects, &[], camera, None, |_| {})
}
```

Gating out `draw_with_ui` does not remove a branch from frame drawing; it
removes the only implementation of it, and obliges someone to write a second
one. That is precisely the offscreen/window divergence ADR 0018 exists about,
and this project has paid for three times — most expensively when the viewer
rasterised at one sample while every AA measurement was taken on the offscreen
path at 4x. A `#[cfg]` on the frame loop manufactures that class of bug on
purpose, and the second implementation would be the one the *player* runs and
no gate renders.

`crates/loom_render/Cargo.toml` accordingly has no `[features]` block at all;
`egui`, `egui-ash-renderer` and `egui-winit` sit at `:11-13` beside `ash`.

There is one consequence that reaches outside this ADR and is worth stating
here because it is where the boundary gets tested: the sRGB pre-warp
`loom_render::ui::tok` lives in `loom_render`, not in `loom_editor::theme`
(which re-exports it). Its own doc comment gives the reason — `ui.rs` is linked
by the runtime as well, and putting the compensation in the editor would leave
a shipped game's HUD holding only the half of the correction that darkens it.
That is not an editor palette in the renderer, which this boundary does forbid;
it is a colour-space correction owned by the module that sets the
specialization constant.

## Why the `loom` binary carries no `required-features`

The tempting spelling is `required-features = ["editor"]` on `[[bin]] loom`, so
that a build without the editor cannot produce a binary with a dead `--edit`.
It is wrong by a wide margin: `loom validate`, `loom render`, `loom sim`,
`loom scene --tx` — the entire agent surface, and everything `cargo xtask`
drives, including all four green checks — live in that binary. Requiring the
feature makes the agent's whole tool API unbuildable without the editor UI, and
makes the gates unrunnable in exactly the configuration a headless machine would
want. The manifest comment at `crates/loom_cli/Cargo.toml:12-16` records this so
the next person to reach for `required-features` reads the reason first.

The gating that *is* wanted is per-subcommand — `#[cfg(feature = "editor")]` on
`edit` and `run --edit`, with a one-line refusal otherwise. That is not in the
source yet, and the honest statement of today's state is the next section.

## The two check-deps rules, and what each actually proves

Both live in the `loom_editor` stanza of `scripts/check-deps.sh:68-89`, in the
shape of the existing `loom_agent` rule above it (`:59-66`).

**Rule 1 — nothing but `loom_cli` may depend on `loom_editor`** (`:72-78`). It
walks every workspace crate and fails if any but `loom_editor` and `loom_cli`
lists it. This is the containment guarantee, and because egui is deliberately
*not* gated, **the whole removability claim rests on this one edge**. Without
it, the first `loom_render` or `loom_physics` file that reaches for a panel type
would make the editor unremovable while every build stayed green.

**Rule 2 — `cargo tree -p loom_cli --no-default-features -e normal` must not
mention `loom_editor` or `egui_dock`** (`:84-88`). It proves the *dependency
edge* drops out with the feature, which is what containment means at the
manifest level; it is a `cargo tree` query and needs nothing to compile.

**It does not prove `--no-default-features` builds, and today it does not.**
`crates/loom_cli/src/log.rs:19` is an unconditional
`pub use loom_editor::console::{Entry, Level};`, and `run.rs` — 2,506 lines
holding the winit loop, the camera, Play, *and* the editor's event handling —
imports `loom_editor` at the top with no `#[cfg]`. Splitting that file is the
expensive half of the split and it is deliberately deferred; **a compiling
`--no-default-features` is Stage 5's exit criterion, not this ADR's.** The
distinction is stated here rather than discovered later, because a rule that
sounds like it proves the build works and only proves the manifest is exactly
the kind of gate this repository has been fooled by before.

Note also that a green `check-deps.sh` is only meaningful because the script
guards its own preconditions: every rule is gated on a `grep` over `cargo
metadata`, so a manifest that fails to load would skip every stanza and still
print `dependency rules: ok`. That guard is at `:21-25` and it is why the two
rules above can be trusted at all.

## What this costs

- **Every shipped `loom-play` binary carries egui, `egui-ash-renderer` and
  `egui-winit`,** whether or not the game authors a HUD. That is the price of
  refusing the second frame implementation, and it is paid in bytes rather than
  in a divergence nobody can see.
- **The boundary is a rule, not a compiler error, in one direction.** The
  compiler stops `loom_editor` from calling `loom_cli`. Nothing but rule 1 stops
  a new crate from depending on `loom_editor`, and nothing but review stops an
  editor-shaped type from being added to `loom_render` — where it would be
  perfectly legal and permanently unremovable.
- **Two manifests must move together on an egui upgrade.** egui, `egui-winit`
  and `egui-ash-renderer` in `loom_render`, `egui_dock` in `loom_editor`; the
  one-line check is
  `cargo tree -p loom_editor -e normal | grep -oE "egui v[0-9.]+"` printing
  exactly one version.
- **`loom_cli` now has a feature that its own source does not honour.** Until
  Stage 5 splits `run.rs`, `--no-default-features` is a manifest configuration
  that fails to compile, and the gate that could catch that is a full build,
  which `green.sh` does not run in that configuration.

## What it forecloses

A third `loom_runtime` crate holding the nine modules both binaries need. It was
considered and is a rename with no new boundary in it — the line it would draw
is the one `loom_cli`'s feature flag already draws, and it would cost every
`use` path in the workspace. If `run.rs`'s split at Stage 5 turns out to need a
home for the shared half that is not `loom_cli`, that is the moment to
reconsider, and it is a mechanical change rather than a design one.

It also forecloses, for now, `cargo xtask docs --check` in `green.sh`: an xtask
that checked generated editor docs would make `xtask` depend on `loom_editor`
and compile the editor on every green run, which collides with rule 1's intent
even where it satisfies its letter.

## How it would be reversed

Reversing the *crate* is a `git mv` and a manifest edit; nothing in
`loom_editor` knows it is a separate crate except its own `use` lines. Reversing
the *egui decision* — gating egui out of `loom_render` — requires first removing
egui from the HUD, since the HUD is the reason the runtime links it. Do that,
and the gate becomes possible; do it without, and `hud.rs` stops compiling in
the configuration players run. The order matters and it is the whole argument.

## Sources

- `crates/loom_cli/src/hud.rs:16`, `:137` — the HUD is egui.
- `crates/loom_render/src/viewer.rs:1041-1042` — `draw` is a wrapper.
- ADR 0018, for what a second frame implementation costs this project.
- `docs/design/editor/PLAN.md` §2.1, §2.2 and §3's ADR 0022 row, which resolved
  this against editor doc 06 §1 and doc 01 §2.1.
