# Review 2 — the constraints lens on documents 09–12

*Adversarial review of `09-agent-panel.md`, `10-foliage-and-scatter-painting.md`,
`11-visual-identity.md` and `12-project-model-revised.md`, against `CLAUDE.md`,
`00-survey-constraints.md` and `PLAN.md`'s twelve allocated ADRs. Design phase — **no `cargo`
command was run**. Every `file:line` below was read in this worktree at `62f9ebe`; §7 lists the
claims I could not settle without a compiler.*

*This document looks only for what is wrong. The four documents are individually strong and I
have not written down anything good about them, which is the assignment rather than a verdict.*

---

## 0. The three findings that are structural rather than local

**The round-2 documents repeated, exactly, the failure `PLAN.md` §3 was written to prevent.**
§3 opens: *"Four documents claimed 0022 and two claimed 0023. Allocated here, once, before any is
written."* Round 2 was written by four authors who each read that sentence and each then claimed
0033. There are now **nine** distinct new ADRs wearing three numbers.

**Nobody owns any combined number.** Not the ADR count, not `SCENES`/`GOLDEN`, not the `Tab` enum,
not `loom.toml`'s field list, not `PLAN.md` §2.6's union list, not the XDG state directory. Each
document extends each of those correctly in isolation and none of them is correct as a set. `PLAN.md`
existed because that is what round 1 produced too.

**Two documents amend the same Stage-3 decision in incompatible directions** (§2, M1). The `Tab`
enum is the one thing in the plan that cannot be revised later — `PLAN.md` Stage 3: *"adding
variants later invalidates every saved layout"* — and it now has two contradictory specifications
and one missing variant.

---

## 1. Critical

### C1 · all four · ADR 0033 is claimed four times, 0034 three times, 0035 twice

| # | 09 | 10 | 11 | 12 |
| --- | --- | --- | --- | --- |
| **0033** | the scene journal + `adopt_external` | a foliage mask multiplies the placement rule | the editor colours recency, not authorship | engine-owned assets resolve from the executable |
| **0034** | the agent is a subprocess | a species is a node | UI colour is encoded exactly once | — |
| **0035** | the destructive scope is enforced | grass generation is camera-centred | — | — |

Nine ADRs, three numbers. Worse, three of them are cross-referenced by number *inside* the other
documents' prose (doc 10 §12.4 cites "ADR 0035" for streaming; doc 09 §8 cites "ADR 0035" for the
destructive scope; doc 11 §13 cites "ADR 0034" for the encode), so a reader who resolves the
collision by renumbering must also fix every in-text citation or the documents start pointing at
each other's decisions.

**Fix.** Allocate once, here, in `PLAN.md` §3's table, before any is written — the identical
remedy §3 already applied to 0022/0023:

| # | Title | From |
| --- | --- | --- |
| 0033 | Engine-owned assets resolve from the executable | 12 |
| 0034 | A painted foliage mask multiplies the placement rule | 10 |
| 0035 | A species is a node; a hand-placed instance is a node; a removed instance is a point | 10 |
| 0036 | Grass generation is camera-centred and the CPU pre-applies the shader's cull | 10 |
| 0037 | The scene journal; an external write becomes an undo entry | 09 |
| 0038 | The agent is a subprocess and the panel is not the write path | 09 |
| 0039 | The destructive scope is enforced; a gated transaction becomes a proposal | 09 |
| 0040 | UI colour is authored in display space and encoded exactly once | 11 |
| 0041 | The editor colours recency, not authorship | 11 |

Ordering is by dependency, not by document: 0033 must precede 0038 (the panel needs
`engine_assets()`'s sibling helpers), and 0034 must precede 0036.

---

## 2. High

### H1 · doc 09 · `[agent] command` and `[agent] approve` live in the project's checked-in manifest

Doc 09 §4.2 puts this in `loom.toml`:

```toml
[agent]
command = ["claude", "-p", "--output-format", "stream-json", …]
approve  = "destructive"
```

`loom.toml` is, by ADR 0023, *the* project manifest: checked into the project's git repository,
copied by `loom new`, and read by the hub the moment a directory is opened. Two consequences the
document does not name:

1. **An argv vector in a shared file is arbitrary code execution one click from opening a
   project.** The stated audience is *"other people eventually … build as if strangers will use
   it"* — which means projects will be cloned, downloaded and shared, and each one arrives carrying
   the command the editor will spawn with the project root as its working directory. Nothing in the
   design confirms the command with the user or shows it before the first Send.
2. **A project file can switch off ADR 0035's own gate.** `approve = "none"` is a legal value, in
   the same file, shipped with the project. The one protection the destructive-scope ADR buys —
   *"the default configuration of the shipped panel cannot delete a subtree without a human
   clicking a button"* — is defeated by a line the human never reads. §6.3 is honest that an agent
   that unsets `LOOM_AGENT` is out of policy; it does not notice that the *policy itself* is
   project-supplied.

**Fix.** `command` and `preamble` move to `prefs.toml` under `$XDG_STATE_HOME` (PLAN S9 — the file
that already holds user-global editor configuration). `approve` and `approve_above_nodes` may stay
in `loom.toml` **only as a tightening**: the effective policy is `max(user_policy,
project_policy)`, and a project can never loosen. If a project-supplied command is ever wanted, it
is shown verbatim in a one-time confirmation and stored as an approval keyed by project path — but
the cheaper answer is that it is not wanted.

### H2 · doc 09 · ADR 0035 changes the agent CLI's default behaviour, which `PLAN.md` §1 lists as untouchable

`PLAN.md` §1's "kept, untouched" table ends with: *"The `.loom` format, the CLI, `loom-mcp` — the
agent's surface does not change."* ADR 0035 makes `loom scene --tx` stop applying a class of
transaction by default and exit 0 with `{"status":"proposed"}` instead. That is a change to the
CLI's contract, not an addition to it, and every existing script, MCP caller and agent prompt that
treats exit 0 as "applied" is now wrong in a way that looks like success.

I checked doc 09 §10.2's open question, which turns out favourably and does not rescue the finding:
`RemoveNode` appears at `run.rs:1892` (the editor's own Delete, which goes through `Session::apply`
and is not classified), `ops.rs:73`, `:948` and `:1883` (definition, application, one test).
**Nothing in `xtask`, `scripts/` or the tests drives `loom scene --tx` with a destructive op**, so
no green check breaks on the day it lands.

The sharper problem is the classifier itself. **`SpliceArray { remove > 0 }` is not a destructive
op; it is how you edit an array element in place.** `PLAN.md` R6 establishes that a whole-array
`SetField` collapses `[[node.components.VoxelVolume.ops]]` into one inline array, which is why
`SpliceArray` exists — so changing wave 3 of `WaterBody.waves`, retuning one sculpt stamp,
replacing a paint stroke, and doc 10's `Scatter.remove` append-then-correct are all
`remove: 1, insert: 1`. Under ADR 0035 every one of them is destructive and produces a card.

A gate that fires on routine editing is the blind-approve regression the ADR quotes
`LOOM-IMPLEMENTATION-ORDER.md:451-453` to avoid, arriving through the mechanism built to prevent
it.

**Fix.** Classify on *what is lost*, not on the op's name: `RemoveNode`, `RemoveComponent`, and
`SpliceArray` where `remove > insert` (a net deletion). An in-place replacement is not destructive.
And gate on the *CLI* rather than on the library, so `--tx` keeps its contract for scripts: the
proposal path is opt-in via `LOOM_AGENT=1` (which the panel already sets) rather than default-on
for every caller.

### H3 · doc 12 · `[ship] exclude` misses dot-directories, so `loom ship` on the engine repo ships the workspace anyway

§2 identifies that `.claude/worktrees/` holds full checkouts of this repository — *"this document
is being written inside one"* — and adds the dot-directory skip to `project::scenes()`. The same
insight is not applied to `[ship] exclude`, whose list is:

```
["crates", "xtask", "tools", "scripts", "tests",
 "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "clippy.toml", "docs"]
```

ADR 0032's fixed list excludes `.git`, and `[ship] exclude` is *added* to it. Neither excludes
`.claude`. So `loom ship` on the repo-as-project copies
`.claude/worktrees/<name>/crates/`, `.../xtask/`, `.../Cargo.lock` and a second complete `assets/`
tree — the exact contents `[ship] exclude` was invented to keep out, one directory deeper.

**And V8 passes.** §9's only check is *"assert `crates/`, `xtask/` and `Cargo.lock` are absent from
the output tree while `assets/` and `loom.toml` are present."* Read as root-relative names, they
are absent; the copies are at `.claude/worktrees/…/crates/`.

**Fix.** ADR 0032's fixed exclusion list gains "any root entry whose name begins with `.`", which
also subsumes the existing `.git` entry and matches the rule `scenes()` already takes. V8 becomes a
recursive assertion: no path anywhere in the output tree contains a `crates/` or `target/`
component.

### H4 · doc 10 · the budget meter under-reports by the `LUSH` headroom the same document grants to painting

Verified in `crates/loom_grass/src/lib.rs`:

```rust
const LUSH: f32 = 1.6;                                            // :255
(steepness * soil * lush / LUSH).clamp(0.0, 1.0)                  // :302, coverage
let side = ((rules.density * LUSH * TILE * TILE).sqrt().ceil() …  // :322, candidate grid
```

The candidate grid is `density × LUSH` per m² and ordinary ground accepts `1 / LUSH` of it, which
is what makes `area × density` the blade count today. Doc 10 §2.1 inserts `ground.paint` as
`(steepness * soil * lush * paint / LUSH)` and clamps `value` to `LUSH`. So the **Grow** preset at
1.6 on flat, unrocky, flowless ground gives `1 × 1 × 1.0 × 1.6 / 1.6 = 1.0` coverage — **every
candidate accepted, `density × LUSH` blades, 1.6× the authored density.**

Two things follow, and both are in the document:

- §2.1 property 4 says *"the `density` field in the inspector remains the truth about the field's
  maximum."* It is not. The maximum is `1.6 × density`, and it always was for a gully; painting
  makes it reachable across a whole field rather than in a channel.
- §8's budget meter — *"`45,360 / 262,144 blades · 17%`"*, and §7.1's whole table — computes
  `area × density`. A fully Grow-painted field is 60% larger than the number the meter shows.
  The meter exists specifically to make `MAX_BLADES` truncation visible before it happens
  (`warn_if_grass_truncated`, the straight horizontal edge across the landscape). It is
  under-reporting in exactly the case a painter creates deliberately.

**Fix.** The meter's number is `area × density × max(1.0, max_painted_value)`, computed from the
mask's actual maximum authority-weighted value rather than from the rule alone, and §2.1's property
4 is restated as *"painting cannot exceed the density a gully already reaches"* — which is true and
is the property that matters.

### H5 · doc 09 · §7b's undeclared-alias check cannot live in `Scene::parse` — `check-deps.sh` forbids it

§7b proposes applying the prefab check (`scene.rs:388-404`, verified: it names the alias and lists
the declared keys) to mesh and texture aliases, and adds: *"The check must permit
`loom_asset::primitives::NAMES` and the aliases voxel volumes generate."*

Verified: `pub const NAMES: [&str; 5] = ["box", "plane", "sphere", "cylinder", "capsule"]` is at
`crates/loom_asset/src/primitives.rs:10`. `scripts/check-deps.sh:26-31` fails the build if
`loom_scene` depends on any workspace crate but `loom_reflect`, and `crates/loom_scene/Cargo.toml`
lists `blake3`, `loom_reflect`, `schemars`, `serde`, `serde_json`, `toml_edit`. The voxel-generated
aliases are worse — they are constructed in `loom_cli`, which `loom_scene` cannot see at all.

So the check as specified is green check 1 failing on the day it lands, and the obvious workaround
(copy the five primitive names into `loom_scene`) is a second source of truth for a list that
already caused `blockout.loom`'s alias-shadowing subtlety (doc 12 §3).

**Fix.** The check belongs where the alias set is actually known, which is where `MeshLibrary`
derives `wanted` (`main.rs:1146`) — i.e. as a `loom validate` / Problems-panel diagnostic in
`loom_cli`, not a parse error in `loom_scene`. That also makes it a warning by construction, which
§7b's own escape hatch says it may have to be anyway.

### H6 · doc 12 · `find_root` makes every gated windowed run read the new manifest, so the file's own comment is false

Doc 12 checks a comment into the repository root:

> *"Nothing here is read by `cargo xtask validate` or `cargo xtask image`."*

Verified in doc 02, which doc 12 builds on: `pub fn find_root(start: &Path) -> Option<PathBuf>;
// walk up looking for loom.toml` (`02-project-hub.md:162`), and *"`find_root` is what makes `loom
edit some/scene.loom` work from anywhere: walk up from the …"* (`:184`). `xtask validate` drives
`loom run --edit <scene> --frames` for all 43 scenes (`xtask/src/main.rs:1023`, `:1077` — confirmed
by `PLAN.md` §2.8). From Stage 5, every one of those walks up from `assets/test/…` and finds
`/home/k-dorui/loom/loom.toml`.

The consequences are probably benign — the bindings lookup lands on the same file, and PLAN Stage 3
already has `--frames` ignore the saved layout — but *probably benign* is not what the comment
says, and the comment is the thing a future reader will trust. Concretely, all 43 gated runs switch
from scene-only mode to project mode: the Project panel populates, the state key becomes one shared
project hash instead of nothing, and the hub's recents may acquire a row from a gate run.

Nothing in doc 12 or PLAN says `--frames` suppresses project discovery.

**Fix.** Either state the truth in the comment (*"the manifest is discovered by `find_root` during
`loom run --edit`; nothing in it changes what is rendered"*) and add an assertion to V2 that project
mode does not move a reference, or make `--frames` force scene-only mode and say so. The second is
one condition and keeps the gate measuring what it measured.

### H7 · doc 11 · `tok` lives in `loom_editor`, but the `ui.rs` flip lands in `loom_render` and changes the HUD in shipped games

Doc 11 §2 changes `crates/loom_render/src/ui.rs:88` to `srgb_framebuffer: true` (verified present
and `false` today), and puts the compensating `tok()` pre-warp in
`crates/loom_editor/src/theme.rs`. §12's ADR states the blast radius — *"the HUD draws through the
same pipeline and will also change appearance"* — and then dismisses it: *"the change makes it
correct rather than different."*

It does not, and the document's own §2 says why. The flip removes one of two encodes; the residual
between the vertex shader's `pow(c, 2.2)` and the hardware's piecewise sRGB curve is what `tok`
exists to cancel, and it is **35% in the toe** by doc 11's own worked example (authored 22 →
displays 14). The HUD is `loom_cli/src/hud.rs`, it is game content, and by ADR 0022 `loom-play`
does not link `loom_editor` — **so the shipped game's HUD gets the uncorrected half of a two-part
fix, permanently, with no code path that can reach the correction.** Verified that the HUD is
windowed-only and therefore invisible to the image gate: the only call sites are `run.rs:1070`,
`:1071`, `:1184`.

**Fix.** `tok` belongs beside the thing that owns the encode — `loom_render::ui`, exported as a
plain function. That is not an editor palette in `loom_render` (which ADR 0022 rightly forbids and
doc 11 §14 rightly rejects); it is a colour-space correction owned by the module that sets the
specialization constant, and the editor's token table calls it from `loom_editor` as before.

### H8 · all four · the ADR budget is 12 → 21, not 12 → 15

`PLAN.md` §3: *"**Twelve ADRs — that is the approval budget, and the human should see it as one
number.**"* Doc 09 §8 dutifully restates it as *"the ADR budget moves from twelve to fifteen …
stated as one number here rather than discovered three times"* — while three sibling documents,
written in the same round, add six more. Doc 10 adds three, doc 11 two, doc 12 one.

**Nine new ADRs. The number the human is being asked to approve is twenty-one, and every document
in round 2 believes it is asking for a smaller one.** That is C1's collision seen from the approval
side rather than the numbering side, and it is the one figure the plan explicitly asked to be
presented honestly and once.

---

## 3. Medium

### M1 · docs 09 and 11 · the Agent panel is specified two incompatible ways, both amending the Stage-3-fixed `Tab` enum

- Doc 09 §5.1: *"`Tab::Agent`, docked in the right column **as a tab beside `Inspector`** … Tabbing
  beside the Inspector rather than claiming a third column costs zero horizontal budget."*
- Doc 11 §10: *"**The Agent panel is a vertical split of the right column, not a tab beside the
  Inspector**, and that is the only layout that satisfies user decision 5. A tab hides one panel
  when the other is open; the entire point of 'watch its SceneOps land live' is seeing the
  Inspector's values move while the Agent's transaction log fills."*

Doc 11's argument is the stronger one and cites the user decision directly. But the disagreement is
not decorative: it is the default `DockState` written in Stage 3, and `PLAN.md` Stage 3 fixes that
tree at the same moment it fixes the enum. Whoever implements Stage 3 first wins by accident.

Both documents also independently amend the enum to add `Agent` without knowing the other did, so
the amendment will be applied twice or reviewed as a conflict.

**Fix.** `PLAN.md` Stage 3's `Tab` list becomes eleven variants ending in `Agent`, and the default
layout is doc 11 §10's vertical split, stated once in the plan rather than twice in two designs.

### M2 · doc 10 · ADR 0036 (streaming) hand-ports Slang to Rust — the divergence ADR 0006 exists to prevent

§7.6 proposes pre-applying the shader's own cull on the CPU: *"`grassCullDraw(blade) < falloff(distance)`
(`scene.slang:3223`). The Slang uses `loom_hash`, which is `loom_field::noise::hash` from the
generated `fields.slang` — **frozen ABI** … So the CPU can reproduce the test bit for bit."*

Only the *hash* is frozen and agreement-tested. `grassCullDraw` and the distance falloff are
hand-written Slang with no `loom_field::Expr` behind them, no generator, and no CPU counterpart —
so the proposal writes a second implementation of a formula whose first implementation is in a
shader, which is precisely what CLAUDE.md forbids (*"Never hand-write a field in Slang"*; ADR
0006's whole subject) and what the deferral it is resolving named as the reason to defer:
*"hand-porting Voronoi clumping and the position hash into Slang is the CPU/GPU divergence S2 and
ADR 0006 exist to prevent."* The document has moved the port from Rust→Slang to Slang→Rust and
treated the direction as the problem.

The conservative-distance argument is sound but narrower than the document claims: it guarantees
the CPU's survivors are a superset-safe subset **for the current falloff**. It does not survive
someone tuning `GRASS_FAR` or the falloff curve in Slang alone, which is a one-line shader edit
that no gate can see (grass placement is outside the sim hash, and a still frame at the authored
camera may not contain the boundary).

**Fix.** Either express the cull as a `loom_field` expression so the Slang is generated and the S2
agreement test covers it, or add an explicit CPU/GPU agreement test for the cull in the shape of
`fields.slang`'s — and say in the ADR that the constants are a shared pair that must move together.

### M3 · doc 10 · there is no `Tab::Foliage`, and the palette arrives four stages after the enum is fixed

§12.3 lists `crates/loom_editor/src/panels/foliage.rs` — *"new — species palette, brush settings,
budget meter"* — and §12.4 lands the whole feature at a new Stage 7½. `PLAN.md` Stage 3: *"**The
`Tab` enum is fixed once, here** … adding variants later invalidates every saved layout."* That
rule is why `Problems` and `History` ship from day one and why `Environment`, `Terrain`, `Events`
and `Profiler` were cut rather than reserved.

So either the Foliage palette is not a dockable panel (in which case §12.3's `panels/` placement is
wrong and it belongs in `tools/foliage.rs` as tool-scoped UI), or `Tab::Foliage` must be added in
Stage 3 with an empty state — which is exactly the argument doc 11 §12 makes for `Agent` and doc 10
never makes for itself.

**Fix.** Decide in Stage 3. Tool-scoped brush settings in an overlay or in the Inspector is the
cheaper answer and matches the sculpt brush, which doc 05 also did not give a tab.

### M4 · doc 09 · the journal is written from the shared library write path, so every CLI call, every test and both gates write into the user's home

§3.1: *"`append` is called from `apply_to_file` **inside the existing lock** (`edit.rs:94`)."*
`apply_to_file` is `loom_scene`'s single write path — it is what `loom scene --tx`, `loom place
--op`, `loom prefab`, the editor and every unit test that applies a transaction go through. The
journal is read by exactly one consumer: the editor, to put a label on an adopted write.

Three consequences:

- **`cargo test` and both `xtask` gates acquire a side effect on `$XDG_STATE_HOME`.** Green check 3
  becomes a test suite that writes outside its temp directories.
- **The cap is per scene, not global.** §3.1 caps at *"200 entries per scene, truncating
  oldest-first"* — but the number of *files* is unbounded, and it is keyed by blake3 of a canonical
  path. Tests using a fresh `tempdir` per run produce a new journal file per test per run, forever.
  The 4 GB failure §3.1 anticipates arrives as a million small files instead of one big one.
- It puts a filesystem write inside a lock held across the re-read and the atomic write
  (`edit.rs:346`), lengthening the window in which a concurrent CLI writer blocks, for data no
  writer consumes.

**Fix.** The journal is opt-in, written only when the writer says so — `LOOM_JOURNAL=1`, set by the
editor on the CLI subprocesses it cares about and by nothing else — or written by `Session::save`
and the editor's own applies only, with the agent's writes labelled from the CLI's own JSON output
line, which the panel is already reading (§4.2's `{"type":"tool",…}`). Cap the file count as well
as the entries.

### M5 · doc 12 · `engine_assets()`'s second branch is wrong under `cargo test`, and the third branch is undesigned

§13.4 states the problem clearly and then leaves it: *"`cargo test`'s working directory is the
**crate** directory, not the workspace root … **A test that calls `engine_assets()` will not find
`assets/` from `crates/loom_cli/`.** The two-branch definition may need a third … I have not
designed that third branch and it may be needed on the first test."*

That is green check 3, and V6 and V7 both call through the function. An undesigned branch in the
one function three call sites depend on is not a caveat; it is the design's load-bearing gap, and
it is sitting in §13 rather than in §4.

**Fix.** Design it now: exe dir → cwd → **walk up from cwd for a `loom.toml`** (which is
`find_root(cwd)`, a function the same stage already builds, so it costs nothing) → compiled-in. The
walk-up branch is what makes `cargo test` from any crate directory find the repo root, and it
composes with H6 rather than adding a fourth mechanism.

### M6 · doc 11 · the Stage 0 probe is specified so that it cannot see what the flip actually changes

§2's probe: *"fills the window with a strip of the sixteen tokens and a 0/25/50/75/100% grey ramp,
**at full opacity**, over `chrome_clear`. The human screenshots it … If the sampled bytes equal the
table's hexes within ±2, `tok` is right."*

That validates `tok` on opaque fills, which is the easy half and the half the arithmetic already
settles. The half that needs measuring is **blending**. With a `B8G8R8A8_SRGB` attachment the
hardware decodes the stored value, blends in linear, and re-encodes; today it is doing that on
double-encoded values, and after the flip it does it on correct ones. Every semi-transparent
surface in the design changes — `accent_deep @ α90` row fills, `disabled_alpha = 0.45`, the
`α200` viewport casing, `faint_bg_color` at α80, `window_shadow` at α160 — and so does egui's text
antialiasing weight, which is coverage-blended and is the single most visible thing in the window.
Linear-space text blending is famously thinner than gamma-space; whichever way it goes here, the
probe as written cannot report it.

**Fix.** The probe gains a second row: the same swatches at α128 over a mid grey, plus one line of
body text at each of `text`, `text_weak` and `text_strong`, screenshotted before and after the flip
and compared side by side. That is two more `rect_filled`s and a label, and it is the only
observation that can say whether the type scale in §4 still reads at 13.0.

### M7 · doc 10 · the mask and the removal points are world XZ on a node that has a transform

§3: *"`FoliagePaint` goes on the same node as the `Grass` or `Scatter` it modulates, **projected
top-down over that node's own `half_extent` in world XZ**."* §6.2: *"`Scatter.remove: Vec<[f32; 2]>`
— **world XZ points**."*

§9 considers only one desynchronisation — *"The mask never needs remapping under a sculpt because
sculpting changes height, not ground plan"* — and §13 records only the future lateral-terrain-move
case. The case that exists today is simpler: **move the `Grass` node**. `grass_key` already includes
the node's world translation (`main.rs:1633`), so the field regenerates in its new place; the mask
and the removals do not move with it, so a painted path and the copse's missing tree stay where the
node used to be. The same is true of a parent reparent, and doc 09's bulk-edit failure mode (*"two
hundred `SetTransform`s"*) is a plausible way to trigger it across a scene.

**Fix.** Store both in the **node's local XZ**, which is what the projection already uses for
`half_extent`, and transform at bake time — one matrix multiply in `GroundGrid`, and it makes the
stroke list survive every transform edit the way a prefab override does. Doc 03's `SplatPaint`
almost certainly has the same defect and it should be settled once for both.

### M8 · doc 10 · the `BrushParams` correction has to be applied to `PLAN.md`, not recorded in doc 10

§11 is right and I verified both halves: `scripts/check-deps.sh:26-31` permits `loom_scene →
loom_reflect` only, and `crates/loom_scene/Cargo.toml` carries no `loom_asset`. So `BrushParams` in
`loom_asset::paint` (PLAN S4, restated in ADR 0027's decision text) embedded in a component in
`loom_scene` is green check 1 failing.

The finding against doc 10 is procedural and it matters: **`PLAN.md` says of itself "This file
supersedes the build orders, ADR numbers, file lists and conflicting decisions inside `01`–`07`. …
this one is the instruction."** A correction that lives in doc 10 §11 and not in PLAN's S4 row or
ADR 0027's text will be read by whoever implements Stage 6 — who has no reason to open a foliage
document. Doc 10 §12.2 says ADR 0027 "gains a correction", which is the right instinct and stops
one document short.

**Fix.** Edit `PLAN.md` §2.3's S4 row and ADR 0027's decision text directly, in the same pass that
resolves C1.

### M9 · doc 09 · `adopt_external`'s bookkeeping does not survive the undo that is its whole point

The nine-line snippet (§3.2) pushes the old text, clears redo, pushes a label, and sets
`self.version` and `self.disk` to the adopted text's token. The exit criterion (§8) is *"one Ctrl+Z
restores all six; the scene file after the undo is byte-identical to the scene file before the
approve."*

What the snippet does not show, and what the document does not state, is what `undo()` then does to
`version` and `disk`. After the undo the in-memory text is the *pre-agent* text while the file on
disk holds the *agent's*. If `undo()` recomputes `version` from the restored text (which the
existing snapshot model must, or Ctrl+S would carry a token for text it is not writing) then `disk`
still holds the agent's token and the save is correct. If it does not, the session saves with a
token matching neither. Doc 09 asserts *"undoing it later is an ordinary transaction carrying the
current token"* without showing the field that makes that true.

This is the one place in the document where never-do #15 is close by — a save that carries the
wrong token either force-writes over the agent's work or is rejected for the wrong reason — and it
is left to the reader.

**Fix.** State it: `adopt_external` records the adopted token in `disk` and **not** in a way that
`undo()` inherits, and add a test to §8's list — `undo_after_adopt_saves_against_the_disk_token` —
which is the case `adopted_agent_transaction_undoes_in_one_step` does not cover because it never
saves.

### M10 · doc 11 · ADR 0040's rejection of a UNORM swapchain contradicts §7 of the same document

§12's rejected list: *"a `B8G8R8A8_UNORM` swapchain (moves the scene's own tonemap output, **which
the golden references pin**)."*

§7 of the same document establishes the opposite, correctly: *"`cargo xtask image`, `flythrough`
and `shimmer` all drive `loom render`, which is the offscreen `Renderer` and never constructs a
`Ui`."* The offscreen path has its own colour target and never touches the swapchain, so no golden
reference pins the swapchain format. The rejection may still be right — a UNORM swapchain would
oblige the tonemap to encode sRGB itself, on the window path only, which is a real change — but the
stated reason is not the reason, and an ADR's rejected column is read as settled fact.

**Fix.** Restate the rejection as *"it moves the encode into the tonemap for the window path only,
creating a second place the window and the offscreen path can disagree — ADR 0018's defect class."*

---

## 4. Low

### L1 · doc 10 · `reach_of` needs no new term

§6.2 property 4 and ADR 0035's text both say the removal list *"adds one term to `reach_of`."*
Verified at `crates/loom_scatter/src/lib.rs:731-740`: `own(&Rules) = REACH as f32 *
cell_size(r.spacing)`, and the removal's stated reach is `spacing * 0.45`. `REACH` cells of roughly
half a spacing already dominates it for any `REACH ≥ 2`. The dirty region a *stroke* needs is a
bounding-box growth at the call site, not a change to a function whose signature (`&[Layer]`) has
nowhere to put a point list. Deleting the claim is the whole fix.

### L2 · doc 12 · V3 cannot pass as written

§9 V3: *"the JSON warning count drops by **exactly** the six `asset_file_missing` warnings §8
removes."* §13.1: *"The six … are inferred, not observed … the count of six may be smaller."* A
verification step that asserts a number the document says it does not know is a step that fails on
a technicality and gets weakened by whoever runs it. Assert the *set* difference is non-empty and
contains only `asset_file_missing`, which is what §9 already says is the better check
(*"Diff the warning sets rather than counting them"*) two sentences earlier.

### L3 · all four · nobody owns the combined `SCENES`/`GOLDEN` numbers or `PLAN.md` §2.6's union list

Doc 10 §12.5 moves `SCENES` 48 → 50 and `GOLDEN` 32 → 33. Doc 09 §8 states *"`SCENES` stays at 48
and `GOLDEN` at 32, and S12's budget holds"*, which is true of doc 09 alone and false of round 2.
`PLAN.md` S12's table is now three documents stale.

Likewise §2.6, which says of itself *"The constraints survey §4.J asked for one list. Here it is"*:
doc 09 §8 adds four rows, doc 11 adds three prefs (high contrast, reduce motion, zoom already
there) and doc 12 adds `loom.toml`'s `[ship]` table to the "git is its undo" bucket. Three
extensions, no merge.

**Fix.** One editing pass over `PLAN.md` §2.6 and §2.8 when C1 is resolved. Neither is a design
question.

### L4 · doc 12 · the byte-identity argument is stated against numbers that will have moved by the time it lands

§9's V2 is *"`cargo xtask image`: **28 references**, zero moved"* and §0 promises *"the 43 gated
scenes and the 28 golden references cannot move."* Doc 12 §10 places the work in Stage 5, by which
point `PLAN.md` §2.8 has taken `SCENES` to 45+ (`empty`, `first_person`, `viewport_rect`'s scene)
and `GOLDEN` to 30+. The argument is unaffected — it is about the resolver, not the count — but a
verification step that asserts "28" fails on arithmetic and teaches whoever runs it to edit the
step rather than read it.

### L5 · doc 09 · the proposal queue is keyed by project, and a scene-only `loom scene --tx` has no project

§6's path is `$XDG_STATE_HOME/loom/proposals/<blake3 project>/<token>.json`. `loom scene --tx` is
routinely run on a bare scene with no `loom.toml` above it — that is doc 12 §7's "scene-only mode",
and it is every one of the 43 scenes in this repo before Stage 5. What the queue key is then, and
whether the gate applies at all with no `[agent] approve` to read, is undefined.

### L6 · docs 09, 11, 12 and PLAN · four independent blake3-of-a-path keyings

`PLAN.md` S9 keys `layouts/` by blake3 of the project path. Doc 09 keys `journal/` by blake3 of the
canonical **scene** path, `proposals/` and `context/` by blake3 of the **project** path. Doc 09
§10.4 notices: *"I have not checked whether `PLAN.md` S9's `layouts/` keying has the same property,
and if it does, the two should share one helper rather than each hashing a path its own way."* It
should be one helper with one canonicalisation rule, decided once, because a path that hashes two
ways is a state directory that silently splits in half on the first symlinked project.

---

## 5. Claims I checked that hold up

Stated so the review is falsifiable rather than uniformly negative, and so nobody re-checks them.

- **Doc 10's bit-identity argument for `loom_grass` is sound.** `coverage` is
  `(steepness * soil * lush / LUSH).clamp(0.0, 1.0)` (`lib.rs:302`); inserting `* ground.paint` with
  `paint = lerp(1.0, v, 0.0) = 1.0` is exactly `x * 1.0`, which is exact in IEEE-754. `Ground`
  gains a field with a `Default` (`lib.rs:157`), and every existing test constructs through
  `..Ground::default()`.
- **The crater test and `grass_thins_on_a_slope_and_stops_on_rock` survive.** `steepness` is zero
  past the cutoff and `soil` is zero at `rock = 1.0`; a multiplicative factor cannot restore either.
  This is the specific respect in which doc 10's refusal to copy ADR 0028's `lerp` is correct, and
  it is the strongest single argument in the four documents.
- **Doc 10 §2.1's `viability` placement is right, and for the reason it gives.** `habitable` is
  `viability(rules, ground) > 0.0` (`loom_scatter/src/lib.rs:264-266`) — a hard test at zero, not a
  roll — so an erased region (paint 0) stops competing exactly like a cliff, while a thinned region
  (paint 0.5) still competes and is thinned by `kept`. One factor inside `viability` genuinely gets
  both behaviours, and the crate's own comment at `:304-315` is the argument for it.
- **Doc 11's `tok()` arithmetic is correct.** `tok` is `srgb_encode_gamma22(srgb_decode_piecewise(x))`
  and the pipeline is `srgb_encode_piecewise(pow(·, 2.2))`, so the composition is the identity on
  the authored byte. The reasoning about which shader does what matches
  `ui.rs:88` (`srgb_framebuffer: false`, verified) and the `_SRGB` swapchain preference.
- **Doc 11's "no golden reference contains the HUD" is correct.** The only HUD draw sites are
  `run.rs:1070`, `:1071` and `:1184` — the windowed path. `loom render` constructs no `Ui`.
- **Doc 12's asset-resolution claim is correct and is the deliverable.** Scene-relative resolution
  is untouched, and the reasons given for rejecting a project-relative fallback and for reserving
  `project://` rather than building it are both the argument ADR 0024 already makes.
- **Doc 09's `adopt_external` is not never-do #15.** There is one state, taken whole; no merge
  exists. `accept_disk_version` (`edit.rs:366-385`) is the right precedent. The objection in M9 is
  about bookkeeping, not about the model.
- **Nothing in `xtask`, `scripts/` or the tests drives a destructive `loom scene --tx`** — doc 09
  §10.2's open question, settled: the only `RemoveNode` construction outside `ops.rs` is
  `run.rs:1892`, the editor's Delete, which does not go through the CLI.

---

## 6. What the four documents collectively did not answer

Not defects in any one document — gaps that exist only because four were written in parallel.

1. **The Stage list.** Round 2 adds a Stage 5A (doc 09) and a Stage 7½ (doc 10), both to avoid
   renumbering. `PLAN.md` §4 opens *"Ten stages"* and states an ordering constraint (*"6–8 are
   sequential among themselves; 9 is last"*) that now has two decimals threaded through it. One
   editing pass, but nobody has made it.
2. **The combined `loom.toml`.** ADR 0023 specifies *"five manifest fields, `deny_unknown_fields`"*.
   Doc 12 adds `[ship]`, doc 09 adds `[agent]` with four keys. Under `deny_unknown_fields` a project
   carrying either table against a struct lacking it **fails to load** — which doc 09 §8 spots for
   `[agent]` and neither spots for the pair. The struct must carry both `Option`s from the first
   version that ships, or the engine repo's own checked-in manifest will not load in an editor built
   before doc 09 lands.
3. **The combined XDG state directory.** `prefs.toml`, `layouts/`, `thumbs/` (PLAN), `journal/`,
   `proposals/`, `context/` (doc 09). Six things, three keying schemes (L6), one uncapped file count
   (M4), and no document that lists them together or says what `loom` does when
   `$XDG_STATE_HOME` is unwritable.
4. **Whether the flat surface ramp and the double-encode fix are judged before or after the theme
   lands.** Doc 11 §13 puts `ui.rs:88` in Stage 0 and `apply` in Stage 3, so Stages 0–2 — including
   Stage 1's inspector, the largest new surface in the rework, and Stage 2's *"drag the window edge
   and watch the seam"* — are judged on default egui through a newly-corrected encode. That is
   probably an improvement (today's double encode lifts egui's own colours above what egui intends),
   but no document says so and the human will be looking at a UI that changed for a reason nobody
   wrote down.

---

## 7. What I could not verify

Read-only investigation; **no `cargo` command was run**, per the phase's instruction.

1. **Nothing here has been compiled.** Every dependency, feature-resolution and check-deps claim —
   including H5's and M8's, which are the two I am most confident of — is `cargo tree`'s to settle,
   and neither was run.
2. **H7's severity depends on how much of the HUD is drawn in the toe.** I read `hud.rs`'s call
   sites and its use of `egui::Color32`, not the actual colour values it paints, and a HUD drawn
   entirely in near-white is barely affected by the residual.
3. **M6's blending claim is reasoning from the sRGB attachment rules, not a measurement.** The
   direction of the text-weight change (thinner) is the usual one and I did not verify it on this
   driver. The finding stands either way — the probe cannot see it — but the magnitude is unknown.
4. **H4's 1.6× ceiling assumes the candidate grid is the only limit.** I read `lib.rs:255`, `:302`
   and `:322` and the arithmetic follows, but I did not read `tile()`'s acceptance loop end to end,
   so whether a per-tile clamp caps it lower is unchecked.
5. **H6's consequence list is inferred from doc 02's `find_root` signature**, which is a design
   document rather than code — `loom_scene::project` does not exist yet. If Stage 5 gives
   `loom run --edit` an explicit `--no-project` or scene-only default, the finding reduces to the
   false comment alone.
6. **H3 assumes `loom ship` copies by walking the tree.** ADR 0032 is unwritten and doc 12 §13.5
   records the same uncertainty about whether the exclusion mechanism is a name-prefix test or a
   glob. If it is a glob over root entries only, the `.claude` hole is the same; if it already skips
   dotfiles, H3 evaporates and is worth one line in ADR 0032 saying so.
7. **I did not audit docs 01–07 against 09–12.** `PLAN.md` supersedes them, and the review is scoped
   to the four new documents against the plan; a contradiction that exists only between doc 05 and
   doc 10 is out of scope by construction.
