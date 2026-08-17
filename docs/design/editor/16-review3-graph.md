# 16 — Review 3: constraints, integration and value in the knowledge-graph pair

*Adversarial review of `14-knowledge-graph-core.md` and `15-knowledge-graph-panel.md` against
`PLAN.md`'s twelve stages and twenty ADRs, `CLAUDE.md`'s hard rules, and the repository as it
stands at `62f9ebe`.*

*Design phase — **no `cargo` command was run**. Every fact below was taken with `grep`, `sed`,
`git ls-files` and `wc` in this worktree and the command is quoted where the claim is load-bearing.
§7 lists what I could not settle without a compiler.*

---

## 0. The verdict, in one page

**The subsystem survives, at roughly a fifth of the proposed size, and the two documents cannot
both be implemented.**

The two documents are not two views of one design. They are two designs that claim the same two
ADR numbers, name two different `Tab` variants, specify two incompatible stores, two incompatible
node models, two CLI grammars, two `check-deps.sh` rules, two positions in `loom_agent::TOOLS`, two
state directories and two opposite determinism postures — and doc 15 rejects doc 14's store *by
name*, on four numbered grounds, without knowing doc 14 exists. **This is round 2's structural
failure repeated exactly**: `PLAN.md` §0 records that four parallel documents each claimed ADR 0033
and cross-referenced each other's numbers as though they were stable. Two documents doing it again,
in the round whose whole job was to not, is the first finding and it outranks every technical one.

Three of the technical findings are worse than a numbering collision because they are wrong about
the repository:

- **Doc 15's central measurement is false.** `ScatterExclude.field` is documented, in the source, as
  *"The node path of an earlier `Scatter` field"* and is used in `forest.loom`. §0 states as measured
  fact that no component field anywhere holds a node path, and §6.1 cuts scene nodes, the hierarchy
  surface and the entire intra-file half of the model on that basis. The reversal trigger it names
  fired before the document was written.
- **Both extractors are blind to the most common asset reference in the engine.** `Scatter.mesh` is
  a bare `String` holding an `[[asset]]` alias. It is not an `AssetRef`, it has no file extension,
  and it is indistinguishable by schema from `Name.value`. Neither document's walker sees it and
  neither document's guard test can. The consequence is `--orphans` reporting a mesh that three
  scenes use as unreferenced, which is the exact failure doc 15 §10 calls the riskiest thing in the
  design.
- **Doc 14's determinism exemption is not the rule this workspace has.** `clippy.toml` is
  workspace-wide, `HashMap` and `Instant::now` are both on it, and there are zero `HashMap` imports
  in the whole tree. §7's "uses both freely, and that is correct" fails green check 1.

And the trap ADR 0003 named is present, wearing doc 14's hat rather than the force-directed one.
Both documents cut the force-directed view correctly and for good reasons — doc 15 §4.1 is the best
page in either — but doc 14 then spends the saved week on a SQLite schema, WAL, `BEGIN IMMEDIATE`,
a `user_version` policy, an mtime/size/hash ladder, an unreferenced-node sweep, an incremental
`DELETE`-by-owner protocol and a `--verify` harness whose entire reason to exist is the incremental
protocol — **all decided before the one measurement that says whether an incremental store is needed
at all**, and that measurement is scheduled in the same document as slice 12.1's first commit. That
is the ordering Stages 8 and 9 exist to forbid, quoted approvingly and then inverted.

**What I would ship** is in §5: one crate, no store, no incremental path, no `--verify`, no watcher,
no `mentions`, no `docs/`, no drawing, no `Tab` variant, no impact modal, **one** ADR, and one slice
appended after Stage 6 rather than a stage of its own. It answers *"what would break if I changed
the desk prefab?"* on every prefab in the repo, it answers the three questions `rg` genuinely
cannot, and it is a few hundred lines. §6 states the case against building even that, as strongly
as I can, and then says why it survives.

---

## 1. What I verified, and the command

| Claim | Source | Verified |
| --- | --- | --- |
| 288 tracked files across `assets docs crates` | both, §0 of the brief | **yes** — `git ls-files assets docs crates \| wc -l` = 288; 92 `.rs`, 52 `.loom`, 45 `.png`, 44 `.md`, 24 `.toml`, 10 `.slang`, 9 `.rhai`, 9 `.obj`, 2 `.wav`, 1 `.gltf` |
| authored content under `assets/` | doc 15 §11's scope | **122** — `git ls-files assets \| grep -cE '\.(loom\|png\|obj\|gltf\|rhai\|wav\|toml)$'`; `assets/` holds 133 tracked files total |
| `Scene::{nodes,assets,prefabs,asset_path,prefab_id,scene_id}` are public | doc 14 §2.3 "free" | **yes** — `scene.rs:157,167,179,185,197,203` |
| `Node.{name,parent,path,transform,components,prefab,extends,overrides}` public | doc 14 §2.3 | **yes** — `scene.rs:29-67` |
| `loom_agent::TOOLS` size today | doc 14 "ninth", doc 15 "eleventh" | **8 entries** (`loom_agent/src/lib.rs:24-40`); doc 15's arithmetic is right for the plan's order |
| `components::registry()` type count | doc 14 §2.1 "26, verified" | **24** — `components.rs:1695-1721` |
| `AssetRef` serializes as `{ "asset": "<alias>" }` | doc 14 §2.3 | **yes** — `components.rs:49-53` |
| `Script.path` doc comment says project-relative | doc 14 §11 | **yes, and so does `GameRules.path`** — `components.rs:1676` and `:1684`; two sites, not one |
| `ScatterExclude.field` holds a node path | doc 15 §0 says no such field exists | **false** — `components.rs:1036`, *"The node path of an earlier `Scatter` field"*, used at `forest.loom:143` |
| `Scatter.mesh` is a bare `String` alias | neither | **yes** — `components.rs:964`, *"an `[[asset]]` alias, or a primitive name like `box`"* |
| clippy bans are workspace-scoped | doc 14 §7 says sim-scoped | **workspace** — `clippy.toml` at the root; `HashMap`, `HashSet`, `rand::thread_rng`, `Instant::now`. Zero `HashMap` imports in `crates/` or `xtask/`; every `Instant::now` carries a site `#[allow]` (`run.rs:392,401,495,923,1163,1428`; `main.rs:778,946`) |
| `check-deps.sh` enforces three rules in two shapes | both §3.3/§8 | **yes** — leaf rules at `:19-31`, the reverse containment rule at `:33-44`, ash at `:47-53` |
| prefab files and instances in the repo | doc 14 §6.2, doc 15 §0 | **2 declaring files** (`prefab_room.loom`, `prefab_night.loom`), **3** `prefab =` instances |
| reference PNGs outside `assets/` | neither | **28** — `git ls-files '*.png' \| grep -v '^assets/'`, all under `tests/references/` |
| tracked root directories the walk would see | doc 14 §5.1 | `assets crates docs scripts tests tools xtask` + `.cargo .claude .github` (dot-excluded) |
| `.gitignore` contents | brief's "must be gitignored" | `/target`, `*.spv`, `assets/test/**/*.actual.png`, `.*.lock`, `.*.tmp`, `render.png`, `.claude/worktrees/` |
| alias resolution site, `#Object` split, primitive precedence | doc 14 §2.3, ADR 0024 | **yes** — `main.rs:1146-1170`; a primitive name wins before `path` is consulted |

---

## 2. Dependency, determinism and cache-purity — attack 1

### 2.1 What holds

Both documents get the crate placement right, and it is right for the same reason. `loom_graph →
loom_scene` violates none of `check-deps.sh`'s three rules: the script forbids in-workspace deps on
`loom_reflect`, forbids anything but `loom_reflect` under `loom_scene`, forbids anything depending
on `loom_agent`, and forbids `ash` outside `loom_render*`. **`loom_agent` is untouched by both
designs** — `graph_query` is a row in `TOOLS`, and `loom-mcp` shells out to the `loom` binary, so
the direction of the edge is still nothing→`loom_agent`. Neither document imports `ash`. Neither
puts the index in `loom_scene`, and doc 15 §8's stated reason for that (it would drag file walking
and a cache into the crate every simulation links) is the correct one.

Both get the storage location right and for a better reason than the brief asked for. The brief said
the database must be gitignored; both put it outside the project entirely, and doc 14 §4's sentence
is the one to keep: *"a file that is never in the project cannot be committed by someone who has not
read `.gitignore`. Gitignoring is a request; putting it in user state is a guarantee."* That also
means `.gitignore` needs no new line, which is checkable and true.

Both are right that no simulation or rendering crate can reach the index, and doc 14 §7.3's
formulation — *"linked but never constructed"* rather than *"absent"* — is the honest one. `xtask
image` drives `loom render`; `xtask validate` drives `loom validate`, `loom render` and `loom run
--edit --frames`; from Stage 5 `--frames` forces scene-only mode, and both documents note that a
scene-only session has no project and therefore no index.

### 2.2 F5 — doc 14's determinism exemption is wrong about this workspace · HIGH

§7's closing paragraph:

> On `HashMap` and the wall clock. `loom_graph` uses both freely — `indexed_at`, `elapsed_ms`, hash
> maps in the extractor — and that is *correct*, because clippy's ban is scoped to simulation code
> and this is not it.

It is not scoped. `clippy.toml` sits at the workspace root and clippy reads exactly one of them per
workspace; `disallowed-types` names `std::collections::HashMap` and `HashSet`, `disallowed-methods`
names `rand::thread_rng` and `std::time::Instant::now`. Green check 1 is `cargo clippy --workspace
-- -D warnings`. There is **not one `use std::collections::HashMap` anywhere in `crates/` or
`xtask/`**, and every existing `Instant::now` in the tree carries a per-site
`#[allow(clippy::disallowed_methods)]`. `elapsed_ms` appears in *every* response `loom graph` emits
and in `Refresh`, so this is not a corner.

The paragraph is also self-defeating: §6.6 then requires that "no output path iterates a `HashMap`"
so that `--verify` prints byte-identical JSON, which is the same discipline arrived at from a weaker
premise. Doc 15 §8's "BTreeMap only, no `HashMap`, no `thread_rng`, no wall clock in any query —
mtimes are read in the cache validator, which is I/O, and never in a result" is exactly right and is
what the crate should say.

**Fix.** `BTreeMap`/`BTreeSet` throughout. One `#[allow(clippy::disallowed_methods)]` on the single
timing call, with the reason written beside it in the shape `run.rs:392` already uses. Delete §7's
paragraph and replace it with doc 15 §8's.

### 2.3 F14 — neither `check-deps.sh` rule enforces what it claims · MEDIUM

Doc 14 §3.3 proposes:

```bash
cargo tree -p loom_cli --no-default-features -e normal \
  | grep -qE 'loom_editor|loom_graph|egui_dock|rusqlite' && { echo "FAIL: ..."; fail=1; }
```

That proves `loom-play`'s build is clean. It proves nothing about whether `loom_physics` grew a
`loom_graph` edge next March, which is the containment the document says it is enforcing. The rule
that enforces "only X and Y may depend on Z" already exists in the file, at `:33-44`, iterating
every workspace crate; doc 15 §8 names that shape and doc 14 does not implement it. And neither adds
the converse — *`loom_graph` may depend on `loom_scene` and nothing else* — which needs a fourth
stanza in the shape of the `loom_scene` one at `:26-31`.

**Fix.** Both stanzas, in the two shapes the file already has, plus doc 14's `--no-default-features`
grep, which catches a third thing (feature unification leaking the editor into the runtime binary,
`PLAN.md` R16) and is worth keeping for that.

### 2.4 F15 — "cannot affect a pixel" is true; "cannot affect authored state" is not · MEDIUM

Doc 14 §7.4 gets this right and it should be the wording both documents use:

> If a future feature wants the index to *cause* an edit — "delete the orphaned textures" as a
> button — that edit is an ordinary `Declare` removal through the ordinary op path with the ordinary
> undo, authored by the human who clicked. **The graph proposes; it does not write.**

Doc 15 §8 states the property unqualified — *"no `SceneOp` is ever derived from the index"* — while
§2.2's impact sheet carries a `[ Delete ]` button whose *contents* came from the index. The ops are
the human's and the version token is exact, so nothing is unsound; but the human's decision was
informed by a cache that can be stale, and a design that says "never" about a path it ships is one
someone will later quote against a change that is actually fine.

The enforceable statement, and it is worth writing in the ADR in these words: **no automatic path
exists from the index to authored state, and the dependency rules make one a green-check-1 failure;
a human-mediated path exists by design and its correctness rests on the version token, not on the
index.**

One thing neither document notices: the dependency rules constrain `loom_graph`, not `loom_editor`.
`loom_editor` may legally read the index and construct ops — that is the impact sheet. The guarantee
therefore has to be behavioural at that seam and structural everywhere else, and saying so is
cheaper than discovering it.

### 2.5 F4 — doc 15 puts index-derived data into `loom validate` · HIGH

§3's Problems row: *"`asset_file_missing` already exists per-scene (`main.rs:483`) and now aggregates
across the project instead of only the open scene."* §10's edited-file list: *"`crates/loom_cli/src/
main.rs` (`Some("graph")`, **and the aggregate `asset_file_missing` path at `:483`**)."*

`main.rs:483` is inside `alias_report`, which is `loom validate`'s output. `loom validate` is
`TOOLS`' first entry — `("scene_query", "loom validate")` — and `PLAN.md` §2.11's V3 asserts that
after the project change the warning **set** difference contains only `asset_file_missing`. Making
that warning depend on a project-wide index means:

- `loom validate <scene>` answers differently inside a project and outside one;
- green check 2's warning set becomes a function of what else is on disk;
- the index reaches a command the gate drives, which is the one thing both documents promise it
  never does, and doc 15's own `green_run_writes_no_index` test would not catch it because
  scene-only mode builds no index — the breach appears only when someone runs `xtask validate` from
  a project root, which after Stage 5 the engine repo is.

**Fix.** The aggregation is a *panel* feature. The Problems panel shows per-scene validate output
and, beside it, the output of `loom graph . --broken`. `main.rs:483` is not touched, `loom
validate`'s contract does not move, and the panel gets the strictly better answer anyway because
`--broken` covers all 52 scenes while `SCENES` covers 43.

### 2.6 F18 — the one confident-from-stale answer, in the document that says it has none · LOW

Doc 14 §6.5: *"`loom graph` refreshes before it answers. There is no mode that skips it, which is
why there is no way to get a confident answer from an old index."* Doc 14 §5.4: on a
`busy_timeout` miss, *"the query answers from the index as it stands and sets `"refreshed": false,
"reason": "another process is indexing"`."*

That is the mode. Labelling it is right and is better than blocking. But the exit code is still 0
and the `impact` array is still present and still looks authoritative, and the consumer is a model
that has been handed a JSON object with an `impact` key and an `index` key. ADR 0043's own rejection
list contains *"answering from a cached index without saying so"* — this says so, in a field beside
the answer, which is a weaker mitigation than the ADR's prose implies.

**Fix.** Keep the label, drop the totality claim, and make `--impact` and `--pack` exit 1 when
`refreshed` is false. The other queries can stay at 0. One line, and the sentence in §6.5 becomes
true rather than nearly true.

---

## 3. The trap ADR 0003 named — attack 2

ADR 0003's words: *"attractive infrastructure — a clean schema, a satisfying recursive CTE, a
force-directed view at the end of it — and it is worth roughly a week that M9 does not get."*

**Both documents cut the force-directed view, and both are right.** Doc 15 §4.1 is the strongest
page in either document and its second reason is the one that generalises: a force-directed layout
is non-deterministic, so you cannot compare a screenshot to a screenshot, cannot gate it, and cannot
describe it to someone else — in a project that built `cargo xtask shimmer` because things that move
when they should not are its recurring failure. Doc 14 §8 defers it with a trigger. Neither is
indulgent about the picture.

**But two of the three items ADR 0003 named are still here.** The clean schema and the satisfying
recursive CTE are doc 14 §2.2 and §6.2, and everything that hangs off them:

| Built | Why it exists | Needed for the exit criterion? |
| --- | --- | --- |
| SQLite + `rusqlite` + `bundled` C build | the store | no |
| `PRAGMA journal_mode = WAL`, `busy_timeout`, `BEGIN IMMEDIATE` | two processes writing one store | no — only because there *is* a store |
| `user_version` policy, "no migrations, ever" | schema evolution of the store | no |
| mtime/size → hash → parse ladder, `DELETE FROM edge WHERE owner=?` | incremental update | no |
| unreferenced-`gnode` sweep | a consequence of `INSERT OR IGNORE` sharing | no |
| `--verify`, `incremental_refresh_equals_a_full_rebuild` | proving the incremental path | **only because the incremental path exists** |
| `mentions`, `derive_doc`, indexing `docs/` | the design doc's node kinds | no |
| Q4 split-identity, Q5 orphaned overrides | completeness | no (Q5 is a second opinion on `loom validate`) |
| `Tab::Graph` reserved in Stage 3, unbuilt until Stage 12 | avoiding a layout invalidation | no |

**Every row is a consequence of one decision — that there is a persistent incremental store — and
that decision is taken before the measurement that would justify it.** Doc 14 §5.5 estimates a cold
rebuild at 150–450 ms, §9 records the reversal trigger (*"if cold start measures under ~50 ms,
delete SQLite, `rusqlite`, §4, §5.2 and half of §6"*), §11 admits the parse term "could be wrong by
an order of magnitude", and §5.5 schedules the measurement in "Stage 12's first commit" — after the
schema, the ladder, the WAL policy and the verify harness are already specified as the design. The
document quotes Stages 8 and 9's discipline (*"gated on a measurement, taken before any UI is
drawn"*) while inverting it: here the measurement is taken after the *architecture* is drawn, which
is the expensive half.

Doc 15 §6.3 takes the same measurement seriously and lands on no database at all, with the ceiling
stated (≈3,000 files / 10 MB / 250 ms cold) and SQLite named as the reversal costing one file. **That
is the correct posture and it should win regardless of which document survives the merge.**

### F12 — the store decision and its whole consequence tree precede the measurement · MEDIUM

**Fix.** Slice one is: the measurement, plus an in-memory build with no persistence at all. Time
`Scene::parse` over all 52 files and print it. If it is under ~50 ms the store question never
arises. If it is 150–450 ms, the next cheapest thing is doc 15's single JSON file with `(mtime,
len)` validation — not a database — and `--verify` still does not exist, because a whole-file
rewrite has nothing to drift from. SQLite arrives only if the cache exceeds what `serde_json` should
be asked to load, and by then the six query functions are the only surface and the swap is one file,
which doc 15 §6.3 already argues.

### What I would cut, and still answer the exit criterion

The exit criterion is *"what would break if I changed the desk prefab?"* on a project with 200+
files. Answering it needs: walk the project, parse each `.loom` unresolved, resolve aliases, build a
reverse map, print it. Everything else in both documents is optional. Ranked by cost removed:

| Cut | From | Cost removed | Trigger to build it |
| --- | --- | --- | --- |
| **SQLite, `rusqlite`, WAL, `user_version`, the incremental ladder, `--verify`** | 14 | the largest single block in either document, plus a C dependency and a `cc` requirement | the measurement says a cold rebuild is over ~250 ms *and* a JSON cache is not enough |
| **`mentions`, `derive_doc`, walking `docs/` and `crates/`** | 14 | 44 `.md` + 92 `.rs` files, 975 KB of prose rescanned on every doc save, the lossiest edge kind in the design, and every design document in the impact answer | someone asks twice which document describes a file — `rg` answers it today |
| **The drawing / `graph_view.rs`** | 15 | a panel body, a layout, an `error`-dashed-edge convention | already cut by doc 15 §4.1 with a trigger; keep it cut |
| **`Tab::Graph` / `Tab::References` and the §2.9 amendment** | both | one enum variant, one layout-invalidation risk, one unverified `egui_dock` assumption, and a dead tab for eight stages in doc 14's version | see F16 below — the Problems panel already is the list that outlives navigation |
| **The impact sheet as a widget** | 15 | a modal, an `ImpactChoice` enum, three call sites, a `Display` impl — for **3** prefab instances in the only project that exists | someone is surprised by a deletion. Until then the impact is a sentence in the existing confirmation |
| **Q4 split identity, Q5 orphaned overrides** | 14 | two queries, one of which duplicates `loom validate` | a half-completed rename actually happens |
| **`--orphans`** | both | one query and the single most dangerous wrong answer in the design (§4.3) | the reverse index is trusted, i.e. after F3 is fixed and the walk's scope is settled |
| **`--why`, `--stats`, `--split`, `--pack`'s prose form, `--no-refresh`** | both | five flags | each on first use |

That leaves `loom graph <path> --used-by | --impact | --broken`, `graph_query`, an inspector "Used
by" section, the prefab banner, and two Problems categories. A few hundred lines and no new
dependency.

### F16 — neither `Tab` proposal is needed, and both rest on the same unverified assumption · MEDIUM

Doc 14 §0 spends a variant in Stage 3 with a real unconfigured body; doc 15 §3.1 amends `PLAN.md`
§2.9's "the enum is fixed once" to bind only against removal and reordering. **Both admit in their
own "could not verify" sections that they did not read `egui_dock`'s `DockState` serialization**,
which is the fact the whole disagreement turns on.

The disagreement is unnecessary. Doc 15 §3's argument for a panel is sound — *"a list you navigate
from has to outlive the navigation"* — and the Problems panel is already exactly that: a tab of the
bottom node, holding file-scoped rows you click to navigate, surviving selection changes, landing in
Stage 4. Doc 15's own item 4 puts two new categories there. Put the reference lists there too, keyed
by the current subject, and the enum is untouched, the layout risk is zero, the `egui_dock` question
never has to be answered, and if someone then misses a dedicated References tab **that is data** —
which is the standard doc 15 §10 sets for its own slice 12c and does not apply to its own tab.

---

## 4. What is wrong about the repository — attack 1 and 4, technically

### 4.1 F2 — doc 15's central measurement is false · CRITICAL

§0, stated as one of *"two measurements [that] shaped everything above, and both were taken rather
than assumed"*:

> Across all 52 `.loom` files there are 364 `parent =` keys and **no component field anywhere holds a
> node path** — no `target`, no `surface`, no `owner`.

`crates/loom_scene/src/components.rs:1032-1037`:

```rust
/// "Not within `radius` metres of anything in `field`."
pub struct ScatterExclude {
    /// The node path of an earlier `Scatter` field.
    pub field: String,
```

It is used: `assets/test/forest.loom:143` carries a `[[node.components.Scatter.exclude]]` block, and
`forest.loom:11` describes it as *"the phase's own example"*. `PLAN.md`'s ADR 0026 lists
`Scatter.excludes` among `SpliceArray`'s callers, so the plan knows the field exists.

The consequences in doc 15 are not cosmetic. §6.1 cuts scene nodes as graph nodes and justifies it
with *"nothing outside a scene can point at a node, so a node can never be the target of a
cross-file query"*; §3's Hierarchy row is **"nothing"**, justified as *"measured: intra-scene
references are `parent` and nothing else"*; §11 rejects a use count in the hierarchy as *"measured to
be always zero"*. All three rest on the false premise. The document even names its own reversal
trigger — *"the day a component gains a `NodePath`-typed field, nodes become nodes"* — and that day
was before the document was written.

There is a second-order finding that makes this worth more than the correction. `forest.loom:20`'s
own comment records: *"checks schemas and asset aliases but does not yet follow `Scatter.exclude`"*.
So there is a **known, written-down, unclosed gap** in reference checking, and it is exactly the kind
an index closes — delete or rename a `Scatter` node and every `ScatterExclude` naming it silently
stops excluding. Doc 15's model structurally cannot represent it; doc 14's `node` kind can, for free,
via the `references_file` walker generalised to node paths.

**Fix.** Keep a `node` node kind (doc 14 §2.1's three-kind model), or at minimum add a
`references_node` edge whose target is `node:<file>#<path>`. Re-run the grep before quoting a
measurement — this one is `grep -nE 'pub [a-z_]+: *String' crates/loom_scene/src/components.rs`,
seven hits, readable in ten seconds.

### 4.2 F3 — both extractors are blind to alias-by-bare-`String`, and no test catches it · CRITICAL

`components.rs:959-964`:

```rust
pub struct Scatter {
    /// What to place: an `[[asset]]` alias, or a primitive name like `box`.
    pub mesh: String,
```

Doc 14 §2.3 has two rules for finding references inside a component value:

1. `references_asset` — *"any object with exactly the key `asset` holding a string"*. `Scatter.mesh`
   is a string, not an object. Missed.
2. `references_file` — *"any string … that ends in a known extension (`.rhai .toml .png .obj .gltf
   .wav .loom`)"*. `mesh = "pine"` has no extension. Missed.

Doc 15 §6.1's `RefKind::References` is generated from `AssetRef`-typed fields found via
`loom_reflect::resolve`'s `$ref` walk plus *"a small explicit rule"* for `Script.path` and
`Sound.clip`. `Scatter.mesh` is neither. Missed.

**And the guard test cannot see it.** Doc 15 §10's load-bearing test is
`every_path_shaped_field_in_the_registry_is_indexed` — *"a walk over `TypeRegistry::describe` failing
when a component gains a file-path or `AssetRef` field the extractor does not know about"*. In the
schema, `Scatter.mesh` is `{"type":"string"}`, byte-identical to `Name.value`, `Hud.text` and
`ScatterExclude.field`. The test passes. Doc 14's `asset_ref_walker_agrees_with_the_schema` walks the
26 (24) schemas asserting the `{asset:…}` positions match — same blindness.

Doc 14 §2.3 additionally *asserts* the opposite of the source: *"`AssetRef` already appears on
`MeshRenderer`, `Material` ×2, `WaterBody`, `AudioSource` and **`Scatter`'s ground material**"* —
true of the ground material, and it is the field beside the one that matters.

The failure mode is the worst one available. `--orphans` is `NOT EXISTS (SELECT 1 FROM edge WHERE
e.dst = 'file:' || f.path)`, so a mesh used only by `Scatter` — `forest.loom`, `croft.loom`,
`lanternhead.loom` all carry `Scatter` — is reported as *"nothing uses this"*, which doc 15 §10 names
as the defect that gets a file deleted, and which `CLAUDE.md` records this project shipping twice
already (`meadow` missing from `GOLDEN`, `grass_blades` passing a flat `Ground`).

**Fix, and it is cheaper than either walker.** The engine already knows how to do this:
`MeshLibrary`'s `wanted` set at `main.rs:1146-1170` resolves a name by checking primitives first and
then the declaring scene's `[[asset]]` table. Do the same — **any string field in any component
whose value matches an `[[asset]]` key declared in that scene is a reference to that asset**. It
needs no per-component knowledge, no schema inspection and no extension list; it covers
`Scatter.mesh` and every future alias field for free; and its failure mode inverts from
silent-missing to a spurious edge if someone names a node after an alias. Add
`scatter_mesh_alias_is_indexed` as a named test with `forest.loom` as the fixture, because a general
registry walk provably cannot cover this case.

### 4.3 F7 — `--orphans` reports ~28 false positives on day one, in the only project that exists · HIGH

ADR 0023's exclusion list is `target`, `builds`, `out`, any dot-directory, `*.mine.loom`. The engine
repo's tracked root directories are `assets crates docs scripts tests tools xtask` plus dot-dirs.
So the walk sees `tests/`, which holds **28 reference PNGs** (`git ls-files '*.png' | grep -v
'^assets/'`). Doc 14's Q2 filters `kind IN ('texture','mesh','sound','script','recipe')`, classifies
every one of them as `texture`, finds no inbound edge, and reports all 28 as orphans — sorted by
size, at the top of the list, in a query whose named consumer (§9's cut row) is a *"delete the
orphaned textures"* button.

Two further leaks in the same place, and they are worth more than the count:

- **The walk is a filesystem walk; the counts in both documents are `git ls-files` counts.**
  `assets/test/**/*.actual.png` is gitignored and *exists on disk after any failed image gate*.
  `render.png` is gitignored and exists after any manual check. Both are indexed, both are orphan
  textures, and neither appears in the 288. The index and git therefore disagree about what the
  project contains while both documents present a git number as the index's size.
- `Cargo.lock`, `rust-toolchain.toml`, `LICENSE-*`, `tools/`, `scripts/` and `xtask/` are all indexed
  as `kind='other'` file rows contributing nothing, which is harmless but means the "288 files"
  figure describes neither the walk nor the answer.

**Fix.** Three options in increasing cost: cut `--orphans` until the reverse index is trusted (my
recommendation — see §5); or scope it to files under a directory some scene resolves into; or add
`tests` to the exclusion list and accept that the rule is now "ADR 0023's list plus whatever the
index needs", which is a second answer to "what is in this project" and is the drift ADR 0023 exists
to prevent. Whichever is chosen, both documents must stop quoting `git ls-files` as the walk's
subject.

### 4.4 F6 — `mentions` puts design documents in the impact answer · HIGH

Doc 14's Q1 includes `'mentions'` in the recursive CTE's edge-kind list. So `loom graph
prefabs/desk.loom --impact` returns `docs/design/editor/PLAN.md` alongside the scenes that instance
it, in the query that *is* ADR 0003's exit criterion. A document that refers to a file is not a thing
that breaks when the file changes.

It is also the most expensive and least precise edge kind in the design, by the document's own
numbers: 458 backticked path tokens across `docs/`, 82 in `PLAN.md` alone; resolution is by unique
basename, and this repository's documents cite `lib.rs`, `main.rs`, `ops.rs`, `renderer.rs`,
`viewer.rs` — of which several are ambiguous across 92 `.rs` files. §11 admits *"I do not know today
whether that number is 5 or 200."* It is the only edge kind the document calls lossy, it is the sole
reason `docs/` must be walked at all, and it means editing `PLAN.md` dirties the graph.

**Fix.** Cut `mentions`, cut `derive_doc`, stop walking `docs/` and `crates/`. Doc 15 §11 already
rejects indexing them for the right reason (*"`rg` covers prose better than any index this project
would build"*) — but see F10 immediately below, because that cut has a cost doc 15 does not pay.

### 4.5 F10 — doc 15's scope decision retracts the condition that justified building this · MEDIUM

ADR 0003's revisit condition is a project with 200+ files, and the brief's answer is 288 tracked
files across `assets docs crates`. Verified. But 92 of those are `.rs` and 44 are `.md`, and doc 15
§11 explicitly rejects indexing both: *"Neither is referenced by a `.loom` file and neither
participates in any question in §1."*

What doc 15's index actually covers is `assets/`: **122 authored files**, of which 45 are textures,
52 are scenes, 9 are meshes, 9 are scripts. That is below ADR 0003's bar, computed the ADR's own way.
Doc 15 §12's closing bullet — *"the condition the ADR set for revisiting … was checked (288 tracked
files, 176 asset paths) rather than assumed"* — is checking a number the design then declines to
index.

This is recoverable and the honest version is stronger, because the count was always the wrong
proxy. **176 asset paths, 161 `[[asset]]` declarations, 193 mesh aliases and 136 texture aliases
across 52 scenes is a real cross-file reference web whether or not 92 Rust files are counted
alongside it.** Say that, drop the 288, and the justification stops depending on a number the design
contradicts. Doc 14 has the inverse problem — it indexes `docs/` and `crates/` and therefore *does*
reach 288, at the cost of F6 and F7.

### 4.6 F11 — the index re-implements the loader's resolver · MEDIUM

Alias→path in this engine is: split `#Object`, try `primitives::build(&name)` **first**, then
`scene_asset_path`, then `base.join(raw)` (`main.rs:1146-1170`). ADR 0024 pins the primitive
precedence and records that `blockout.loom` depends on it. Doc 14 §2.3 re-derives all of this inside
`loom_graph`, and adds a constraint the loader does not have: `references_file` fires only when the
joined path *"stays inside the project root"*. The engine's own `base.join("../audio/rain.wav")`
(`sound.rs:57`, `main.rs:3238` — ADR 0036 exists because of it) leaves the root. So the index would
drop an edge the loader resolves, and its answers would describe a slightly different program than
the one that runs.

That is ADR 0006's divergence class arriving in a new place: two implementations of one resolution
rule, the second one in the crate whose only job is to be correct about the first.

**Fix.** Resolution goes through one function. `Scene::asset_path` already exists in `loom_scene`
(`scene.rs:167`); the `#Object` split and primitive precedence belong beside it, moved once and
called from both `main.rs` and `loom_graph`, which is a strictly better place for them than
`main.rs` anyway. If that move is too large for this stage, the index labels its resolved edges as
approximate in the JSON and the ADR says so — but do not let the index own a second copy silently.

### 4.7 F8 — doc 15 ships two indexes with two freshness mechanisms · HIGH

Doc 14 §5.1 states the objection, correctly, against `notify`:

> A watcher would run only in the editor, meaning **the editor and the CLI would have two different
> freshness mechanisms** — two code paths, one of them tested by nobody, in the layer whose only job
> is to be correct about what is on disk. That is the same objection ADR 0037 uses to refuse the
> panel as a write path.

Doc 15 §5 then builds exactly that: the editor spawns a thread that owns an `Arc<Index>` refreshed by
a 1 Hz `stat` sweep, while §7's `loom graph` is *"a fresh process per agent call [that] gets the
cache"* with its own `(mtime, len)` validation. Two derivations of one set of facts, one of them
exercised only with a window open, differing in latency by up to a second.

**Fix.** One `Index` type, one `refresh()`, called by the CLI once per invocation and by the editor
on the poll it already runs — doc 14's arrangement, and it is independent of the store decision.
Doc 15's background thread is still right for the *cold build*; it is the steady-state divergence
that has to go.

### 4.8 F9 — the impact sheet is sold as a check and is decoration on top of one · HIGH

Doc 15 §7: *"the agent cannot supply the impact set, which is the property that makes the card a
check rather than a restatement."* §2.3: the set is *"recomputed on render and again on Approve, and
if it changed between the two the card redraws … rather than applying."*

Three problems, ascending:

1. The set is recomputed from an index that is up to a second stale (§5's 1 Hz sweep), so the
   "check" has a race the thing it decorates does not.
2. The gate that actually holds is `approving_a_stale_proposal_is_refused`, on the version token,
   which is exact and already specified in `PLAN.md` Stage 6. Adding a second refusal axis driven by
   a cache means **Approve can now fail for a reason with no representation in any file** — the
   human is told "this changed while you were reading it" about a change they cannot see by reading
   the scene, which is the opposite of the posture never-do #15 takes.
3. `loom propose --list` gains *"breaks 11 references in 3 scenes"* per row, and §6.4 says scene-only
   mode has no index — so a CLI output shape depends on whether a `loom.toml` is above the cwd. That
   is a contract wobble on the headless proposal path, which ADR 0038 built specifically so the
   headless path is not second class.

**Fix.** The impact block is advisory, labelled as such, and cannot refuse an Approve. `--list`'s
output does not change shape; if the summary is wanted there it is a separate `--impact` flag on
`loom propose`. And per §3's cut table, the sheet is a sentence in the existing confirmation until
someone is actually surprised by a deletion — it fires on **3** prefab instances in the only project
that exists (F19).

### 4.9 F20 — `derive` is not pure as specified · LOW

Doc 14 §3.1 declares `derive(path, bytes, project) -> Derived` and comments *"Pure: no I/O beyond
the bytes handed in, no clock, no database"*, and §6.6 leans on that purity so `--verify` can
re-derive into `:memory:` and diff. But `references_file` and `references_asset` have to decide
whether the joined path exists, or Q3 (broken references) has nothing to test against. Either
existence is decided later from the `file` table — fine, and then `--verify` must run against the
same table or it reports spurious differences — or `derive` stats the filesystem and the comment is
wrong.

**Fix.** Say which. Existence-from-the-table is the right answer and it is one sentence: an edge is
emitted unconditionally, and "broken" is a left join, which is already how Q3 is written.

### 4.10 F17 and F13 — two small placement conflicts · LOW / MEDIUM

**F17, the state directory.** Doc 14: `$XDG_STATE_HOME/loom/graph/<key>.db`. Doc 15:
`$XDG_CACHE_HOME/loom/graph/<key>.json`, and its `green_run_writes_no_index` test asserts the
*cache* path is absent — which would pass trivially against doc 14's design. `PLAN.md` §2.12 puts
thumbnails in cache and everything else in state. A derived index is by definition recomputable, so
cache is right, and the rule worth writing into §2.12 once is: **state holds what the user meant;
cache holds what we can recompute.**

**F13, `bundled` rusqlite in the Windows probe.** Doc 14 §3.3 argues `loom-play` never links the
index, so *"ADR 0032's Windows cross-compilation never has to link SQLite. That also removes the only
reason to worry about `bundled` under mingw."* True for `loom ship`. False for Stage 0 item 7, which
is `cargo check --target x86_64-pc-windows-gnu` over the workspace at default features, and
`loom_cli`'s default is `["editor"]`. §11 already admits the `cc` requirement is unconfirmed. Doc 15
§6.3's objection 2 is right on this point and doc 14's rebuttal addresses a different command.

### 4.11 F1 — the two documents claim the same two ADR numbers with incompatible content · CRITICAL

Recorded last in this section because it is procedural, and first in the summary because it blocks
everything.

| | doc 14's ADR 0042/0043 | doc 15's ADR 0042/0043 |
| --- | --- | --- |
| store | SQLite, `rusqlite` `bundled`, WAL, `user_version` | **no database**; two `Vec`s, two `BTreeMap`s, one JSON cache — `rusqlite` **rejected by name, four grounds** |
| location | `$XDG_STATE_HOME/loom/graph/<key>.db` | `$XDG_CACHE_HOME/loom/graph/<key>.json` |
| node kinds | `file`, `node`, `type` (3) | files only; scene nodes, component types, `child_of` all cut |
| edge kinds | 11, incl. `mentions`, `contains`, `attaches` | 5 (`Declares`, `References`, `Instantiates`, `Extends`, `Entry`) |
| indexes `docs/`, `crates/` | yes (`mentions`, 288 files) | **no** (§11 rejects both; ~122 files) |
| CLI grammar | `loom graph <subject> --impact` (subject first, `main.rs:234`'s convention) | `loom graph --impact <path>` (flag takes the value) |
| `TOOLS` position | "the ninth" | "the eleventh" (8 today + Stage 6's two — arithmetically right) |
| `Tab` | `Tab::Graph`, added **Stage 3**, `PLAN.md` §2.9's rule obeyed | `Tab::References`, added **Stage 12**, §2.9's rule **amended** |
| stage position | after Stage 5, before Stage 11 | after Stage 6, before Stage 11 |
| `check-deps.sh` | `loom_graph → loom_scene` only; one grep | `loom_graph → loom_scene` **and `loom_reflect`**; the `:33-44` shape |
| determinism | `HashMap` and wall clock "freely" | `BTreeMap` only, no `HashMap` anywhere |
| freshness | one `refresh()`, both consumers | editor thread + 1 Hz sweep; CLI validates its own cache |
| `--verify` | central proof obligation | absent |
| impact sheet | absent | central, three call sites |

Neither document can be built without the other being wrong about a decision it states as decided,
and both write "ADR 0042" into `docs/decisions/` and both edit `PLAN.md` §3's table to "twenty-two".
`PLAN.md` §0 already records that this exact thing happened in round 2 and §3 allocates numbers
"once, before any is written" — the mechanism exists and was not used.

**Fix.** `PLAN.md` arbitrates before either document is implemented, in the shape §2 already uses.
My ruling, for what it is worth, is in §5: doc 15's store, node-model *corrected by F2*, and impact
posture; doc 14's single-`refresh()` freshness model, its ownership-by-file edge model, its
unresolved-scenes rule (§3.2 is the best argument in either document and is correct), its CLI
grammar, and its §7.4 wording on writes. **One ADR, not two** — doc 15 §9's closing paragraph
already argues that the CLI and MCP shapes are clauses of the store decision rather than a separate
approval, and it is right.

---

## 5. What survives, and what it looks like

One crate, `loom_graph`, depending on `loom_scene` and nothing else. No store. No incremental path.
No watcher. Slotted as **a slice after Stage 6**, not a stage — it is smaller than Stage 5's hub.

**Slice A — the index and the CLI.**

- The walk is `loom_scene::project::walk()` generalised from `scenes()` (doc 14 §10's edit: one
  function, two callers), scoped to the project's authored content, with the exclusion set stated in
  filesystem terms rather than `git ls-files` terms (F7).
- Parse each `.loom` with `Scene::parse`, **unresolved** — doc 14 §3.2, adopted whole, including the
  sentence *"a consumer that renders, simulates, picks or measures must go through
  `prefab_load::for_reading`; the index must not, because it is the one consumer whose subject is
  the reference and not the result"* as a module doc comment.
- Edges, five kinds, every one owned by the file whose text asserted it (doc 14's `owner` column
  concept, without the column): `declares`, `instantiates`, `extends`, `references_asset`,
  `references_node`. The fifth is F2's fix. `contains`/`child_of`/`attaches` are cut — the hierarchy
  is the picture of `parent` and `describe_type` answers types.
- **Alias resolution is F3's rule**: a string field whose value matches a declared `[[asset]]` key is
  a reference, resolved through one shared function with the loader (F11).
- In memory. Two `BTreeMap` adjacency maps. **Measure the cold build first** and let the number
  decide whether anything is persisted.
- `loom graph <subject> --used-by | --impact | --broken`, subject first, one JSON line, exit 0/1/2,
  every response carrying the `index` block doc 14 §6.5 specifies, and no mode that answers from a
  build that did not complete.
- `graph_query` in `loom_agent::TOOLS`, named questions, not SQL — doc 15 §7 is right that a SQL
  passthrough is the thing that gets proposed next and is right to reject it in the ADR.

**Slice B — the two surfaces that pay for it.**

- The inspector's **Used by** section (doc 15 §3, row 1). One section, present when non-empty.
- The **prefab banner** — *"3 scenes instance this prefab · Show"* — which is doc 15 §0's own
  nomination for highest value per line and is ADR 0003's exit criterion answered at the moment the
  question is live. One query, one line.
- Two **Problems** categories, in the panel and not in `loom validate` (F4).

**Not built:** the store, `--verify`, `--orphans`, `--why`, `--stats`, `--split`, `--pack`'s prose
form, `mentions`, `derive_doc`, `docs/` and `crates/`, the drawing, any `Tab` variant, the impact
modal, ADR 0043. Each has a trigger in §3's table.

**Tests that ship with it**, keeping the two from doc 15 §10 and doc 14 §10 that carry weight, plus
the one F3 requires:

| Test | What it stops |
| --- | --- |
| `scatter_mesh_alias_is_indexed` | **F3** — the bare-`String` alias hole, which no schema walk can see |
| `scatter_exclude_node_path_is_indexed` | **F2** — the intra-file node reference |
| `an_unparseable_file_reports_error_not_emptiness` | doc 14 §5.3 — the half-written file reading as "references nothing", which is `CLAUDE.md`'s named S4 regression shape |
| `impact_terminates_on_a_prefab_cycle` | the index must be total on files the loader would reject |
| `impact_reports_truncation_at_the_hop_limit` | a silently short answer |
| `queries_are_sorted_and_two_runs_agree` | diffability of `graph_query` output |
| `the_index_is_not_opened_by_render_or_sim` | doc 14 §7.3 |
| `green_run_writes_no_index` | doc 15 §8, pointed at whichever directory survives F17 |

---

## 6. Is it worth building at all — attack 5

### The case against, as strongly as I can make it

**The agent already has ripgrep, and ripgrep answers four of the seven questions in doc 15 §1.**
*"What uses this texture?"* is `rg -l tiles_albedo assets/` over 394 KB of scene text — one tool
call, no crate, no cache, no stage, no ADR. *"What breaks if I change this prefab?"* is `rg -l
'lamp.loom' assets/`. Both return in milliseconds and both are already in the agent's hands today.

**The corpus is smaller than one context window.** All 52 scenes are 394,509 bytes. A 1M-token
context holds the entire authored surface of this project several times over. The design doc's
argument was *retrieval beats a dump* and doc 15 §7 concedes the collapse in its own words: *"at 350
nodes the whole index would fit in a context window. What the agent actually gets from it today is
not compression but **direction**."* That is a much weaker claim than the one ADR 0003 deferred.

**The exit criterion fires on almost nothing.** Two prefab-declaring files, 3 `prefab =` instances,
2 `extends`. *"What would break if I changed the desk prefab?"* has, in this repository, at most
three answers, and `rg` finds them.

**The broken-reference half already exists.** `alias_report` emits `asset_file_missing` per scene
(`main.rs:483`) and `cargo xtask validate` runs it across 43 of the 52. The gap is nine scenes and
project-wide aggregation — a `for` loop, not an index.

**ADR 0003's condition was not really met.** 288 counts 92 Rust files and 44 markdown files that doc
15 then declines to index; the actual subject is 122 files, below the ADR's own bar (F10).

**And the cost is not small in either document.** Doc 14 is a new crate, a C dependency, a database,
a WAL policy, eleven edge kinds, five queries, a verify harness, a panel, a `Tab` variant reserved
eight stages early, and two ADRs — against `rg`. Doc 15 is smaller and still a crate, a cache
format, a background thread, a modal, a `Tab` variant, an amendment to a rule `PLAN.md` fixed
deliberately, and two ADRs.

### Whether it survives

**It survives, and the surviving core is about a fifth of what is proposed.**

Three things `rg` provably cannot do, and they are the whole product:

1. **Resolve an alias.** `Material.albedo_map = { asset = "tiles" }` and `Scatter.mesh = "pine"` are
   references to files whose paths live in a different block of the same file. Going from
   `assets/textures/tiles_albedo.png` back to the nodes that use it is two greps and a human join,
   per texture, every time. This is the one operation the index performs that nothing else in the
   toolchain does, and 136 texture aliases and 193 mesh aliases means it is the *common* case, not
   the exotic one.
2. **Reverse the relation.** `rg` finds forward mentions of a literal. It does not answer "what has
   no inbound edge", and it does not answer "what does this file transitively reach" without the
   agent writing the traversal by hand each time — which is exactly the token spend doc 15 §7 says
   the tool removes.
3. **Cover every scene.** `xtask validate` walks `SCENES` (43); the project has 52 `.loom` files.
   A project-wide broken-reference list is not derivable from any existing command.

Add the one piece of evidence neither document used, which is the best argument in the file:
`forest.loom:20` records, in a checked-in comment, that `loom validate` *"checks schemas and asset
aliases but does not yet follow `Scatter.exclude`"*. There is a known, unclosed, written-down
reference gap in this engine right now, and a reverse index over resolved aliases is where it
closes — for free, once F2's node kind is kept.

So: **build slice A and slice B in §5. Do not build a stage.** And take doc 15 §10's own standard
and apply it upward — *"if 12a and 12b have been in use for a week and nobody has missed 12c, that is
data, and the honest response is to not build it"* — because the same sentence, applied to the
store, the `--verify` harness, the impact modal and the `Tab` variant, deletes all four.

ADR 0003 said the graph might *"stay deferred indefinitely and become the visualization it partly
always was."* Both documents kill the visualization and keep the index. That is the right half to
keep. The remaining discipline is to not rebuild the deferred week under a different name.

---

## 7. What I could not verify

- **No `cargo` command was run** (instructed; another workflow is compiling in parallel worktrees).
  Every dependency, feature-resolution, compile-time and timing claim in both documents — and every
  one implied by §5 — is unchecked by a compiler.
- **The cold-build time.** Both documents' store decisions rest on it and neither measured it. It
  remains the first thing this subsystem should do, and §5's slice A is written so that the answer
  can still change the design.
- **`egui_dock`'s `DockState` serialization** under an added `Tab` variant. Both documents assert
  opposite consequences and both admit they did not read it. §3's F16 makes the question moot rather
  than answering it.
- **`rusqlite`'s current version and whether `bundled` needs a C compiler on a cold CI checkout.**
  Doc 14 §11 flags both. §5 removes the dependency, so this stops mattering unless the measurement
  brings it back.
- **`loom_reflect::resolve`'s ability to identify an `AssetRef`-typed field by `$ref`.** Doc 15 §13
  flags it; F3 makes the alias-set rule the primary mechanism, which does not need it, so it
  degrades from load-bearing to an optimisation.
- **How many of the 458 backticked doc tokens are ambiguous basenames.** Doc 14 §11 says "5 or 200".
  §4.4 cuts `mentions`, so it stops being a question.
- **How many of the 161 `[[asset]]` declarations carry an `id`.** Doc 14 §11 flags it; Q4 is cut in
  §5, so it stops being a question.
- **Whether `Scene::parse` validates every component against its schema on every parse.** Both
  documents' cost estimates hinge on it and neither traced it. It is part of the first measurement.
- **Whether anyone will look at a Used-by section or read the prefab banner.** `PLAN.md` R24
  material, and the reason slice B is two surfaces rather than six.
