# Build & ship — turning a project into a game for Linux and Windows

*Design phase, editor rework. Reads on top of `00-survey-existing.md`,
`00-survey-engine-surface.md` and `00-survey-constraints.md`. Every claim about the tree is
cited to `file:line` at `62f9ebe`; every claim about this machine was checked with a
read-only command and the command is named. **Nothing was built** — see §10.*

---

## The two facts that decide most of this, and both are already true

**Shaders are already inside the binary.** `crates/loom_render/src/lib.rs:73-99` embeds every
compiled module with `include_bytes!(concat!(env!("OUT_DIR"), "/scene.spv"))` and friends.
`build.rs` runs `slangc` on the *host* at compile time and cargo compiles build scripts for the
host even during a cross build. SPIR-V is target-independent. **So a shipped game carries no
`.slang`, no `.spv` file, no Slang runtime, and no shader compile at startup — and the Windows
cross-compile inherits all of that for free.** This is the single largest de-risking fact in the
whole ship story and it cost nothing, because it was decided for a different reason (never-do #9,
"never let `build.rs` swallow a shader compile error"). The only obligation it creates is that
`slangc` must be on `PATH` during a cross build, which it is: it is a host tool
(`which slangc` → `/home/k-dorui/.local/bin/slangc`).

**Asset paths already resolve relative to the scene file, not the working directory.**
`main.rs:237` calls `set_scene_base(path)` for every subcommand, and `scene_base()`
(`main.rs:3757`) is what `[[asset]]` paths, terrain recipes, scripts and the rain recording
(`main.rs:3236`) are joined against. `MeshLibrary::for_scene(&scene, base)` and
`MaterialLibrary::for_scene(&world, &scene, base)` take the base explicitly
(`main.rs:571-573`). **So in-editor and shipped resolution are already the same code**, provided
the shipped tree preserves the project's relative directory structure. That is why §3 copies the
asset tree verbatim rather than flattening or content-hashing it: any renaming scheme would have
to be *undone* at load, and the undoing is where the divergence lives.

There is exactly one path in the runtime that consults the process working directory, and it is
the shipping blocker the engine-surface survey already named as gap 11: `run.rs:2243` loads
`assets/input/default.toml` relative to cwd. §3.3 fixes it in three lines.

---

## 1. What the runtime binary is

### The decision

**One new crate `loom_editor`, and a second binary `loom-play` inside `loom_cli` that is built
with `--no-default-features`.** egui becomes an optional dependency of `loom_render` behind a
non-default `editor` feature; `loom_editor` turns it on; `loom_cli` turns it on only through its
own default feature, which the ship build switches off.

```toml
# crates/loom_render/Cargo.toml
[dependencies]
egui                = { version = "=0.35.0", optional = true }
egui-ash-renderer   = { version = "=0.12.0", features = ["gpu-allocator"], optional = true }
egui-winit          = { version = "=0.35.0", default-features = false, optional = true }

[features]
default = []                                              # nobody gets egui unless they ask
editor  = ["dep:egui", "dep:egui-ash-renderer", "dep:egui-winit"]
```

```toml
# crates/loom_cli/Cargo.toml
[[bin]] name = "loom"       path = "src/main.rs"      required-features = ["editor"]
[[bin]] name = "loom-play"  path = "src/bin/play.rs"

[dependencies]
loom_render = { path = "../loom_render", default-features = false }
loom_editor = { path = "../loom_editor", optional = true }

[features]
default = ["editor"]
editor  = ["dep:loom_editor", "loom_render/editor"]
```

`default = []` on `loom_render` rather than `default = ["editor"]` is deliberate: with the
default on, every crate in the workspace that happens to depend on `loom_render` re-enables egui
by accident and the gate is decorative. With it off, the demand is written down in exactly two
places — `loom_editor`'s manifest and `loom_cli`'s `editor` feature — and `cargo tree` can be
asked whether anything else grew one.

### Why not feature flags alone, and why not a separate `loom_runtime` crate

**Feature flags alone were rejected** because the boundary would not be checkable. The research
doc says it plainly — "Prefer a separate crate over `#[cfg(feature)]` sprinkled through runtime
crates — it makes the dependency boundary CI-checkable"
(`docs/design/loom-pcg-and-editor.md:173`) — and `scripts/check-deps.sh` is the reason that
argument lands here rather than being a matter of taste: this project already enforces its crate
graph mechanically, and a rule it can express is worth more than a rule it cannot. Four thousand
lines of editor behind `#[cfg(feature = "editor")]` inside `loom_cli` would also be four thousand
lines that nothing type-checks in the runtime configuration.

**A third crate `loom_runtime` was rejected as premature.** The shipped binary needs `play.rs`,
`hud.rs`, `scene_view.rs`, `materials.rs`, `particles.rs`, `weather.rs`, `sound.rs`,
`telemetry.rs`, `log.rs` and the window/camera/event-loop half of `run.rs` — nine modules that
the `loom` CLI binary needs too. Splitting them out creates a crate that is "`loom_cli` with the
subcommand dispatch removed", which is a rename dressed as a boundary. **`loom_cli` becomes the
runtime library plus the agent CLI, and `loom_editor` is what leaves.** If the CLI's own
dependency set later becomes a burden on the shipped binary (it is not today — `serde_json` and
the JSON reporting are kilobytes), the split becomes worth doing and it is a mechanical move at
that point.

The line of the split, which is the sibling documents' work and stated here only so the seam is
agreed: **`run.rs` divides into a runtime `window.rs` — winit `ApplicationHandler`, `FlyCamera`,
the play driver, the GPU upload scheduling of `scene_view` — and an editor layer holding the tool
state machine, `transact`/`transact_as`, the file watcher and the panels.** `panels.rs` and
`gizmo.rs` go wholesale to `loom_editor`. Everything on the checklist in
`00-survey-existing.md` §14 that concerns teardown order, key latching and `--frames` lives on the
runtime side and must survive the move unchanged.

### The one thing this gate must not become

ADR 0018's consequences record the warning verbatim: "the forward pass wrote a different
destination depending on an environment variable — **in the window, which is where the human
judges everything**", and that class of divergence has cost this project three defects
(`docs/decisions/0018-the-frame-is-computed-in-float.md:179-183`). **A feature flag that changes
what the renderer draws would be the fourth.**

It does not, and the code is already shaped so it cannot. `Viewer::draw` (`viewer.rs:922`) and
`Viewer::draw_with_ui` (`viewer.rs:936`) are already separate entry points; the `ui` pass
(`viewer.rs:1590-1619`) declares one use, `(post_id, Access::ColorWrite)`, and is the last thing
before present. The feature gate removes `draw_with_ui`, `ui.rs` and that one pass. **The forward
pass, the tonemap, MSAA, the resolve and CMAA2 are all unconditional and stay unconditional.**

The assertion that keeps it that way is in §6: **a shipped-configuration binary must render a
golden scene and match the reference the editor-configuration binary produced.** That is one
extra scene run in `cargo xtask image` and it is what converts "the gate does not change the
image" from a claim into a check.

---

## 2. What the shipped thing looks like

```
out/proving-ground-linux-x86_64/
    proving-ground              # loom-play, renamed
    loom.toml                   # the project manifest, copied verbatim
    assets/                     # verbatim copy of <project>/assets
        games/proving_ground.loom
        meshes/ textures/ scripts/ audio/ input/
    .loom-build.json            # the build report; also the overwrite marker (§5)
```

Windows is the same tree with `proving-ground.exe`, plus whatever mingw runtime DLLs §4.4 turns
out to require.

**The executable is renamed, the assets are not.** Renaming the binary is free — it is a file
copy — and it is what makes a shipped game feel like a game rather than like a tool. Renaming
anything inside `assets/` would require rewriting every alias in every scene, which is the
diff-destroying operation this format exists to avoid.

---

## 3. Asset paths, in-editor and shipped

### 3.1 One root, computed two ways

```rust
// crates/loom_cli/src/project.rs  (new, ~30 lines)

/// The directory every project-relative path resolves against.
///
/// In the editor it is the project the hub opened. In a shipped game it is the
/// directory holding the executable — never the working directory, because a
/// player's launcher sets that to anything at all.
pub fn project_root(explicit: Option<&Path>) -> Result<PathBuf, String>
```

Shipped: `std::env::current_exe()`, then `parent()`, then `canonicalize()` so a symlinked launcher
resolves to where the assets actually are. Editor: the path the hub opened. **Two callers, one
function, and that is the entire parity mechanism.**

### 3.2 Boot

The runtime reads `loom.toml` from the root and takes `game.startup_scene` — a project-relative
path — then calls the existing `set_scene_base` on it. Everything downstream is unchanged, because
everything downstream already resolves against the scene's directory.

**The manifest format is not mine to define.** It belongs to the project/Hub design
(`00-survey-constraints.md` §4H), and the shape of that decision is an ADR that document owns.
Build-and-ship needs exactly three keys and will accept any schema that carries them:

| Key | Used by | Why |
| --- | --- | --- |
| `game.name` | ship | the executable's name, and the window title |
| `game.startup_scene` | runtime, ship | what the game opens; ship asserts it exists |
| `build.targets` (list of triples) | ship UI | which targets the Build panel offers |

If that document lands the manifest in a `loom_project` crate, `project.rs` reads it from there.
If it puts the struct in `loom_asset`, likewise. **What must not happen is a second manifest
reader**: the ship step and the runtime must deserialize through the same type, or a project the
editor accepts will produce a game that will not start.

### 3.3 The one real bug, and its three-line fix

```rust
// run.rs:2242-2251 today
fn load_bindings() -> ActionMap {
    let path = std::path::Path::new("assets/input/default.toml");   // relative to cwd
```

Relative to the process working directory. In the repo that is the workspace root and it works by
coincidence. In `out/proving-ground/` launched from a desktop shortcut it silently falls through
to the compiled-in copy — which is *survivable*, because `loom_input::DEFAULT_BINDINGS` is an
`include_str!` of the same file (`loom_input/src/lib.rs:263`) — but it means **a project can never
own its own bindings in a shipped build**, which is exactly what a rebinding UI would author.

The fix is `load_bindings(root: &Path)` joining `root.join("assets/input/default.toml")`, with
the compiled-in copy still the fallback and a malformed file still non-fatal (losing your camera
to a typo in a config remains a bad trade — that reasoning in the current doc comment is right and
survives).

### 3.4 What gets copied

**The whole asset tree, minus two things.** Copy `<root>/assets/**` preserving structure, and
`loom.toml`, and nothing else. Exclude:

- **`assets/shaders/`** — grep says the only runtime readers of `.slang` are two tests
  (`loom_render/src/lib.rs:221` reads `scene.slang` to cross-check `WATER_VERTS`;
  `loom_cli/src/main.rs:4649` reads `rain.slang`), both `#[cfg(test)]`. 340 KB and no runtime
  consumer. *This is the one exclusion I am least sure of and §6's smoke run is what would catch
  it being wrong.*
- **`*.meta`** — `loom_asset::meta` (`meta.rs:21-160`) is an import-time content-hash ledger with
  no caller outside its own crate. It is editor bookkeeping.

**No reachability analysis, and that is deliberate.** Shipping only the assets a scene actually
references needs an asset graph, and `meta.rs` is the crate that would hold it and is currently
dead code. This repo's `assets/` is 132 MB of textures and 53 MB of meshes; a real project's
would be shipped whole and nobody would notice for a year.

```rust
// ponytail: whole-tree copy. Reachability pruning wants loom_asset::meta to
// come alive and an alias graph to walk; do it when a ship is too large to
// upload, not before.
```

**Symlinks are resolved, not copied.** A symlink into `/mnt/data` that works on this box is a
dangling link on a player's. Copy the target.

---

## 4. The Windows cross-compile

This is the largest technical risk in the rework and it deserves the most concrete treatment.
What follows is what I could establish without building, then the plan that finds the rest out in
the first hour rather than the last week.

### 4.1 Target: `x86_64-pc-windows-gnu`, not `-msvc`

The MSVC ABI needs `lld-link` plus a copy of the Windows SDK import libraries and the MSVC CRT.
Getting them onto a Fedora box means `xwin` downloading Microsoft's redistributables, which is a
licence question, a large moving part, and a thing that breaks when Microsoft moves a URL. The
GNU ABI needs `x86_64-w64-mingw32-gcc`, which **is already installed on this machine**:

```
$ which x86_64-w64-mingw32-gcc      → /usr/bin/x86_64-w64-mingw32-gcc
$ rpm -q mingw64-gcc                → mingw64-gcc-16.1.1-1.fc44.x86_64
```

and twenty-six other `mingw64-*` runtime packages are present (pulled in by wine). **And both
`windows_x86_64_gnu` and `windows_x86_64_gnullvm` are already in `Cargo.lock`** alongside
`windows_x86_64_msvc` — the `windows-targets` chain that `winit` and `cpal` sit on resolves for
the GNU ABI without touching the lockfile. Checked by name against the lock.

The cost of `-gnu` is a runtime dependency on `libgcc_s_seh-1.dll` and `libwinpthread-1.dll`
unless they are statically linked; §4.4 covers it.

**Alternative rejected:** shipping a Windows build by asking the user to build it on Windows. It
is the honest low-effort answer and it fails the brief — the user's decision 3 is "Windows
cross-compiled from Fedora", and there is no Windows machine here to build on either.

### 4.2 Per-dependency assessment

Read from `Cargo.lock` and from vendored source. **Unproven except where marked.**

| Crate | Windows story | Confidence |
| --- | --- | --- |
| `ash =0.38.0` | `Entry::load()` does `libloading` on the string `"vulkan-1.dll"` — `entry.rs:63`. **No link-time Vulkan dependency, no SDK, no import library.** | **verified by reading the source** |
| `ash-window =0.13.0` | Win32 branch present at `lib.rs:44` and `:139`; `enumerate_required_extensions` handles `RawDisplayHandle::Windows`, so `run.rs:2197` needs no change. | **verified by reading the source** |
| `winit =0.30.13` | Windows backend is `windows-sys`, pure Rust bindings, no C. | high, unproven |
| `cpal =0.18.1` | The `alsa`/`alsa-sys` path (which needs `pkg-config` and C headers) is `cfg(target_os = "linux")`; Windows is WASAPI through the `windows` crate. **A Windows resolve should not contain `alsa-sys` at all** — V0 below is the check. | high, unproven |
| `rapier3d`, `parry3d`, `nalgebra`, `simba` | pure Rust | high |
| `rhai` | pure Rust | high |
| `png`, `flate2`/`miniz_oxide`, `gltf`, `zune-jpeg` | pure Rust; `miniz_oxide` in the lock means no zlib C dependency | high |
| `blake3` | **The one crate with a C/assembly build.** Depends on `cc` (`Cargo.lock:188`); ships `blake3_*_x86-64_windows_gnu.S`. `cc` selects `x86_64-w64-mingw32-gcc` for this triple automatically. **This is the predicted first failure.** | medium |
| `x11-dl`, `wayland-*`, `alsa-sys` | all unix-gated; must not appear in a Windows resolve | high |
| `egui*` | **not in the runtime binary** — §1 strips them. Cross-compiling the editor is explicitly out of scope. | n/a |

**`blake3` matters more than its size suggests**, because it is what `VersionToken` is
(`docs/format/README.md:384`, `loom_scene` depends on it). If it has to fall back to blake3's
`pure` feature, that is a *speed* change and not a semantic one — BLAKE3's output is
spec-defined, so a version token computed by the Rust implementation and one computed by the
assembly implementation are the same bytes. **That is the escape hatch and it costs nothing but
throughput on a hash of a kilobyte-scale file.**

**Stripping the editor from the shipped build halves this risk and that is worth saying out
loud.** Three egui crates, an egui-to-Vulkan renderer and egui's font stack never have to
cross-compile at all. The decision in §1 was made for boundary reasons and pays here.

### 4.3 What has to change in the repo

```toml
# rust-toolchain.toml
targets = ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-gnu"]
```

```toml
# .cargo/config.toml
[target.x86_64-pc-windows-gnu]
linker = "x86_64-w64-mingw32-gcc"
ar     = "x86_64-w64-mingw32-ar"
```

Note that the existing `[target.x86_64-unknown-linux-gnu]` clang+mold entry is per-target and is
not inherited, so the Windows entry is additive and cannot regress the Linux build.

Fedora packages: `mingw64-gcc` (present). `mingw64-gcc-c++` is **not** installed and is probably
not needed — nothing in the dependency set compiles C++ — but it is one `dnf install` if a
`cc`-driven crate turns out to want it.

### 4.4 The DLL question

A `-gnu` binary normally needs `libgcc_s_seh-1.dll` and `libwinpthread-1.dll` beside it.
`-C target-feature=+crt-static` is supposed to link them statically for this target. **I could not
verify that this toolchain does it**, so the ship step handles both cases: after linking, run
`x86_64-w64-mingw32-objdump -p <exe>` and read the import table. Any imported DLL that is not a
Windows system DLL gets copied from `/usr/x86_64-w64-mingw32/sys-root/mingw/bin/` into the output
directory, and the build report lists what it copied. **This turns an unknown into a printed
fact** rather than into a game that fails to start on a machine nobody here owns.

The same objdump answers a second, sharper question: **`vulkan-1.dll` must NOT appear in the
import table.** If it does, `ash` was linked rather than dlopened and the game will refuse to
start on any machine without the Vulkan SDK. That check costs one line and it is the highest-value
assertion in the whole cross-compile.

### 4.5 The verification plan — prove it in the first hour

**This work is sequenced first in the build-and-ship phase, before any Build panel is drawn.**
The rationale is blunt: if the cross-compile is impossible, the UI for it is worthless, and the UI
is the cheap half. Each step below has a falsifiable output and a named attribution.

**V0 — the dependency graph, no compilation at all.** `cargo tree --target
x86_64-pc-windows-gnu -p loom_cli --no-default-features -e normal`. This is a metadata operation:
seconds, no codegen, no linker. It answers, before anything is built, whether `alsa-sys`,
`x11-dl` or the wayland stack leak into a Windows resolve, and which `windows-*` crates arrive.
**Run V0 against today's tree, before any refactor** — the `--no-default-features` form needs §1's
work, but the plain form does not, and the answer for egui is interesting on its own.

**V1 — `cargo check --target x86_64-pc-windows-gnu`.** Type-checks every crate for Windows
without linking. This is where our *own* `cfg(unix)` assumptions surface. Type-checking is much
cheaper than codegen and it finds a different class of failure than V2 does.

**V2 — build leaf-first, in dependency order, one crate at a time.**
`loom_scene` → `loom_asset` → `loom_physics` → `loom_render` → the binary. **The order is the
point.** blake3 is the predicted failure and it enters at `loom_scene`, the first step; finding it
there costs one minute and names one crate, whereas finding it at the end of a twenty-minute
whole-workspace build names nothing.

**V3 — link, and read the result.** `cargo build --release --target x86_64-pc-windows-gnu
--bin loom-play --no-default-features`. Assert `file` reports `PE32+ executable (console)
x86-64`, and run the §4.4 objdump checks.

**V4 — run it, here, under Wine.** This machine has `wine-11.0 (Staging)` and
`~/.wine/drive_c/windows/system32/vulkan-1.dll` — winevulkan, which forwards to the same NVIDIA
ICD the Linux build uses. Both were checked. So:

```
wine out/proving-ground-windows/proving-ground.exe --frames 1
```

exercises PE loading, the mingw CRT, `Entry::load()` finding `vulkan-1.dll` by name, winit
creating a Win32 window, `VK_KHR_win32_surface`, swapchain creation and one frame. **It is not a
Windows test. It is a "did we build something that can start" test**, and it is the difference
between finding out now and finding out from a user. The `--frames n` flag exists precisely so the
whole window lifecycle can be driven headlessly (`xtask/src/main.rs:1024`), and it earns its keep
a second time here.

Ray tracing is not a blocker for V4: `device.rs:78-80` treats
`VK_KHR_acceleration_structure` / `ray_query` / `deferred_host_operations` as an availability
check with a fallback, so a Wine session that does not expose them still renders.

**V5 — pixel-compare under Wine.** `wine proving-ground.exe --render cave.loom --out wine.png`,
then `loom compare wine.png tests/references/cave.png` at the existing ADR 0005 tolerance. If it
matches, the cross build is not merely *running*, it is producing the same image. Caveat stated
in the report: winevulkan is a thin translation over the same driver, so this proves **the build**,
not **the platform**.

**V6 — the honest gap, written into the ADR.** None of the above touches a real Windows kernel, a
real Windows Vulkan ICD, or a non-NVIDIA GPU. §7.14 of the build brief anticipated exactly this
("If distribution ever matters, budget real time"). So the ADR states what "Windows supported"
means, in these words: **"the Windows build links, starts, and renders a reference-matching frame
under Wine on the development machine. It has never been run on Windows."** That sentence is the
deliverable. An unqualified "Windows supported" would be the lie.

---

## 5. The Build UI

### The shape: the editor shells out to `loom ship`

**The Build button spawns `loom ship` as a subprocess and streams its JSON lines into the console
panel.** It does not call the packaging code in-process.

That is the lazy answer and it is also the correct one, because it makes the human's build and the
agent's build *literally the same execution*. M12's exit criterion — "the same edit made by hand
and by agent produces an identical diff" (`LOOM-BUILD-BRIEF.md:285`) — is a property about
authoring; this is the same property applied to shipping, and getting it for free by not
duplicating the code is better than getting it by testing for it. It also disposes of threading:
`cargo build` takes minutes and the editor must keep repainting, and a subprocess with a pipe
solves that without `loom_cli` growing its first thread.

**That last point is not incidental.** `main.rs:3736-3741` records that `SCENE_BASE` is a
`thread_local!` and that the reasoning depends on "nothing in `loom_cli` or `loom_render` spawns a
thread". A build thread would make that comment false, and the next person to read it would
believe it. A subprocess keeps it true.

### `loom ship`

```
loom ship [--target <triple>] [--out <dir>] [--project <dir>] [--dry-run]
```

One JSON object per line, in the existing house style (`main.rs:1-8`: "Every subcommand emits
structured JSON on stdout and uses the exit code as the coarse signal"). Progress lines carry
`{"stage": "compile", "done": 47, "total": 214}`; the final line is the report. `--dry-run` runs
every assertion in §6 and copies nothing, which is the pre-flight the human wants before a
five-minute build.

Progress comes from `cargo build --message-format=json-render-diagnostics`, parsed with
`serde_json` (already a `loom_cli` dependency): `compiler-artifact` lines advance the counter,
`compiler-message` lines carry the rendered diagnostic straight through to the console. No new
dependency, no cargo-progress-bar crate.

New module: **`crates/loom_cli/src/ship.rs`**, plus a `Some("ship") => ship::run(args)` arm in the
dispatch at `main.rs:226-306` and a `("ship", &[...])` row in the `FLAGS` table at `main.rs:176`
so an unknown flag is a failure rather than a no-op.

### The panel

A modal dialog, not a dock tab. Building is an occasional, blocking, whole-project act; a
permanently docked panel would be furniture. It carries:

- **Target** — a segmented control reading `build.targets` from the manifest, defaulting to the
  host. Each target shows its readiness inline: a Windows entry with no `x86_64-w64-mingw32-gcc`
  on `PATH` renders disabled with the `dnf install mingw64-gcc` line as its tooltip. **Telling the
  human what is missing at the moment they want it is worth more than a documentation page.**
- **Output directory** — a path field defaulting to `<project>/out/<name>-<target>`, with the
  overwrite rule below shown as text, not discovered by losing files.
- **Profile** — fixed at release, displayed and not editable. §6.5 is why.
- A **Check** button running `--dry-run`, and a **Build** button.
- During: a determinate bar, the current crate name, and a Cancel that kills the child.
- After: pass/fail per assertion, the elapsed time, the output size, **Reveal** (`xdg-open`), and
  for the host target a **Run** button that spawns the shipped executable *from its own directory*
  — which is the one-click proof that §3's root resolution works.

### Overwrite safety

**`loom ship` refuses to write into a non-empty directory it did not create.** The marker is
`.loom-build.json` at the top of the output directory: present means this tool made it and may
clean it, absent-and-non-empty means stop and say so. The failure this prevents is someone
setting the output directory to `~` and cleaning it, which is a class of bug this project already
takes seriously enough to have a hook for elsewhere.

### Build is outside undo, and that is written down

`00-survey-constraints.md` §4J asks the design phase to produce the list of editor actions that
are not `SceneOp`s. Build contributes two entries and no more:

**Pressing Build changes no authored state at all** — it reads the project and writes into an
output directory outside it. There is nothing to undo, and Ctrl+Z correctly does nothing.

**Editing build settings changes `loom.toml`, which is diffable text but is not scene text.** It
therefore falls outside `Session`'s undo stack, which is a stack of scene files
(`edit.rs:314-323`). This is the same exemption the project manifest as a whole carries and it is
the manifest ADR's to state; recorded here so it is not discovered later. The mitigation is that
`loom.toml` is a small text file under version control, so `git diff` is the review channel and
`git checkout` is the undo — which is the same answer this project gives for terrain recipes
(`assets/test/vale.toml`), an existing precedent rather than a new exemption.

---

## 6. What a clean shippable build must assert

Ordered cheapest-first, so a broken project fails in a second rather than after a compile. Each
names the failure it prevents.

**6.1 Every `.loom` under the asset root validates.** Through `loom validate`'s code path, not a
re-implementation, and through `prefab_load::for_reading` — `prefab_load.rs:7-12` states that a
reader skipping resolution "reintroduces exactly that bug", and the engine-surface survey caught
two existing offenders (`scene_view.rs:110`, `main.rs:3440`). **A new command that reads scenes is
exactly where that regresses**, so this is the third place to get it right. Prevents shipping a
scene the runtime will refuse to open.

**6.2 Every asset alias resolves.** `alias_report(&scene, base)` (`main.rs:350`) already returns
`(unresolved, missing)` and `loom validate` already calls it; ship promotes a non-empty result
from warning to error. Prevents the commonest shipping bug there is — a texture that resolved on
the dev box and is not in the tree.

**6.3 No path escapes the project root.** Every `[[asset]]` path, `Script.path`, `GameRules.path`
and terrain recipe path is canonicalized and asserted to be under the root. **This is the most
valuable check in the list and it is the only one with no existing analogue**, because it fails
*only* when shipped: an asset at `../../shared/textures/rock.png` works perfectly in the editor and
is a dangling reference the moment the tree is copied. It is also where a symlink pointing at
`/mnt/data` is caught (§3.4).

**6.4 The startup scene is named in the manifest and exists.** A one-line check for the failure
mode of a game that builds successfully and opens nothing.

**6.5 The profile is release and `debug_assertions` is off.** Not a convention — a hard assertion,
because `instance.rs:118-120` makes validation mandatory when `cfg!(debug_assertions)` and returns
`ValidationLayerMissing` when the layer is absent. **A debug-profile shipped game refuses to start
on any machine without `VK_LAYER_KHRONOS_validation` installed**, which is every player's machine.
The error message is excellent and entirely wrong for that audience. This is verified from source,
it is a genuine footgun, and it costs one comparison to close.

**6.6 The shipped binary contains no egui.** `cargo tree -p loom_cli --no-default-features -e
normal` must not mention `egui`, `egui-winit` or `egui-ash-renderer`. Add it to
`scripts/check-deps.sh` beside the existing `ash`-containment grep, so it is a green-check rule
rather than a ship-time one and a regression is caught by whoever caused it. A second rule with
it: **nothing but `loom_cli` may depend on `loom_editor`** — the same shape as the existing
`loom_agent` rule, and for the same reason.

**6.7 Objdump the Windows import table** (§4.4): no `vulkan-1.dll`, and every non-system DLL
copied and listed.

**6.8 Smoke-run the shipped tree.** Launch the shipped executable *from the output directory* with
`--render <startup scene> --out <tmp>.png --frames 1` and require exit 0 and a non-empty PNG.
**This is the only assertion that proves the assets folder is complete**, because every other check
reads the manifest and this one reads the disk the player will read. It is what would catch §3.4's
shader exclusion being wrong, or a texture the alias report never mentioned because nothing named
it.

For the cross target the same run happens under Wine when Wine is present, and **skips honestly**
when it is not — the exact pattern `scripts/green.sh` already uses for `cargo xtask validate`
without a GPU. A skipped check is reported as skipped, never as a pass.

**6.9 Not asserted, deliberately:** the determinism hash. It belongs to `cargo test` and re-running
it per ship would add minutes to every build for a property that did not change because assets were
copied.

---

## 7. Files touched

| File | Change |
| --- | --- |
| `crates/loom_render/Cargo.toml` | three egui deps become `optional`; `[features] default = [] / editor = [...]` |
| `crates/loom_render/src/lib.rs` | `#[cfg(feature = "editor")]` on `mod ui`, `pub use ui::Ui`, `pub use egui` |
| `crates/loom_render/src/viewer.rs` | `#[cfg(feature = "editor")]` on `draw_with_ui` (`:936`) and the `ui` pass (`:1590-1619`). `draw` (`:922`) unchanged |
| `crates/loom_cli/Cargo.toml` | two `[[bin]]`s; `loom_render` with `default-features = false`; optional `loom_editor`; `[features] default = ["editor"]` |
| `crates/loom_cli/src/bin/play.rs` | **new**, ~120 lines: root, manifest, startup scene, bindings, window in play mode; `--render`/`--frames` for the smoke check |
| `crates/loom_cli/src/project.rs` | **new**, ~30 lines: `project_root`, manifest read |
| `crates/loom_cli/src/ship.rs` | **new**, ~400 lines: assertions, cargo driver, tree copy, objdump, report |
| `crates/loom_cli/src/main.rs` | `Some("ship")` arm at `:226`, `FLAGS` row at `:176`, USAGE entry |
| `crates/loom_cli/src/run.rs` | `load_bindings(root)` at `:2242`; the editor/runtime split of the rest |
| `crates/loom_editor/` | **new crate**: `panels.rs`, `gizmo.rs` and the editor half of `run.rs`, plus the Build modal |
| `.cargo/config.toml` | `[target.x86_64-pc-windows-gnu]` linker entry |
| `rust-toolchain.toml` | add `x86_64-pc-windows-gnu` to `targets` |
| `scripts/check-deps.sh` | the two rules from §6.6 |
| `xtask/src/main.rs` | one extra render in `image`: a golden scene through the no-default-features binary (§1) |
| `docs/decisions/00NN-*.md` | the two ADRs in §8 |
| `docs/editor/building-and-shipping.md` | end-user documentation (user's decision 4) |

**No new `SceneOp`s, and no changes to the nine that exist.** Shipping reads authored state and
writes outside the project; it never authors. That is the whole of its relationship to never-do
#16.

**No new runtime dependency.** `serde_json` (cargo's message stream), `std::process` (cargo,
objdump, wine) and `std::fs` (the tree copy) are all present. `walkdir` is already in the lock via
another crate but is not a `loom_cli` dependency; `std::fs::read_dir` recursion is a dozen lines
and does not justify adding one (never-do #6 makes every addition a deliberate act, and this one
would not survive the question).

---

## 8. ADRs required

### ADR A — The editor is a separate crate and the runtime binary is built without it

- **Decision touched:** new. Brief §3 lists `loom_editor/` in the planned layout
  (`LOOM-BUILD-BRIEF.md:107`) but no such crate exists, and egui is an unconditional dependency of
  `loom_render` (`loom_render/Cargo.toml:11-13`), so every build today links the editor.

> **Decision.** The editor's UI moves into a new `loom_editor` crate. egui, `egui-winit` and
> `egui-ash-renderer` become optional dependencies of `loom_render` behind a **non-default**
> `editor` feature, which `loom_editor` enables and `loom_cli` enables only through its own
> default feature. `loom_cli` gains a second binary, `loom-play`, built with
> `--no-default-features`; that binary is what ships. The feature gates exactly two things — the
> `ui` render-graph pass (`viewer.rs:1590-1619`) and `Viewer::draw_with_ui` — and gates nothing in
> the forward pass, the tonemap, MSAA, the resolve or CMAA2, because ADR 0018 already paid three
> defects for letting the window and the offscreen path diverge. Two CI rules keep the boundary
> honest, added to `scripts/check-deps.sh` beside the existing `ash` containment rule: a
> `--no-default-features` dependency tree containing egui is a failure, and anything other than
> `loom_cli` depending on `loom_editor` is a failure. `cargo xtask image` renders one golden scene
> through the shipped-configuration binary and requires it to match the reference the
> editor-configuration binary produced, which is what converts "the gate does not change the
> image" from a claim into a check.

Consequences worth writing into the ADR: `loom_cli` becomes the runtime library as well as the
agent CLI, which is a role it already had and did not have a name for; a third `loom_runtime`
crate was considered and rejected as a rename of the nine modules both binaries need; and
stripping egui from the shipped build removes three crates and a font stack from the Windows
cross-compile, which is a material reduction in the risk ADR B addresses.

### ADR B — Windows is cross-compiled for the GNU ABI, and "supported" means what Wine proved

- **Decision touched:** `rust-toolchain.toml`, which pins one target; `.cargo/config.toml`, which
  configures a linker for that target only; and brief §7.14, which asks for exactly this to be
  written down.

> **Decision.** Windows builds target `x86_64-pc-windows-gnu`, linked with
> `x86_64-w64-mingw32-gcc` from Fedora's `mingw64-gcc`. The MSVC ABI is rejected: it needs the
> Windows SDK import libraries and CRT on a Linux box, which is a licence question and a large
> moving part, while the GNU toolchain is one `dnf install` and both `windows_x86_64_gnu` and
> `windows_x86_64_gnullvm` are already in `Cargo.lock`. Shaders need no cross-compilation work at
> all: `build.rs` runs `slangc` on the host and the SPIR-V is embedded with `include_bytes!`, so
> the shipped game carries no `.slang`, no `.spv` and no runtime compiler. The Vulkan loader is
> found at runtime by `libloading` on `"vulkan-1.dll"` (`ash/src/entry.rs:63`) so there is no
> build-time Vulkan dependency; the ship step proves this by asserting `vulkan-1.dll` is absent
> from the linked import table. **Verification is sequenced first, before any build UI exists**,
> in six steps that fail attributably: dependency-graph resolution for the target, `cargo check`,
> leaf-first compilation, link, a Wine smoke run against this machine's winevulkan, and a Wine
> pixel-compare against the golden reference. **"Windows supported" is defined as: the build
> links, starts, and renders a reference-matching frame under Wine on the development machine. It
> has never been run on Windows, and the documentation says so in those words.**

Consequences: `rust-toolchain.toml` gains a second target and `.cargo/config.toml` a second
`[target]` block, both additive. The `-gnu` ABI may require `libgcc_s_seh-1.dll` and
`libwinpthread-1.dll` beside the executable; the ship step reads the import table and copies
whatever is actually needed rather than guessing. `blake3` is the one dependency compiling C for
the target and its `pure` feature is a semantics-preserving fallback if the mingw build fails.
Golden images are **not** run on the second target — a Wine render is a build check, not a
platform check, and treating it as a gate would be the vacuous-green failure `instance.rs:66-70`
already warns about in another context.

### Not mine, but depended on

The **project manifest ADR** (`00-survey-constraints.md` §4H) owns `loom.toml`'s schema and its
location. Build-and-ship needs `game.name`, `game.startup_scene` and `build.targets`, and needs
the runtime and the ship step to deserialize through one type. If that ADR lands the manifest
somewhere other than `loom_asset`, §3.2 follows it — nothing else here moves.

---

## 9. Order of work

1. **V0 and V1 from §4.5, against today's tree.** Hours, not days, and no refactor required. If
   the dependency graph or the type-check says Windows is impossible, everything below changes.
2. **ADR A: the crate split and the feature gate.** The `loom-play` binary, `project_root`, the
   `load_bindings` fix, the two `check-deps.sh` rules, the golden-image cross-check. Ships a Linux
   runtime with no editor in it, verifiable by `cargo tree`.
3. **`loom ship` for the host target**, with §6's assertions and §3.4's copy. Ships a playable
   Linux folder. The agent can now ship; the editor still cannot.
4. **V2–V5: the Windows cross-compile end to end.** ADR B is written *after* this, with the
   measurements in it, following ADR 0019's pattern of recording the constants the measurement
   chose rather than the ones the design guessed.
5. **The Build modal.** Last, because it is a process runner over a thing that already works, and
   because a UI over an unproven pipeline is how a phase spends its budget on the wrong half.

---

## 10. What I could not verify

**Nothing was built.** The design phase forbids `cargo build`, `cargo test`, `cargo clippy` and
`cargo xtask` (parallel cargo builds have twice frozen this machine), so every cross-compile claim
below rests on reading source and querying package state, never on a compiler's opinion.

- **Whether anything cross-compiles at all.** V0 through V3 are unrun. `cargo tree --target` is a
  metadata operation and would have been safe, but it is `cargo` and the instruction was
  categorical. **V0 is the first thing to run and it is nearly free.**
- **`blake3`'s C/assembly build under mingw.** Predicted to work — the crate ships
  `*_windows_gnu.S` and `cc` selects the mingw compiler by triple — but this is the highest-
  probability failure in the set and it is unproven. The `pure` fallback is reasoned, not tested.
- **`-C target-feature=+crt-static` for `x86_64-pc-windows-gnu`.** I do not know whether this
  toolchain links libgcc and winpthread statically. §4.4's objdump-and-copy handles both outcomes,
  which is why I did not need to resolve it — but I did not resolve it.
- **`winit 0.30.13`'s Windows backend compiling with this exact lockfile.** High confidence,
  no evidence.
- **`cpal 0.18.1`'s cfg boundaries.** I read the lock, not the crate's `Cargo.toml` cfg
  expressions. If `alsa-sys` appears in a Windows resolve, V0 catches it in seconds and the answer
  is a `[target.'cfg(unix)'.dependencies]` on `loom_audio` or a feature-gated silent backend.
- **Whether Wine's winevulkan will run this renderer.** Wine 11.0 Staging and
  `~/.wine/drive_c/windows/system32/vulkan-1.dll` were both confirmed present. Whether swapchain
  creation, dynamic rendering, descriptor indexing, buffer device address and 4x MSAA all work
  through it is unknown. Ray query is safe because `device.rs:78-80` already treats it as optional;
  the rest is not.
- **Whether a headless `--render` works under Wine**, which V5 depends on. Untested.
- **Cargo's feature resolution behaving as §1 describes** under `resolver = "3"` — that
  `--no-default-features` on `loom_cli` plus `default-features = false` on its `loom_render`
  dependency actually keeps egui out. This is how the documentation says it works and it is how
  I have seen it work; it is not how I have seen it work *in this workspace*. **The check in §6.6
  is precisely a check for me being wrong about this**, which is why it is a green-check rule and
  not a ship-time one.
- **Whether `assets/shaders/` is genuinely unneeded at runtime.** `rg` found two readers of
  `.slang` and both are `#[cfg(test)]`. A grep is not a run. §6.8's smoke test against the
  stripped tree is what would catch it, and I could not run that either.
- **Ship size and build time.** This repo's `assets/` is 132 MB of textures plus 53 MB of meshes;
  what a real project weighs, and how long a cold release cross-build takes, are both unmeasured.
- **The Linux target's glibc floor.** Built on Fedora 44, the binary will not start on a distro
  with an older glibc, and I did not check which version that is or how far back it matters.
  The fix, if it ever does, is an older sysroot or a container build — deferred here with the
  reason stated rather than discovered by a bug report.
- **What the manifest actually looks like.** §3.2 depends on a document being written in
  parallel. I specified the three keys I need rather than inventing a schema that would then have
  to be reconciled.
