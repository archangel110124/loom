# ADR 0033 — The UI encodes its colours once, and the token table is pre-warped

- **Date:** 2026-08-16
- **Status:** **accepted**
- **Decision touched:** none of the locked decisions in `CLAUDE.md`. This is a
  one-line specialization constant and one small pure function; no pass, no
  descriptor, no format change.
- **Plan row:** `docs/design/editor/PLAN.md` §3's row **0033** — *"UI colour is authored in display space and encoded exactly once"*. `crates/loom_render/src/ui.rs:189` cites ADR 0033 for this decision, and agrees.

## Context — the chrome was encoded twice and nobody could see it

`egui-ash-renderer 0.12.0` applies a gamma correction in **both** shader
stages. Its vertex shader raises the vertex colour to the power 2.2
(`shader.vert:25`, `oColor = SRGBtoLINEAR(vColor)`), and its fragment shader
applies the inverse when specialization constant 0 is `false`
(`shader.frag:23`, the `if (SRGB_FRAMEBUFFER)` branch, falling through to
`LINEARtoSRGB`). `ui.rs` declared `srgb_framebuffer: false`, so the two
exponents cancel exactly and the shader pair was an **identity on the vertex
colour** — it handed the framebuffer the `Color32` byte unchanged.

The framebuffer was not expecting a byte. `create_swapchain`
(`crates/loom_render/src/viewer.rs:2308`) prefers `B8G8R8A8_SRGB` (`:2342`), and `Ui` is
constructed with `viewer.color_format()`, so the hardware then applied the sRGB
*encode* to a value that was already display-encoded. Every UI colour arrived
lifted: `#16191E`, the panel ground, reaches the display as `#535860` — the
inverse computation is one line and the arithmetic is
`255 · srgb_encode(22/255) = 83`. Doc 11 §2 works the consequence through the
palette: a `text_strong`-on-`surface` ratio designed at 14.6:1 (recomputed and
pinned by `contrast_floor_holds`, `crates/loom_editor/src/theme.rs:429`) lands
near 6.7:1 on the screen.

Nobody complained because there was no design language to notice it against.
The editor's chrome was default-egui grey, and grey lifted by a stop still
looks like grey lifted by a stop looks like grey. It became urgent only when a
palette was about to be tuned by eye on top of it, because every hex chosen
that way would have been chosen to cancel a bug and would have broken on the
day the bug was fixed.

## The decision

**`srgb_framebuffer: true` (`crates/loom_render/src/ui.rs:192`), so exactly one
encode happens — the hardware's — and every authored token is pre-warped
through `loom_render::ui::tok` (`ui.rs:50`), which is the exact inverse of what
remains.**

The pipeline after the flip is three stages and they do not cancel:

1. `shader.vert:25` raises the vertex colour to 2.2.
2. `shader.frag:23` passes the result through unchanged, because the constant
   is now `true`.
3. The `B8G8R8A8_SRGB` swapchain applies the piecewise sRGB encode.

The residue is real and it is not small. Stage 1 is the *gamma-2.2
approximation* of sRGB; stage 3 is the *piecewise* curve with its linear toe.
They agree in the highlights and diverge in the shadows, which is where a dark
editor lives. So a hex authored naively still arrives wrong — this time about
36% too dark rather than four stops too bright.

`tok` inverts that composition per channel: sRGB-decode (undoing stage 3),
then `powf(1/2.2)` (undoing stage 1), `ui.rs:55-63`. The whole function is
`ui.rs:50-67` — one closure, no state, no allocation.
`tok_round_trips_through_the_real_pipeline` (`theme.rs:461`) replays all
three stages forward on the warped byte and asserts the framebuffer lands
within one byte of what the table asked for.

### The direction of the correction is a property of the flag, not of the shader

This is the part worth writing down, because it is the trap: **under
`srgb_framebuffer: false` the correct compensation is the opposite one.** With
the constant `false` the shader pair cancels, stage 3 runs uncompensated, and
colours arrive *brighter* — `#16191E` as `#535860`. Applying `tok` in that
world would darken an already-lifted colour by the wrong curve and roughly
double the error rather than remove it. The flag and the function are two
halves of one correction and neither is meaningful alone.

### Why `tok` lives in `loom_render::ui` and not in `loom_editor::theme`

Doc 11 §2.2 wrote the function into `theme.rs`. It ships in `ui.rs`, and
`theme.rs:37` is a bare `pub use loom_render::ui::tok`.

The reason is that **`ui.rs` is linked by the runtime, not only by the
editor**. `crates/loom_cli/src/hud.rs` draws a shipped game's HUD through the
same `egui::Ui`, produced by the same `Ui` object, past the same specialization
constant. The flip in `ui.rs:192` therefore darkens the HUD of every game this
engine ships, whether or not `loom_editor` is in the binary. Putting the
compensating half in the editor crate would hand a shipped game **only the half
of the fix that darkens it**, permanently, and with no obvious place for anyone
to look. `crates/loom_render/src/lib.rs:48` makes `ui` public for exactly this
and says so in the comment above it.

That is also the whole argument against the tempting alternative of baking the
correction into the token hexes by hand: it would be invisible and unreviewable
in a table of 23 constants (`DARK`, `theme.rs:126`), and it would be
uncopyable — the HUD has no token table.

### Doc 11's stated arrival value is wrong

`docs/design/editor/11-visual-identity.md:108` says `#16191E` arrives as
`#0E0E10` without the correction. **The true value is `#0E1218`.** Doc 11
applied one channel's correction to all three; the transfer function is
per-channel and non-linear, so `0x19` and `0x1E` do not land where `0x16` does.
`without_tok_the_panel_is_a_third_too_dark` (`theme.rs:498`) is the
recomputation and asserts `[0x0E, 0x12, 0x18]` at `theme.rs:505`.

Doc 11's *magnitude* claim survives and nothing downstream of it moves:
`0x16 → 0x0E` is 36% darker, which is the "about 35%" the design argues from,
and `theme.rs:507` pins that claim rather than the three bytes, so a future
retune of `surface` does not falsify the sentence the design rests on.

## What was rejected

**Leaving the double encode and tuning the palette against it.** Every contrast
ratio in every theme document would have been fiction, and the palette would
have broken on the day someone noticed the flag. This is the version that costs
the most later and looks free today.

**A `B8G8R8A8_UNORM` swapchain.** It would also produce one encode, by removing
the hardware's. It was rejected *not* because golden references pin the
swapchain — they do not; the offscreen path never touches it — but because it
moves the encode into the tonemap on the **window path only**, which creates a
second place the window and the offscreen renderer can disagree. This project
has paid for that class of divergence three times (the viewer drawing at one
sample while every AA number was taken offscreen is the worst of them), and the
`_SRGB` swapchain keeps the two paths agreeing by construction.

**Doing the pre-warp at use sites.** `tokens()` (`theme.rs:172`) applies `tok`
once, to every field, and returns a struct that is threaded. Calling `tok` at a
call site is how half an editor ends up double-corrected, and the doc comment at
`theme.rs:169` says so. `every_token_is_warped` (`theme.rs:563`) is the guard
against a new `Tokens` field being added and left raw — one uncorrected hex
among 23 looks like a slightly wrong shade, not like a bug.

## What it costs

**The token table is not the bytes egui receives.** `DARK` is authored in the
colour intended *on the screen* (`theme.rs:122-125` says to read it that way),
so a debugger, a `Color32` printed at a breakpoint, and an egui inspector will
all disagree with the table by the warp. Anyone comparing a value from the
table against a value at runtime and expecting equality is confused, and the
only defence is the doc comment.

**The flip moves alpha compositing from gamma space into linear space.** With
the constant `false` the fragment shader was blending display-encoded values;
with it `true` the blend happens on the linear values the vertex shader
produced. That changes every α-composited surface in the interface, and it
changes egui's glyph coverage — text gets *lighter* in linear blending at the
same coverage. Nothing in this project measures text weight. It is a real look
change that was accepted on the argument that the pre-flip weight was itself an
artifact.

**No gate can see any of this, and that is not fixable from here.**
`cargo xtask image` drives the offscreen `Renderer`, which never constructs a
`Ui` — the only `Ui::new` call in the workspace is in `crates/loom_cli/src/run.rs`,
on the windowed path. Both green checks pass identically with the constant set
either way. The tests in `theme.rs` replay the three stages **arithmetically**,
which proves the algebra is an inverse and proves nothing about the pixel: they
would pass unchanged if `ui.rs:192` were flipped back tomorrow. The real check
is a human sampling swatches off a screenshot, and the plan's `--theme-probe`
that was to automate the sampling is **not in the tree**.

**The citations rot.** The shipped comment at `ui.rs:174` points at
`viewer.rs:2172` and PLAN.md §3 points at `viewer.rs:2101` for the swapchain
format; it is at `viewer.rs:2342` today, and `ui.rs:88` in both documents is now
`ui.rs:192`. The vendored shader references (`shader.vert:25`, `shader.frag:23`)
are pinned by an exact dependency version and are the stable half. Any move of
this correction should re-check the line numbers rather than copy them.

## How it would be reversed

Set `srgb_framebuffer` back to `false` at `crates/loom_render/src/ui.rs:192`
**and delete `tok`'s body in the same commit**, replacing it with the identity.
The two must move together — leaving `tok` in place under `false` is strictly
worse than the original bug, because it adds a darkening to a lifting. Then
re-tune `DARK`, because every hex in it was chosen against a display that
encodes once.

Adopting a `UNORM` swapchain instead is a larger reversal: `tok` becomes the
identity, the tonemap acquires the encode, and the offscreen path needs the
same encode added at the same point or the two paths diverge. The mechanism is
recorded here so that cost is visible before anyone starts.
