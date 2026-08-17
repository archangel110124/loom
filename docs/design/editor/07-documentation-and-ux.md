# Design — documentation, in-editor help, and the visual language

*Editor rework, design phase. Reads on top of `00-survey-existing.md`, `00-survey-engine-surface.md`
and `00-survey-constraints.md`; every fact those establish is taken as given and not re-derived.
Nothing here was built or run — this is design, and §14 lists what I could not check.*

---

## 1. The spine: two keys, and one source per fact

**Everything below hangs off two keystrokes and one rule.**

- **F1 answers "what is this".** Whatever the pointer is over — a field, a panel, a gizmo handle, a
  problem in the Problems panel — F1 explains it in place. It never navigates away.
- **Ctrl+P answers "how do I do that".** One palette, fuzzy-searched, listing every command, every
  component type, every node in the scene and every asset, each with its keybinding and a
  one-sentence description.

That split is memorable in a way a menu tree is not, and it is what makes the editor learnable
without a manual. The manual then exists for the third question — *why does this system work this
way* — which is the only one prose is actually good at.

**The rule is that no fact about the engine is written down twice.** A component's field
description exists exactly once, as a `///` doc comment on the Rust struct, and it reaches the
schema, the validator's rejection `hint` (`loom_reflect/src/lib.rs:117-120`), the inspector tooltip
(`panels.rs:798-803`), the F1 popover and the printed reference by the same route. A command's name
and help exist exactly once, in a table. Every duplicated string in a documentation system is a
string that will be wrong within a month, and the cheapest way to write correct documentation is to
make the incorrect version impossible to author.

This is not a new principle in this project — it is the one `loom_field::all()` and
`assets/shaders/generated/fields.slang` already run on (ADR 0006), applied to prose.

---

## 2. The documentation set

Six pieces, in `docs/guide/`, shipped alongside the binary (§11) so F1's external links work
offline. **Two are generated and must never be hand-edited; four are prose and are the only files a
human writes.**

```
docs/guide/
  README.md                  the 60-second start, and the map of everything below
  01-first-hour.md           prose: install → hub → new project → a lit room → Play → save
  02-the-window.md           prose: a tour of each panel, what it is for, what it will not do
  03-tasks/                  prose: one file per system, task-shaped
    build-and-block-out.md   primitives, transforms, snapping, prefabs, duplication
    sculpt-terrain.md        voxel ops, recipes, `loom terrain`'s four metrics
    surfaces.md              materials, the four painting systems, decals
    weather-and-life.md      wind, rain, clouds, grass, scatter, water
    make-it-a-game.md        scripts, GameRules, HUD, enemies, navigation, the event log
    ship-it.md               the build, the assets folder, the two targets
  04-reference/
    components.md            GENERATED from the type registry — do not edit
    commands.md              GENERATED from the command table — do not edit
    errors.md                prose: one section per error code, in plain language
  05-you-and-the-agent.md    prose: transactions, labels, version tokens, what Ctrl+Z can and
                             cannot reach, and how to recover when it cannot
```

**`05-you-and-the-agent.md` is the most important file in the set and it does not exist in any other
engine's documentation.** Everything else is a variation on documentation this industry has written
a hundred times; the co-authoring model is genuinely novel, its failure modes are subtle (§8), and a
user who does not understand version tokens will experience the editor as flaky rather than as
careful. It gets written first and it gets linked from the divergence banner.

**Every prose page ends with the same section: "the same thing from the command line."** A page that
teaches placing a crate on a table ends with the `loom place --op` JSON that does it. This is not a
courtesy to CLI users — it is how the human learns the vocabulary the agent speaks, which is what
makes reviewing the agent's diffs possible. Property 2 says the agent must be able to do everything
the human can; printing the equivalent in every how-to is what keeps that claim honest and visible.

I considered and rejected an in-editor manual reader (rendering the markdown inside a panel with
`egui_commonmark`). It is a new pinned dependency and a second markdown renderer whose output must
be kept looking sane alongside the one GitHub already gives us, in exchange for prose a user reads
once. **Long prose opens in the system handler** — `xdg-open` on Linux, `cmd /c start` on Windows,
both `std::process::Command`, no dependency. If the Help panel ever becomes the primary reading
surface rather than the fallback, revisit; the trigger is the human saying they never leave the
window to read.

---

## 3. The component reference is generated, and this is the load-bearing decision of the whole doc set

**The registry already carries everything a reference page needs, and I verified this by reading the
schema-consuming code rather than assuming it.** `TypeRegistry::describe` returns the schemars root
schema (`loom_reflect/src/lib.rs:46`), and the code that already consumes it proves what is in
there: `description` per field from the `///` comment (`lib.rs:117-120`), `minimum`/`maximum` from
`#[schemars(range(...))]` (`lib.rs:337-342`), `type` (`lib.rs:285`), `enum` — including the `oneOf` +
`const` spelling that documented enum variants produce (`lib.rs:233-258`) — and `default` per field,
which is what `App::add_component` writes when you add a component (`run.rs:1607-1620`).

So the reference is a fold over `components::registry()`:

```
crates/loom_cli/src/main.rs   components::registry()          the 24 registered types
xtask/src/main.rs             "docs" => docs(check: bool)     new subcommand
docs/guide/04-reference/components.md                          the output, committed
```

Per type: the struct's own doc comment as the intro, then a table of field · type · default ·
range or allowed values · description. Ordered by `type_names()`, which is a `BTreeMap` and
therefore stable — the registry's own doc comment says iteration order is observable and that this
is why it is not a `HashMap` (`loom_reflect/src/lib.rs:18-20`). A generated file that reorders itself
between runs produces a diff on every regeneration and gets ignored within a week; this one cannot.

**`cargo xtask docs --check` fails when the committed file differs from what the registry would
produce now**, and it joins `scripts/green.sh`. This is the same mechanism as
`tests/references/MANIFEST.txt` — a regeneration is a readable line in a commit rather than a
silent drift — and it is the entire reason to generate rather than write. Without the check, a
generator is just a hand-written file with extra steps, because nothing stops someone editing the
output.

Three honest limits, each of which the generator must handle rather than paper over:

**Not every field has a default.** `add_component` skips fields whose schema declares none
(`run.rs:1607-1610`), which means some do not have one. The reference prints `—` and the prose
column says "required"; it must not invent a zero.

**The schema is one level deep.** `loom_scene/src/scene.rs:638` notes this explicitly, and it is why
the current inspector shows `VoxelVolume.ops` as `"4 items"` (`panels.rs:110-121`). Nested types —
`GroundLayer`, `WaveSet`, `Pontoon`, `ScatterExclude`, `AssetRef` — live in `$defs` and are reachable
by following `$ref`, which the validator already does (`lib.rs:205-216`). **The generator follows
the same `resolve` path and emits a sub-table per nested type**, so the reference is complete where
the inspector currently is not. Reusing `resolve` rather than writing a second walker is deliberate:
two walkers over the same schema is two answers to what a type is.

**`VoxelVolume.ops` is untyped JSON** (`components.rs:481`) and the op vocabulary lives in the
component's doc comment as prose — the survey records that the doc comment *is* the schema there.
The generator cannot do better than print that comment. The reference says so out loud in that one
section rather than pretending the array is documented, and `loom validate`'s `invalid_voxel_op`
messages (which enumerate the fields each op kind takes, per `docs/format/README.md` §6) are named
as the discovery mechanism. **An admitted gap in a reference is worth more than a confident silence**,
and this is the only one.

The rule this establishes belongs in `CLAUDE.md` next to the field and golden-scene rules, because
it has the same shape: **documenting a component means writing its doc comment. Never write a
component's fields into prose — the reference is generated and your prose will be deleted by the
next regeneration.**

---

## 4. One command table, and everything is a view of it

**This is the one thing in my scope I think needs an ADR** (§12), because it constrains all future
editor work and because it is the mechanism that makes the command reference non-drifting.

```rust
// crates/loom_editor/src/command.rs — data, not a trait (never-do #12)
pub struct Command {
    /// Stable id. Used by the keymap, the palette's recents, and the docs anchor.
    pub id: &'static str,        // "node.duplicate"
    pub title: &'static str,     // "Duplicate"
    /// One sentence, imperative, no jargon. The palette row, the toolbar tooltip,
    /// the F1 answer and the reference all print exactly this string.
    pub help: &'static str,
    /// The `loom_input` action this is bound to, or "" for palette-only.
    /// The displayed keybinding is looked up here, never typed into `help`.
    pub action: &'static str,    // matches assets/input/default.toml
    /// What must be true to run it, and therefore what the palette says when it is not.
    pub needs: Needs,            // Always | Editing | Selection(1) | Selection(1..) | Playing
    /// The transaction label this command writes, with {} filled from the selection.
    pub label: Option<&'static str>,   // "Duplicate {node}"
}
pub const COMMANDS: &[Command] = &[ /* … */ ];
```

The palette lists it. The menus and toolbar are filtered slices of it. F1 with nothing hovered opens
it in help mode. `commands.md` is generated from it. The keybinding column is resolved through
`loom_input::ActionMap` (`assets/input/default.toml`), so **the documented key and the key that
actually fires are the same lookup** — the classic documentation lie, told by every editor, is
structurally unavailable here.

**`label` is why this table earns its keep beyond discoverability.** `CLAUDE.md` requires every
transaction to be usefully labelled because labels land in the human's log panel *and in git
history*, and today those strings are written inline at each call site in `run.rs`. Hanging them off
the command means the palette can show you the label before you run the thing — "Duplicate Crate" —
which is both a preview and a guarantee that no action ships with a label of "update scene".

**`needs` is a discoverability decision, not a plumbing one.** A command whose preconditions are
unmet is **shown, greyed, with the reason** — "Duplicate · select a node first". Hiding it is the
common choice and it is wrong: a command you cannot see is one you will never learn exists, and the
palette is the only place a new user goes looking.

The palette does **not** become a second write path. It emits the same intents the toolbar does,
which funnel through `transact`/`transact_as` (`run.rs:1707-1756`) and therefore through
`Session::apply`, which overwrites `expect_version` with the session's own token
(`edit.rs:299-311`). Never-do #16 and #15 hold by construction, exactly as they do today.

Fuzzy matching is about thirty lines — subsequence match, with a bonus for matches at word starts
and a penalty for gap length — and takes no dependency. The candidate list is four sources merged:
commands, registered component types (which run "Add *X*"), the scene's node paths (jump-and-select),
and the asset aliases the scene resolved (`MeshLibrary::names`). Four sources in one list is what
separates a palette that feels like a real tool from a keyboard-driven menu.

---

## 5. In-editor help

### Tooltips

Already right and worth protecting: the field doc comment becomes the tooltip
(`panels.rs:798-803`), and the comment in that file — *writing a good doc comment, teaching the
agent, and labelling the editor are all one act* — is the thesis of this entire document. The rework
extends it in two directions.

**Component headers get the struct's doc comment**, taken from the schema root's `description`. The
inspector currently shows a bare type name, so `ParticleEmitter`'s twenty-four fields arrive with no
statement of what the thing is or that the defaults are deliberately a smoke plume
(`components.rs:498-501`). That sentence exists; it just never reaches the screen.

**Every tooltip's last line is its constraint**, rendered from the same `minimum`/`maximum` the
slider is already bounded by. "0.0..=10000.0" under the prose costs nothing and pre-empts the most
common class of validation error.

### F1

F1 is a popover anchored to whatever is under the pointer, and it is **rendered from the schema, not
from a markdown file**. Over an inspector field it shows: field name, declared type, default, range
or allowed values, the doc comment, and a "Reference →" link that opens `components.md` at that
anchor in the system handler. Over a panel it shows a one-paragraph description from a
`const PANEL_HELP: &[(&str, &str)]` in `help.rs`. Over a gizmo handle it shows the axis, the current
value, and the nudge keys. With nothing hovered it opens the palette in help mode.

Rendering from the schema rather than parsing the generated markdown is the lazy choice and also the
correct one: it needs no file IO, no markdown parser, cannot go stale relative to the running build,
and works in a shipped folder where the docs directory was deleted.

### The Problems panel replaces the console as the place you look

The console stays (it is the engine's log, and `log.rs`'s repeat collapsing at `log.rs:41-60` is
load-bearing — the view re-derives per frame and one missing asset wrote hundreds of identical lines
a second). But **problems are not log lines**, and mixing them was the old design's mistake. §7 has
the shape.

---

## 6. Onboarding a brand-new project

**The teaching vehicle is the base scene and a four-step strip, not a document.** Nobody reads the
document.

`loom-editor` with no argument opens the **Hub**: recent projects (each with its name, path, and
when it was last opened), a "New project" button, and an "Open" button. New project asks for a name,
a folder, and a template — *Empty*, *First person*, *Third person* — then creates the folder,
copies the template, and opens its base scene.

The templates are `assets/templates/<name>/` directories containing a `project.toml` and a
`scenes/main.loom`. They are ordinary scenes, which is the point: `assets/games/proving_ground.loom`
already proves a whole game is one file. The constraints survey (§5) confirms templates need no ADR;
the project manifest's schema does, and that is ADR-H's job, not mine.

**The base scene teaches through its own comments.** Comments and hand formatting survive every
write (`docs/format/README.md` §2.1, `ops.rs:157-159`), so a scene file can carry annotation that
the agent's edits will not destroy:

```toml
# Your first scene. Everything here is text you can edit by hand or the agent can edit
# for you — both go through the same door. Delete these comments whenever you like.
[scene]
format = 1
```

That comment is read by more users than `01-first-hour.md` will ever have, because it is in the
artifact rather than beside it.

**The task strip** is a single 32px row across the bottom of the viewport with four steps, each
completing when the corresponding thing actually happens:

1. **Add something** — any `SpawnNode` lands
2. **Move it** — any `SetTransform` lands
3. **Press Play** — play mode starts
4. **Save it** — Ctrl+S succeeds

Detection is by watching the ops in the transaction stream and the play state, not by scripting a
fake tutorial. **A tutorial that congratulates you for something you did not do is worse than no
tutorial**, and reading real ops costs one `match` in the strip's update. Dismissed forever by a
button or by finishing; one boolean in the prefs file.

Prefs live at `$XDG_CONFIG_HOME/loom/editor.toml` (falling back to `~/.config/loom/editor.toml`) and
carry: recents, window geometry, dock layout, zoom factor, reduce-motion, high-contrast, and the
"seen the strip" flag. This is user state, not project state, and putting a recents list in a project
file would be a category error — the constraints survey reaches the same conclusion (§4.H) and asks
ADR-H to fix the location. **My contribution to that ADR is the field list above and the argument
that it is TOML and schema-validated like everything else**, so that the editor's own settings are
diffable text for the same reason scenes are.

---

## 7. Telling a person what went wrong

Three error classes reach the user, and they need three different treatments. The unifying rule:
**the headline never contains an error code, a Rust type name, or a unit the user did not type.**
The code is not deleted — it moves behind "Copy for the agent", which yields the raw structured JSON.
One error value, two audiences, and neither one is served the other's format.

### Validation errors

The structured shape is already normative and already good: `error`, `node`, `field`, `value`,
`constraint`, `hint`, where `hint` is the field's doc comment (`docs/format/README.md` §6;
`loom_reflect/src/lib.rs:349-362`). A `fn explain(&FieldError) -> Explanation` in `help.rs` maps it
onto four parts:

| Part | Source | Example |
| --- | --- | --- |
| Headline | `match` on `error` | "That light is brighter than a light can be." |
| What happened | `field`, `value`, `constraint` | "Intensity is 40000; the maximum is 10000." |
| What to do | `hint` verbatim | "Interior lights are typically 100-800." |
| Fix | a `Command`, when one exists | **Set it to 10000** |

The Fix button issues one `SetField` in one transaction and is therefore one Ctrl+Z, like everything
else. Only mechanical fixes get a button — clamp to range, remove an unknown field, spell a field
the way the schema does (the validator already offers the near-match list, `lib.rs:105-112`). A fix
that requires judgement gets no button, because a button that guesses is how a user learns to
distrust every button in the application.

**The `match` over error codes is the only place error prose is written, and `errors.md` is the same
strings in narrative form.** They will drift. The cheap guard is a test that walks `crates/` for
string literals in an `error:` position and asserts each has a `##` heading in `errors.md` — about
fifteen lines, false-positives possible (which merely demand a doc section), and I would mark its
ceiling in a `ponytail:` comment. When `explain` receives a code it has no arm for, it renders the
raw structured error *and* logs `no help text for error code X`, so the gap surfaces the first time
a human hits it rather than never.

**Physical sanity warnings belong here too.** `loom_physics::sanity::check_scene`
(`sanity.rs:48`) runs inside `loom validate` and its output never reaches the window
(survey gap 7). It is the same shape as a validation error and it goes in the same panel, live, as
you author — never-do #10 (no trimesh on a dynamic body) is a rule a person should learn while
placing the collider, not from a CI failure an hour later.

### Version-token rejection

The banner exists (`panels.rs:347-371`) and its behaviour is correct: two versions, both intact,
nothing merged, each button labelled with what is lost, and "Keep mine" correctly calls
`Session::accept_disk_version` so the next Ctrl+S is not refused (`run.rs:702-717`,
`edit.rs:366-385`). **The rework is entirely wording and one addition.**

Wording: not `stale_version`, not "version token mismatch". *"The scene changed on disk while you
were editing. You have 7 unsaved changes; the file gained 3 nodes."* Then two buttons naming their
cost: **Use the version on disk** (discards your 7 changes) and **Keep mine** (your next save
overwrites the 3 nodes).

The addition is that count. `SceneView::changes_from` already diffs node-by-node into
Added/Removed/Moved/Edited (`scene_view.rs:184-221`) and already drives the fading agent-change
overlay. **A choice between two versions is not decidable without knowing what is in them**, and
"3 nodes changed: Office/Desk, Office/Lamp, Yard/Fence" is the difference between a decision and a
coin flip. A "Show the diff" link opens the full text diff, which the transaction machinery already
produces (`Applied::diff`, `ops.rs:123`).

### Failed builds

Only the ship flow (`03-tasks/ship-it.md`) triggers a compile. Cargo's rendered diagnostics go into
the Problems panel verbatim in monospace, with the first `path:line:col` in the output made
clickable. **Deliberately not parsing `--message-format=json`** — that is a real parser against an
unstable-ish schema for output cargo already renders better than we would. Ceiling: no per-error
rows, no severity filtering. Upgrade when a build produces enough errors that scrolling is the
problem, which for a single-developer project is not soon.

---

## 8. Undo/redo, given that the undo stack is not the editor's

This is the subtlest part of the design and the one most likely to be got wrong, because the shared
model has a consequence nobody has written down yet.

**Every agent write that reaches a clean editor destroys the human's entire undo history.**
`poll_file` sees the file move, calls `Session::reload` (`run.rs:684-697`), and `reload` clears
`undo`, `redo` and `gesture` (`edit.rs:395-403`). The reasoning in that function is sound and I am
not proposing to change it — *"offering it anyway would let a user undo their way onto someone
else's work"* is correct, and a stack of whole-scene snapshots cannot be rebased onto a file that
moved. Unsaved work is safe: if `dirty`, the conflict banner fires instead and nothing is lost
(`run.rs:648-659`). What is lost is the ability to step back through *your own already-saved* work.
The user's experience is: I saved, the agent wrote, now Ctrl+Z does nothing and says nothing.

**Silence is the bug, not the clearing.** Four affordances fix it, and none of them changes a line
of `loom_scene`.

**The History panel replaces the console's transaction column.** Labels newest-last from
`Session::history`, with a caret marking where you are; entries below the caret are the redo branch,
greyed. Clicking entry *N* calls `undo()` *N* times — the same code path, never a jump, because a
jump would be a second undo mechanism and never-do #16 exists to prevent exactly that.

**The agent's writes draw a rule across the panel**: `—— the agent wrote here · steps above cannot
be undone ——`. The rule is what makes an invisible mechanism visible, and it turns "Ctrl+Z is
broken" into "ah, that is why".

**Undo names its target, everywhere.** The toolbar button reads *Undo Move Crate*, its tooltip reads
*Ctrl+Z · undoes "Move Crate" (12 ops)*, and when the stack is empty the button is greyed with a
tooltip saying **why** — either "nothing to undo" or "the scene was reloaded after the agent wrote;
earlier steps are gone". The op count is what teaches the twelve-ops-one-step rule without a
sentence of prose.

**Below the undo stack, the recovery story is git, and the editor says so.** When undo cannot reach
something, the message names the file and offers **Show file history**, which runs
`git log -p -- <scene>` and shows the output. Every scene is text in a repository; that *is* the
backup, and inventing a `.loom-backups/` directory would be new invisible state that the agent
cannot see, which is the property this project spent its architecture on. If `git` is absent or the
file is untracked, the message degrades to naming the path and saying it is a text file — the
assumption gets a `ponytail:` comment rather than an ADR, because the editor is stripped from the
shipped runtime anyway and its only user is a developer.

**Coalescing is taught by showing it happen.** During a gizmo drag or a slider scrub the top history
entry carries a dot and reads *Move Crate (dragging)*; on mouse-up it settles into a normal row.
That single visual makes "one gesture is one undo step" self-evident, and it also exposes the
`gesture_epoch` behaviour — let go, grab again, and a *second* row appears, which is precisely what
the epoch bump on mouse release does (`run.rs:898`).

The first Ctrl+Z of a session raises a three-second toast — *"Undid: Move Crate. Ctrl+Y redoes."* —
once, ever, flagged in prefs. Everything else here is passive.

---

## 9. Accessibility

The basics, chosen because they are the ones actually used rather than the ones that look thorough.

**Everything is reachable from the keyboard, and that falls out of the palette rather than needing
per-feature work.** No command exists that the palette cannot run, because the palette is generated
from the table that defines what commands are (§4). This is the strongest accessibility property in
the design and it costs nothing extra.

**Focus is always visible** — a 2px inset accent ring on the focused widget, shown always rather
than only after a Tab. And the Tab rule from the current editor must survive the rewrite verbatim:
egui claims Tab unconditionally as its focus key, so Tab is un-consumed unless a **text field**
specifically has focus (`ui.rs:164-173`, used at `run.rs:852`). `wants_keyboard` is the wrong test
and is already dead code.

**Contrast**: every text token is at least 4.5:1 against its surface and every boundary at least
3:1. §10 lists the pairs with the ratios I computed; I calculated them by hand from the sRGB
relative-luminance formula and did not run a checker (§14).

**Colour is never the only channel.** Axis handles already carry X/Y/Z letters (`AXIS_NAMES`,
`panels.rs:100`) — keep them. Severity carries an icon and a word, not just a hue. A prefab override
gets a left bar *and* a bold field label. Agent change marks carry a text label already
(`panels.rs:663-695`). Red/green/blue axes are the worst possible triple for deuteranopia and are
kept anyway because the convention is worth more than the fix; the letters are the accommodation.

**Zoom**: Ctrl+= / Ctrl+− / Ctrl+0 drive egui's zoom factor over 0.75–2.0, persisted. Everything is
laid out in egui points, so hit targets, spacing and text scale together and no second layout is
needed.

**Hit targets are 24px minimum** for every row and button. The one exception is the gizmo grab
radius, which is 10px (`GRAB_PIXELS`, `gizmo.rs:36`) because it is a precision instrument and
widening it makes overlapping handles ambiguous. **The accommodation is an equal-power alternative
rather than a bigger target**: IJKLUO nudge already exists (`run.rs:2113-2137`) and the inspector's
numeric fields are exact. Anything achievable by dragging is achievable by typing.

**Reduce motion** sets every UI animation duration to zero and turns the six-second agent-change
fade (`CHANGE_FADE`, `run.rs:419`) into a static outline with a dismiss button. Nothing in the
chrome flashes and nothing animates above 3 Hz.

**Screen-reader support is deliberately not built, and this is the one place I am cutting.**
`egui-winit` is pinned with `default-features = false` (`loom_render/Cargo.toml:13`), which means
its `accesskit` feature is off. Enabling it is a feature flag plus a pinned `accesskit_winit`, and
on X11 it goes through AT-SPI. I am not proposing it because there is no stated need, egui's
per-widget labelling is partial so the result would be a half-navigable tree that reads as support
without being it, and the constraints survey's ADR-E (new UI dependencies) would have to absorb it.
**Stated plainly so it is a decision rather than an oversight**, with the revisit trigger being an
actual user who needs it.

---

## 10. The visual language

### What "sleek" means for this specific tool

Not thin lines and not dark-for-its-own-sake. For a 3D editor sleek means one thing precisely:

> **The chrome is greyscale. Every colour in the interface is data.**

Axis red/green/blue, agent teal, error red, warning amber, the override violet — each of those hues
means exactly one thing, everywhere, and nothing decorative is allowed to use them. The consequence
is that a coloured pixel in the window is *informative by construction*, which is what makes a
dense tool scannable at a glance, and it is a rule a single developer can hold in their head while
adding the two hundredth widget. It is also the reason to reject the tempting alternative of a
per-panel accent or a coloured toolbar: those cost the property outright, permanently, for a
screenshot's worth of gain.

The second half of sleek is restraint in the count of things: **three surface levels, one radius,
one border weight, five spacing values, six type sizes.** A design system small enough to be
remembered is one that gets followed; a token set of forty is one that gets ignored by the third
feature.

### Palette — one dark theme

I am not shipping a light theme. A second palette is a second set of contrast checks and a second
thing to keep consistent, for a tool used against a 3D viewport where dark chrome is correct. The
low-vision answer is the zoom factor plus a high-contrast toggle that swaps four tokens, not a
whole second theme.

| Token | Hex | Use | Contrast |
| --- | --- | --- | --- |
| `bg_0` | `#0E1013` | window ground, dock gutters, the space between panels | — |
| `bg_1` | `#16191E` | panel surface — the default background of everything | — |
| `bg_2` | `#1E222A` | raised: toolbar, tab bar, popovers, menus, hovered rows | — |
| `sunken` | `#0A0C0F` | text inputs, the console, the viewport letterbox | — |
| `line` | `#2A2F39` | 1px separators, panel edges, table rules | 1.9:1 on `bg_1` — decoration only |
| `line_strong` | `#3A414F` | control borders, the active dock tab's underline | 3.1:1 on `bg_1` ✓ |
| `fg_0` | `#E6E9EF` | primary text, values, node names | 13.6:1 on `bg_1` ✓ |
| `fg_1` | `#A7AFBD` | field labels, secondary text, units | 6.9:1 on `bg_1` ✓ |
| `fg_2` | `#6B7280` | disabled text, placeholders | 3.0:1 — **non-text only**, never a label you must read |
| `accent` | `#7C5CFF` | selection fill, focus ring, active tool, override bar | 4.1:1 — fills and rings, not text |
| `accent_text` | `#A18FFF` | accent-coloured text and links | 6.7:1 on `bg_1` ✓ |
| `error` | `#F2555A` | errors, destructive confirmations | 5.3:1 ✓ |
| `warn` | `#E0A33C` | warnings, sanity findings, "two undo steps" notices | 8.1:1 ✓ |
| `ok` | `#52C07A` | validation passed, build succeeded | 7.4:1 ✓ |
| `agent` | `#34D3C0` | the agent's change marks and its history rows | 9.6:1 ✓ |
| `axis_x` | `#E2544F` | existing, `panels.rs:95` — unchanged | — |
| `axis_y` | `#7CC860` | existing, `panels.rs:96` — unchanged | — |
| `axis_z` | `#5494E8` | existing, `panels.rs:97` — unchanged | — |

**The accent is violet, and that is a considered choice rather than a taste.** The obvious accent for
a dark tool is blue, and blue is already the Z axis (`#5494E8`) — a selection highlight a user could
mistake for a depth handle is a real error in a 3D viewport, not an aesthetic quibble. Violet is
distinct from all three axis hues, from the agent teal, from error red and warning amber, and it
reads as "you did this" rather than as data. The two-token split (`accent` for fills, `accent_text`
for text) exists because `#7C5CFF` is 4.1:1 — fine for a 3px ring, short of the bar for a label.

High contrast toggle swaps exactly four: `bg_1 → #000000`, `bg_2 → #0C0C0C`, `line → #5A6172`,
`fg_1 → fg_0`. Ten lines, and it is the accommodation that actually gets used.

### Type

**Ship Inter (Regular and SemiBold, SIL OFL 1.1) and keep egui's built-in Hack for monospace.**
egui 0.35 ships Ubuntu-Light, Hack, NotoEmoji and emoji-icon-font (verified in
`epaint-0.35.0/src/text/fonts.rs:513-585`), and Ubuntu-Light *is* the egui default look — the single
highest-leverage change between "looks like an egui app" and "looks designed" is the font, for about
600 KB and one `include_bytes!`. It does introduce a binary asset class this repo does not have and
a licence file, which is why it belongs in ADR-E's scope (§12) rather than being slipped in.

Six sizes at zoom 1.0, and no others:

| px | Weight | Use |
| --- | --- | --- |
| 11 | Regular | axis letters, badge counts, the fps/nodes/draws readout |
| 13 | Regular | **the default** — every control, every inspector row, every list |
| 13 | Hack | paths, version tokens, JSON, console output, numeric readouts |
| 15 | SemiBold | panel headings, dock tab labels |
| 18 | SemiBold | dialog titles, project names in the Hub |
| 24 | SemiBold | the Hub headline, and nothing else |

Line height 1.35. No italics — a second font file for no informational gain. Numeric fields use Hack
so that columns of values align, which is the only reason a monospace font belongs in a UI at all.

### Space

**A 4px grid, five values: 4, 8, 12, 16, 24.** Row height 24, toolbar buttons 28, panel padding 8,
label-to-control gap 8, section gap 16, dialog padding 24.

**The inspector's label column is fixed at 96px.** Every control in the panel starts at the same x,
which is the single most visible "designed rather than assembled" cue in the whole interface and
costs one constant. Labels longer than the column ellipsize with the full text in the tooltip —
which already exists, since the tooltip is the doc comment.

Radius 4 everywhere, except 0 on dock tabs and the toolbar: **flat edges read as chrome, rounded
edges read as content**, and that distinction is what makes a docked layout parse at a glance.
Borders are 1px `line`, always; the only 2px stroke in the interface is the focus ring.

### Icons

**Draw them, do not ship them.** The tool needs roughly fourteen icons — move, rotate, scale, eye,
lock, play, pause, step, stop, chevron, folder, cube, brush, warning — and every one is lines, arcs
and rectangles. One `icons.rs` of about 120 lines against `egui::Painter` covers it.

Rejected: an icon font, because it is a new binary asset class, a licence question, and a glyph
lookup table, and its stroke weight will not match the gizmo handles that are already hand-drawn
lines in the same window. Rejected: an SVG set, because rasterising it needs a dependency
(`resvg` or similar) for one screenful of geometry.

16px on a 24px row, 1.5px stroke, drawn in the current text colour so they inherit hover and
disabled state for free.

### Motion

120 ms ease-out for hover, press and tab changes; 0 ms for anything the simulation drives — a value
that eases toward its target is a value you cannot trust while scrubbing. `Context::animate_bool_with_time`
is exactly enough and needs no animation system. The agent-change fade stays at its existing six
seconds. Reduce-motion zeroes all of it.

---

## 11. What this adds to ADRs already identified

I am not proposing new ADRs for these; I am naming the requirements my design places on ADRs the
constraints survey already says are needed, so they are not negotiated away without noticing.

**ADR-E (new UI dependencies)** must additionally decide: the Inter font files and their SIL OFL
licence text in-repo; that **no icon font or SVG rasteriser is added** (icons are painter geometry);
and that `accesskit` stays off with the reason recorded (§9), so that "we have no screen-reader
support" is a decision in the record rather than a discovery.

**ADR-F (stripping the editor from the runtime build)** must additionally decide the shipped folder
layout, because two of my deliverables live in it:

```
<game>/
  loom-editor[.exe]      developer-only, absent from a release build
  <game>[.exe]           the runtime
  assets/                shaders, meshes, textures, audio, scripts, input/default.toml
  docs/                  the guide, so F1's "Reference →" works offline
  projects/
```

And it must fix the bug that makes this layout not work today: `assets/input/default.toml` is
loaded **relative to the process cwd** (`run.rs:2242-2251`), so a shipped folder only runs when
launched from the right directory. Resolving assets against `std::env::current_exe()`'s parent with
cwd as fallback is three lines and it is a shipping blocker, not a nicety (survey gap 11).

**ADR-H (projects and the Hub)** must additionally decide that the editor's own preferences are TOML
at `$XDG_CONFIG_HOME/loom/editor.toml`, carrying recents, window geometry, dock layout, zoom,
reduce-motion, high-contrast and the onboarding flag. The argument is the project's own: settings are
authored state, and authored state in this codebase is diffable text.

**ADRs A–D (the four painting systems)** carry a documentation consequence worth stating in each:
**if a paint stroke becomes a stroke/region list on a component, its reference page is generated
like every other component and costs nothing. If any of them becomes a bitmap, that system's
documentation has to explain an artifact the user cannot diff, the agent cannot review and Ctrl+Z
cannot reach** — and the sentence "everything you author is text you can read" stops being true,
which is the sentence `05-you-and-the-agent.md` is built on. That is a documentation cost of the
bitmap option and it belongs in the ADRs' consequences.

---

## 12. The ADR this design needs

One, and I want to be honest that a reviewer could reasonably call it a convention rather than a
decision. I think it clears the bar because it constrains every future editor feature and because it
is the mechanism that keeps a whole reference file from drifting.

> ### ADR 00XX — Every editor action is a row in one command table
>
> **Decision.** The editor has exactly one command vocabulary: `loom_editor::command::COMMANDS`, a
> `&'static [Command]` of plain data. The command palette, the menus, the toolbar and its tooltips,
> the F1 help, the displayed keybindings and `docs/guide/04-reference/commands.md` are all views of
> that table, and none of them carries a second copy of a command's title, help text, keybinding,
> availability rule or transaction label. Keybindings are resolved through `loom_input::ActionMap`
> from `assets/input/default.toml` rather than typed into the table, so the documented key and the
> key that fires are one lookup. **Adding an editor action means adding a row.** An action with no
> row is not in the palette, is not in the reference, and does not exist. `cargo xtask docs --check`
> fails when the committed reference differs from what the table would produce, and joins
> `scripts/green.sh`.
>
> **Consequences.** A dev-only or hidden action needs an explicit `Visibility::Hidden` rather than
> simply being absent, so hiding is a choice on the record. The table is data, not a trait, so
> never-do #12 does not fire. `xtask` gains a dependency on `loom_editor`, which must be checked
> against `scripts/check-deps.sh`. Commands whose preconditions are unmet are shown greyed with the
> reason, never hidden — a command you cannot see is one you will never learn.

The generated **component** reference needs no ADR: the constraints survey (§5) already places
end-user documentation outside approval, and generating rather than hand-writing it is a plain
implementation choice. What it does need is a line in `CLAUDE.md` beside the `loom_field::all()` and
`GOLDEN` rules: **documenting a component means writing its doc comment; never write component
fields into prose.**

---

## 13. Files and modules

New, in `crates/loom_editor/` (the crate ADR-F creates):

| Module | What it holds |
| --- | --- |
| `command.rs` | `Command`, `Needs`, `COMMANDS`, and the one `match` from `id` to an editor intent |
| `palette.rs` | the fuzzy matcher (~30 lines) and the palette window over four candidate sources |
| `help.rs` | F1 popovers rendered from `TypeRegistry::describe`; `PANEL_HELP`; `explain(&FieldError)` |
| `problems.rs` | the Problems panel; validation errors, sanity findings, build output |
| `history.rs` | the History panel, the agent rule, the dynamic Undo label |
| `theme.rs` | the §10 tokens applied to `egui::Style`/`Visuals`; high-contrast and zoom |
| `icons.rs` | ~14 painter icons |
| `onboard.rs` | the Hub, template instantiation, the four-step task strip, prefs read/write |

Changed:

| File | Change |
| --- | --- |
| `xtask/src/main.rs` | `"docs" => docs(check)`, alongside `image`/`flythrough`/`shimmer` (`:227-230`) |
| `scripts/green.sh` | add `cargo xtask docs --check` |
| `crates/loom_cli/src/run.rs:2242-2251` | resolve `assets/` against `current_exe()`, cwd as fallback |
| `crates/loom_render/Cargo.toml` | Inter font files (ADR-E), pinned exactly |
| `CLAUDE.md` | the doc-comment rule (§3) |

New content: `docs/guide/**` (six prose files, two generated), `assets/templates/{empty,first_person,third_person}/`, `assets/fonts/Inter-{Regular,SemiBold}.ttf` + `LICENSE-OFL.txt`.

Untouched, deliberately: `loom_scene/src/edit.rs` and `ops.rs`, `loom_reflect/src/lib.rs`. **Every
mechanism in this design reads those; none of them changes one.** If an implementation of this doc
finds itself editing `edit.rs`, something has gone wrong — most likely an attempt to make undo
survive a reload, which §8 explains is the wrong fix for the right complaint.

---

## 14. What I could not verify

Design phase; no builds were run, per the brief. These are the specific things I would check first.

- **That schemars puts a struct's own doc comment in the root schema's `description`.** §5's
  component-header tooltips depend on it. I confirmed the *field* case by reading the code that
  consumes it (`loom_reflect/src/lib.rs:117-120`) but found no consumer of the root description, so
  I am inferring from schemars' behaviour rather than from this codebase. If it lands in `title` or
  is absent, that tooltip needs a different source.
- **Whether `Session` exposes `history()` publicly.** I read the field (`edit.rs:182`) and the
  survey says the console shows the labels, so an accessor almost certainly exists — but I did not
  find its signature, and §8's History panel needs it plus the undo/redo depths.
- **How many fields actually carry a `default`.** `add_component` skips those that do not
  (`run.rs:1607-1610`), which proves some do not, but not which or how many. If it is most of them,
  the reference's default column is mostly `—` and the table wants a different shape.
- **The contrast ratios in §10 are hand-computed** from sRGB relative luminance. The arithmetic is
  simple and I believe it, but I ran no checker, and `fg_2` at 3.0:1 is the one sitting exactly on a
  threshold.
- **egui 0.35's exact API names** — `Context::set_zoom_factor`, the `Visuals` fields the theme
  writes, `animate_bool_with_time`'s signature. I did not read the egui source for these and
  never-do #5's reasoning ("recalled shapes are confidently wrong") applies to egui as much as to
  `ash`.
- **Whether `xtask` may depend on `loom_editor`** without tripping `scripts/check-deps.sh`. The
  three stated rules do not forbid it, but the script greps for `use ash` outside `loom_render*` and
  I have not traced whether pulling `loom_editor` into xtask's graph does anything the script
  objects to. It also means `cargo xtask docs` builds the editor, which is a compile-time cost on a
  project whose resequencing trigger is a one-minute warm build.
- **The size of the generated `components.md`.** Twenty-four types with nested `$defs` expanded is
  plausibly 1,500+ lines. One file or one file per type is a judgement I would make after seeing it,
  not before; the generator is the same either way.
- **Whether the error-code grep test has acceptable false-negative behaviour.** I described it (§7)
  as fifteen lines and marked its ceiling, but I did not enumerate the construction sites to check
  that `error:` as a string is actually the shape they all take.
- **Everything about the painting systems' documentation is conditional on ADRs A–D.** §11 states
  the consequence of each branch; I cannot write the how-to for a system whose authored artifact has
  not been decided.
