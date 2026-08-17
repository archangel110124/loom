# The Loom editor's visual identity

*Round 2. This document is the implementable form of `PLAN.md` §2.7 (S11) and of ADR 0030's
theme half. It does not re-litigate which of doc 01 §6.1 and doc 07 §10 wins — `PLAN.md` already
ruled — it merges the ruling into one table of values an implementer can type, and it corrects
three things in the ruled palette that do not survive being checked.*

*Design phase. **No `cargo` command was run.** Every egui API in §11 was read from
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/{egui,epaint,emath}-0.35.0/` and every
`file:line` in the engine was read in this worktree at `62f9ebe`. §13 lists what a compiler or a
GPU would have to settle.*

---

## 1. The idea, in one sentence

> **The chrome is greyscale sheets separated by hairlines; every colour is data; and every state
> that matters is carried on an *edge*, never on a fill.**

The first two clauses are doc 07 §10's rule, which `PLAN.md` correctly called the strongest single
idea in either theme document. The third is this document's contribution and it is what makes the
identity Loom's rather than a well-behaved dark theme.

**Call it the warp.** On a loom the warp is the set of threads held under tension before any
picture exists — the structure the image is made *on*, not part of the image. That is exactly what
an editor's chrome is to a scene, and it is the honest reading of this engine's own model: the
`.loom` file is the structure, the render is the picture. So the interface's one recurring mark is
a **2 px thread along one edge** of whatever has state:

| Thread | Edge | Means |
| --- | --- | --- |
| `accent` | top of a tab | this tab is active |
| `accent` | left of a row | this node is selected |
| `accent` | left of a field | this value is an unsaved change, or a prefab override |
| `agent` | left of a row | somebody else wrote this, recently |
| `error` / `warn` | left of a row | this row is the problem |

One motif, five meanings, one `rect_filled` each. It is cheaper in coloured pixels than Unity's
solid selection fills and Blender's outline-plus-fill, which is what lets "every colour is data"
survive contact with the two-hundredth widget: **a fill puts a thousand coloured pixels on screen
to say one thing; a thread puts forty.** And it composes — a selected node the agent just touched
shows both threads, stacked, without either winning.

The second half of the identity is that **surface separation is carried by hairlines and not by
luminance.** `raised` against `surface` is 1.1:1, which is invisible on its own and deliberately
so: a 1 px rule at a boundary reads as a drawn edge where a luminance step reads as a smudge. The
whole window is therefore very flat, with high-contrast text and a handful of threads sitting on
it. That is the intended reading — the chrome recedes and the scene comes forward — and §4 names
the one accommodation for when it recedes too far.

---

## 2. Before any hex means anything: the UI encodes its colours twice

**Verified from source, not measured, and it invalidates every contrast number in both round-1
theme documents.** `PLAN.md` R7 flagged this as a suspicion. It is not a suspicion; the shaders
are readable and I read them.

`crates/loom_render/src/ui.rs:88` sets `srgb_framebuffer: false`. That is a specialization constant
on `egui-ash-renderer 0.12.0`'s fragment shader
(`src/renderer/vulkan.rs:78-88`, `constant_id: 0`). The two shaders ship as source in the crate:

```glsl
// shaders/shader.vert:25
oColor = SRGBtoLINEAR(vColor);          // pow(c, 2.2)

// shaders/shader.frag:23-27
if (SRGB_FRAMEBUFFER) { finalColor = oColor * texture(...); }
else                  { finalColor = LINEARtoSRGB(oColor * texture(...)); }   // pow(c, 1/2.2)
```

With the constant `false` the two exponents cancel exactly, so the shader pair is an **identity on
the vertex colour** — it emits the `Color32` byte value unchanged. `viewer.rs:2101-2102` then
prefers a `B8G8R8A8_SRGB` swapchain, and `Ui` is constructed with `viewer.color_format()`
(`run.rs:799`), so the hardware applies the sRGB *encode* to a value that was already encoded.

**The magnitude is not subtle.** Displayed byte = `255 · srgb_encode(c/255)`:

```
authored  #16191E (22,25,30)   ->  displays as roughly (83,88,96)  #535860
authored  #E6EAF0 (230,234,240) ->  displays as roughly (244,246,250)
```

The panel ground lifts from near-black to mid-slate while the text barely moves, so the designed
14.6:1 of `text_strong` on `surface` arrives on the display as **6.7:1**. Nobody has complained
because there is no design language yet to notice it against; a deliberate dark palette notices on
the first frame, and every contrast number in doc 01 §6.1 and doc 07 §10 is currently fiction.

### The fix, and the residue it leaves

**`ui.rs:88` becomes `srgb_framebuffer: true`.** One line, in Stage 0, and it makes the fragment
shader skip its encode so the hardware does the only one.

That is not quite exact either, because the vertex shader's `pow(c, 2.2)` is the gamma-2.2
approximation of sRGB while the hardware decodes/encodes with the piecewise sRGB curve. The two
agree in the highlights and diverge in the toe:

```
authored 230 -> displays 231       authored 22 -> displays 14
```

Deep chrome comes out about 35% darker than authored, in the safe direction (more contrast, not
less), but it means a hex in a table still is not what a colour picker on the screenshot reports.
So `theme.rs` carries one nine-line function and every token goes through it:

```rust
/// The token table is written in the colour we want on the *screen*. egui's vertex
/// shader raises it to 2.2 and the sRGB swapchain encodes the result, and those two
/// are not inverses in the toe — a #16191E panel arrives as #0E0E10. This pre-warps
/// each channel so that what the table says is what the display shows.
/// Verified against `egui-ash-renderer-0.12.0/src/shaders/shader.vert:25` and the
/// swapchain format chosen at `viewer.rs:2101`. Measured, not assumed, by the Stage 0
/// swatch probe.
fn tok(hex: u32) -> egui::Color32 {
    let ch = |b: u32| {
        let s = (b & 0xFF) as f32 / 255.0;
        let linear = if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) };
        (linear.powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8
    };
    egui::Color32::from_rgb(ch(hex >> 16), ch(hex >> 8), ch(hex))
}
```

**The probe that proves it, in Stage 0, before any hex is tuned.** `loom edit --theme-probe` (about
fifteen lines in `loom_editor`) fills the window with a strip of the sixteen tokens and a
0/25/50/75/100% grey ramp, at full opacity, over `chrome_clear`. The human screenshots it with any
tool and samples three swatches. If the sampled bytes equal the table's hexes within ±2, `tok` is
right and every ratio in §3 is a real ratio. **This is the only way to verify a colour system in
this project** — the golden gate cannot see it, because `cargo xtask image` drives `loom render`,
which is the offscreen `Renderer` and never constructs a `Ui`.

**This is ADR 0034** (§12). It is ADR-shaped rather than a bug fix because `ui.rs` is shared with
the HUD, which is game content that ships to players, and because `tok` is a non-obvious
correction that the next reader will otherwise delete as superstition.

---

## 3. The colour system

Sixteen chrome tokens and nine data tokens. **Every value below is a screen-space sRGB hex passed
through `tok`.** Contrast is WCAG relative luminance against `surface`, recomputed here rather than
copied — I checked doc 01 §6.1's arithmetic on four rows and it is sound, so the rows it stated are
reproduced with its numbers and the new rows carry mine.

### Ground and surfaces

| Token | Hex | egui | Where |
| --- | --- | --- | --- |
| `ground` | `#0E1013` | — (painted) | behind the dock, the gutters `egui_dock` leaves between panels, viewport letterbox, viewport with no scene, viewport-overlay text chips |
| `surface` | `#16191E` | `panel_fill` | every panel body. The default background of the application |
| `raised` | `#1E232A` | `window_fill`, tab bar, toolbar, menu bar, table header | anything that sits on top of a panel |
| `sunken` | `#0F1216` | `extreme_bg_color` | text fields, the console body, code blocks, the value track of a slider |
| `hover` | `#262C35` | `widgets.hovered.weak_bg_fill` | a hovered row or button |
| `press` | `#2F3742` | `widgets.active.weak_bg_fill`, `widgets.open.weak_bg_fill` | a pressed button, an open menu's button |
| `line` | `#2E3540` | `widgets.noninteractive.bg_stroke` | every 1 px separator and panel edge. **1.43:1 — decorative** |
| `line_strong` | `#414A58` | `widgets.inactive.bg_stroke` | control borders, the splitter while dragged. **1.97:1 — decorative** |

**Three corrections to the palette `PLAN.md` §2.7 ruled in, and they are the reason to check
arithmetic rather than inherit it.** Doc 01 §6.1 gives `line = #262C35`, which is **1.25:1** against
`surface` — below the threshold at which a 1 px rule is visible at all, which would delete the one
mechanism the whole surface strategy rests on. It is lightened to `#2E3540`. Doc 01 also gives
`line` and `bg_hover` the same value, so a hovered row would be exactly the colour of the rules
around it; `hover` keeps `#262C35` and `line` moves, which separates them. And doc 01 assigns
`line_strong` to the *focused field border*: at 1.97:1 that fails WCAG's 3:1 for a meaningful
non-text indicator, so **the focus ring is always `accent`** (6.47:1) and `line_strong` is demoted
to decoration. Hairlines in a dark UI genuinely live at 1.4–2:1 — a 1 px mark is detected by edge
contrast, not area contrast — but a focus ring is not a hairline, and conflating them is how a
keyboard user loses the caret.

### Text

| Token | Hex | Contrast | Where |
| --- | --- | --- | --- |
| `text_strong` | `#E6EAF0` | **14.60:1** | headings, tab labels, the selected row, a value being edited, viewport chips |
| `text` | `#C3CAD4` | **10.67:1** | body — every control label and list row |
| `text_weak` | `#7C8794` | **4.83:1** | field labels, units, counts, secondary paths, placeholder text |
| `text_disabled` | `#4C5561` | 2.29:1 | `add_enabled(false)`. **Exempt, and therefore never the only carrier of meaning** — a disabled command also shows its reason in the tooltip (ADR 0031 already requires this) |

`text_weak` at 4.83:1 clears 4.5:1 by a hair. **It may not be darkened without recomputing**, and
that sentence belongs in the doc comment above the constant.

### Accent — warp violet

| Token | Hex | Contrast | Where |
| --- | --- | --- | --- |
| `accent` | `#A78BFA` | **6.47:1** | every thread, the focus ring, the active tool, links, a slider's filled track, the selection outline's core in the viewport |
| `accent_deep` | `#6E5BC4` | — | the selection *fill* behind a row, at α90. Composited over `surface` it is `#353058`, on which `text_strong` reads **10.14:1** |

**The accent is violet at ~260°, and the argument is spatial rather than aesthetic.** Every hue this
editor already spends means something: warm red, green and blue are the three gizmo axes
(`panels.rs:95-99`, unchanged since M12), cyan is the agent, red is error, amber is warning, green
is ok. Violet is the furthest unclaimed hue from all of them. The default choice for a dark tool is
blue — and blue is the Z axis, so a blue selection highlight in a 3D viewport is a selection a user
can misread as a depth handle. That is a real error, not a quibble.

Warp violet is also the only token that appears in both the chrome and the viewport, which is
deliberate: it is the colour of *your* attention, and your attention is the one thing that spans
both.

### Semantic data colours

| Token | Hex | Contrast | Means, everywhere, and nothing else |
| --- | --- | --- | --- |
| `ok` | `#6FCF97` | 9.3:1 | validation passed, an assertion held, a build succeeded, a brush that adds |
| `warn` | `#E8B84B` | 9.6:1 | a physical-sanity finding, "this is two undo steps", a degenerate-UV refusal |
| `error` | `#F0736D` | 6.2:1 | a parse failure, a rejected transaction, a stale version token, a brush that subtracts |
| `agent` | `#78C8FF` | 9.7:1 | somebody else wrote this. **Unchanged** — it is `panels.rs:679`'s existing `(120,200,255)` and a meaning a user has already learned |
| `axis_x` | `#E2544F` | 4.7:1 | X, everywhere, forever |
| `axis_y` | `#7CC860` | 8.6:1 | Y |
| `axis_z` | `#5494E8` | 5.7:1 | Z |
| `chrome_casing` | `#0A0C0F` | — | the dark under-stroke every viewport mark is drawn on (§7) |
| `chrome_core` | `#F2F5FA` | — | the bright over-stroke for viewport marks that carry no semantic (grid, hover bracket) |

`axis_x` at 4.71:1 is the other row that only just clears, for the same reason and with the same
prohibition.

### The four states the task asked for, and where they actually live

**`selected` is `accent`**, as a 2 px left thread plus an `accent_deep` α90 row fill. **`hovered` is
`hover`**, a fill with no thread — hover is not a state you can act on later, so it earns no
colour. **`dirty` is `accent`**, and it never contends with selection because the two occupy
disjoint surfaces: dirty appears only on tab titles, the status bar and the save affordance;
selection appears only on rows and in the viewport. That partition is what lets the whole editor
run on six hues instead of seven, and adding a seventh is exactly the token creep that makes a
design system stop being followed.

**`agent`-authored versus human-authored is the one the task asks for that cannot be given
honestly, and pretending otherwise would be the worst kind of decoration.** Verified:
`loom_scene::ops::Transaction` (`ops.rs:102-114`) carries `label`, `ops`, `dry_run` and
`expect_version` — **no author.** The `.loom` file records no authorship either. What the editor
actually has is `run.rs:426-465`'s `agent_marks`: a session-local list of nodes whose *content
changed under a reload the editor did not cause*, decayed over `CHANGE_FADE` seconds. That is
recency, and it is inferred, and it is gone when you restart.

So the rule is: **the agent colour marks a recent write, never an owner.** A node the agent created
last week is drawn exactly like one you created last week, because that is the truth. Human
authorship gets **no colour at all** — the default `text` — which is correct on its own terms: you
do not need to be told which parts of your file are yours. §12 drafts **ADR 0033** to record this,
because a future reader will otherwise implement a persistent "agent-owned" tint and quietly make
the scene format carry provenance it was never designed to carry.

### There is no light theme

**One theme, dark, and the reason is not preference.** The editor's main content is a lit 3D scene
whose average luminance this engine controls and the chrome does not; `cave` and every night scene
sit near black, and a light chrome around them makes the viewport read as a hole. A second palette
is also a second set of contrast checks, a second `Visuals`, and a second thing that goes stale on
the third feature.

The genuine accessibility need underneath "we should have a light theme" is *contrast*, not
*polarity*, and it is met by two things that already have homes in `PLAN.md` S9's `prefs.toml`:

**Zoom.** `Context::set_zoom_factor` (`context.rs:2269`) scales points, so every size in §5 and §6
scales together with no second table. Ctrl+`+` / Ctrl+`-` / Ctrl+`0`, persisted.

**High contrast, five tokens.** A `bool` in `prefs.toml` that swaps exactly:

```
surface       #16191E -> #000000
raised        #1E232A -> #0C0C0C
line          #2E3540 -> #6B7484     (5.4:1 — the rules become structure rather than decoration)
text_weak     #7C8794 -> #C3CAD4     (= text; the weak tier disappears)
text_disabled #4C5561 -> #6B7484     (readable, 5.4:1)
```

Ten lines in `theme.rs`, no second `Visuals`, and it is the accommodation that gets used. If a
light theme is ever genuinely wanted it is a third `const` block behind the same `tok`, which is
the entire point of having a token table rather than literals.

---

## 4. Type

`Style::text_styles` is a `BTreeMap<TextStyle, FontId>` (`style.rs:261-288`). The scale is doc 07
§10's, which `PLAN.md` ruled in because each size has a stated reason, with doc 01 §6.2's monospace
numerics — the highest-value line in either document — and two corrections.

| `TextStyle` | Size | Family | Where |
| --- | --- | --- | --- |
| `Small` | 11.0 | Proportional | axis letters, badge counts, the fps/nodes/draws readout, tooltip second lines |
| `Body` | 13.0 | Proportional | **the default.** Every control, every inspector row, every list row |
| `Button` | 13.0 | Proportional | same, so a button never sits a half-pixel off the label beside it |
| `Monospace` | 12.0 | Monospace | paths, version tokens, transaction labels, console output, **and every numeric field** |
| `Heading` | 15.0 | Proportional | dock tab labels, inspector component headers, dialog section titles |
| `Name("Title")` | 18.0 | Proportional | dialog titles, project names in the Hub |
| `Name("Display")` | 24.0 | Proportional | the Hub headline, and nothing else in the application |

**Numeric fields use the monospace family, and this costs one line and fixes the worst thing about
the current inspector.** A column of `DragValue`s in a proportional font jitters horizontally as
digits change under a drag, and a transform inspector is nine of them side by side. It is also why
the identity does not need a font with tabular figures: `epaint 0.35` exposes no OpenType feature
control, so `tnum` is unreachable and a monospace family is the only mechanism available.

**Monospace is 12.0 against a 13.0 body, not 13.0.** Doc 07's table says 13 for both; Hack's
x-height runs visibly larger than Ubuntu-Light's at the same pixel size, so 13/13 makes every path
and every number look one step bigger than the label beside it. 12.0 optically matches. Re-check on
any font swap — it is a metric of the pair, not a constant.

**Doc 07's "line height 1.35" is not implementable as a global rule and I checked.** There is no
line-height field on `FontId`, `FontTweak` or `Style`; `TextFormat::line_height: Option<f32>` exists
(`epaint/src/text/text_layout_types.rs:486`) and `RichText::line_height` sets it
(`egui/src/widget_text.rs:174`), but only per text run. So: **single-line rows get their pitch from
`interact_size.y` + `item_spacing.y` (§5), and 1.35 is applied via `RichText::line_height` on the
four multi-line surfaces only** — the console, tooltips, F1 help popovers, and the divergence
banner's body. Stating it as a global rule would have sent an implementer looking for a setting
that does not exist.

### The font decision, and the gap the sequencing leaves

`PLAN.md` §2.7 and ADR 0030 rule for doc 01's sequencing: **ship on egui's bundled fonts, and adopt
Inter only if the human still reads the result as default egui after the palette, spacing and
radius land.** That is right — a font is a new binary asset class and a licence entry, and it
should be spent only if the cheaper change did not work. Verified, the bundled set is
`Ubuntu-Light`, `Hack`, `NotoEmoji-Regular` and `emoji-icon-font`
(`epaint/src/text/fonts.rs:508-561`).

**The consequence nobody stated: the bundled set has exactly one weight.** Doc 07's scale asks for
SemiBold at 15, 18 and 24, and Ubuntu-**Light** has no bold companion registered. So on the bundled
fonts the type scale ships *weightless*, and headings are differentiated by **size and
`text_strong`** alone. That works — it is how the whole flat-chrome strategy works — but an
implementer who reads "15 SemiBold" and finds no way to set weight will invent something. Write it
down: **there is no bold in slice one; the weight column activates with the font swap or not at
all.**

If the swap is taken, it is pinned now so that taking it costs a copy rather than a fresh decision:

| | |
| --- | --- |
| Proportional | **Inter**, Regular + SemiBold, from the latest tagged release of `github.com/rsms/inter` |
| Monospace | **JetBrains Mono**, Regular, from the latest tagged release of `github.com/JetBrains/JetBrainsMono` |
| Licence | Both **SIL Open Font License 1.1**. The archives' `OFL.txt` is copied verbatim to `assets/fonts/OFL.txt` |
| Provenance | `assets/fonts/SOURCES.txt` records, per file, the release tag, the URL and the `sha256sum` at the moment it was vendored. **A font file with no recorded provenance is a licence problem waiting to be someone else's** |
| Loaded by | `loom_editor`, through `Context::set_fonts` (`context.rs:2038`) with a `FontDefinitions` whose `families` lists Inter first and keeps `NotoEmoji-Regular` and `emoji-icon-font` as fallbacks — dropping them loses every emoji glyph the console already prints |
| Size | ~4 files, roughly 600 KB, `include_bytes!` |

**Fonts are editor-only, and that is a decision rather than an oversight.** `loom-play` does not link
`loom_editor` (ADR 0022), so a shipped game's HUD keeps egui's bundled fonts and will not match the
editor's. That is correct: the HUD is *game content* (`hud.rs`, a scene component), and a game's
font is the game's choice. If a scene ever wants to choose one it is a `Hud` component field and a
`[[asset]]` of kind `font`, which is a different design and not an identity concern.

---

## 5. Space and shape

**A 4 px base unit used at 4 / 8 / 12 / 16 / 24, and no other value appears anywhere.** Five values
is a scale a single developer can hold; a scale of eleven is one that gets rounded by eye at the
third feature.

Against `Style::spacing` (`Spacing`, field names verified at `style.rs:384-462`):

```rust
spacing.item_spacing   = vec2(8.0, 4.0);
spacing.button_padding = vec2(8.0, 4.0);
spacing.window_margin  = Margin::same(8);      // Margin is i8 in 0.35, not f32
spacing.menu_margin    = Margin::same(6);
spacing.indent         = 14.0;
spacing.interact_size  = vec2(56.0, 22.0);
spacing.icon_width     = 14.0;
spacing.icon_width_inner = 8.0;
spacing.icon_spacing   = 6.0;
spacing.slider_width   = 120.0;
spacing.combo_width    = 120.0;
spacing.tooltip_width  = 320.0;
spacing.menu_width     = 220.0;
spacing.scroll.bar_width = 8.0;                // thin, and only over the content
```

**`interact_size.y = 22.0` against egui's default 18.0 is the single change that most makes this
read as an application rather than a debug overlay.** Rows get room. With `item_spacing.y = 4.0` a
list row pitches at 26 px, and 22 px of control at 13 px text on a 26 px pitch is the density
Unity and Blender both converge on from opposite directions.

Fixed heights, which are the only numbers outside the scale and are stated once here so nobody
guesses:

```
menu bar     26      toolbar      34      status bar   22
tab strip    26      panel toolbar row 24
dock gutter   4      (painted `ground`, and it is what separates two panels)
inspector label column  96      (fixed; §6)
icon box     16      inside a 22-high control
```

**Radius — `CornerRadius`, `u8` per corner in 0.35 (`epaint/src/corner_radius.rs:13-25`), not
`Rounding`:**

```
widgets (buttons, fields, combos, checkboxes)  4
windows, menus, popovers, tooltips             6
panels, tab strip, toolbar, status bar         0
viewport chips                                 3
```

**Flat edges read as chrome and rounded edges read as content**, and that distinction is what makes
a docked layout parse at a glance. It is also structural: a rounded panel corner inside a dock
leaves a notch that shows `ground` through it, which looks like a rendering bug.

**Shadows: `window_shadow` and `popup_shadow` only.** `Shadow { offset: [0, 4], blur: 16, spread: 0,
color: ground @ α160 }` for windows, and the same at `blur: 10` for popups. **No shadow on any
docked panel** — a docked panel has no elevation to express, and giving it one is how a dock starts
looking like a pile of cards.

---

## 6. How a panel is composed, so that ten panels look like one family

**There is no per-panel title header, and rejecting it is the decision that makes the family work.**
The obvious composition — every panel opens with a 24 px header carrying its name — is what a
floating-window UI needs and a docked one does not: `egui_dock`'s tab strip already names the
panel, so a header repeats it, and ten panels × 24 px of repetition is a lost inspector section. The
tab strip *is* the header.

Every dockable panel is therefore exactly this, and the family comes from the parts being identical
rather than from a decoration being shared:

```
┌─ tab strip (egui_dock, 26, `raised`, 2px `accent` thread on the active tab's top edge) ─┐
├─ toolbar row (24, `raised`, hairline below) — PRESENT ONLY IF the panel has controls ───┤
│  [icon 16] [mode toggles] ················flex················ [count `text_weak`] [⌕]  │
├─ body (`surface`, Frame::inner_margin = Margin::same(8), scrolls) ──────────────────────┤
│                                                                                         │
├─ footer (22, `raised`, hairline above) — PRESENT ONLY IF the panel has a summary ───────┤
│  8 nodes · 2 hidden · Show all                                                          │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

Which panels get which: Hierarchy (toolbar: filter; footer: node count + the hidden-set bar
`PLAN.md` §2.6 requires), Inspector (neither — the component headers are the structure), Project
(both), Console (toolbar: level filters + Clear; no footer), Problems (footer: counts by severity),
History and Transactions (footer: position in the stack), Prefabs (toolbar: filter), Agent (footer:
the input row), Scene and Game (neither; §7 is their chrome).

**Separators.** `widgets.noninteractive.bg_stroke = Stroke::new(1.0, line)`, which is what
`egui::Separator` paints. Inside a panel body a section break is a full-bleed hairline with 12 px
above and 8 px below — asymmetric on purpose, so the rule attaches to the section it opens.
`Visuals::indent_has_left_vline = true` with `line`, because a tree with no rails is unreadable at
depth 4.

**The inspector's label column is fixed at 96 px.** Every control in the panel starts at the same x.
This is the single most visible "designed rather than assembled" cue in the whole interface and it
costs one constant; doc 07 §10 is right about it and it is repeated here because it is the kind of
thing that gets lost between documents. Labels longer than the column ellipsize, with the full text
plus the schema doc comment in the tooltip — which already exists, because `TypeRegistry::describe`
is where the tooltip comes from.

**Component headers** are `Heading` at `text_strong` on a `raised` band with a 4 px radius on the
top two corners only (`CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }`), a chevron icon, and the
component's schema root description as the tooltip. A field the instance overrides carries the
`accent` thread on its left edge; that is the same thread as "unsaved", and it is the same meaning —
*this differs from what it would otherwise be* — which is why it does not need its own colour.

**Every panel defines an empty state, and this is where user decision 7 is actually paid for.** A
stranger meets an empty editor before they meet any feature. The block is always: a 32 px icon in
`text_disabled`, one line of `Body`/`text_weak` saying what the panel is for in plain words, and
exactly one button that does the thing:

```
Hierarchy      "This scene has no nodes yet."               [ Create ▾ ]
Inspector      "Select a node to edit it."                  (no button)
Project        "No project open — this is a single scene."  [ Create a project… ]
Console        "Nothing logged yet."                        (no button)
Problems       "No problems. Validation is clean."          (no button, `ok` icon)
History        "Nothing to undo yet."                       (no button)
Transactions   "No transactions this session."              (no button)
Prefabs        "No prefabs yet — make one from a selection." [ Create prefab… ]
Agent          "Ask for a change and watch it land."        (the input row, focused)
Scene          "No scene open."                             [ Open… ] [ New scene ]
```

Note what "Problems" says when it is empty. *"No problems"* with an `ok` icon is a different
sentence from a blank panel, and it is the one that tells a stranger the panel is working.

---

## 7. Viewport chrome that survives a snowfield and a night scene

This is the hardest constraint in the identity, because the background is literally anything this
engine renders — `meadow` in daylight, `cave` at near-zero, a squall, `puddles` with a specular
highlight through it. A single-colour outline is legible on one of those and invisible on another.

> **Every mark drawn over the scene is drawn twice: a 3.0 px casing in `chrome_casing` at α200,
> then the mark itself at 1.5 px on top.**

The casing carries the edge against a snowfield; the core carries it against a night scene; against
anything in between both contribute. It costs one extra `Painter::line_segment` per segment and it
is one helper — `overlay::stroked(painter, points, color)` — that every viewport mark goes through,
so the rule is enforced by there being no other way to draw.

Rejected, with reasons: **inverting against the scene** needs a read of the colour image, which an
egui overlay drawn after the tonemap cannot do without a sampler and a descriptor set (and would
land us in ADR 0025's rejected texture path); **an XOR or difference blend** is unavailable — egui
0.35 has no per-shape blend mode; **animated dashes** put motion in a still frame, which is noise
in a tool whose subject is motion, and it would make every screenshot the human takes of a bug
disagree with the next one.

**The existing agent overlay violates this today** and it is a real defect, not a hypothetical:
`panels.rs:680` paints a bare 1.5 px `(120,200,255)` stroke and `:686` paints bare text. On a bright
render both vanish. Routing it through `stroked` and putting its label in a chip is part of Stage 4.

### The marks, concretely

**Selection is eight corner brackets, not a wireframe box.** The node's projected AABB, with each
of the twelve edges drawn only for the first and last 12% of its length, casing + `accent` core.
A full box occludes the thing it selects and reads as a bounds gizmo; brackets read as *selected*
and leave the subject visible. There is no silhouette outline because there is no ID buffer, and
adding one is a pass, a pipeline and a golden scene for an outline. With more than one node
selected, each gets brackets and the union gets one 1 px full box in `accent` at α90 — so
"six things" and "one big thing" are never confused.

**Hover** is one bracket set at α110 in `chrome_core`, never `accent`. Hover is not a state you can
act on later and does not earn the attention colour.

**The gizmo** keeps `axis_x/y/z` unchanged, at 2.0 px core over the casing. The axis under the
cursor or being dragged goes to α255 and lifts 20% toward `text_strong`; the other two drop to α90.
Plane handles are filled quads in the plane's two axis colours mixed 50/50, at α60, with a 1.5 px
cased outline. The live numeric readout during a drag is `Monospace` 12 `text_strong` in a chip,
anchored 12 px right of the handle's tip.

**The grid is bounded, faint, and tool-scoped, and that is the honest design rather than a
compromise.** It is a 20 m × 20 m lattice on the active snap plane, 1 m minor / 5 m major, drawn as
`Painter` lines projected through `gizmo::View::project` — the same projection picking uses, so it
cannot drift from the thing it is a guide for. Alpha falls to zero at the patch edge over the outer
20%, and scales with `|dot(view_dir, plane_normal)|` so it disappears at grazing angles instead of
becoming a moiré band. Minor lines are `chrome_core` α40 with no casing; major are α70 with a 2 px
`chrome_casing` α120 casing; the two lines through the origin are `axis_x` and `axis_z` at α110.
**It appears only while a snapping tool is active** (Move, Create, Place) or when `G` toggles it.

The reason it is not an always-on infinite ground grid: an egui overlay is not depth-tested, so an
infinite grid would draw *through* every hill in the scene. A depth-correct grid is a shader, a
pipeline, a pass and a golden scene — for scenery. **Bounded and tool-scoped is not the cheap
version of an infinite grid; it is the correct one**, because the grid is a snapping affordance and
a snapping affordance that persists when nothing snaps is furniture.

**The brush cursor** (Stage 6 onward) is a 32-segment ring of the brush radius, projected onto the
surface under the cursor through the same `View`, cased, with an inner ring at `hardness × radius`
in the same colour at α110 — the only honest way to show hardness, since a hardness slider tells
you a number and nothing about what it will do. The core colour is the operation, not the tool:
`ok` for add, `error` for subtract or erase, `accent` for paint and splat. The radius in metres
rides in a chip at the ring's 3-o'clock. **Radius is world metres everywhere** (ADR 0027), so the
ring genuinely shrinks with distance and that is the feature, not a bug to correct.

**Text over the scene is always in a chip**, never bare and never haloed: `ground` at α220,
`CornerRadius::same(3)`, 4 px horizontal and 2 px vertical padding, `text_strong`. A halo needs four
offset draws and still fails on a mid-grey background; a chip is one `rect_filled` and cannot fail.

**None of this touches a gate**, and it is worth saying so: `cargo xtask image`, `flythrough` and
`shimmer` all drive `loom render`, which is the offscreen `Renderer` and never constructs a `Ui`.
The viewport chrome is exercised only by `xtask validate`'s windowed `loom run --edit --frames`
half — which does run it under the validation layers, so a `Painter` that allocates an unbounded
shape list still gets caught.

---

## 8. Motion

**`Style::animation_time = 0.12`, and `Context::animate_bool_with_time_and_easing` with
`emath::easing::cubic_out`** (verified: `context.rs:3103-3108`, `emath/src/easing.rs:57`). 120 ms
out-easing on hover, press, tab change, collapsing headers, the panel-toolbar filter opening, and
the row thread appearing.

**Zero milliseconds for anything a drag or the simulation drives.** The gizmo handles, every
`DragValue`, the brush ring, the viewport image itself, and the position of an agent mark are
instantaneous. A value that eases toward its target is a value you cannot trust while scrubbing,
and in an editor whose selling point is a deterministic simulation that is worse than merely
annoying — it makes the tool look like it is lying.

**Two deliberate exceptions to "everything animates at 120 ms".** The agent-change fade keeps its
existing six-second linear decay (`Editor::CHANGE_FADE`) — it is a decay, not a transition, and
easing it would make recent and old changes look alike in the middle. And **the divergence banner
does not animate at all**: it must not read as a toast that will go away on its own, because the
one thing a version-token rejection must not look like is transient.

**Reduce motion is a `bool` in `prefs.toml`, not an egui setting** — I checked, `egui::Options` has
no `reduce_motion` in 0.35. It sets `animation_time = 0.0` and turns the agent decay into a step at
six seconds, which keeps the *information* (this is recent) while removing the *movement*.

---

## 9. Every control, in every state

Five `WidgetVisuals` do most of the work, which is the point of expressing the identity through
egui's own model rather than around it. `Widgets::style()` (`style.rs:1266-1279`) routes: pressed
**or focused** → `active`, hovered → `hovered`, open menu → `open`, else `inactive`.

| | `bg_fill` | `weak_bg_fill` | `bg_stroke` | `fg_stroke` | radius | expansion |
| --- | --- | --- | --- | --- | --- | --- |
| `noninteractive` | `surface` | `surface` | 1.0 `line` | 1.0 `text` | 4 | 0.0 |
| `inactive` | `raised` | `raised` | 1.0 `line_strong` | 1.0 `text` | 4 | 0.0 |
| `hovered` | `hover` | `hover` | 1.0 `line_strong` | 1.0 `text_strong` | 4 | 0.0 |
| `active` | `press` | `press` | **2.0 `accent`** | 1.0 `text_strong` | 4 | 0.0 |
| `open` | `press` | `press` | 1.0 `line_strong` | 1.0 `text_strong` | 4 | 0.0 |

**`expansion = 0.0` on every row, against egui's defaults.** A widget that grows on hover breaks the
alignment of the column it sits in, and a 96 px label column exists precisely so that column holds.

**The focus ring is `active.bg_stroke` at 2.0 px `accent`**, which means a pressed button also shows
it. That reads correctly — both are "this is the thing responding to you" — and it is the only way
to get a focus ring out of egui 0.35 without wrapping every widget.

Plus, set on `Visuals` directly:

```rust
selection.bg_fill = accent_deep @ α90;   selection.stroke = Stroke::new(1.0, text_strong);
hyperlink_color   = accent;              faint_bg_color   = raised @ α80;   // striped grids
extreme_bg_color  = sunken;              code_bg_color    = sunken;
warn_fg_color     = warn;                error_fg_color   = error;
window_fill       = raised;              window_stroke    = Stroke::new(1.0, line_strong);
panel_fill        = surface;             disabled_alpha   = 0.45;
weak_text_color   = Some(text_weak);     button_frame     = true;
striped = true;  slider_trailing_fill = true;  indent_has_left_vline = true;
handle_shape = HandleShape::Rect { aspect_ratio: 0.4 };
interact_cursor = Some(CursorIcon::PointingHand);
window_corner_radius = 6.into();  menu_corner_radius = 6.into();
```

`slider_trailing_fill = true` with `accent` is what makes a slider read as a value rather than as a
knob on a rail, and it is off by default in egui.

The exceptions the `Widgets` table cannot express, which are therefore the whole of what a panel
author has to remember:

| Control | Rest | Hover | Active / focused | Disabled | Selected |
| --- | --- | --- | --- | --- | --- |
| **List / tree row** | transparent, `text` | `hover` fill | — | `text_disabled` | `accent_deep` α90 fill + 2 px `accent` left thread + `text_strong` |
| **Dock tab** | `raised`, `text_weak`, radius 0 | `hover`, `text` | — | — | `surface` fill (continuous with the body below it, so tab and content read as one sheet) + 2 px `accent` top thread + `text_strong` |
| **Toolbar tool button** | transparent, icon in `text` | `hover` | `press` | `disabled_alpha` | `accent_deep` α90 + icon in `accent` |
| **Text field** | `sunken`, 1 px `line_strong` | 1 px `line` brighter | **2 px `accent`** border | `sunken`, `text_disabled` | — |
| **`DragValue`** | `sunken`, `Monospace` 12 | `hover`, `↔` cursor | 2 px `accent`, value in `text_strong` | `text_disabled` | — |
| **Checkbox / radio** | `sunken` box, 1 px `line_strong` | `hover` box | 2 px `accent` | α45 | box `accent`, mark `ground` |
| **Menu item** | transparent, `text` | `hover` full-bleed | — | `text_disabled` + reason in the tooltip | — |
| **Link** | `accent`, no underline | `accent` + underline | — | — | — |

**A disabled command is shown, greyed, with its reason** — ADR 0031 already decided this and it is
repeated here because the visual half is where it gets dropped. Hiding an unavailable command is
how a stranger concludes the feature does not exist.

---

## 10. The default layout

```
┌────────────────────────────────────────────────────────────────────────────────────────────┐
│ Loom   File  Edit  Create  Node  Tools  Window  Help                        ⌕  Ctrl+K       │ 26
├────────────────────────────────────────────────────────────────────────────────────────────┤
│ ▣ ↻ ⤢ │ ⊞ 0.25 │ ∠ 15° │ ⌂ world │ ✥ ▦ 🖌 │   ▶  ⏸  ⏭  ⏹   │      quay.loom ●   7f3a91e2  │ 34
├──────────────────────┬──────────────────────────────────────────┬──────────────────────────┤
│ Hierarchy         ⌕  │▍Scene │ Game                             │ Inspector                │ 26
│──────────────────────┤                                          ├──────────────────────────┤
│ ▾ quay               │                                          │  quay_wall               │
│   ▸ ground           │                                          │ ┌ Transform ───────── ▾ ┐│
│▍  quay_wall       ◆  │                                          │ │ Position   0.00  1.50 ││
│     lantern_a        │            ┌ ─          ─ ┐              │ │ Rotation   0     0    ││
│▍  lantern_b       ◆  │                                          │ │ Scale      1.00  1.00 ││
│   ▾ props            │              (the scene)                 │ └───────────────────────┘│
│       crate_01       │                                          │ ┌ Material ─────────── ▾┐│
│       crate_02       │            └ ─          ─ ┘              │ │▍albedo    ███ #8A7B6C ││
│       crate_03       │                                          │ │ roughness ────●────── ││
│                      │                          ┌─────────────┐ │ └───────────────────────┘│
│                      │                          │ 12.4 m      │ │                          │
│                      │                          └─────────────┘ ├──────────────────────────┤
│                      │                                          │ Agent                    │ 26
│                      │                                          ├──────────────────────────┤
│                      │                                          │ ◆ Reposition quay wall   │
│                      │                                          │   2 nodes · 1 undo step  │
│ 8 nodes · 2 hidden   │                                          │ ▸ ask for a change…      │
├──────────────────────┴──────────────────────────────────────────┴──────────────────────────┤
│▍Console │ Problems 2 │ History │ Transactions │ Project │ Prefabs                          │ 26
│ 14:02:11  agent · Reposition quay wall: 2 nodes                                            │
│ 14:02:11  scene reloaded — your undo history was cleared because the agent wrote           │
├────────────────────────────────────────────────────────────────────────────────────────────┤
│ ● unsaved  │  144 fps  │  8 nodes  │  12 draws  │  Move · world · snap 0.25                │ 22
└────────────────────────────────────────────────────────────────────────────────────────────┘
   260 pt                        flex                                    380 pt
```

`▍` is the 2 px `accent` thread; `◆` is an `agent` mark; `●` is dirty in `accent`. Left column 260
pt, right column 380 pt split 60/40 between Inspector and Agent, bottom node 200 pt.

**The Agent panel is a vertical split of the right column, not a tab beside the Inspector**, and
that is the only layout that satisfies user decision 5. A tab hides one panel when the other is
open; the entire point of "watch its SceneOps land live" is seeing the Inspector's values move
while the Agent's transaction log fills. They must be visible at once.

**This forces one amendment to `PLAN.md` Stage 3.** The `Tab` enum is fixed once, in Stage 3,
because adding a variant later invalidates every saved `DockState`. The enum as planned is `Scene`,
`Game`, `Hierarchy`, `Inspector`, `Project`, `Console`, `Problems`, `History`, `Transactions`,
`Prefabs` — **there is no `Agent`**, and there must be. By `PLAN.md`'s own rule it has to be added
in Stage 3 with the rest, even though the panel's body is another document's design and may arrive
in a later stage. A variant that renders its empty state for two stages is exactly what the rule is
for.

---

## 11. How this maps onto egui 0.35, concretely

One file, `crates/loom_editor/src/theme.rs`, and **no panel anywhere sets a colour** — a panel that
needs one reads a token. Applied through `Context::all_styles_mut` (`context.rs:2145`), which
mutates the light *and* dark `Style`, so the palette holds regardless of what
`ThemePreference`/system theme egui thinks it is following. That is why `all_styles_mut` and not
`set_style`.

```rust
// crates/loom_editor/src/theme.rs
use loom_render::egui::{self, Color32, CornerRadius, FontId, Margin, Stroke, TextStyle, vec2};
use loom_render::egui::epaint::Shadow;
use loom_render::egui::FontFamily::{Monospace, Proportional};

pub struct Tokens { pub ground: Color32, /* … the 25 of §3 … */ }

pub const DARK: [u32; 25] = [0x0E1013, 0x16191E, /* … */];
pub const HIGH_CONTRAST_OVERRIDES: [(usize, u32); 5] = [/* §3 */];

fn tok(hex: u32) -> Color32 { /* §2 */ }

pub fn tokens(high_contrast: bool) -> Tokens { /* map tok over DARK, patch 5 if high_contrast */ }

pub fn apply(ctx: &egui::Context, t: &Tokens) {
    ctx.all_styles_mut(|s| {
        s.visuals.dark_mode = true;
        s.visuals.panel_fill = t.surface;
        s.visuals.window_fill = t.raised;
        s.visuals.extreme_bg_color = t.sunken;
        s.visuals.widgets.noninteractive = w(t.surface, t.surface, 1.0, t.line,        1.0, t.text);
        s.visuals.widgets.inactive      = w(t.raised,  t.raised,  1.0, t.line_strong, 1.0, t.text);
        s.visuals.widgets.hovered       = w(t.hover,   t.hover,   1.0, t.line_strong, 1.0, t.text_strong);
        s.visuals.widgets.active        = w(t.press,   t.press,   2.0, t.accent,      1.0, t.text_strong);
        s.visuals.widgets.open          = w(t.press,   t.press,   1.0, t.line_strong, 1.0, t.text_strong);
        s.visuals.selection.bg_fill = t.accent_deep.gamma_multiply(0.35);
        // … the rest of §9's Visuals block …
        s.spacing = spacing();                       // §5
        s.text_styles = text_styles();               // §4
        s.animation_time = 0.12;                     // §8
        s.visuals.window_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0,
                                           color: t.ground.gamma_multiply(0.63) };
    });
}
```

`text_styles` is the `BTreeMap` of §4, including the two `TextStyle::Name` entries, which panels
reach through `TextStyle::Name("Title".into())`. `Margin` is `i8` in 0.35 and `CornerRadius` is four
`u8`s — both changed from earlier egui and both are places a recalled API would be confidently
wrong.

**Files this identity touches**, and no others:

| File | Change | Stage |
| --- | --- | --- |
| `crates/loom_render/src/ui.rs:88` | `srgb_framebuffer: true` | 0 |
| `crates/loom_editor/src/theme.rs` | new — tokens + `tok` + `apply` + the swatch probe | 1 (tokens), 3 (`apply`) |
| `crates/loom_editor/src/icons.rs` | new — §12's sixteen | 3 |
| `crates/loom_editor/src/dock.rs` | tab visuals, the accent thread, the gutter fill | 3 |
| `crates/loom_editor/src/panels/*.rs` | §6's composition, empty states, tokens instead of literals | 1–4 |
| `crates/loom_editor/src/overlay.rs` | `stroked`, `chip`, brackets, the grid | 4 |
| `crates/loom_editor/src/gizmo.rs` | handle colours and the active-axis rule | 4 |
| `crates/loom_editor/src/tools/{paint,sculpt}.rs` | the brush ring | 6, 7 |
| `assets/fonts/` + `SOURCES.txt` + `OFL.txt` | only if the swap is taken | 3 |
| `docs/guide/03-the-interface.md` | what each colour means, for a stranger | 9 |

### Icons — sixteen, drawn, not shipped

ADR 0030 rules for hand-drawn `egui::Painter` geometry and the argument is right: an icon font's
stroke weight will not match the hand-drawn gizmo handles in the same window, and it costs a
dependency, a licence entry and a binary asset class for one screenful of lines. **It says "~14";
the actual list is sixteen** and pinning it here stops the set growing by improvisation:

`move · rotate · scale · eye · lock · play · pause · step · stop · chevron · folder · cube ·
brush · sculpt · warning · agent`

`chevron` is drawn once and rotated for all four directions; `warning` doubles as the Problems
icon; `agent` is a needle-and-thread mark, three segments, and it is the only icon in the set that
is ever drawn in a colour other than the current text colour.

What makes sixteen hand-drawn icons look like a set rather than sixteen drawings:

- **16 × 16 point box with a 1 pt inset**, so a 1.5 pt stroke never clips the allocation.
- **One weight: `Stroke::new(1.5, ui.visuals().text_color())`.** Taking the colour from the current
  `WidgetVisuals` is what makes icons inherit hover, active and disabled for free — no icon has a
  state of its own.
- **Three primitives only**: straight segments, full circles, and quarter-arcs as 8-segment
  polylines. No bezier, no fill except on `play`, `pause` and `stop`, whose transport convention is
  filled everywhere and would look broken outlined.
- **Every endpoint lands on a 2 pt sub-grid** inside the box, so at zoom 1.0 every stroke centre
  falls on a half-pixel and the whole set is crisp. At other zoom factors it is not, and that is
  accepted rather than solved — the alternative is per-zoom geometry.

The module is two functions: `draw(painter: &Painter, rect: Rect, icon: Icon, stroke: Stroke)` with
one `match` of point lists, and `button(ui: &mut Ui, icon: Icon, tip: &str) -> Response` that
allocates `interact_size`. **Icons never appear without a label** except in the toolbar's tool
group and the tab strip, where there are four of them and the user learns them once. An icon-only
inspector is a memory test.

---

## 12. ADRs

### Proposed: ADR 0033 — the editor colours recency, not authorship

> **Decision.** The `agent` colour marks a *recent write the editor did not cause*, decayed over
> `CHANGE_FADE` seconds and held only in session memory. It never marks ownership. Human-authored
> content gets no colour at all. `loom_scene::ops::Transaction` gains **no** author field and the
> `.loom` format gains **no** provenance key, because a scene file describes a scene and not who
> typed it, and because a provenance field would be a second source of truth that goes wrong on the
> first `git merge`, the first `cp`, and the first hand edit.
>
> **Rejected:** a persistent agent tint on nodes the agent created (needs authorship in the file —
> a `format` bump, a migration, and a field every hand-edit invalidates); authorship in the
> transaction log only (survives a session but not a restart, so the same node is tinted or not
> depending on when you opened the editor, which is worse than either honest answer); inferring
> authorship from git blame (an authored scene is often not in git, and a squashed commit erases it).
>
> **Consequence:** the History panel's agent rows and the viewport's agent marks are the *only*
> provenance surfaces, and `docs/guide/05-you-and-the-agent.md` must say in words that the blue
> means "just now", not "theirs".

### Proposed: ADR 0034 — UI colour is authored in display space and encoded exactly once

> **Decision.** `crates/loom_render/src/ui.rs` sets `srgb_framebuffer: true`, because the swapchain
> is `B8G8R8A8_SRGB` (`viewer.rs:2101`) and `egui-ash-renderer 0.12.0`'s shader pair is an identity
> on the vertex colour when the constant is `false` — so the hardware encode is a second one and the
> UI currently displays every colour lifted (a `#16191E` panel arrives as `#535860`, and a designed
> 14.6:1 arrives as 6.7:1). The editor's token table is written in the colour intended **on the
> screen**, and `theme::tok` pre-warps each channel by the residual between the shader's gamma-2.2
> and the hardware's piecewise sRGB, so a hex in the table equals a pixel on the display within
> ±2 bytes. `loom edit --theme-probe` renders the swatch strip that proves it, since no golden
> image can — `cargo xtask image` drives the offscreen `Renderer`, which never constructs a `Ui`.
>
> **Rejected:** leaving it and tuning a palette to compensate (every ratio would be fiction and the
> palette would break the day someone fixes the encode); a `B8G8R8A8_UNORM` swapchain (moves the
> scene's own tonemap output, which the golden references pin); doing the correction in the token
> hexes by hand (the arithmetic would be invisible and unreviewable in a table of 25 constants).
>
> **Blast radius, stated:** the HUD draws through the same pipeline and will also change
> appearance. It is game content, no golden reference contains it (`xtask image` never opens a
> window), and the change makes it correct rather than different.

### Amend ADR 0030 — editor UI dependencies, icons and fonts

Four additions, no reversals. **(a)** The icon list is fixed at the sixteen named in §11 with the
four geometry rules, replacing "~14 hand-drawn shapes". **(b)** The font *candidates* are pinned now
even though the *decision* stays deferred: Inter and JetBrains Mono, both SIL OFL 1.1, vendored to
`assets/fonts/` with `OFL.txt` and a `SOURCES.txt` recording release tag, URL and sha256 per file.
**(c)** Record that egui's bundled fonts have exactly one weight, so the type scale's SemiBold
column is inert until the swap — headings are differentiated by size and `text_strong` alone in
slice one. **(d)** Record that fonts are `loom_editor`-only and a shipped game's HUD therefore keeps
egui's bundled fonts, deliberately.

### Amend `PLAN.md` §2.7 and doc 01 §6.1 — three palette corrections

`line` `#262C35` → `#2E3540` (1.25:1 is below the visibility threshold for a 1 px rule, which would
delete the mechanism the surface strategy rests on); `hover` stops sharing a value with `line`; and
the focused-field border moves from `line_strong` (1.97:1, fails WCAG 3:1 for a meaningful non-text
indicator) to `accent` (6.47:1). Also: `Monospace` is 12.0, not doc 07's 13.0; and doc 07's global
1.35 line height does not exist as a setting in egui 0.35 — it is applied per run through
`RichText::line_height` on the four multi-line surfaces.

### Amend `PLAN.md` Stage 3 — the `Tab` enum gains `Agent`

User decision 5 makes the agent a first-class docked panel and the planned enum has no variant for
it. By Stage 3's own rule — adding a variant later invalidates every saved `DockState` — it must be
added there, with an empty state, even if the panel's body lands in a later stage.

**No ADR for the theme itself.** `PLAN.md` §3 explicitly lists the theme among the things that are
not ADRs, and this document is the specification it points at.

---

## 13. Where this belongs in the ten stages

**Mostly Stage 3, as planned, split so that nothing is built twice.**

| Stage | What lands | Depends on |
| --- | --- | --- |
| **0** | The R7 probe (§2), `ui.rs:88`, ADR 0034. **Before any hex is tuned** — this is already Stage 0 in `PLAN.md` and §2 only makes it precise | nothing |
| **1** | `theme.rs` exists as a **token module only** — the 25 `Color32` constants, `tok`, `tokens()`, no `apply`. The inspector reads tokens from its first line instead of literals | Stage 0 |
| **3** | Everything else: `apply`, `Spacing`, `text_styles`, `icons.rs`, panel composition, empty states, motion, the tab-strip thread, `--theme-probe`, high contrast, the font-swap checkpoint | Stages 1–2 |
| **4** | Viewport chrome — `stroked`, `chip`, selection brackets, the gizmo restyle, the grid, the agent-mark casing fix | Stage 2's `to_viewport`/`to_window` |
| **5** | The Hub's `Name("Display")` style and the hub-specific empty states | Stage 3 |
| **6, 7** | The brush ring, in its three operation colours | Stages 3–4 |
| **9** | `docs/guide/03-the-interface.md` — what each colour means, written for someone who has never seen the tool | Stage 4 |

**The Stage 1 split costs nothing and prevents a rewrite.** `PLAN.md` puts the theme in Stage 3 and
that is right — it is doc 01's step-4 experiment, applied over the old panels, and it is the
cheapest possible test of whether the palette reads as sleek. But Stage 1 writes the inspector,
which is the largest new surface in the rework, and if it is written with `Color32::from_rgb`
literals then Stage 3 re-reads every line of it. A token module with no `Visuals` application is
half a day and it makes Stage 3 a one-file change, which is what makes it reversible, which is the
property doc 01 chose the experiment for.

**Stage 3's human checkpoint is the only gate this subsystem has**, and `PLAN.md` R17 already says
so: *does it read as sleek?* Add one observation to it — **run `--theme-probe` and sample three
swatches before judging.** If the encode is still wrong, the human will be judging a palette that
is not the one in this document, and they will conclude the palette is bad.

---

## 14. What I rejected, and why

**A light theme.** §3. The viewport's average luminance is the engine's to choose and the chrome's
to live with; zoom plus a five-token high-contrast swap meets the real need.

**Per-panel accent colours, or a coloured toolbar.** They cost "every colour is data" outright and
permanently, for a screenshot's worth of gain. Once one panel is teal, a coloured pixel no longer
means anything.

**A per-panel title header.** §6. The tab strip is the header; a second one repeats it ten times.

**An icon font (`egui-phosphor`), and an SVG set.** ADR 0030's reasoning stands and §11 only makes
the geometry explicit. Adding `resvg` to rasterise one screenful of lines is the clearest case of
the obsolete-style trap the project rules warn about.

**A full wireframe box for selection.** It occludes the thing it selects and reads as a bounds
gizmo. Corner brackets say "selected" and get out of the way.

**An infinite depth-tested ground grid.** §7. A pipeline, a pass, a barrier and a golden scene, for
scenery — and the bounded tool-scoped version is the *better* affordance, not the cheaper one.

**Alpha inversion, difference blending, and animated dashes for viewport legibility.** §7. The first
two are unreachable from an egui overlay; the third puts motion in a still frame in a tool whose
whole verification story is about telling real motion from artifacts.

**A seventh hue for `dirty`.** §3. Surface partition costs zero tokens and does the same job.

**Authorship in the scene file.** ADR 0033. A `.loom` file describes a scene.

**A shared `theme.rs` in `loom_render`.** Doc 07 §13 puts fonts there. ADR 0022 draws the boundary
at `loom_editor`, and a runtime binary that does not link the editor has no business carrying its
palette.

---

## 15. What I could not verify

**Nothing here has been compiled or rendered.** In descending order of how much would move:

1. **The double encode is read from source, not measured.** I read
   `egui-ash-renderer-0.12.0/src/shaders/shader.{vert,frag}`, the specialization-constant wiring at
   `renderer/vulkan.rs:78-88`, `ui.rs:88`, and the swapchain preference at `viewer.rs:2101-2102`,
   and the chain is unambiguous on paper. I did not disassemble the `.spv` files that are actually
   `include_bytes!`d, and they could in principle differ from the `.glsl` beside them. **The Stage 0
   swatch probe is the measurement**, and until it runs, every contrast number in §3 is
   conditional on the fix landing.
2. **`tok`'s exact residual.** The correction assumes the vertex shader's `pow(c, 2.2)` and the
   hardware's sRGB curve are the only two transforms in the path. If the driver or the compositor
   applies anything else — a display-P3 conversion, an HDR path — the swatch probe will show it and
   the constant changes. It will not change the *structure*.
3. **The contrast numbers I did not recompute.** I recomputed `text_strong`, `text`, `text_weak`,
   `accent`, `line`, `line_strong` and the `accent_deep` composite from WCAG relative luminance and
   they agree with doc 01 §6.1 to two decimals. `ok`, `warn`, `error`, `agent` and the three axis
   rows are doc 01's, unchecked by me, and doc 01's method is demonstrably right.
4. **Whether the flat surface ramp survives a cheap panel.** `raised` against `surface` is 1.1:1 by
   design. On a good IPS panel that is a legible edge; on an eight-bit TN it may collapse into one
   grey. I have no second display to check on. The high-contrast toggle is the answer if it does,
   and that is why it swaps `line` rather than only the backgrounds.
5. **Whether `egui_dock 0.20.1` lets the tab strip be styled this way at all.** The 2 px accent
   thread on the active tab, the `surface`-filled active tab continuous with the body, and the
   `ground` gutter are all assumptions about what its `TabViewer`/`Style` exposes. It is not in this
   machine's registry — `PLAN.md` R10 already schedules `cargo add --dry-run` for Stage 3. **If it
   does not expose them, the fallback is drawing the strip ourselves inside the tab body**, which
   costs the thread its position rather than its meaning.
6. **Whether sixteen hand-drawn icons actually look like a family.** The four geometry rules are the
   ones that usually make it work and they are cheap to follow, but nobody has drawn them. This is
   `PLAN.md` R17's territory and it is unautomatable.
7. **Inter's and JetBrains Mono's current release tags and file hashes.** I did not fetch them —
   which is exactly why §4 specifies recording them in `SOURCES.txt` at vendoring time instead of
   pinning a version number I would be guessing at.
8. **Whether `Painter` shape counts for the grid matter.** A 20 × 20 m lattice at 1 m is ~82
   polylines, doubled by the casing, per frame. That is nothing next to a tessellated panel, but I
   have not measured egui's tessellation cost on this machine and the grid is the only overlay that
   scales with a setting.
