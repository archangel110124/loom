# Design — the project model, revised: the engine repo is a project too

*Editor rework, round 2. Written against `62f9ebe` after reading `PLAN.md`, `02-project-hub.md`,
the three `00-survey-*` documents, and the code every claim below cites. **No `cargo` command was
run** — design phase. §11 lists what that leaves unverified.*

*This document revises ADR 0023 and ADR 0024 as PLAN.md allocated them. Both are still unwritten,
so the revisions are edits to a decision statement rather than superseding ADRs. It proposes one
genuinely new ADR, **0033**.*

---

## 0. The answer in one paragraph

**The engine repository becomes a project by gaining one checked-in file — `loom.toml` at the root
— and nothing else changes.** No scene moves, no path is rewritten, and *no code that resolves an
asset path is touched*, so the 43 gated scenes and the 28 golden references cannot move: not
"should not", cannot, because the resolver is the same function operating on the same bytes.
Project-relative asset paths are **rejected**, for the same reason ADR 0024 already accepts a
loud break on rename. What the decision *does* expose is a real defect the design set had not
named: **three different things in this engine call themselves "assets", and only one of them
resolves correctly outside this repo.** That is ADR 0033, and it is the whole of the new work.

---

## 1. Three namespaces, and why only one of them is fine

Everything that reads a file at runtime does it in one of three ways. I read all seven sites.

**Scene-relative — the authored namespace, and it is correct.** `base` is always the scene file's
parent directory (`main.rs:571`, `scene_view.rs:115`, `main.rs:3828`, `:3908`, `:4017`), and it is
joined onto an advisory relative path at every consumer: meshes at `main.rs:1163`, textures at
`materials.rs:107` and `:255`, scripts at `play.rs:1090` and `:1123`, audio clips at
`sound.rs:122`, prefab files at `prefab.rs:135` (with an absolute-path branch at `:133`), and
terrain recipes at `main.rs:3126` through the `SCENE_BASE` thread-local (`main.rs:3748-3757`).
Seven sites, one rule, no registry. **This namespace needs no change and gets none.**

**Cwd-relative — one site, and it is a shipping bug.** `load_bindings` (`run.rs:2242-2251`) reads
the literal path `assets/input/default.toml` against the *process working directory*. It works
here only because `xtask` runs the binary with cwd at the repo root (`repo_root()`,
`xtask/src/main.rs:1332`). A shipped `exe + assets/` folder launched by double-click gets the
compiled-in fallback and the user's rebinding silently does nothing. The engine survey already
flagged this; doc 02 §8 named the fix.

**Engine-relative pretending to be scene-relative — two sites, and this is the one the decision
exposes.** `sound.rs:57` loads the weather bed as `base.join("../audio/rain.wav")` and
`main.rs:3238` does the same for `loom audio`. That is *the engine's own* recording, addressed as
though it were the scene's. It resolves in this repo purely because every gated scene lives at
`assets/test/` or `assets/games/`, so `../audio/` lands on `assets/audio/rain.wav` — which exists.
**In a project created by `loom new`, whose scene is `scenes/main.loom`, it resolves to
`<project>/audio/rain.wav`, which does not exist, and rain silently degrades to the synthesiser.**
No error, no warning: `sound.rs:53-59` logs "no rain recording; synthesising the weather" at info
level and carries on. The first standalone project that rains would sound different from every
scene in this repo, and nothing would say why.

`assets/templates/` (doc 02 §8) is a fourth instance of the same problem that has not been built
yet, which is why it is cheaper to name the namespace now than to discover it three times.

---

## 2. What marks the engine repo as a project

One file, checked in, at the repository root:

```toml
# loom.toml — this directory is a Loom project.
#
# The engine repository is a project like any other: `loom edit .` opens it,
# the hub lists it, and the fifty scenes under assets/ are its scenes. It is
# also the engine's own source tree, which is why [ship] excludes that half.
#
# Nothing here is read by `cargo xtask validate` or `cargo xtask image`. The
# gates address scenes by repo-relative path and resolve assets relative to
# each scene file, exactly as they did before this file existed.

[project]
format = 1
id = "…"                                         # generated once, then stable
name = "Loom engine"
main_scene = "assets/games/proving_ground.loom"

[engine]
version = "0.0.0"

[ship]
exclude = ["crates", "xtask", "tools", "scripts", "tests",
           "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "clippy.toml", "docs"]
```

**`main_scene` is `proving_ground.loom` because it is the only scene in this repo that is a
game.** It is already in `GOLDEN`, so the hub's thumbnail subprocess renders something that is
pixel-guarded elsewhere, and a thumbnail that regresses is a thumbnail whose regression another
gate already caught.

**Two additions to ADR 0023's field list, both small, both forced by this decision.**

`[ship] exclude` is a new optional array of root-relative names, defaulting to empty, added to
ADR 0032's fixed exclusion list rather than replacing it. It exists because the repo-as-project is
the one project whose root contains a Rust workspace, and "press Build and ship the compiler" is
a bad first experience for the strangers this rework is now built for. It is ten lines in
`loom ship` (a `starts_with` test per entry, same shape as the fixed list) and `loom new` writes
no `[ship]` table at all, so template projects are unaffected. `Project` must carry
`ship: Option<Ship>` for `deny_unknown_fields` to accept the table — that is the whole cost.

**`project::scenes()` must skip dot-directories, and this repo is why.** A naive `**/*.loom` walk
from `/home/k-dorui/loom` descends into `.claude/worktrees/`, which holds full checkouts of the
repository — this document is being written inside one. The hub would list every scene two or
three times, and each duplicate would open a different file with the same name. The skip list is
`target`, `builds`, `out`, and **any directory whose name begins with `.`**. That last clause is
the one that is not obvious, and it is worth the comment it will get.

`/builds` joins `.gitignore` in the same commit, next to the existing `/target`.

---

## 3. `[[asset]]` path resolution: unchanged, and that is the deliverable

**Resolution stays exactly what it is: an advisory relative path joined onto the declaring scene
file's own directory.** No project root enters the resolver. No search path. No fallback. The
function that does it (`main.rs:1150-1163`) is not edited by this design.

That is not a preference, it is the byte-identity argument. A gate that compares pixels cannot be
argued into safety; it can only be given nothing to react to. **Every change in this document is
either a new file nothing reads (`loom.toml`), or a lookup on a code path no golden render
executes (§4).**

I checked the claim rather than asserting it. Across all 25 scenes carrying an `[[asset]]` block,
**176 `path` values: 165 resolve scene-relative, and zero resolve relative to the repository
root.** Of the eleven that do not resolve scene-relative as written, nine carry a `#Object`
selector that `main.rs:1155-1162` strips before joining (`../meshes/lantern.obj#WallLantern…` in
five scenes, `../meshes/trees9.obj#…` four times in `trees.loom`), and the remaining two are
`blockout.loom:13` and `office.loom:18`, which point at `assets/primitives/box.glb` and
`assets/props/desk.glb` — directories that do not exist. `blockout` is unaffected because its
alias is `box` and `loom_asset::primitives::build` answers first (`main.rs:1146`); `office` draws
a substituted unit box with a warning and has done so since it was written.

**So even a project-relative *fallback* would be provably inert in this repository.** The reason
to reject it anyway is that it would not stay inert.

### Why project-relative paths are rejected

The tempting version is a fallback: try scene-relative, then try project-relative. It is eight
lines and it is exactly the construction to refuse, because **a fallback resolver turns a broken
reference into a working one at a distance.** A scene moved from `scenes/` to `scenes/act1/` keeps
loading — until the day someone opens it outside a project, or ships it, or the project root moves
— and the failure then arrives far from the edit that caused it. That is precisely the failure
mode ADR 0024 rejects UUID sidecars to avoid: *"path-relative resolution fails loudly at the point
of authorship, which is better feedback than a stale manifest."* A silent second chance is a stale
manifest with no file.

The second tempting version is a sigil — Godot's `res://`, spelled `project://` here — resolving
explicitly from the project root with no ambiguity and no precedence rule to write down. **This is
the right escape hatch and it is not needed yet.** It costs a `strip_prefix` in one function and a
paragraph in `docs/format/README.md`, and every case it solves today is solved better by the
editor's own asset picker computing the relative path (~15 lines of `std::path` component
comparison, no crate) and by the agent doing what it already does successfully in 165 places.

**Reserve the spelling, do not build it.** ADR 0024 gains one sentence naming `project://` as the
reserved prefix for project-root resolution, so that when it is wanted nobody invents `$/`, `//`
or a bare leading `/` — and a bare leading `/` in particular must stay meaning *absolute
filesystem path*, which is what `base.join()` already does and what `prefab.rs:133` explicitly
branches on.

**There is therefore no precedence rule and no ambiguity to report**, which is the point. The one
ambiguity that does exist today is unrelated and pre-existing: a mesh alias that is also a
primitive name resolves to the primitive and the `[[asset]] path` is never consulted
(`main.rs:1146`, and `alias_report` mirrors it at `main.rs:456-460`). `blockout.loom` relies on
that. It is worth one sentence in the format spec and no code.

---

## 4. ADR 0033 — engine-owned assets resolve from the executable

This is the new decision, and it is the only behavioural change in the document.

> **Decision.** Three files are owned by the *engine* rather than by any project: the input
> bindings (`input/default.toml`), the weather recording (`audio/rain.wav`), and the project
> templates (`templates/`). They resolve through one function,
> `loom_scene::project::engine_assets() -> PathBuf`, defined as **`<exe dir>/assets` if it exists,
> otherwise `assets` relative to the working directory** — the second branch being what keeps
> `cargo run`, `cargo test` and both `xtask` gates working from the repository root.
>
> Where a project may legitimately own its own copy — bindings and the weather bed — the lookup
> order is **project root → `engine_assets()` → compiled-in**, and only for these named files. It
> is not a general search path: `[[asset]]` paths never consult it (§3).
>
> `loom ship` copies the project root and does not inject engine-owned assets. A shipped game that
> wants the rain recording owns a copy at `<project>/assets/audio/rain.wav`; one that does not gets
> the synthesiser, which is the same fallback the engine already takes.

**The property that makes this cheap is a coincidence worth stating out loud: in the two cases
that matter, the engine's assets and the project's assets are the same directory.** In this
repository, `engine_assets()` is `<repo>/assets` and the project root's assets are `<repo>/assets`.
In a shipped game, `loom ship` puts the executable at the project root, so `<exe dir>/assets` *is*
the project's `assets/`. Only during development of a standalone project are the two different,
and that is the single case the two-step lookup exists for.

Concretely, three call sites change:

| Site | Today | After |
| --- | --- | --- |
| `run.rs:2242-2251` `load_bindings` | `assets/input/default.toml` against cwd | `<root>/assets/input/default.toml` → `engine_assets()/input/default.toml` → `loom_input::DEFAULT_BINDINGS` |
| `sound.rs:57` weather bed | `base.join("../audio/rain.wav")` | `<root>/assets/audio/rain.wav` → `engine_assets()/audio/rain.wav` → synthesiser |
| `main.rs:3238` `loom audio` | same join | same order as `sound.rs` |

**In this repository every one of those resolves to the byte-identical file it resolves to today**
— `<repo>/assets/input/default.toml` and `<repo>/assets/audio/rain.wav`, which exist (verified:
`assets/audio/{hum,rain}.wav`, `assets/input/default.toml`). In scene-only mode there is no project
root and the first step is skipped, which is today's behaviour minus the `../` guess.

**Neither site can move a golden reference**, and that is checkable rather than hopeful:
`Sound::start` has exactly one caller, `run.rs:1338`, on the windowed play path. `loom render` —
which is what `xtask image` and 43 of `xtask validate`'s invocations drive — never constructs a
`Sound`. `loom audio` is driven by no gate.

`engine_assets()` lives in `loom_scene::project` beside `Project`, because `loom_editor` (the hub,
for templates) and `loom_cli` (bindings, audio) both need it and neither can call the other —
`loom_cli` depends on `loom_editor`, not the reverse (PLAN §2.1). It needs only
`std::env::current_exe`, so `loom_scene`'s dependency rule (`scripts/check-deps.sh:25-31`,
`loom_reflect` only) is untouched — the same argument doc 02 §2 makes for putting `Project` there.

---

## 5. The hub lists both, and it lists them the same way

**A recents row is a path plus what `loom.toml` says, and the engine repo produces one like any
other directory.** No special case, no "engine project" badge, no branch in the hub. It sorts by
`last_opened` with everything else. Its card reads *Loom engine*, `~/loom`, and a thumbnail of
`proving_ground.loom` rendered by the subprocess doc 02 §4 specifies.

Two consequences of it being a real row rather than a pinned entry, both good. The repo appears in
recents only after someone opens it, so a fresh install shows the empty state doc 02 §7 designed —
the engine's own tree does not squat in a new user's launcher. And **opening it exercises the
project model against fifty real scenes with imported meshes, textures, scripts, prefabs and
terrain recipes on day one**, which was doc 02's stated payoff and is now free rather than
aspirational.

The one asymmetry is deliberate: the hub's *Templates* rail lists directories under
`engine_assets()/templates/`, not under any project. Templates are engine content and belong to
the binary that creates from them.

---

## 6. `loom new` and the shipped game

**`loom new` is unchanged from doc 02 §8** — refuse a non-empty target, copy the template
directory, rewrite `loom.toml`'s `id`/`name`/`engine.version`, regenerate `[scene] id` and every
`[[prefab]] id`, print one JSON line. Two clarifications this revision adds:

It writes **no `[ship]` table**, because a template project has no source tree to exclude, and an
empty array in every generated manifest is a field users have to learn for no reason.

It writes **only scene-relative asset paths**, because that is the only kind there is. The
templates' scenes sit at `scenes/main.loom` and reference `../assets/scripts/fps.rhai` — the same
one-up-one-down shape as `assets/test/*.loom`'s `../textures/…`, which is why doc 02 §1 chose
`scenes/` beside `assets/` and why that choice survives this revision intact.

**A shipped game resolves assets identically to development, because it is the same join over the
same tree.** `loom ship` copies the project root (ADR 0032), which preserves every relative
distance between a scene and the files it names, so a path that resolved in the editor resolves in
the shipped folder by construction. The only lookup that changes is `engine_assets()`, and §4
explains why it lands on the same directory.

The honest gap, stated because a stranger will hit it: **a shipped game whose project never copied
`rain.wav` rains with the synthesiser.** That is a template question, not an engine one — the
weather templates, when they exist, should copy the file the way `first_person` copies
`fps.rhai` — and `loom ship`'s JSON report is the right place for a one-line note when a scene
declares `Rain` and the tree carries no `assets/audio/rain.wav`.

---

## 7. Migration: none, and the proof is that no scene is read

**Zero migration. No `.loom` file is opened, parsed, rewritten or re-blessed by anything in this
design.** The work is: one new file at the repo root, one new module in `loom_scene`, three
changed lookups on paths no golden render walks, one line in `.gitignore`, and one one-line bug
fix (§8).

For existing *user* projects there is nothing to migrate either, because there are none — this is
the first release with a project concept. For a directory that has scenes and no manifest, the
answer is already designed: `find_root` returns `None`, the editor runs in scene-only mode, which
is exactly today's `loom run --edit` (doc 02 §2), and the hub's Open dialog offers *"No project
here. Create one?"* (doc 02 §7).

---

## 8. One live bug this subsystem owns, and it is one line

`alias_report` checks an asset file's existence with `base.join(p).exists()` (`main.rs:483`)
**without stripping the `#Object` selector** that `MeshLibrary` strips forty lines earlier
(`main.rs:1155-1162`). So `props.loom`, `stoneyard.loom`, `moraine.loom`, `mountain_pass.loom`,
`croft.loom` and `trees.loom` each emit an `asset_file_missing` warning for a mesh that loads
perfectly — six of the 43 gated scenes, warning about a file that is there.

It is a warning rather than an error, so no gate fails and it has survived. **It belongs to this
design because it is a path-resolution site that disagrees with the resolver**, which is the
category of defect this document exists to close, and it is `split_once('#')` in one expression.

**Stage 0**, with a regression test that validates `props.loom` and asserts zero
`asset_file_missing` warnings.

---

## 9. Verification plan

Ordered so that a failure at step *n* stops step *n+1*. Steps 1–3 are the byte-identity proof the
user asked for; 4–8 are the new behaviour.

**V1 — nothing authored changed.** `git status --porcelain` after the whole change lists
`loom.toml`, `.gitignore`, `docs/`, `crates/loom_scene/src/project.rs`, and the three edited
`loom_cli` files. **No `.loom` file, and no file under `tests/references/`.** If a scene appears in
that list the design was not followed.

**V2 — `cargo xtask image`: 28 references, zero moved, and `tests/references/MANIFEST.txt`
unchanged byte-for-byte.** This is the load-bearing check and it is expected to be trivially green,
because no code on the render path was edited. **A single moved reference means something reads
`loom.toml` that should not** — investigate, never `--bless`.

**V3 — `cargo xtask validate`: 43 scenes, zero Vulkan messages**, and the JSON warning count drops
by exactly the six `asset_file_missing` warnings §8 removes. Diff the warning sets rather than
counting them, so a warning that vanished for the wrong reason is visible.

**V4 — a test that locks the rejection in.** `project_root_paths_do_not_resolve`: write a temp
project with `loom.toml`, a scene at `scenes/a.loom`, and a texture at `assets/t.png`; declare
`path = "assets/t.png"` (project-relative, correct-looking, wrong); assert the load falls back and
`loom validate` reports `asset_file_missing`. **This test is the design.** Without it the fallback
gets added by someone who reasonably thinks it is missing.

**V5 — the repo is a project, as a unit test, not a manual step.**
`engine_repo_is_a_project`: `project::load(repo_root())` succeeds, and `project::scenes()` returns
a set containing every entry of `xtask`'s `SCENES` list. Cheap, and it fails the day someone moves
a scene without telling the hub. It also fails if the dot-directory skip is dropped, because
`.claude/worktrees/` would double the set — assert the returned paths are unique.

**V6 — cwd independence, which is the standalone case in one command.** `loom new /tmp/p
--template first_person`, then from `/tmp` (**not** the repo root) run
`loom render /tmp/p/scenes/main.loom --out /tmp/p.png` and require a non-empty PNG with a non-empty
alias report. Today the bindings lookup would silently take the compiled-in copy; this is the check
that says whether `engine_assets()` works from a `cargo run` build.

**V7 — the weather bed, both ways.** In the repo, `loom audio assets/test/squall.loom` still
reports `source = "recording"` (it resolves through the new order to the same file). In
`/tmp/p` with no `assets/audio/`, it reports `"synthesised"` and says so once, not silently.

**V8 — `loom ship` on the repo itself.** Run it and assert `crates/`, `xtask/` and `Cargo.lock`
are absent from the output tree while `assets/` and `loom.toml` are present. This is the only
check `[ship] exclude` needs.

**What would move a reference, stated so nobody is surprised.** Nothing in this document. The
candidates I considered and rejected each would have: a project-relative *fallback* (`office.loom`
and `blockout.loom` would newly resolve if anyone ever created `assets/props/` or
`assets/primitives/`, silently changing two renders); making `[[asset]] path` strict rather than
warning (those same two scenes would fail to load, and `office` is in `SCENES`); moving the fifty
scenes into `scenes/` (rewrites `SCENES`, `GOLDEN`, every reference filename and `MANIFEST.txt` —
doc 02 §1 rejected it and this revision agrees emphatically).

---

## 10. Where this belongs in PLAN.md, and what it depends on

**Stage 5, unchanged — plus one line in Stage 0.** No new stage.

Stage 5 already owns `loom_scene::project`, `loom edit`, `loom new`, the hub, the bindings-path
fix and `loom ship`. This revision adds to that stage: the repo's `loom.toml`, `[ship] exclude`,
the dot-directory skip in `scenes()`, `engine_assets()` and its three call sites, `/builds` in
`.gitignore`, and V4–V8. It removes nothing from the stage.

Stage 0 gains the `#Object` fix from §8 — one line and a regression test, sitting beside the other
one-line fixes already scheduled there. It has no dependency on anything else in this design and
should not wait for Stage 5.

**Dependencies.** The manifest, `engine_assets()` and the resolution rules depend on nothing and
could land at Stage 0 if `loom.toml` were wanted earlier; the hub UI depends on Stage 3 (the dock
and theme), exactly as PLAN.md already has it. Nothing in Stages 6–8 (painting, sculpting) depends
on this: strokes are scene text and paint textures are `[[asset]]` references, both of which
resolve through the unchanged scene-relative path.

---

## 11. ADRs — one new, two amended

### New: **ADR 0033 — engine-owned assets resolve from the executable, not from the scene**

> **Decision.** The engine owns three assets that no project authors: the default input bindings,
> the weather recording, and the project templates. They resolve through
> `loom_scene::project::engine_assets()`, which is `<exe dir>/assets` when that exists and
> `assets` relative to the working directory otherwise — the second branch being what keeps
> `cargo run`, `cargo test` and both `xtask` gates working unchanged from the repository root.
> Bindings and the weather bed additionally consult the open project's root first, so a project
> may own its copy; templates never do, because they belong to the binary. This is **not** a
> search path for `[[asset]]` entries, which continue to resolve relative to the declaring scene
> file and only that way.
>
> It replaces two constructions that were wrong outside this repository: `load_bindings`' cwd
> relative literal (`run.rs:2242`), under which a shipped game's rebinding silently does nothing,
> and `base.join("../audio/rain.wav")` (`sound.rs:57`, `main.rs:3238`), which addresses an
> engine-owned file as though the scene owned it and works here only because every gated scene
> happens to sit one directory below `assets/`.
>
> **Rejected:** a general search path (project → engine → cwd) for all asset paths — it turns a
> broken reference into a working one at a distance, which is the failure ADR 0024 rejects UUID
> sidecars to avoid. An `[engine_assets]` key in `loom.toml` — a value that would be right exactly
> once and wrong after the first `loom ship`. Embedding `rain.wav` in the binary the way
> `DEFAULT_BINDINGS` is embedded — 3 MB of WAV in every build to avoid one lookup.

### Amended: **ADR 0023 — a project is a directory with a `loom.toml`**

Still unwritten, so these are edits to the decision statement rather than a superseding ADR:

1. **Add:** *"The engine repository is itself a project, marked by a `loom.toml` checked in at its
   root, with `main_scene = "assets/games/proving_ground.loom"`. Its layout — fifty scenes under
   `assets/`, no `scenes/` directory — is the case the 'layout is a convention, not a contract'
   rule exists to permit, and no scene moves. Nothing in `cargo xtask validate` or
   `cargo xtask image` reads the manifest."*
2. **Add a sixth field:** *"`[ship] exclude`, an optional array of root-relative names added to
   `loom ship`'s fixed exclusion list. It defaults to empty and `loom new` does not write it; it
   exists because the engine repository is the one project whose root contains a compiler
   workspace."*
3. **Add to the scene glob:** *"`scenes()` skips `target`, `builds`, `out` and any directory whose
   name begins with a dot — the last because `.claude/worktrees/` holds full checkouts of this
   repository and a hub that lists every scene three times is wrong before it is confusing."*

The rest of ADR 0023 — five fields, `deny_unknown_fields`, one reader in `loom_scene::project`, no
scene or asset list, all editor state under `$XDG_STATE_HOME`, `[engine] version` advisory, newer
projects refused and offered read-only — stands unchanged.

### Amended: **ADR 0024 — an `[[asset]]`'s `path` is resolved; `id` is reserved**

Two sentences added to the decision, no change to what it already says:

1. *"Resolution is relative to the declaring scene file and to nothing else. There is no
   project-relative resolution and no fallback: a path that does not resolve from the scene's own
   directory does not resolve. `project://` is **reserved** as the spelling for project-root
   resolution should it ever be wanted; a leading `/` keeps meaning an absolute filesystem path,
   which is what `base.join` and `prefab.rs:133` already implement."*
2. *"A mesh alias that is also a primitive name resolves to the primitive and its `[[asset]] path`
   is never consulted (`main.rs:1146`). `blockout.loom` depends on this."*

`docs/format/README.md:160-162` is amended by ADR 0024 as already planned; this revision adds that
the example comment at `:127` (*"advisory only — never resolved by path"*) is amended in the same
edit, since it is the line a reader copies.

### Not an ADR

`/builds` in `.gitignore`; the `#Object` fix (a defect against the resolver's own behaviour forty
lines away); `engine_assets()`'s two-branch definition (recorded *in* ADR 0033, not beside it).

---

## 12. Alternatives rejected

**Project-relative resolution as a fallback.** §3. Eight lines, provably inert in this repository
today, and a reference that resolves for a reason the author cannot see is worse than one that
breaks at the moment of the edit — ADR 0024's own argument, applied to itself.

**`project://` built now.** §3. Correct, unambiguous, and premature. Reserved instead, which costs
one sentence and keeps the second person who wants it from inventing a third spelling.

**Moving the fifty scenes into `scenes/` so the repo matches the template layout.** Rewrites
`SCENES` (43 entries), `GOLDEN` (28), every filename under `tests/references/`, `MANIFEST.txt` and
hard-coded test paths (`main.rs:5312` and neighbours) — a large purely cosmetic diff across
precisely the files the golden gate's authority rests on. Doc 02 §1 rejected it; this revision
rejects it harder, because the user decision that motivated this document is *the repo keeps
working*.

**A separate `loom.toml` for the repo kept out of git, generated on demand.** It would mean the
hub's behaviour differs between a fresh clone and a used one, and that CI never sees the file the
developer sees. Checked in, or not at all.

**A `[project] kind = "engine"` discriminator.** One field whose only consumer would be a branch
in `loom ship`, which `[ship] exclude` already answers with data instead of a mode.

**Resurrecting `loom_asset::meta`.** Unchanged from doc 02 §5 and PLAN's cut list. The
repo-as-project decision does not weaken the case against it: the repo's 176 asset paths resolve
today without any identity layer, and this document's whole argument is that touching resolution
is what would break the gates.

**Making `[[asset]] path` strict — a missing file is an error rather than a warning.** Attractive
for strangers, and it would fail `office.loom` on load. `MeshLibrary`'s degrade-visibly rule
(`main.rs:1141-1145`) is deliberate and older than this design. If it is ever revisited, it is its
own ADR with its own scene audit, not a side effect of adding a manifest.

---

## 13. What I could not verify

Read-only investigation, no `cargo` command run — per the phase's instruction. Marked rather than
guessed:

1. **The six `asset_file_missing` warnings in §8 are inferred, not observed.** I verified the code
   path (`main.rs:483` joins without splitting `#`), that `props.loom:112-115` declares
   `mesh_Lantern` with a `#`-suffixed path, and that the file exists — but I did not run
   `loom validate` to see the warning, and `alias_report` only fires for an alias a node actually
   references. I read `props.loom:277` referencing `lantern_albedo`; I did not confirm every one of
   the six scenes has a `MeshRenderer` on the `#`-suffixed alias. The fix is correct regardless;
   the count of six may be smaller.
2. **`Sound::start` has one caller and `loom render` never constructs one** — established by grep
   (`run.rs:1338` is the sole hit) rather than by running the renderer. It is the claim that makes
   §4 safe for the image gate, so it deserves a second pair of eyes.
3. **Whether `xtask`'s `run()` sets `current_dir(root)`.** I read `repo_root()`
   (`xtask/src/main.rs:1332`) and every invocation passing `&root`, and the scene paths in `SCENES`
   are repo-root-relative so cwd must be the root — but I did not read `run`'s body. If it does
   not, `load_bindings`' cwd branch is already resolving somewhere unexpected in the gate.
4. **`engine_assets()`'s `<exe dir>` branch under `cargo run`.** `target/debug/loom` has no
   sibling `assets/`, so the cwd branch must fire — which is the intent — but `cargo test`'s
   working directory is the *crate* directory, not the workspace root, which is why
   `main.rs:5312` says `../../assets/test`. **A test that calls `engine_assets()` will not find
   `assets/` from `crates/loom_cli/`.** The two-branch definition may need a third: exe dir → cwd →
   walk up for a `loom.toml`. I have not designed that third branch and it may be needed on the
   first test.
5. **`loom ship`'s exclusion mechanism.** ADR 0032 is unwritten and I assumed a name-prefix test.
   If it turns out to be a glob matcher, `[ship] exclude` should take globs and my example list
   needs trailing slashes.
6. **`toml_edit`'s ergonomics for `[ship]`** — same caveat doc 02 §12.8 records for `[[prefab]]`.
   The capability is certain; the shape of the call is not.
7. **The scene count `project::scenes()` returns for this repo.** I did not enumerate `**/*.loom`
   from the root; V5 asserts a superset relation rather than a number for exactly that reason.
8. **Whether any user-visible surface other than the hub calls `scenes()`.** If `loom validate`
   ever grows a "validate the whole project" mode it inherits the dot-directory rule, and that
   should be one function, not two.
