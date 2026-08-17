# Review — the usability lens

*Adversarial review of `docs/design/editor/00`–`07`, read in full at `62f9ebe`. The brief for this
editor is "sleek, good-looking, and above all easy to use." This document assumes that is the
acceptance criterion and judges the set against it. Every citation is `doc §` or `file:line` and was
read; nothing was built or run.*

**Verdict up front.** The set is technically excellent and usability-incomplete. It is at its best
where it is closest to Vulkan and at its worst where it is closest to the user. Seven documents
spend roughly 7,000 lines specifying the render-graph consequences of a docked viewport, the mip
chain of a paint mask, and the import table of a cross-compiled PE — and **no document specifies the
inspector**, which is the surface a user touches more than everything else in the set combined.
Where two documents do cover the same ground, they disagree: on the theme, on the icons, on the
fonts, on the panel set, and on which key opens the command palette. An implementer handed this set
today would have to invent the most-used half of the editor and arbitrate the other half.

That is fixable, and the fixes are small relative to what has already been done. What follows is the
list, worst first.

---

## 1. The five worst usability decisions

### 1.1 Nobody designed the inspector, and every document points at a document that does not exist

`05 §6` defers component gizmos: *"they belong with the inspector design, not this one."*
`07 §5` specifies inspector *tooltips* and `07 §10` specifies its *label column width*, and neither
specifies a widget. `01 §2.2` lists `Inspector` as a tab and dispatches to `panels/inspector.rs`
with no content. So the inspector design is referenced three times and written zero times.

This is not a gap at the edge. The existing-editor survey names the consequence itself
(`00-survey-existing.md §5`): strings are **read-only labels**, so `Script.path`, `Hud.text`, every
material texture alias and every mesh alias can be read and not typed; there is no enum widget, no
asset picker, no colour picker, no override marker, no revert affordance, and multi-selection shows
*a count*. The engine-surface survey puts a number on it — *"roughly a third of the authored surface
of this engine is display-only in the editor today"* — and locates it in one `match`.

The user-visible result of shipping this set as written: a new user creates a cube, opens the
inspector, and cannot change its colour except as three unlabelled drag values, cannot attach a
script, cannot pick a mesh except by clicking a button in a different panel that applies to their
entire selection, and cannot tell that a field they are editing is a prefab override. Every one of
those is a first-hour experience. No amount of dock-tab polish compensates.

The same silence covers the **Hierarchy** (no search, no collapse, no scroll-to-selection, no
virtualization — all named as absent in the survey, none designed), the **Project browser** (`01`
gives it a full-width strip and a thumbnail justification; `02 §5` explicitly declines to build asset
identity, thumbnails or an import step; `05 §13` needs a `Declare` op for mesh import "which is the
asset panel's whole reason to exist" — three documents, three different assumptions, no panel), and
the **material workflow** (there is none; `Material` is inspector-numeric, and `03 §6` discovers in
passing that inspector edits to `Material` **probably do not reach the GPU at all today**).

**Fix.** Add doc `09-inspector-and-panels.md` and make it the *first* thing built, ahead of docking.
It owns: the widget table keyed off the schema (string, enum via `oneOf`+`const`, `AssetRef` picker,
`[f32;3]` colour detection, array-of-object rows via `05`'s `SpliceArray`), override display and
per-field revert (`RevertOverrides` exists and the editor has never issued it), multi-edit semantics,
and the hierarchy's search/collapse/virtualize behaviour. `01`'s theme step 4 — *"theme.rs alone,
over the old panels"* — is the right shape of experiment and should be applied here: fix the widget
table over the *old* layout first, and the editor is measurably more usable before a single tab
moves. The docking work is the part users notice least and it is the part specified most.

### 1.2 The visual and interaction language is specified twice, incompatibly, so it is specified zero times

`01 §6` and `07 §10` both give a complete, confident, contrast-checked design system. They are
different systems. An implementer must pick, and neither document acknowledges the other exists.

| | `01 §6` | `07 §10` | Consequence |
| --- | --- | --- | --- |
| Command palette | **Ctrl+K** (`§7`, rejecting Ctrl+Shift+P because that is Pause) | **Ctrl+P** (`§1`, one of "two keystrokes" the whole doc hangs off) | `01 §7` binds **Ctrl+P to Play**. `07`'s spine keystroke starts the simulation. |
| Accent | `#A78BFA` | `#7C5CFF` | two violets, two contrast tables |
| Agent colour | `#78C8FF` "**unchanged**" — and it is, `panels.rs:679` is `rgb(120,200,255)` | `#34D3C0` teal | `07` silently changes a colour `01` pins as invariant |
| `line` / `bg_raised` / `error` | `#262C35` / `#1E232A` / `#F0736D` | `#2A2F39` / `#1E222A` / `#F2555A` | near-miss values, worst kind to reconcile |
| Icons | **`egui-phosphor = "=0.13.0"`**, "rejected: hand-drawn `egui::Shape` paths (a week and looks it)" | **hand-drawn painter geometry**, "rejected: an icon font" | each doc's *rejected* option is the other's decision |
| Fonts | egui's bundled fonts first; Inter only if the human still reads it as default | ship **Inter now**, ~600 KB, `include_bytes!` | opposite sequencing, both feed ADR-E |
| Type scale | 14/13/13/11/12 | 11/13/13/15/18/24 | two scales |
| Row height | `interact_size.y = 22` | row height **24** | two |
| Panel set | `Tab` enum: Scene, Game, Hierarchy, Inspector, Project, Console, Transactions, Prefabs, Environment, Terrain, Events, Profiler | modules: command, palette, help, **problems**, **history**, theme, icons, onboard | `07`'s Problems panel — *"replaces the console as the place you look"* — **has no tab in `01`'s layout**, and neither does History |
| Crate / file | `crates/loom_cli/src/editor/theme.rs` | `crates/loom_editor/src/theme.rs` | two theme files |

This is worse than an unresolved question, because both documents read as settled. The Ctrl+P
collision in particular is the kind of thing that ships: `07` names it a spine, `01` names it Play,
and nothing in either doc raises a flag.

**Fix.** One document owns the design system and the keymap; the others reference it. Ship `01`'s
palette (it is the one with the axis-hue argument and the computed ratios), `07`'s type scale (it has
a stated reason for each size), `07`'s hand-drawn icons for v1 (`egui-phosphor` is a fine dependency
but it postpones ADR-E for one screenful of geometry, and stroke weight matching the hand-drawn gizmo
handles is a real argument `01` does not answer), and **Ctrl+K for the palette with Play moved off
Ctrl+P entirely** — F5 is the industry key for Play and is unbound here. Add `Problems` and `History`
to the `Tab` enum or delete them from `07`; a validation error currently has nowhere to appear in the
default layout.

### 1.3 "All four painting systems" is four tools with three brush models, two commit models, and two contradictory positions on erasing

The brief asked for four painting systems. The set delivers four, and they do not share a brush.

| | splat / vertex (`03`) | UV paint (`04`) | decals (`04`) |
| --- | --- | --- | --- |
| Brush type | typed `Brush { radius, hardness, strength, flow, spacing }` | untyped `Vec<serde_json::Value>`, `kind` discriminator | n/a — a node with a transform |
| `radius` units | **world metres** ("a screen-space radius cannot be serialised") | **UV fraction** (`0.031`) | n/a |
| Erase | **"there is no `mode = "erase"` field** — erasing is painting toward zero, and a separate mode would be a second spelling of the same arithmetic" | `{ kind = "erase", … }` — a separate mode | n/a |
| Commit | per drag-segment, ~10 tx/sec, `apply_coalescing`, gesture key | **once, on mouse-up**, `apply`, no coalescing | `SetTransform` gesture |
| Preview | derived from text via a bake cache | **explicitly permitted to diverge from the scene text** | n/a |
| Code home | new crate `loom_paint` | module `loom_asset::paint` | `loom_cli/src/scene_view.rs` |

Every row is a difference a user feels. The radius slider means metres in one mode and a UV fraction
in another, so the *same brush size number* paints a 1.5 m footprint on terrain and a wall-spanning
smear on a 20 m wall. Undo granularity differs: on terrain a drag is one Ctrl+Z per gesture *via
coalescing*, on a wall it is one Ctrl+Z per *stroke* because nothing is written until mouse-up — the
same to the user by luck, not by design, and it diverges the moment either doc is changed. And one
doc argues at length that an erase mode is redundant while its sibling ships one.

There is also no UI for the thing a painter does most: **there is no layer stack, no mode picker, no
statement of what happens when you click a surface that has no paint component yet, and no
explanation of how the four composite.** `04 §1.3` picks a shader order (paint under the ground
layer, decals after) and calls it "a judgement call"; nothing surfaces that order to the user.
`03 §14` defers "a paint-layer stack with visibility toggles" — that is not a nice-to-have, it is how
a person fixes a mistake without erasing by hand.

**Fix.** One `loom_paint::Brush` and one `Stroke` type with a coordinate-space tag
(`World { points: Vec<[f32;3]> }` / `Uv { points: Vec<[f32;2]> }`), typed in both cases —
`03`'s argument for typing over the `VoxelVolume.ops` precedent is correct and `04` should not have
copied the untyped precedent. **Radius is always world metres**, projected into UV at bake time using
the mesh's texel density; that single change makes the brush feel identical across all four modes,
which is what "coherent" means. **One commit model: mouse-up, one transaction** — `04`'s is the
better one and `03`'s per-segment coalescing is write pressure bought for nothing. Erase is
`strength = 0`, per `03`. And a Paint panel with a mode row (Layer / Texture / Vertex / Decal), a
target readout ("painting `quay_wall` · texture 1024 · 51 texels/m"), and a stroke list with
visibility toggles — which costs almost nothing, because the strokes are already an ordered array and
`SpliceArray` already deletes an element.

### 1.4 Voxel sculpting ships the data structure as the user interface

`05 §10` is the most honest section in the set — *"the lie would be presenting sculpting as if it
edits voxels. It does not, it appends to a list"* — and the honesty is precisely the problem, because
the design's response to every consequence is **to show the user the list and warn them**.

What a person sculpting terrain gets: a brush that appends one CSG primitive per stamp (`05 §10.1`:
"a 5 m drag with a 1 m brush is ten ops"), a panel with one row per **stamp** reading
`subtract sphere r=2.4 at 64, 18, 51`, a warning that the list is non-commutative, a warning that
deleting op 7 of 40 "will change the shape in ways that look like corruption", a bake-time counter
that warns past a threshold, and **no smooth brush and no flatten brush** — deferred in `§10.6`
"until a terrain author asks for them twice." Smooth and flatten are the second and third tools every
sculptor reaches for. Deferring them guarantees the first session ends in "this is unusable", and no
amount of correct reasoning about op kinds changes that.

Two further leaks. The op list *only grows* (`§10.7`) with "no honest compactor", so a long session
degrades continuously with the only remedy being a future CLI command. And `05 §16.3` admits the
central cost — a re-bake during a stroke — **is unmeasured**, while `§10.5` builds an incremental
preview on top of it whose agreement with the truth is *also* unverified (`§16.4`). The design is
sound; the experience it describes is a text editor for CSG.

**Fix, three parts, none large.**
(a) **Group by stroke, not by stamp.** Give every op an optional `stroke` integer written by the
brush. The panel shows one row per stroke — *"Carve path · 10 stamps"* — and "delete stroke" is one
`SpliceArray` over the run. That is one extra field and it turns a 400-row wall of text into a
40-row history a person can read. It stays diffable and the agent can write it.
(b) **Ship `smooth` and `flatten` as op kinds before the sculpt UI**, not after. `05 §10.6` already
establishes they are representable, deterministic and diffable; the only open item is bake cost,
which `§16.3` says must be measured first anyway. Measure once, add both.
(c) Take `§16.3`'s measurement **before** the panel is drawn, and if incremental preview cannot be
proven bit-identical to `bake` (`§16.4`), fall back to re-bake on release *by design* rather than
discovering it.

### 1.5 The default path from Hub to a moving character does not produce a moving character

Trace it as a user who has read nothing, which is the brief's test.

Hub (`02 §7`) → **New Project** → a form with Name, Location, Template. The templates are named
`empty`, `first_person`, `third_person` (`02 §8`) and the empty state "shows the three template
cards" with no statement of what they do beyond their names. A first-time user picks the one that
sounds like a starting point — Empty — because that is what "empty project" means everywhere else.

They get the base scene (`02 §9`): ground, a cube, a light, and **a `Camera` with no
`CharacterController`**. The onboarding strip (`07 §6`) then instructs them, in step 3, to **Press
Play**. `02 §9` states what happens when a scene has one half of the rig and not the other: the
console prints *"no player rig — flying instead"* — and calls that "the exact failure the engine
survey says should be a UI state rather than a log line." The design identifies this failure, names
it unacceptable, and then routes its own default onboarding path straight into it.

Two more problems on the same path. `third_person` **ships documented-broken**: it depends on
`Camera.boom`, which `02 §9` proposes and admits does not exist, with a stated fallback that
"is playable and it pitches wrong" and a suggestion to say so in a header comment. Shipping a
template whose header comment apologises for it is worse than shipping two templates. And nothing on
the first-run path teaches **camera control**: look is right-mouse-held (`assets/input/default.toml`,
verified), the four-step strip never mentions it, and a user who left-drags will marquee-select
(`05 §5`) rather than orbit.

**Fix.** Default the New Project template to **First person**, and name templates by outcome, not by
implementation: *"Walk around (first person)" · "Follow a character (third person)" · "Blank scene"*,
in that order, with the blank one last. Make Play on a rig-less scene the **UI state the design
already says it should be**: a viewport banner *"No player in this scene — flying camera instead"*
with an **Add Player** button that spawns the `CharacterController` + child `Camera` + `Script` in one
transaction. That button is the single highest-value affordance in this entire set: it converts the
worst first-run outcome into a one-click success and it is ~20 lines over ops that all exist. Hold
`third_person` until `Camera.boom` lands. Add a fifth strip step, first: **Look around** — completes
on the first right-drag.

---

## 2. Set-level incoherence beyond the theme

These are not usability opinions; they are two documents specifying incompatible things.

**The viewport is a rect in `01` and a texture in `05`.** `01 §1` chooses render-to-sub-rect over
render-to-texture, at length, and calls it "the one decision that matters". `05 §6.9` says: *"This is
a hard dependency on the render-to-texture viewport (**ADR I**) and the authoring layer cannot be
finished before it."* `05` was written against the design `01` rejects. The *coordinate* consequence
is the same either way (a rect origin and extent), so the damage is limited — but `05`'s build order
now blocks on an ADR that will not be written, and nobody reconciled them.

**The shipped runtime cannot draw its own HUD.** `06 §1` makes egui optional behind a non-default
`editor` feature and `06 §6.6` promotes *"the shipped binary contains no egui"* to a green-check rule
in `scripts/check-deps.sh`. But `crates/loom_cli/src/hud.rs:16` is `use loom_render::egui;` and
`hud.rs:137` takes `&mut egui::Ui` — the **game's** HUD, authored as scene content, drawn during
Play, listed in `06 §1`'s own set of nine modules the shipped binary needs. `02 §6` spotted this and
says so explicitly: *"the shipped runtime links egui regardless, and 'stripping the editor' means not
linking `loom_editor` — not making egui optional in `loom_render`."* `06` did not read `02`. As
written, ADR A either fails its own CI rule or ships a game with no score display. The user-visible
consequence is the whole point of `Hud` being scene content.

**ADR numbers collide.** `01 §10` claims **ADR 0022** for the viewport sub-rect and **0023** for the
CMAA2 reorder. `02 §11` claims **ADR 0022** for "a project is a directory with a `loom.toml`" and
**0023** for asset path resolution. `04 §4.3` hedges ("next free number is 0022; the exact number
depends on the sibling docs"), `05 §13` and `06 §8` use letters. Four documents, three numbering
schemes, two direct collisions. Trivial to fix and guaranteed to cause a bad merge if it is not.

**`01`'s default layout is not the Unity layout it claims to copy.** `01 §3` says copying Unity
exactly buys "the only free familiarity available" — then adds a second full-width bottom strip Unity
does not have. Unity puts Project and Console as *tabs of one bottom node*; `01` stacks
`Console|Transactions|Events` at 180pt **and** `Project` at 160pt, under a 28px menu, a 36px toolbar
and above a 22px status bar. At 1920×1080 that is a viewport of roughly 1340×654 — 42% of the window
— in an editor whose subject is a 3D scene. The justification for the separate Project strip is that
"it will hold thumbnails and folders", a feature `02 §5` explicitly declines to build.

**The agent is the differentiator and the layout demotes it.** The one thing this editor has that no
other has is a human and an agent authoring the same file live. In the default layout that surfaces
as a `Transactions` tab sharing a node with Console and Events (so it is hidden by default), plus
"⌁ agent idle" in a 22px status bar. Meanwhile six tabs no other engine has — Terrain, Environment,
Events, Profiler, Prefabs, Transactions — are in the vocabulary with **no design for any of them**.
If the default layout should express anything, it is the co-authoring model.

---

## 3. The brief's questions, answered directly

**Is the default layout good, or Unity cargo-culted?** Cargo-culted in the arrangement and
under-thought in the contents. Copying Unity's four-region arrangement is genuinely defensible and
`01 §3` defends it well. But the doc then departs from Unity in the one place Unity is right (one
bottom node, not two), keeps twelve tabs it does not design, omits the two panels its sibling calls
essential (Problems, History), and never asks what *this* editor's distinguishing surface is.

**Is the voxel sculpting UX honest and usable?** Honest, emphatically. Usable, no — see §1.4. The
data model is not merely visible, it *is* the interface, and the two brushes that would hide it are
deferred.

**Can a new user get from hub to a moving character without reading docs?** No — see §1.5. They can
if they pick the right template, which nothing tells them to do, and the default path runs into a
failure the design itself calls unacceptable.

**Is the painting UX coherent across four modes?** No — see §1.3. Two stroke schemas, two commit
models, two units for `radius`, contradictory erase semantics, no mode picker, no layer stack.

**Is the visual design specified concretely enough to build?** It is specified twice and therefore
not at all — see §1.2. Individually, each specification *is* concrete enough: `01 §6` and `07 §10`
both give hexes, computed contrast ratios, spacing scales, radii and named egui fields. That is
better than most design docs manage.

**Would it actually look good?** Probably yes, with one reservation. The two strongest calls in the
set are `07 §10`'s *"the chrome is greyscale; every colour in the interface is data"* — a rule a
single developer can actually hold, and the reason a dense tool stays scannable — and `01 §6.2`'s
monospace numeric fields, which stop a transform inspector jittering under a drag. The violet accent
is argued from hue collision with the three axis colours and the agent mark rather than from taste,
which is the right way to pick an accent. The reservation is `01 §6.1`'s deliberate choice to carry
surface separation on 1px strokes with a 1.12:1 fill difference: correct in principle, and the first
thing to look wrong on a display with any gamma deviation. Judge it at `01 §11` step 4, which is
exactly what that step exists for.

---

## 4. Remaining findings, ranked

**HIGH — `01 §7` refuses W/E/R for gizmo modes on a premise it did not check.** The stated reason is
that W and E fly the camera, "an inherited constraint … written here so it is refused once." Verified
in `assets/input/default.toml:11-27`: `move_forward` is `KeyW`, `move_up` is `KeyE`, and **`look` is
`MouseRight`, held**. Unity and Unreal both resolve this identically — fly keys are live *only while
the look button is held*. Gating the `fly` context on `look` is one condition and it frees W, E, R,
Q and F for the bindings every 3D artist already has in their fingers. Refusing the industry-standard
keymap forever, to preserve a binding that is only reachable while the right button is down, is a
usability cost paid for nothing. Keep 1/2/3 as aliases.

**HIGH — `04 §4.1` discards a paint stroke on a version-token rejection, and reads never-do #15 more
strictly than it is written.** The doc argues that replaying the stroke onto reloaded text is an
auto-merge and therefore forbidden, concluding *"losing at most one stroke is the correct price."*
`CLAUDE.md` says the opposite in its own words: *"**Expect version-token rejections** and handle them
by re-reading and re-applying, not by forcing the write."* Re-applying an authored action the user
just performed, against text they can see, is the prescribed handling — auto-merging is silently
reconciling two *divergent states*, which this is not. In an editor whose premise is that an agent
writes the same file concurrently, "your stroke is gone, see the console" will happen often.
**Fix:** hold the stroke, raise the existing divergence banner with a third button — *Reapply my
stroke* — alongside the two that exist. The user chooses; nothing merges.

**HIGH — `03 §6` finds that inspector `Material` edits probably never reach the GPU today, and no
slice owns fixing it.** *"As far as reading shows, an inspector edit to `Material.roughness` in
`loom run --edit` does not reach the GPU"* — `Materials::new` uploads in the constructor and `Viewer`
has no `set_materials`. This is filed as a prerequisite of painting. It is a standalone usability
defect on the most-used inspector component in the engine, it predates this rework, and it should be
a slice of its own with `03 §13.2`'s check ("change a roughness slider and look") run first.

**MEDIUM — `01 §1.7` forecloses undocking the viewport, and that is a real workflow, not an edge
case.** `allowed_in_windows` returns `false` for `Scene` and `Game`, permanently. Dragging the game
view to a second monitor is standard practice for anyone with two displays, and this box has a
multi-head desk (`LOOM_WINDOW_AT` exists because the gate scatters five windows). The trade is
defensible; it should be stated in the *user* documentation as a limitation rather than only in an
ADR's consequences, and the trigger for the texture upgrade should include it explicitly.

**MEDIUM — pointer capture during Play in a docked Game tab is "noted rather than solved"
(`01 §1.6`).** `CursorGrabMode::Confined` confines to the window, not a sub-rect. So the flagship
"press Play and walk around" flow has an unresolved input behaviour in exactly the layout this rework
introduces. `Locked` covers it in the common case; the fallback path does not, and the fallback is
what fires on the machines that need it.

**MEDIUM — the base scene's ground is a scaled box, so the terrain tooling has nothing to act on**
(`02 §9`). The reasoning is good (a `VoxelVolume` renders in the inspector as *"3 items"*, so a
beginner's first terrain encounter is untouchable). But the consequence is that the sculpt brush —
one of the two headline authoring tools — has no target in the scene every new project opens to, and
"add terrain" is not in the create menu (`05 §4` creates six primitives and no volume). Add
**Create → Terrain** issuing `SpawnNode` + a `VoxelVolume` with one `terrain` op, so the brush has
something to bite on the first time it is armed.

**MEDIUM — painting a prefab instance duplicates the whole stroke array as an override, and the
mitigation is documentation** (`03 §2`: *"worth saying out loud in the docs so nobody reports it as
bloat"*). Correct per ADR 0008 and genuinely unavoidable, but the inspector must *show* it — a
per-instance stroke count and an "overridden" bar — or the first user to paint six instances of a
crate will find a scene file six times larger with no explanation in the interface.

**MEDIUM — `07 §6`'s onboarding strip detects real ops, which is right, but its four steps teach the
wrong four things.** Add / Move / Play / Save omits looking around (§1.5) and omits saving's actual
hazard, which is the divergence banner. Five steps: Look · Add · Move · Play · Save.

**LOW — discoverability of the shell's own bindings.** Shift+Space maximise, Ctrl+1…9 tab focus and
`` ` `` for previous node (`01 §7`) are unguessable and appear in no menu. `07`'s command table is
the right home for them; `01`'s table is not wired to it. One more reason the two documents need to
merge their keymap.

**LOW — `02 §12.9` leaves the file picker unresolved** ("the text field is the lazy answer and I
would ship it first … a hub with no file picker will feel unfinished"). Correct on both counts. For
a launcher — the first screen anyone sees — typed paths are the wrong first impression. `rfd` behind
ADR-E, or an in-app directory browser over `read_dir`, which is ~60 lines and takes no dependency.

---

## 5. What the set gets right, briefly

Worth recording so the fixes above do not read as a rejection. `05 §1`'s `Outcome` enum makes
never-do #16 a type rather than a discipline — a tool has no `&mut Session` and nowhere to put
non-op state, which is the strongest structural idea in the set. `07 §3`'s generated component
reference with `cargo xtask docs --check` makes drifting documentation impossible rather than
discouraged. `07 §8` is the only place in this repo that has noticed that **an agent write destroys
the human's saved undo history silently**, and its four affordances — a History panel, a rule drawn
where the agent wrote, an Undo button that names its target and its op count, the dragging dot —
teach the co-authoring model without a paragraph of prose. `01 §1`'s render-to-rect keeps
`loom render` and `loom run` byte-identical, which is the invariant this project has paid three
defects for. `06 §4.5`'s V0–V6 sequencing, and its refusal to say "Windows supported" when it means
"it started under Wine on this machine", is exactly the honesty the operating rules ask for.

---

## 6. What I could not check

Design phase; nothing was built or run, per the brief.

- **Whether the two theme tables actually look different in a window.** The hexes differ; whether the
  difference is visible is a judgement made by looking, which `01 §11` step 4 exists to enable.
- **Whether `05`'s "hard dependency on ADR I" is a real blocker or a stale sentence.** The coordinate
  work is identical under either viewport design, so I believe the damage is a build-order reference
  and nothing more; I did not trace every consumer.
- **Whether a shipped `loom-play` can be made to draw a HUD without egui.** I verified `hud.rs` uses
  egui and that `06` gates egui out; I did not evaluate how much of the HUD path could survive a
  reimplementation, only that `06` does not mention the problem and `02` says it is fatal.
- **The 42% viewport figure** is arithmetic on `01 §3`'s stated point sizes at scale 1.0, not a
  screenshot.
- **Whether gating the `fly` context on `look` breaks anything else.** I read
  `assets/input/default.toml` in full and the survey's account of key latching; I did not read
  `loom_input`'s context-switching code, so "one condition" is an estimate.
- **Every claim about what a first-time user would do.** No user was observed. `05 §15` is right that
  "whether the gizmo feels attached rather than geared" and "whether the sculpt list is
  comprehensible at forty ops" need a session with a human, and so does most of §1 above.
