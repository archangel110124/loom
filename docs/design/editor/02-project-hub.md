# Design — the project model, the Hub, and the scenes a new project opens to

*Editor rework, design phase. Written against `62f9ebe` after reading
`00-survey-existing.md`, `00-survey-engine-surface.md` and `00-survey-constraints.md`.
Nothing here was built or run — every claim about existing code is cited to `file:line`
and was read; every claim about behaviour I could not read is in §12.*

---

## 0. The shape of the answer, before the detail

**A Loom project is a directory containing a `loom.toml`. That is the entire concept, and
almost everything else in this document falls out of refusing to make it more than that.**

There is no asset database, no import step, no project-scoped registry and no `.loom/`
dropping in the user's folder. The hub finds scenes by globbing, thumbnails by shelling
out to `loom render`, and templates by copying a directory. Every one of those is the
laziest thing that works, and each is defended below against the more elaborate version I
rejected.

The three decisions that carry the most weight, stated up front so a reader who disagrees
can stop here and argue:

1. **The project's folder layout is chosen so that today's path resolution already works.**
   `MeshLibrary::with_cache` joins an `[[asset]]`'s advisory `path` onto the scene file's
   own parent directory (`crates/loom_cli/src/main.rs:571`, `:1163`), which is why
   `assets/test/props.loom` reaches its mesh as `../meshes/rock_beach.obj`. A project
   layout of `scenes/` beside `assets/` makes that `../assets/meshes/…` and needs **zero
   engine change**. Every layout that reads better on paper needs a resolver.
2. **The hub is a subcommand of the same binary, not a second executable.** `loom edit`
   with no argument is the hub; with an argument it is the editor. Justified in §6.
3. **A template is a directory that is copied, not an `extends` chain.** A new project must
   be free-standing; `extends` is a live link (`crates/loom_scene/src/scene.rs:54-59`) and
   would make deleting the engine's sample folder break the user's game. §8.

---

## 1. What a project is on disk

```
KiteHollow/
  loom.toml                 the manifest — the only file that makes this a project
  scenes/
    main.loom               what the hub opens
  assets/
    meshes/                 .obj / .gltf
    textures/               .png
    audio/                  .wav
    scripts/                .rhai
    prefabs/                *.loom used as prefabs
    input/
      default.toml          this project's bindings
  builds/                   shipped output — gitignored
```

**Nothing enforces this layout and nothing resolves against it.** It is a convention that
`loom new` creates and the hub displays; a project that puts everything in one flat folder
works identically, because resolution is per-scene-file and relative. The layout exists so
that a human opening someone else's project knows where to look, not so the engine can find
things.

**`scenes/` sits beside `assets/` rather than inside it** so that a scene's asset references
read `../assets/textures/foo.png` — one level up and back down, exactly the shape
`assets/test/*.loom` already uses. Putting scenes inside `assets/scenes/` would read
`../textures/…`, which is marginally shorter and puts authored source inside a folder named
for imported artifacts. Not worth it.

**`builds/` is the ship target's destination** and belongs to the packaging design, not this
one. It is named here only so that the hub's glob knows to skip it — a shipped build
contains a copy of every scene, and a hub that lists them twice is confusing before it is
wrong.

### How this relates to the repo's existing `assets/`

The engine repo becomes a project by adding one file at its root:

```toml
# loom.toml
[project]
format = 1
id = "…"
name = "Loom engine"
main_scene = "assets/games/proving_ground.loom"
```

**No files move.** Moving the fifty scenes in `assets/test/` into a `scenes/` directory
would rewrite every path literal in `xtask/src/main.rs`'s `SCENES` (43 entries) and
`GOLDEN` (28), every reference filename under `tests/references/`, `MANIFEST.txt`, and the
scene paths hardcoded in tests (`crates/loom_cli/src/main.rs:5312` and its neighbours).
That is a large, purely cosmetic diff across the exact files the golden gate's authority
rests on. **Reject it.** The repo is a project with an unusual layout, which is precisely
the case the "layout is a convention, not a contract" rule exists to permit.

The payoff is real: opening the engine repo in the hub exercises the project model against
fifty scenes with imported meshes, textures, scripts, prefabs and terrain recipes on day
one, rather than against three templates the design invented.

### The manifest

```toml
# loom.toml — this directory is a Loom project.
#
# Everything here is advisory except `format`. The engine finds scenes and
# assets by walking the directory and by resolving each scene's own relative
# paths; nothing is registered, and adding a file makes it available.

[project]
format = 1
id = "b91c7a04-3e28-4f5d-8a16-72d0e9c4b135"
name = "Kite Hollow"
main_scene = "scenes/main.loom"

[engine]
version = "0.0.0"
```

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `project.format` | integer | yes | Manifest format version. `1`. Higher than the binary understands is a **load error**, never a best-effort parse — the same rule `[scene] format` follows (`docs/format/README.md` §3). |
| `project.id` | UUID | yes | Stable project identity. The hub keys its recents entries on the *path* and uses `id` only to notice that two entries are the same project moved. |
| `project.name` | string | yes | What the hub card says. Defaults to the directory name at creation and is then the user's to change. |
| `project.main_scene` | path, project-relative | yes | What `loom edit <project>` opens and what the thumbnail renders. |
| `engine.version` | semver string | yes | The `loom` version that last wrote this manifest. Advisory — see §3. |

Five fields. Things deliberately **not** in it, each with the reason:

- **No asset list, no scene list, no manifest of contents.** Globbing `*.loom` under the
  project root answers "what scenes are here" without a file that can be wrong. A declared
  list is a second source of truth that drifts the first time someone copies a file in with
  `cp`, and the failure mode is a scene that exists and is invisible.
- **No dependency or package list.** There is no package ecosystem to depend on.
- **No build settings, no window size, no render settings.** None exists yet. Adding a
  `[window]` table the day a project needs one is additive and free (`docs/format/README.md`
  §9); inventing it now is a table of values nothing reads.
- **No editor layout.** Panel geometry is per-user, not per-project, and belongs beside the
  recents list (§4). Two people opening one project on one machine is not a case this
  engine has.

**Schema-validated on load, like everything else authored here.** The struct derives
`serde::Deserialize` with `#[serde(deny_unknown_fields)]` — so a typo is an error naming the
key rather than a silently ignored line, which is the same defect class as
`prefab_load::for_reading` (a key the parser does not understand is a key it *ignores*, and
that shipped once already). It also derives `JsonSchema`, which costs one line and makes
`loom describe Project` work, so the agent can author a manifest without reading this
document.

---

## 2. Where the code lives

**`loom_scene::project` — a module, not a crate.**

```rust
// crates/loom_scene/src/project.rs
pub struct Project { pub format: u32, pub id: String, pub name: String,
                    pub main_scene: PathBuf, pub engine_version: String, pub root: PathBuf }
pub enum ProjectError { Io(std::io::Error), Toml(String), Format { found: u32, understood: u32 },
                        MissingScene(PathBuf) }

pub fn load(root: &Path) -> Result<Project, ProjectError>;
pub fn save(p: &Project) -> Result<(), ProjectError>;   // format-preserving, via toml_edit
pub fn find_root(start: &Path) -> Option<PathBuf>;      // walk up looking for loom.toml
pub fn scenes(p: &Project) -> Vec<PathBuf>;             // glob, skipping builds/ and target/
```

Roughly 200 lines. It goes in `loom_scene` because that crate already owns every dependency
it needs — `toml_edit = "=0.25.13"`, `serde`, `schemars`, and the format-preserving-DOM
write path that keeps a human's comments in `loom.toml` alive across a save
(`crates/loom_scene/src/ops.rs:157-159`). It adds no workspace dependency, so
`scripts/check-deps.sh:26-31`'s rule that `loom_scene` may depend only on `loom_reflect`
still passes untouched.

**Alternative considered: a `loom_project` crate.** Rejected. It would be one file, it would
need `toml_edit` and `serde` and `schemars` pinned a second time, and it would add a node to
a crate graph whose compile time is explicitly a stop-and-fix condition past one minute warm
(`LOOM-IMPLEMENTATION-ORDER.md:574`). The objection that "a project is not a scene" is real
but weak: `loom_scene` is the authored-text layer of this engine, and a project manifest is
authored text.

**Alternative considered: `loom_asset`.** It owns identity and paths (`meta.rs`) and would be
the right home if projects had an asset database. They do not (§5), so it would be a module
in a crate for reasons the module does not use.

`find_root` is what makes `loom edit some/scene.loom` work from anywhere: walk up from the
scene until a `loom.toml` appears, stop at the filesystem root. No `loom.toml` found is not
an error — it is **scene-only mode**, which is exactly today's `loom run --edit` and is how
every existing workflow and the windowed half of `cargo xtask validate` keeps working.

---

## 3. Pinning the engine version, so an old project is a defined situation

**Two version numbers exist and only one of them is normative.**

`[scene] format` is the contract. A scene file declaring a format higher than the binary
understands is a load error by spec (`docs/format/README.md` §3), migrations run on load,
and §9 defines exactly which changes need a bump. That machinery is already right and this
design does not touch it.

`[engine] version` is **advisory**. It records which `loom` last wrote the manifest, and its
only job is to let the hub produce a good sentence instead of a stack trace. The policy, in
full:

| Situation | What happens |
| --- | --- |
| `engine.version` == `env!("CARGO_PKG_VERSION")` | Open. Nothing is said. |
| Project older | **Open normally.** The card shows "last opened with 0.3.1". The manifest is rewritten to the current version on the first save, not on open — opening a project to look at it must not dirty it. |
| Project **newer** | **Refuse, and offer "Open read-only".** |
| No `loom.toml`, `--` a bare scene | Scene-only mode. No version check, because there is no manifest to check. |

**The newer case is the only one that gates behaviour, and it gates it at the hub rather
than at scene load.** The failure it prevents is not hypothetical: a project written by a
future `loom` may contain `format = 2` scenes, and today those fail inside
`Scene::parse` — which in the editor means a window that opens onto
`show_invalid`'s "last good view" of nothing (`run.rs:504-514`), a hierarchy panel with no
nodes, and no explanation. **One sentence in the hub is worth more than any amount of
graceful degradation downstream.** Read-only is offered because read-only mode already
exists and is already safe: `open_scene` opens a `Session` only when editable
(`run.rs:2306-2309`), so a viewer that cannot write cannot corrupt a project it does not
understand.

**Rejected: refusing to open older projects, or running a project-level migration.** There is
nothing to migrate. A project is five manifest fields and a directory of files whose own
formats carry their own versions. A project-level migration system would be scaffolding for
a migration that has never been needed.

**Rejected: pinning the engine per project the way a toolchain file does** (`loom edit`
downloading or launching the matching engine build). This is a single-developer engine with
no release channel and no distribution. Note it in the ADR as the upgrade path if the engine
ever ships to more than one person.

---

## 4. Where the hub's own state lives

Recents are **user state, not project state** — the constraint survey names this (§H) and it
is the first time this repo has needed a location outside its own tree.

```
$XDG_STATE_HOME/loom/hub.toml        default ~/.local/state/loom/hub.toml
$XDG_CACHE_HOME/loom/thumbs/<hash>.png   default ~/.cache/loom/thumbs/
```

**State, not config**, because every byte of it is written by the program rather than by the
human. `~/.config` is for files a person edits; a recents list is not one. `~/.cache` is for
files whose loss costs a re-render, which is exactly what a thumbnail is.

```toml
# ~/.local/state/loom/hub.toml
# Written by the Loom hub. Safe to delete — you lose the recents list.
format = 1
new_project_dir = "/home/k-dorui/projects"

[[recent]]
path = "/home/k-dorui/projects/KiteHollow"
name = "Kite Hollow"
last_opened = 1786742400          # unix seconds
engine_version = "0.0.0"
```

**One file, and `new_project_dir` is in it under protest.** A default directory for new
projects is a preference and by XDG's letter belongs in `~/.config/loom/`. It is here
because it is the only preference that exists, and a second file holding one string is worse
than a slightly impure first one.

```rust
// ponytail: one state file until there is a second kind of preference.
// Split new_project_dir into ~/.config/loom/hub.toml when the second one lands.
```

XDG resolution is `std::env::var("XDG_STATE_HOME")` falling back to `$HOME/.local/state` —
six lines. **Rejected: the `directories` or `dirs` crate.** Two environment lookups on one
platform, with no macOS target, no Windows editor target and no web target, do not justify a
pinned dependency and its transitive tree.

**A missing directory is never auto-pruned from the list.** A project on an unmounted drive
or a detached USB stick is not a deleted project. The card renders greyed with "not found at
this path" and a Remove button, which is both the honest behaviour and less code than
deciding when absence is permanent.

### Thumbnails

**The hub's thumbnails are the engine's own headless renderer, invoked as a subprocess of
itself.**

```
std::env::current_exe()  render <project>/<main_scene>  --size 480x270  --out ~/.cache/loom/thumbs/<blake3 of canonical project path>.png
```

That is the whole implementation: one `std::process::Command` on a background thread, at
most one in flight, results dropped into the cache and picked up on the next repaint. It
reuses `loom render` exactly as it stands (`main.rs` dispatch, `--size`, `--out`), which is
the CLI-first property applied to the editor's own furniture. A failure produces no
thumbnail and no dialog — the card falls back to a flat colour derived from the project id,
which is a legible placeholder rather than a broken-image icon.

Generated **lazily on first sight of a card with no cached image, and refreshed when the
main scene's mtime is newer than the thumbnail's.** Not on open, not on close: a render at
close makes quitting slow for a benefit nobody is looking at.

**Rejected: capturing the editor's own framebuffer on project close.** It needs a readback
from the window path, which I could not confirm exists (§12), it puts a
GPU-stall-and-encode on the quit path, and the PNG encoder is measured at ~10 ms fixed plus
~11 ms/megapixel (`CLAUDE.md`) — cheap, but not free, and paid at the worst moment. If a
window readback does turn out to exist and be cheap, swapping the implementation is local to
one function.

**Rejected: an in-process render from the hub.** The hub has no `Device` yet — it is an egui
window with no scene — so this means standing up a second Vulkan instance inside the editor
process to render a picture. A subprocess is the same work with a process boundary that
makes a driver crash in an unfamiliar project someone else's problem.

---

## 5. What the hub deliberately does not do

Recorded so nobody rebuilds it: **the hub does not resurrect `loom_asset::meta`.**

`crates/loom_asset/src/meta.rs` is 160 lines of `AssetId`/`Meta`/`Manifest` — UUID sidecars,
BLAKE3 content hashes, a shipped runtime manifest — and the engine survey confirms it is
**dead code with no caller outside the crate**. It is a good design for a problem this
engine does not have yet, and a project system is exactly the moment somebody would decide
it is time. It is not.

What it would buy is asset identity that survives rename and move. What it costs is an
import step, `.meta` files on disk beside every asset, a manifest that must be regenerated
and can go stale, and the moment where a `.meta` and its file get separated by a `git mv`
and a reference breaks in a way that has no error message. **Today the engine resolves
assets by joining an advisory relative path onto the scene's directory** (`main.rs:1163`,
verified) — a rename breaks the reference immediately, loudly, and at the place the human
just typed. That failure mode is worse in theory and better in practice, and it costs
nothing.

This has a consequence for `loom new` that §8 spends a paragraph on, and it exposes a live
contradiction between spec and implementation that §11 raises as an ADR.

---

## 6. Launching: one binary, a new subcommand

**Recommendation: `loom edit`, in the same binary as everything else.**

```
loom edit                      → the hub
loom edit <dir>                → open that project at its main_scene
loom edit <scene.loom>         → open that scene; project found by walking up, or scene-only
loom new <dir> [--template <name>] [--name <string>]
```

Four reasons, in the order they should convince:

**1. The windowed half of green check 2 drives this binary.** `cargo xtask validate` opens
five windows through `loom run --frames n` (`xtask/src/main.rs:1024`, `:1077`) and tears
them down under the validation layers. A second executable means a second thing the gate has
to launch, name, position with `LOOM_WINDOW_AT`, and keep alive — and the survey is blunt
that if `--frames` and `--play` stop working, the windowed half of the gate goes dark.
Keeping the editor in `loom` keeps the gate's arrangement exactly as it is.

**2. The agent already has this binary, and `loom-mcp` wraps it.** CLI-first is a locked
decision. A hub in a second executable is invisible to the agent; a subcommand is one line
in `USAGE` and one entry in `FLAGS`.

**3. The runtime/editor split is a different axis, and a second binary buys nothing on it.**
The ship target is *runtime versus development tool*, not *hub versus editor* — the shipped
game never contained a hub in any design. That split is ADR F's business (the constraint
survey, §4.F): a separate `loom_editor` crate, and a runtime binary that does not link it.
Worth noting for whoever writes ADR F: **`hud.rs` draws the game's HUD with egui**
(`crates/loom_cli/src/hud.rs:16`), so the shipped runtime links egui regardless, and
"stripping the editor" means not linking `loom_editor` — not making egui optional in
`loom_render`. That materially shrinks ADR F, and it means the hub decision here does not
constrain it.

**4. The alternative's cost is real and its benefit is aesthetic.** A `loom-hub` binary needs
a second `[[bin]]`, a second copy of argument handling, a second entry in every doc, and a
hand-off to the editor that is `exec` with arguments either way. The only genuine argument
for it — that the hub should start fast without paying the editor's link cost — is answered
by the fact that they are the same process and the hub draws before any `Device` exists.

### What happens to `loom run --edit`

**It keeps working, forwarding to the same entry function in scene-only mode.** One line.
`loom run` stays the small, drivable viewer the gate uses (`--frames`, `--play`, read-only,
no `Session`); `loom edit` is the application that grew a hub, a docked layout and a project.
Splitting them is what stops `run.rs`'s flag surface from becoming the editor's front door.

`FLAGS` in `main.rs:143-176` gains two rows, and this is not bookkeeping — **an unknown flag
is a failed invocation, not a no-op**, and the table is the only thing that makes it so:

```rust
("new",  &[("--template", true), ("--name", true)]),
("edit", &[("--frames", true), ("--play", false)]),
```

---

## 7. The hub UI

One window, no docking, no panels. It is a launcher and it should look like one.

**Left: a vertical rail** — Projects · Templates · Learn. Three items, because there are
three things. Learn opens the end-user documentation (§10) in the system browser rather than
rendering Markdown in egui.

**Centre: the recents list, newest first.** Each row is a card:

- **thumbnail** (480×270 cached PNG, or the id-derived flat colour)
- **name**, large; **path**, small and dimmed, `~`-abbreviated
- **last opened**, relative ("2 hours ago", "yesterday", "3 weeks ago") — an absolute
  timestamp is precise and unreadable, and this is the one number a human scans
- **engine note**, only when it differs: "last opened with 0.3.1", or in the newer case a
  warning colour and "needs Loom 0.5 or later"
- **a `⋯` menu** with exactly two items: *Show in files* and *Remove from list*

**Remove from list is worded that way and never any other way.** It removes a row. There is
no "Delete project" anywhere in the hub — deleting a directory tree from a launcher is not a
feature this engine needs, and `SceneOp::RemoveNode` already requires an explicit
`destructive` scope for deleting one *node*. A launcher that can erase a project with a
mis-click is the opposite of that posture.

**Top right: two buttons — New Project and Open.** Open is a directory picker landing on
`new_project_dir`; picking a directory with no `loom.toml` offers *"No project here. Create
one?"*, which is the correct handling of the most common thing a user will do by mistake
and turns an error into the action they wanted.

**New Project is a form, not a wizard.** Name, location, template — three fields on one
panel with a live preview of the resulting path and the template's thumbnail. The Create
button is disabled with the reason shown inline when the target exists or is not writable,
which is the same discipline the toolbar's `add_enabled(editing, …)` already follows in
`panels.rs`.

**Empty state matters more than the populated one**, because it is the first thing anyone
ever sees. With no recents, the centre shows the three template cards directly with a single
line above them: *"Create a project to begin."* No illustration, no tour, no dismissible
tips.

**Whether the hub is a separate window or a full-window state of the editor window** is
worth stating: it is the **same window**, in a different state. Creating a second winit
window means a second surface, a second swapchain and a second teardown order to get wrong —
and `run.rs:294-335` documents two crashes already paid for teardown ordering with one
window. The hub state has no `Device` at all; it is egui over a cleared surface. Choosing a
project transitions the same window into the editor.

Which raises the one genuine wrinkle: **egui needs a Vulkan device to draw**
(`egui-ash-renderer`, `crates/loom_render/src/ui.rs:34-99`), so "the hub has no Device" is
wrong as stated — it needs one, it just needs no `Viewer`. The honest version: **the hub
creates the `Instance`, `Device`, surface, swapchain and `Ui`, and does not create a
`Viewer` until a project is chosen.** That is a real saving (no scene, no meshes, no TLAS,
no render graph) and it is the split the code already has, since `Ui::new` takes the device
and builds its own allocator independently of the viewer's (`ui.rs:40-54`).

---

## 8. Templates and `loom new`

**A template is a project directory under `assets/templates/<name>/`, and creating from it
is a recursive copy plus three edits.**

Each template is a *real project* — it has its own `loom.toml` and can be opened in the hub
directly. That is how templates get tested: they are exercised by the same code path a user
exercises, rather than by a fixture that resembles one.

```
assets/templates/
  empty/            loom.toml  scenes/main.loom
  first_person/     loom.toml  scenes/main.loom  assets/scripts/fps.rhai  assets/input/default.toml
  third_person/     loom.toml  scenes/main.loom  assets/scripts/fps.rhai  assets/input/default.toml
```

**Rejected: templates as `extends` chains.** `extends` is scene inheritance on the root node
(`crates/loom_scene/src/scene.rs:54-59`) and it is a *live* link — editing the template later
changes every project made from it, and deleting the engine's `assets/templates/` breaks
them all. A starting point must be free-standing. `extends` also cannot carry a script file,
an input map or a folder layout, which is most of what a template is. It remains the right
tool for variants of a scene inside one project, which is what it was built for.

**Rejected: templates fetched or generated.** No.

### What `loom new` does

`crates/loom_cli/src/new.rs`, and the hub's Create button calls the same function directly —
not a subprocess. Parity with the agent is a property of the *code path*, not of the process
boundary (this is the same argument `transact`/`transact_as` make one level down).

1. Refuse if the target exists and is non-empty. Refuse before copying anything.
2. Copy the template directory recursively.
3. Rewrite `loom.toml`: fresh `project.id`, `project.name` from `--name` or the directory
   name, `engine.version` from `env!("CARGO_PKG_VERSION")`.
4. **Rewrite `[scene] id` and every `[[prefab]] id` in every copied `.loom` file** with fresh
   UUIDs, via `toml_edit` so comments and formatting survive.
5. Print one line of JSON — path, id, scene count — like every other subcommand.

**Step 4 is the one with a judgement call in it.** Two projects created from one template
sharing a scene UUID is Unity's duplicate-GUID bug in miniature. Today it breaks nothing,
because nothing in this engine resolves anything by `id` — scenes resolve prefabs and assets
by file-local `key`, and meshes load by path (§5). So this is a *latent* problem being
pre-empted at the cost of about fifteen lines.

`[[asset]] id` is left alone, and that asymmetry is deliberate: an asset id is meant to name
an imported file's identity through `meta.rs`'s sidecar, which does not run, so the values
sitting in scenes today are hand-written UUIDs matching nothing. Regenerating them would be
theatre. Leaving them stable at least keeps two copies of one texture agreeing about what
they are.

```rust
// ponytail: regenerate scene and prefab ids only; asset ids are inert (loom_asset::meta
// is dead code). Revisit the whole question the day an asset registry exists — that is
// also the day this becomes a real collision rather than a tidy one.
```

### Getting `assets/templates/` from a shipped editor

The templates directory has to be findable from the executable, not from the current working
directory. **This is the same defect the engine survey flags as a shipping blocker**:
`load_bindings` reads `assets/input/default.toml` relative to the process cwd
(`crates/loom_cli/src/run.rs:2242-2247`), so a shipped `exe + assets/` build only works when
launched from the right directory.

The fix is one function and it belongs here because the hub is the first thing that needs it:

```rust
/// The engine's own assets directory: `<exe dir>/assets` if it exists, else
/// `assets` relative to the cwd (the repo layout, for `cargo run`).
fn engine_assets() -> PathBuf
```

Both templates and the fallback input bindings go through it. The cwd branch is what keeps
`cargo run` and every test working from the repo root. **This does not belong to the hub
design** beyond being named — it is the packaging design's problem — but the hub is the
first caller and the one-line fix should land with it rather than being discovered during
packaging.

---

## 9. The base scene, and the three templates

### `empty` — the base scene every new project opens to

**Five nodes, zero external files.** No `[[asset]]` block at all: `box` resolves
procedurally through `loom_asset::primitives::build` (`main.rs:1146`), so this scene renders
correctly wherever it is copied to, before the user owns a single asset. That property is
worth more than anything a nicer-looking template could contain.

| Node | Components | Why it is there |
| --- | --- | --- |
| `World` | `Environment` | **The most important node in the file.** Sun direction, strength and colour, ambient, sky zenith/horizon, light fog. Without it the defaults apply and the first thing a new user does is fix the lighting. |
| `World/Ground` | `MeshRenderer{box}`, `Material` | 20 × 0.5 × 20 via `transform.scale`. Albedo a **dark desaturated green**. |
| `World/Cube` | `MeshRenderer{box}`, `Material` | At `[0, 0.5, 0]`. Something to click on frame one, so the gizmo has a target before the user has created anything. |
| `World/Light` | `Light` | At `[2.5, 3.0, 2.0]`, intensity ≈ 120. |
| `World/Camera` | `Camera{fov 55}` | At `[4.5, 2.6, 6.0]`, pitched ≈ −12°. |

Three of those five choices have a reason that is not obvious:

**The ground is a scaled box, not a `VoxelVolume`.** A voxel volume bakes on every load, and
its op list renders in the inspector as *"3 items"* and nothing else (`panels.rs:110-114`) —
so the beginner's first encounter with terrain would be a thing they can see and cannot
touch. A box is editable with the gizmo they were just handed. Terrain arrives when the
terrain tooling does.

**The ground is dark green rather than the neutral grey every other engine ships.**
`CLAUDE.md` records the rule from P2: *"That only works because the ground under a grass
field is authored the colour of grass … Any scene with a `Grass` field owes its ground the
same"* — `meadow`'s brown soil read as ploughed earth once the density falloff thinned the
field. The first thing a user adds to a ground plane is grass. Authoring the base scene's
ground green costs one line and pre-empts a documented failure the user would otherwise hit
and blame on the grass.

**The camera is authored rather than left to auto-framing.** `Camera` absent means the
renderer frames the whole scene (`components.rs:688-692`), which is right for an agent
authoring blind and wrong here: it means `loom render main.loom` and the editor viewport
show different things the moment the user moves the camera, and it hides the fact that a
camera is a node you can select. Authoring it teaches the model in the first thirty seconds.

**Engine features demonstrated:** `Environment` (sun/sky/ambient/fog), `MeshRenderer` with a
primitive, `Material`, `Light` (and the `intensity = d²/albedo` trap at a value that reads),
`Camera`, `Transform` sugar including non-uniform `scale`, and the one-root rule.

### `first_person`

The base scene plus a rig, on a 40 × 40 ground. Adds:

| Node | Components | Demonstrates |
| --- | --- | --- |
| `World/Player` | `CharacterController{height 1.8, radius 0.35, step_height 0.35}`, `Script{"../assets/scripts/fps.rhai"}` | The movement model is a **file**, not engine Rust. Hot reload exists (`loom_script` `ScriptWatcher`). |
| `World/Player/Eye` | `Camera{fov_y_degrees = 90}` at `[0, 0.75, 0]` | The first-person rig. Verified in `crates/loom_cli/src/play.rs:1558-1573`: **yaw is written to the character and pitch to the camera**, which is exactly why the camera must be a *child* of the controller. |
| `World/Steps` ×3 | `MeshRenderer{box}`, `Material` | Boxes at 0.3 / 0.6 / 0.9 m. Proves `step_height` and gives jumping somewhere to go. |
| `World/Ramp` | `MeshRenderer{box}`, `Material`, rotated 20° | Proves `max_slope`. |
| `World/Ball` | `MeshRenderer{sphere}`, `RigidBody{dynamic, mass 4}`, `Material{metallic}` | Something physics moves when you shoot near it. |
| `World/Boom` + two children | `Blast{armed = false}`, two `ParticleEmitter`s (additive fire, alpha smoke) | Left-click does something. Lifted verbatim from `assets/test/camera.loom`, which is the existing working proof of this rig. |
| `World/Sign` | `Hud{anchor, text, size}` | The HUD is scene content, so moving the score is an edit (`hud.rs:1-14`). |

Ships `assets/scripts/fps.rhai` (copied from `assets/scripts/`) and
`assets/input/default.toml`, so **the project owns its bindings** — the second half of the
cwd-relative fix in §8.

**Pointer capture at Play works because the scene has both halves** — a `CharacterController`
and a `Camera` (`run.rs:1329-1351`). A template that had one and not the other would produce
the "no player rig — flying instead" console line as the user's first experience of Play,
which is the exact failure the engine survey says should be a UI state rather than a log
line.

### `third_person`

Identical to `first_person` in ground, course, physics and script — **the same
`fps.rhai`**, which is the point worth demonstrating: the movement model is expressed in the
character's own frame and does not know or care where the camera is. Two differences:

| Node | Components | Demonstrates |
| --- | --- | --- |
| `World/Player/Body` | `MeshRenderer{capsule}`, `Material`, scaled to the controller | The `capsule` primitive, and the thing first-person does not need: a visible avatar. |
| `World/Player/View` | `Camera{fov_y_degrees = 70, boom = 3.5}` at `[0, 1.5, 0]` | A boom camera. |

**`Camera.boom` does not exist and this template needs it.** This is the one place in this
document where a template drove an engine change, and it should be visible rather than
buried.

The reason it is needed: `apply_look` writes pitch onto **the camera node itself**
(`play.rs:1565`), not onto a parent. So a camera node placed behind and above the player
pitches *in place* at the end of a rigid offset — looking down aims at the ground in front
of the player rather than orbiting the camera up and over. That reads as a badly-mounted
shoulder cam, and every user's first instinct will be that third-person is broken.

The fix is a field, and it is small:

```rust
// crates/loom_scene/src/components.rs — Camera
/// Metres to pull the eye back along its own +Z after rotating. 0 is a camera
/// at its node; 3.5 is a third-person boom. The node's position is what the
/// camera orbits, so a boom camera's node belongs at the character's head.
#[schemars(range(min = 0.0, max = 50.0))]
pub boom: f32,     // default 0.0
```

applied at the single place a `CameraView` is derived — `loom_ecs::World::active_camera`,
`crates/loom_ecs/src/lib.rs:469-487`, which already has the node's global transform and
builds `{ eye, target, fov_y_degrees }`. Roughly five lines: move `eye` back along the
node's local +Z by `boom`, leave `target` where it is.

**This needs no ADR and no `format` bump.** `docs/format/README.md` §9 puts "adding a new
optional field with a default" in *may change without a migration*, and `boom = 0.0`
reproduces today's behaviour exactly — every existing scene, every golden reference and the
sim hash are untouched by construction. It does want a line in the commit saying so.

```rust
// ponytail: no spring arm, no collision. The boom clips through walls.
// Upgrade path is one raycast against the collision world (loom_physics::RayHit
// exists) shortening `boom` to the first hit. Add it when a level has walls.
```

**If `Camera.boom` is rejected**, the fallback that works today with no engine change is a
fixed shoulder cam: `Camera` on a node at `[0.7, 1.6, 3.2]` relative to the Player, no
boom. It is playable and it pitches wrong. Say so in the template's header comment rather
than shipping it silently.

### Where the templates go in the gates

**All three scenes go in `xtask`'s `SCENES`** (currently 43). They must load, resolve, bake
and validate clean, and the specific thing that guards is the failure mode that would
otherwise be invisible: **a template whose relative asset paths are wrong falls back to a
box with a `log::warn` and renders**. `MeshLibrary` degrades rather than failing
(`main.rs:1141-1145`, "a missing mesh should be *visible*, not fatal"), which is right for
an agent and means a broken template ships looking almost fine.

**`empty`'s base scene also goes in `GOLDEN`**, and this is an extension of the stated rule,
so it needs its reason on the record. `GOLDEN`'s rule is *rendering paths*, and the base
scene covers none that `primitives` and `materials` do not. It belongs anyway because **it
is the one scene in this repo whose appearance is itself the deliverable** — every user's
first frame — and no other gate can see a regression in it. One reference PNG, one line in
`MANIFEST.txt`, and re-blessing stays a readable commit.

`first_person` and `third_person` stay out of `GOLDEN`: their coverage is genuinely
duplicated, and their interesting behaviour is motion under Play, which no still can show.
They are candidates for the `--play` list if their frame cost is ever worth guarding; at
five boxes and a sphere it is not.

---

## 10. What is outside undo, stated plainly

The constraint survey (§4.J) asks the design phase to produce the list of editor actions
that are not `SceneOp`s. Here is the hub's share of it, with what replaces Ctrl+Z in each
case:

| Action | Outside undo because | What the user gets instead |
| --- | --- | --- |
| Create a project | It is a directory tree, not scene text | The confirmation names the created path. Undoing is deleting a folder, and the hub does not offer to do that for you. |
| Open / close a project | No authored state changes | — |
| Remove from recents | The list is user state in `~/.local/state` | The button says "Remove from list". Re-open the project and it returns. |
| Edit `loom.toml` in a settings panel | It is text, it is diffable, it is in git — but it is not scene text, and `Applied::undo` is *the previous scene file* (`ops.rs:126-128`) | **The panel shows no undo affordance at all.** Not a greyed one — none. An undo button that silently does not reach the thing under it is worse than no button. |
| Thumbnail refresh | A cache file | — |

**Rejected: giving `loom.toml` its own version token and undo stack.** It would be
symmetrical and it is precisely what never-do #16 forbids — a second undo stack in the
editor, with its own semantics, that Ctrl+Z either does or does not reach depending on
which panel has focus. The manifest is five fields edited rarely; git is its undo.

---

## 11. ADRs

### ADR 0022 — A project is a directory with a `loom.toml`

**Needed.** The constraint survey raises it as §4.H, and it introduces a new authored
artifact class, which `docs/format/README.md` §9's logic says belongs beside the scene
format spec.

> **Decision.** A Loom project is any directory containing a `loom.toml` manifest at its
> root. The manifest carries five fields — `format`, `id`, `name`, `main_scene` and the
> engine version that last wrote it — and nothing else; in particular it carries **no list
> of scenes and no list of assets**, because a declared list is a second source of truth
> that drifts the first time a file is copied in by hand. Scenes are found by globbing
> `*.loom` beneath the root; assets are resolved exactly as they are today, by joining an
> `[[asset]]`'s advisory relative `path` onto the referencing scene file's own directory.
> The folder layout `loom.toml` / `scenes/` / `assets/` / `builds/` is a **convention that
> `loom new` creates and nothing enforces**, chosen so that a scene's asset references read
> `../assets/…` and therefore work under the resolver that already exists.
>
> The manifest is normative text: it is specified in `docs/format/PROJECT.md`, validated on
> load with unknown keys as errors, and versioned by its own `format` integer under the same
> stability rules `.loom` follows.
>
> `[engine] version` is **advisory**. Scene `format` remains the only compatibility
> contract. An older project opens silently and is rewritten to the current version on its
> first save; a project written by a strictly newer engine is **refused at the hub with a
> sentence, and offered read-only**, because a `format = 2` scene otherwise fails inside the
> parser and presents as an editor that opened onto nothing.
>
> The hub does **not** resurrect `loom_asset::meta`'s UUID sidecars and manifest. Asset
> identity by content hash is a real design for a problem this engine does not have; until
> it does, path-relative resolution fails loudly at the point of authorship, which is better
> feedback than a stale manifest.
>
> Hub state lives outside every project, at `$XDG_STATE_HOME/loom/hub.toml`, and thumbnails
> at `$XDG_CACHE_HOME/loom/thumbs/`. A project directory acquires no engine-written files.

### ADR 0023 — An `[[asset]]`'s `path` is resolved; `id` is reserved

**Needed, and I found it while designing this rather than looking for it.**
`docs/format/README.md` §3 states that `path` is "a **hint for humans and nothing else** —
never resolved, never trusted, never used to load", and that "identity is the UUID, not the
path". **The implementation does the opposite**: `MeshLibrary::with_cache` calls
`scene_asset_path` and joins the result onto the scene's parent directory
(`crates/loom_cli/src/main.rs:1150-1163`), and nothing anywhere resolves an `id`. Every
scene in `assets/` depends on the implementation's behaviour, not the spec's.

That contradiction is currently harmless and is exactly the kind of thing that rots. **The
project model formalises a folder layout that only works because paths resolve**, so this is
the moment to close it.

> **Decision.** `[[asset]].path` is normative and is resolved relative to the directory of
> the scene file that declares it. `[[asset]].id` is **reserved**: it is written, it is
> preserved across edits, and nothing resolves by it. `docs/format/README.md` §3 is amended
> to say so. Content-hashed asset identity (`loom_asset::meta`) remains unbuilt; when it is
> built, `id` becomes the primary key and `path` the fallback, and that transition is a
> `format` bump with a migration, not a silent change of meaning.

*This ADR is about the format, not the hub. If the editor rework's asset/import design also
raises it, they should merge — one ADR, whoever writes it first.*

### Not an ADR

- **`Camera.boom`** — additive optional field with a default. `docs/format/README.md` §9
  puts it in *may change without a migration*, `boom = 0.0` is today's behaviour, no golden
  re-bless, no format bump. Flagged loudly in §9 so a reviewer can still say no.
- **The hub as a subcommand** — an entry in `USAGE` and two rows in `FLAGS`.
- **The templates and the base scene** — `.loom` files. The constraint survey already says
  these need no ADR.
- **End-user documentation** — prose in `docs/editor/`.

### ADRs this design depends on but does not own

- **ADR F (editor/runtime crate split).** Determines which crate `hub.rs` lives in. This
  design assumes `loom_editor` and does not need it to exist first — the hub can ship as a
  `loom_cli` module and move. **One finding for whoever writes it:** `hud.rs` uses egui at
  *runtime*, so the shipped runtime links egui regardless and the split is about not linking
  `loom_editor`, not about making egui optional in `loom_render`.
- **ADR G (Windows cross-compilation).** Determines whether `engine_assets()`'s
  `<exe dir>/assets` branch has a second layout to satisfy.

---

## 12. What I could not verify

Read-only investigation only; no build, no run. These are the gaps, marked rather than
guessed:

1. **I did not run anything.** No `cargo` command of any kind was executed, per the design
   phase's instruction. Every behavioural claim is from reading source.
2. **Whether the window path can read back its framebuffer cheaply.** It decides only
   whether thumbnails could be captured from the editor instead of rendered by a subprocess.
   I designed the subprocess route, which needs no answer; if a readback exists, swapping is
   local to one function.
3. **Whether `loom render --size 480x270` accepts an odd aspect or a size that small.**
   `--size WxH` is in `FLAGS` for `render` (`main.rs:151`) and I did not read the parser or
   any minimum. If 480×270 is rejected the thumbnail size changes and nothing else does.
4. **The exact five lines of the `Camera.boom` change.** I read `CameraView`'s definition
   (`crates/loom_ecs/src/lib.rs:39-45`) and confirmed `active_camera` builds it at `:469-487`,
   but I did not read the body, so "move `eye` back along the node's local +Z" is inferred
   from the struct's doc comment (*"a point one metre in front of the eye, along the node's
   −Z"*) rather than from the arithmetic. The sign convention is the thing most likely to be
   backwards on the first attempt.
5. ~~Whether the editor's opening framing needs a second `boom` implementation.~~
   **Checked, and the answer is no** — `FlyCamera::at` takes a `loom_ecs::CameraView`
   (`run.rs:88`, called at `run.rs:352-354`), so applying `boom` inside
   `World::active_camera` reaches the editor's opening view, Play, and `loom render`
   through one site. Left in this list because it was a real risk and the answer is
   load-bearing for §9.
6. **Whether any `.meta` files exist on disk.** I listed `assets/meshes/` and
   `assets/textures/` and saw none, and `meta.rs` has no caller outside its crate — but I
   did not search the whole tree, so "the manifest machinery has never run" is strong
   inference, not proof.
7. **`loom_input::ActionMap`'s behaviour when a project ships a partial `default.toml`.** The
   templates copy the whole file, so this only matters if a user deletes bindings from
   theirs; `load_bindings` falls back to the compiled-in copy on a *parse error*
   (`run.rs:2242-2251`) and I did not check what a valid-but-incomplete file does.
8. **Whether `toml_edit` can set a value inside `[[prefab]]` array-of-tables entries as
   simply as I assume** in `loom new` step 4. `loom_scene` uses it for exactly this class of
   edit, so the capability is certain; the ergonomics are not.
9. **Directory-picker dialogs.** egui has no native file dialog and I did not check whether
   any pinned dependency provides one. **Open** and the New Project location field may need
   either a new pinned dependency (`rfd`, which pulls GTK or an XDG portal on Linux) or a
   plain text field with path completion. **The text field is the lazy answer and I would
   ship it first** — this is a single-developer engine on one machine and a path is
   typeable — but a hub with no file picker will feel unfinished, so it is a real open
   question rather than a settled one.
10. **`env!("CARGO_PKG_VERSION")` is `"0.0.0"` for every crate in this workspace**
    (`crates/loom_cli/Cargo.toml:3`). The version-pinning policy in §3 is therefore
    *correct and inert* until the workspace starts versioning itself. That is fine — the
    field records what it records — but nobody should read §3 and expect it to fire.
