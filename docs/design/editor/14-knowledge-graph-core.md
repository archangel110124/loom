# 14 — `loom_graph`: the index itself

*Design for the subsystem ADR 0003 deferred and the user has now accepted. This document is
**additive to `PLAN.md`** — it proposes one new stage (12), two new ADRs (0042, 0043) and one
amendment to a decision `PLAN.md` owns (§2.9's `Tab` enum). It does not re-litigate anything
`PLAN.md` settled.*

*Design phase. **No `cargo` command was run** — another workflow is compiling in parallel
worktrees. Every `file:line` and every count below was read in this worktree at `62f9ebe` with
`grep`/`sed`/`git ls-files`. §11 lists what could not be checked without building, and every
timing in this document is an estimate that says so.*

---

## 0. Where this belongs, and what it depends on

**Proposed Stage 12, sequenced after Stage 5 and before Stage 11.** No renumbering: `PLAN.md` §4
already decouples number from order ("5 can be slotted anywhere after 3; 6 needs 3, 4 and 5; 11 is
last"), so a Stage 12 with a stated position is the plan's own idiom rather than a decimal.

| Slice | Contents | Needs |
| --- | --- | --- |
| **12.1** | `loom_graph`, the schema, extraction, `refresh`, `loom graph <subject> --impact\|--orphans\|--broken\|--verify` | Stage 5 (`loom_scene::project`) |
| **12.2** | the two-hop context pack, `graph_query` in `loom_agent::TOOLS` | 12.1 |
| **12.3** | the `Tab::Graph` panel body | 12.1, Stage 3 |

**One piece cannot wait for Stage 12 and must land in Stage 3: the `Tab::Graph` enum variant.**
`PLAN.md` §2.9 fixes eleven variants in Stage 3 and states that adding one later invalidates every
saved layout. The rule there is aimed at *undesigned* tabs; the graph view is now designed, so the
cheap move is to spend one enum line in Stage 3 and give it the body `Agent` gets — the real
unconfigured state, *"No index for this project yet · Build index"*. Twelve variants, and the
budget argument is unchanged because the twelfth is designed rather than aspirational.

Dependency on Stage 5 is hard: the indexer's file enumerator is `loom_scene::project`'s walk, with
ADR 0023's exclusion list (`target`, `builds`, `out`, any dot-directory, `*.mine.loom`). Writing a
second walker would be a second answer to "what is in this project", which is the drift ADR 0023
exists to prevent.

Dependency on Stage 0 is soft and worth stating because the task brief raises it: **Stage 0's
`prefab_load::for_reading` fix does not gate this work, because the graph deliberately reads scenes
*unresolved*.** §2.3 argues that at length. The two are the same insight pointing opposite ways: a
renderer that skips resolution draws nothing, and an indexer that performs it destroys the edges it
exists to record.

---

## 1. What this is, in one paragraph

**A SQLite file in user state, derived entirely from the project's text, holding three kinds of
node and eleven kinds of edge, refreshed by a stat sweep rather than a watcher, queried through
`loom graph` — one more subcommand with the exit contract every other subcommand has — and wrapped
as the ninth `graph_query` entry in `loom_agent::TOOLS`.** It answers *"what would break if I
changed the desk prefab?"* with a recursive CTE over file-ownership, and it hands the agent a
two-hop neighbourhood of about fifteen file rows instead of a directory listing it would have to
read one file at a time.

It is a cache. Delete it and the next command rebuilds it. It never contains a fact the files do
not, it is never written into the project directory, and nothing that renders a pixel or advances a
tick can call into it.

---

## 2. The schema

### 2.1 Three node kinds, not six — and the reason is the incremental update

The design doc's §2.7 schema has six `kind` values (`scene|prefab|asset|component_type|script|
system`) and a uuid primary key. **Both are changed here, and the second change is the load-bearing
one.**

**Identity is derived from the path, never stored.** A uuid primary key has to be minted somewhere
and remembered, and the only place to remember it is the database — at which point the database
holds a fact the files do not, which is the exact failure the design doc's own property 1 forbids.
Deriving the id means re-indexing a file twice produces byte-identical rows, which is what makes
`DELETE`-then-`INSERT` a safe incremental update and what makes `--verify` (§6.3) a meaningful
diff rather than a comparison of two random-uuid sets.

**And `scene`, `prefab`, `asset` and `script` collapse into one kind: `file`.** A prefab is a
`.loom` file that some other file declares in `[[prefab]]`; that is a property of the *referrer*,
not of the file, and both prefab files in this repo (`assets/test/prefab_room.loom`'s
`prefabs/lamp.loom`, and `prefab_night.loom`'s) are ordinary scenes. Modelling prefab-ness as a node
kind means deciding it at index time from a fact that lives in another file, which is a
cross-file dependency in what must otherwise be a per-file derivation. As a *kind of edge* it is
free. The remaining kinds:

| kind | id grammar | count here |
| --- | --- | --- |
| `file` | `file:<project-relative path>` | ~290 real + the referenced-but-missing |
| `node` | `node:<project-relative path>#<NodePath>` | ~420 |
| `type` | `type:<ComponentTypeName>` | 26 (`components::registry()`, verified) |

Paths are project-relative, `/`-separated, lexically normalised (`..` resolved, no symlink
following), and a node whose path escapes the project root is not created — the reference becomes a
`broken` row instead. That normalisation is the same rule §2.12's path-keying helper needs, and it
should be the same function.

### 2.2 The SQL

```sql
PRAGMA journal_mode = WAL;      -- two processes; §5.4
PRAGMA foreign_keys = ON;
PRAGMA user_version = 1;        -- a mismatch deletes the file and rebuilds. No migrations, ever:
                                -- it is a cache, and a migration path is a promise that it is not.

-- One row per file the walk saw. THE FRESHNESS UNIT AND THE INCREMENTAL UNIT.
CREATE TABLE file (
  path      TEXT PRIMARY KEY,   -- project-relative, normalised
  kind      TEXT NOT NULL,      -- scene|script|mesh|texture|sound|recipe|doc|shader|other
  mtime_ns  INTEGER NOT NULL,
  size      INTEGER NOT NULL,
  hash      TEXT,               -- blake3 of the bytes; NULL for files we never open (§5.2)
  status    TEXT NOT NULL,      -- 'ok' | 'error'
  detail    TEXT                -- the SceneError JSON when status='error'; NULL otherwise
) STRICT;

-- Identities. NOT owned by any one file: a prefab file is named by three scenes.
CREATE TABLE gnode (
  id     TEXT PRIMARY KEY,
  kind   TEXT NOT NULL,         -- file|node|type
  name   TEXT NOT NULL,         -- display: basename, node name, or type name
  detail TEXT                   -- json; e.g. {"components":["Light","Material"]} on a node
) STRICT;

-- Claims. EVERY EDGE IS OWNED BY EXACTLY ONE FILE — the file whose text asserted it.
CREATE TABLE edge (
  owner TEXT NOT NULL REFERENCES file(path) ON DELETE CASCADE,
  src   TEXT NOT NULL REFERENCES gnode(id),
  dst   TEXT NOT NULL REFERENCES gnode(id),
  kind  TEXT NOT NULL,
  meta  TEXT NOT NULL DEFAULT '' -- json: the field path, the declared alias and id, the line
) STRICT;

CREATE INDEX edge_src   ON edge(src, kind);
CREATE INDEX edge_dst   ON edge(dst, kind);
CREATE INDEX edge_owner ON edge(owner);      -- required, or ON DELETE CASCADE scans
CREATE INDEX gnode_kind ON gnode(kind);
```

**`owner` is the column the whole design turns on, and it does not appear in the design doc's
schema.** Three consequences fall out of it and none of them costs a line of Rust:

1. **Incremental update is two statements.** Re-indexing file `F` is `DELETE FROM edge WHERE
   owner = ?F` then the inserts, in one transaction. There is no reference counting and no
   traversal.
2. **The impact query traverses to `owner`, not to `src`** (§6.1). "What would break" is a question
   about *files*, and `src` is often a scene node three levels inside one. Walking to the owner
   gives the file with no join and no second edge kind.
3. **Node rows can be shared.** `gnode` is inserted `ON CONFLICT DO NOTHING`, so three scenes naming
   one prefab file produce one `gnode` and three edges. Rows that end up referenced by nothing are
   swept at the end of every refresh with one statement:
   `DELETE FROM gnode WHERE id NOT IN (SELECT src FROM edge UNION SELECT dst FROM edge);`

`STRICT` needs SQLite ≥ 3.37 (2021) and is worth it: a typo that stores an integer in `kind` fails
at the insert rather than at the query six months later.

### 2.3 Eleven edge kinds, and which ones need real parsing

| kind | src → dst | derived from | cost |
| --- | --- | --- | --- |
| `contains` | file → node | `Scene::nodes()` | **free** |
| `child_of` | node → node | `Node.parent` | **free** |
| `attaches` | node → type | `Node.components` keys | **free** |
| `declares_prefab` | file → file | `Scene::prefabs()`, path joined scene-relative; meta `{key,id}` | **free** |
| `declares_asset` | file → file | `Scene::assets()`, likewise; meta `{key,id}` | **free** |
| `instantiates` | node → file | `Node.prefab` alias → the declaration | **free** |
| `extends` | file → file | root `Node.extends` alias → the declaration | **free** |
| `overrides` | node → node | `Node.overrides` keys; meta the dotted key | **free**, unchecked (§6.5) |
| `references_asset` | node → file | a JSON walk of the component value for `AssetRef` | **~30 lines** |
| `references_file` | node → file | a JSON walk for path-shaped strings | **~15 lines** |
| `mentions` | file(doc) → file | a byte scan of markdown for backticked paths | **~40 lines, lossy** |

"Free" means literally free: `loom_scene::Scene` already exposes `nodes()`, `prefabs()`, `assets()`,
`asset_path()`, `prefab_id()` and `scene_id()` as public functions (verified,
`crates/loom_scene/src/scene.rs:135-213`), and `Node` already carries `parent`, `path`,
`components`, `prefab`, `extends` and `overrides` as public fields (`scene.rs:29-67`). **Eight of
the eleven edge kinds are a `for` loop over data the parser already produced.** That is the whole
reason this subsystem is a week and not a month, and it is why building it in `loom_graph` rather
than re-parsing TOML is not a preference.

**What needs real parsing, and what is deliberately not parsed:**

- **`references_asset`.** A component's asset references are `AssetRef` values, which serialize as
  `{ "asset": "<alias>" }` (verified, `components.rs:47-52`). The walker is structural: descend the
  component's `serde_json::Value`, and any object with exactly the key `asset` holding a string is a
  reference; resolve the alias through the same file's `[[asset]]` declarations. **No per-component
  knowledge, so a component type added tomorrow is indexed with zero graph code** — which matters,
  because `AssetRef` already appears on `MeshRenderer`, `Material` ×2, `WaterBody`, `AudioSource`
  and `Scatter`'s ground material, and the four painting systems in Stages 7–10 will each add more.
  The structural walk is checked against the schema by a test, not trusted: §10.
- **`references_file`.** Some references are plain strings, not `AssetRef`: `Script.path`
  (`components.rs:1675`), the second script field at `:1683`, and `VoxelVolume.ops[].recipe`
  (`:423`), which is inside an untyped op array. Rather than a hardcoded (type, field) list that
  goes stale, the rule is general: **any string anywhere in a component value that ends in a known
  extension (`.rhai .toml .png .obj .gltf .wav .loom`) and, joined scene-relative, stays inside the
  project root, is a file reference.** Meta carries the JSON pointer to the field
  (`VoxelVolume.ops.3.recipe`) so a human can see where it came from. False positives are possible
  and harmless — a spurious edge, never a missing one. Verified this is the right resolution base:
  `play.rs:1090` reads a script as `base.join(script)` and `proving_ground.loom:89` writes
  `path = "../scripts/fps.rhai"`, so scripts are **scene-relative**, exactly like `[[asset]]`.
  *(Finding worth a one-line fix in Stage 0's spirit: `Script.path`'s own doc comment says
  "Project-relative path to a `.rhai` file" and the implementation is scene-relative. The comment is
  wrong, in the same way ADR 0024 found `docs/format/README.md` §3 wrong about `[[asset]].path`.)*
- **`mentions` is honestly lossy and says so.** The repo's docs do not use markdown links for paths:
  `PLAN.md` contains **zero** `](path)` links and **82 distinct backticked path-shaped tokens**;
  across `docs/` there are **458**. So the extractor scans for backticked tokens matching
  `[\w./-]+\.(rs|loom|md|toml|slang|rhai|sh)(:\d+)?` and emits an edge **only when the token
  resolves to exactly one indexed file** — as a project-relative path, or as a unique basename
  (`renderer.rs`). Ambiguous basenames emit nothing and are counted in the refresh report, so the
  loss is a number a human can look at rather than a silence.
- **Not parsed at all, deliberately: `.rhai` bodies and `.rs` source.** The design doc's
  `reads_component` / `writes_component` / `emits` / `listens` edges and its `system` node kind both
  need a real parser (`syn` for Rust, a rhai AST walk), and this engine has no `system` as a data
  object — systems are Rust functions. A script is a **leaf**: it is reachable *from* scenes and it
  points at nothing. No script in the repo uses `import` (checked: `grep -l import
  assets/scripts/*.rhai` is empty), so there is not even an intra-script edge to miss. **Trigger to
  build it:** a script gains an `import`, or someone asks "which scripts write `detonate`" twice.
  The whitelist of host variables lives in `loom_script`, and taking that dependency for one query
  is not paid for yet.

---

## 3. Extraction

### 3.1 The one entry point

```rust
// crates/loom_graph/src/extract.rs
/// Everything one file contributes. Pure: no I/O beyond the bytes handed in,
/// no clock, no database.
pub struct Derived {
    pub kind: FileKind,
    pub status: Status,          // Ok | Error(Vec<SceneError>)
    pub nodes: Vec<GNode>,
    pub edges: Vec<Edge>,        // `owner` filled by the caller
}

pub fn derive(path: &RelPath, bytes: &[u8], project: &ProjectIndexView) -> Derived;
```

`derive` is pure and takes the bytes, which is what makes `--verify` (§6.3) able to re-derive
everything into an in-memory database and diff it, and what makes the unit tests file-free.

Dispatch is by extension: `.loom` → `derive_scene`, `.md` → `derive_doc`, everything else → a `file`
row with `status='ok'` and no edges. **A texture contributes no edges and that is not a gap** — it is
a leaf, and every edge that concerns it is owned by the scene that named it, which is exactly where
a change to that scene needs to invalidate it.

### 3.2 Scenes are read **unresolved**, and that is the sharpest decision in this document

`derive_scene` calls `Scene::parse` and stops. It does **not** call `prefab::resolve` /
`prefab_load::for_reading`.

The task brief flags this, and `PLAN.md` §2.14 is emphatic that a reader skipping resolution is a
live bug class — so the reason has to be better than "cheaper", and it is. **Three reasons, and any
one of them is sufficient:**

1. **Resolution erases the edge the exit criterion needs.** `prefab::resolve` replaces an instance
   node with the prefab's expanded sub-tree. After it, `LampLeft` carries a `Light` component and
   *no `prefab` field*. The `instantiates` edge — the entire answer to *"what would break if I
   changed the desk prefab?"* — exists only in the unresolved text.
2. **Resolution breaks per-file ownership, and with it incremental update.** A resolved
   `prefab_room.loom` contains `lamp.loom`'s nodes. Storing them as `prefab_room.loom`'s rows means
   editing `lamp.loom` must dirty every scene that instances it — the precise inverse of "touch one
   scene, re-index one scene". It also produces edges that are *false as stated*:
   `prefab_room.loom --references_asset--> shade.png` when `prefab_room.loom` never mentions
   `shade.png`, so an impact query on `shade.png` would name the wrong file to go and edit.
3. **Two hops recovers everything resolution would have given.** Scene → prefab → the prefab's
   assets is depth 2, which is the pack depth the design doc specifies. Nothing is lost; it is
   *labelled* instead of flattened, which is strictly more information.

**The rule, stated so it survives being quoted out of context:** *a consumer that renders,
simulates, picks or measures must go through `prefab_load::for_reading`; the index must not, because
it is the one consumer whose subject is the reference and not the result.* `loom_graph`'s
`derive_scene` carries that sentence as a doc comment, in the shape `prefab_load.rs`'s own module
doc already takes.

**What the graph consequently cannot answer, admitted rather than discovered:** whether an
`overrides` key names a node that actually exists inside the prefab. §6.5 shows that this is
recoverable by a join *because the prefab file's own nodes are indexed as its own rows* — but the
join's exact shape depends on how a prefab-internal path composes, which §11 lists as unverified.
`loom validate` reports orphaned overrides today through `for_reading_with_warnings`, and the graph
does not duplicate it.

### 3.3 Dependency rules — where `loom_graph` sits and who may call it

```
loom_reflect ── loom_scene ──┬── loom_asset ── loom_render ── loom_editor ──┐
  (nothing)     (+project)   │                                    │        │
                             └── loom_graph ──────────────────────┘── loom_cli
                                 (+ rusqlite)                          ├─ bin loom       (default)
                                                                       └─ bin loom-play  (--no-default-features)
```

**`loom_graph` depends on `loom_scene` and nothing else in the workspace.** Verified legal against
`scripts/check-deps.sh`: the script enforces exactly three rules — `loom_reflect` has no in-workspace
deps, `loom_scene` may have only `loom_reflect`, nothing may depend on `loom_agent`, and nothing
outside `loom_render*` may import `ash`. `loom_graph → loom_scene` violates none of them, and the
design doc §2.13's "everything else may depend on them" is explicit.

**Who may depend on `loom_graph`: `loom_cli` and `loom_editor`, and nothing else.** This is one
*edit* to a rule ADR 0022 already writes rather than a new rule — `check-deps.sh` gains two names:

```bash
# ADR 0022, extended by ADR 0042: the runtime binary links neither the editor nor the index.
cargo tree -p loom_cli --no-default-features -e normal \
  | grep -qE 'loom_editor|loom_graph|egui_dock|rusqlite' && { echo "FAIL: ..."; fail=1; }
```

`loom_graph` is `optional = true` in `loom_cli`'s manifest and joins the existing feature:
`editor = ["dep:loom_editor", "dep:loom_graph"]`. **`loom-play` has no index and needs none** —
a shipped game answers no questions about its own source tree — so ADR 0032's Windows
cross-compilation never has to link SQLite. That also removes the only reason to worry about
`bundled` under mingw.

**`loom_agent` does not depend on `loom_graph`,** because it depends on nothing; `loom-mcp` shells
out to the `loom` binary (`main.rs:225`, `std::process::Command::new(loom)`). `graph_query` is one
more row in `TOOLS`. CLI first, MCP second, unchanged.

---

## 4. Where the database lives

**`$XDG_STATE_HOME/loom/graph/<key>.db`, keyed by project path through §2.12's one path-keying
helper.** Not `<project>/.loom-cache/`.

ADR 0023 already decided this — *"a project directory acquires no engine-written files"* — and it is
a better answer than the brief's "must be gitignored", because **a file that is never in the project
cannot be committed by someone who has not read `.gitignore`.** Gitignoring is a request; putting it
in user state is a guarantee. The knock-on is that `.gitignore` needs no new line at all, and that a
fresh clone and a CI runner both start cold, which is the correct behaviour for a cache.

`§2.12`'s table gains one row:

| Path | Holds | Keyed by | Rotation |
| --- | --- | --- | --- |
| `graph/<key>.db` | the derived index | project path | deleted on `user_version` mismatch |

**When `$XDG_STATE_HOME` is unwritable, `loom graph` opens `:memory:`, warns once, and answers**,
paying a cold start per invocation. That is §2.12's stated posture ("warns once in the console and
runs on defaults; it never fails to open") applied to a database rather than a layout file, and
`rusqlite::Connection::open_in_memory()` makes it a one-line branch.

**Scene-only mode has no project and therefore no index.** `loom graph` outside a project exits 2
with `{"error":"no_project","hint":"run `loom graph` from a directory under a loom.toml, or `loom
new`"}`, and the `Graph` tab shows the same sentence with a **Create `loom.toml` here** button —
the one-click repair the plan's onboarding clause asks of every stage.

---

## 5. Freshness: a stat sweep, and no watcher

### 5.1 The decision, and the crate not taken

**No `notify`. No file watcher. No background thread. Freshness is `refresh()`, a stat sweep the
caller runs before it answers.**

The task brief asks for a watcher crate pinned or polling justified. Polling, and the justification
is not laziness in three parts:

- **The consumer that needs continuous freshness is already polling.** The editor polls the scene
  file four times a second (`run.rs`, the `poll_file` path) and the plan keeps that. `refresh()` on
  the same tick costs one `read_dir` walk and ~290 `statx` calls — **estimated well under 5 ms**,
  and zero parses when nothing changed.
- **The consumer that actually issues queries is a subprocess.** `graph_query` runs `loom graph`
  through `loom-mcp`, which spawns a process per call. A watcher's entire value is state kept
  between events, and a process that exits after one query keeps none. A watcher would run only in
  the editor, meaning **the editor and the CLI would have two different freshness mechanisms** —
  two code paths, one of them tested by nobody, in the layer whose only job is to be correct about
  what is on disk. That is the same objection ADR 0037 uses to refuse the panel as a write path.
- **`notify` is worse at the two cases that matter.** It fires *during* a write (inotify's
  `IN_MODIFY` arrives before the writer is done), it silently drops events on queue overflow, and it
  behaves differently across backends. A stat sweep has neither failure: it reads the state, not the
  event.

Cost of being wrong: if `refresh()` measures slow enough to be felt in the editor's 4 Hz poll, the
fix is to run it on a 1 Hz tick or on window focus, not to add a dependency.

### 5.2 The mechanics

```rust
pub struct Refresh { pub added: usize, pub changed: usize, pub removed: usize,
                     pub errors: usize, pub ambiguous_mentions: usize, pub elapsed_ms: u64 }

impl Index {
    pub fn open(project_root: &Path) -> Result<Index, GraphError>;
    pub fn refresh(&mut self) -> Result<Refresh, GraphError>;
}
```

1. Walk with `loom_scene::project::walk()` — ADR 0023's exclusions, plus `assets/shaders/generated/`
   (a build artifact; `fields.slang` is generated by `build.rs` and indexing it would make the graph
   report a source file the human must not edit).
2. For each file, `metadata()` → `(mtime_ns, len)`. Equal to the stored pair → **skip, no read**.
3. Differ or absent → read, `blake3` the bytes, and if the hash also matches the stored one,
   **update `mtime_ns` and stop** — a `touch` or a save-with-no-change costs a hash, not a parse.
   The corpus is small enough for this to be free: 394 KB of `.loom` + 28 KB of `.rhai` + 975 KB of
   `.md` (measured with `wc -c`), and blake3 runs at ~1 GB/s.
4. Hash differs → `derive`, then in one transaction:
   `DELETE FROM edge WHERE owner=?; UPSERT file; INSERT OR IGNORE gnode; INSERT edge;`
5. Rows in `file` the walk did not see → `DELETE FROM file WHERE path=?`, cascading the edges.
6. Sweep unreferenced `gnode` rows (§2.2), once, at the end.

`hash` is `NULL` for files never opened — textures, meshes, sounds, `.wav`. They contribute no edges,
so their content cannot change the graph, and hashing 45 PNGs on every refresh would be the one
expensive thing in an otherwise free sweep. Their **existence** still matters (§6.3's broken-reference
query), and existence is what `stat` reports.

### 5.3 Deletes, renames and partial writes

**A rename is a delete plus an add, and no rename detection is wanted.** Ids are path-derived, so
`git mv prefabs/lamp.loom prefabs/lantern.loom` removes the old file row, adds the new one, and
leaves every `declares_prefab` edge pointing at `file:prefabs/lamp.loom` — which now has no `file`
row and shows up in the broken-references query. **That is the correct answer**, because a rename
whose referrers were not updated *is* breakage. Rename detection by content hash would find the
"same" file and hide it.

**A partial write is never indexed as truth, by three mechanisms in ascending order of how much they
cost:**

1. **Nothing the engine writes is ever observable half-written.** `loom_scene::write_atomically`
   (`edit.rs`, public and re-exported from `lib.rs:12`) is write-temp-then-rename, so every CLI
   write, every editor save and every agent transaction lands atomically. The exposure is a human's
   `$EDITOR` or a shell redirect, which is real but narrow.
2. **A file that does not parse contributes no edges, and says why.** `derive` returns
   `Status::Error(errors)`, the file's previous edges are deleted anyway, and `file.status` becomes
   `'error'` with the `SceneError` JSON in `detail`. **The one thing that must never happen is a
   half-written file reading as "parses fine, references nothing"** — which is exactly the shape
   `CLAUDE.md` names as the likeliest S4 regression ("a key it does not understand is a key it
   ignores"). Because a truncated TOML file fails to parse rather than parsing to an empty document,
   the error path is the one that fires. The invariant is a test:
   `an_unparseable_file_reports_error_not_emptiness`.
3. **The next refresh fixes it**, because the writer's final `close` moves mtime again.

### 5.4 Two processes

The editor and the agent subprocess both run `loom graph` and both call `refresh()`. WAL plus
`PRAGMA busy_timeout = 5000` and `BEGIN IMMEDIATE` around the write makes concurrent refreshes safe.
**If the lock is not obtained inside the timeout, the query answers from the index as it stands and
sets `"refreshed": false, "reason": "another process is indexing"` in the response** — labelled, not
silent, which is §6.4's whole discipline.

### 5.5 Cold start on 288 files — estimated, and measured before any UI

`git ls-files assets docs crates | wc -l` = **288** (verified), of which 52 `.loom`, 48 `.md`,
93 `.rs`, 45 `.png`, 9 `.rhai`, 9 `.obj`, 10 `.slang`.

| Term | Estimate | Basis |
| --- | --- | --- |
| walk + 288 `statx` | < 5 ms | ~350 dirents after exclusions |
| parse 52 `.loom` (394 KB) | **100–400 ms** | `toml_edit` DOM + `schemars` validation per component |
| hash 1.4 MB of text | 1–2 ms | blake3 |
| scan 48 `.md` (975 KB) | < 20 ms | one byte pass |
| insert ~1,000 `gnode`, ~2,500 `edge` | < 20 ms | one transaction |
| **total, cold** | **estimated 150–450 ms** | |
| **after one scene edit** | **estimated < 20 ms** | one parse (worst case `croft.loom`, 46 KB) |

**The parse term is the only one that could be wrong by an order of magnitude, and it is measured in
Stage 12's first commit, before any query surface is written** — the discipline Stages 8 and 9
already use ("gated on a measurement, taken before any UI is drawn"). `loom graph . --verify
--timing` prints the breakdown.

**If cold start exceeds ~1 s**, the fallback is named now so it is not invented under pressure:
index `.loom` with plain `toml`/`serde` instead of `Scene::parse`, because the format-preserving DOM
that makes writing safe buys the *reader* nothing. That is a second parser and therefore a
divergence risk, so it is taken only on a measurement, and it would need the agreement test
`plain_parse_derives_the_same_edges_as_Scene_parse` over all 52 scenes.

**And if cold start comes in under ~50 ms, the honest response is to delete the database entirely**
and derive in memory per invocation, which deletes §4, §5.2 and half of §6 with it. That trigger is
recorded in ADR 0042's consequences, not buried here.

---

## 6. `graph_query`

### 6.1 The surface

CLI first, one subcommand, the existing conventions: the subject is the first argument
(`main.rs:234` — *"Every subcommand takes its subject as the first argument"*), output is one JSON
line, exit `0` ok / `1` the thing was invalid / `2` the invocation was wrong.

```
loom graph <subject> [--impact] [--pack] [--hops N] [--orphans] [--broken] [--split] [--verify]
```

`<subject>` is a project-relative path, or `.` for the project. `--orphans`, `--broken`, `--split`
and `--verify` take `.`.

`loom_agent::TOOLS` gains `("graph_query", "loom graph")` — **the ninth always-loaded tool**, against
the design doc's intended ten. The existing test `every_tool_wraps_a_real_subcommand` covers it for
free.

### 6.2 Q1 — the exit criterion

*"What would break if I changed the desk prefab?"* — `loom graph prefabs/desk.loom --impact`.

The design doc's one-hop query is wrong for this project in three ways: it returns `e.src`, which
here is a scene *node* and not a file; it is one hop, so a prefab instanced by a prefab is missed;
and it omits `references_asset`, so the question does not work for a texture.

```sql
WITH RECURSIVE impact(id, depth) AS (
        SELECT :subject, 0
    UNION
        SELECT 'file:' || e.owner, impact.depth + 1
          FROM edge e
          JOIN impact ON e.dst = impact.id
         WHERE impact.depth < :hops
           AND e.kind IN ('instantiates','extends','declares_prefab','declares_asset',
                          'references_asset','references_file','mentions')
)
SELECT substr(i.id, 6)            AS file,
       f.kind                     AS kind,
       min(i.depth)               AS depth,
       (SELECT group_concat(DISTINCT e2.kind) FROM edge e2
         WHERE 'file:' || e2.owner = i.id AND e2.dst IN (SELECT id FROM impact)) AS via
  FROM impact i LEFT JOIN file f ON f.path = substr(i.id, 6)
 WHERE i.depth > 0
 GROUP BY i.id
 ORDER BY depth, file;
```

**The step is `e.dst → 'file:' || e.owner`, and that is the trick.** It walks from a thing to the
*file that claimed it*, so one traversal answers at file granularity with no join, no second edge
kind and no case analysis over `src` being a node or a file.

- **`UNION`, never `UNION ALL`.** A prefab cycle would otherwise not terminate. `prefab::library_for`
  presumably rejects cycles, but the index must be total on files the loader would reject — that is
  half of what it is for.
- **`--hops` defaults to 4, and truncation is never silent.** The chain is scene → prefab → prefab →
  asset, with `extends` able to add one. Measured ceiling in this repo: prefab nesting depth is 1
  (two prefab files, `prefab_room.loom` and `prefab_night.loom`, verified). When the frontier at
  `depth = hops` is non-empty the response carries `"truncated": true, "frontier": N`.

Response:

```json
{"subject":"prefabs/desk.loom","hops":4,"truncated":false,
 "impact":[{"file":"assets/games/office.loom","kind":"scene","depth":1,"via":"declares_prefab,instantiates"},
           {"file":"assets/test/lab.loom","kind":"scene","depth":1,"via":"declares_prefab,instantiates"},
           {"file":"docs/guide/02-scenes.md","kind":"doc","depth":1,"via":"mentions"}],
 "index":{"schema":1,"files":288,"errors":0,"refreshed":true,"elapsed_ms":6}}
```

### 6.3 The other four named queries

**Q2 — orphans** (the design doc's agent-cleanup task):

```sql
SELECT f.path, f.kind, f.size FROM file f
 WHERE f.kind IN ('texture','mesh','sound','script','recipe')
   AND NOT EXISTS (SELECT 1 FROM edge e WHERE e.dst = 'file:' || f.path)
 ORDER BY f.size DESC;
```

**Q3 — broken references**, which is `asset_file_missing` across the whole project in one statement,
and which single-file `loom validate` structurally cannot give:

```sql
SELECT e.owner, e.kind, substr(e.dst, 6) AS target, e.meta
  FROM edge e LEFT JOIN file f ON f.path = substr(e.dst, 6)
 WHERE e.dst LIKE 'file:%' AND f.path IS NULL
 ORDER BY e.owner, target;
```

It will legitimately fire — ADR 0024 keeps a missing asset a *warning* (`office.loom` is in `SCENES`
and would fail to load otherwise), and doc 12 found two `[[asset]]` paths pointing at directories
that have never existed. So Q3's output is a report, and `loom graph . --broken` exits 0 with the
list, not 1. Only an invocation error exits 2.

**Q4 — split identity**, a class of bug no per-file check can see: one prefab or asset uuid declared
against two different paths, which is what a half-completed rename leaves behind.

```sql
SELECT json_extract(meta,'$.id') AS id,
       count(DISTINCT dst)       AS paths,
       group_concat(DISTINCT substr(dst, 6)) AS at
  FROM edge
 WHERE kind IN ('declares_prefab','declares_asset')
   AND json_extract(meta,'$.id') IS NOT NULL
 GROUP BY id HAVING paths > 1;
```

This is the query that justifies keeping the uuid in `meta` rather than making it a node kind: the
identity is *recorded*, the path is the id, and the disagreement between them is a `GROUP BY`.

**Q5 — orphaned overrides across files, without resolution.** An `overrides` key like
`"Shade::Material.albedo"` names a node inside the prefab; the prefab file's nodes are indexed as
*its own* rows, so the check is a join rather than an expansion. Shape unverified (§11) — the
composition rule for a prefab-internal path is the part I did not read — and it is a *second* opinion
on something `loom validate` already reports, so it ships in 12.1 only if the join turns out to be
three lines.

### 6.4 The two-hop context pack

`loom graph assets/games/proving_ground.loom --pack` — the design doc's "answer to full context".

**The pack names files and the edge that reached them. It never inlines a file's contents.** That
one rule is what bounds it: reading a file is the agent's next tool call and it already has one, so
inlining would be `render_preview` returning the whole framebuffer. Downward two hops (`contains` is
excluded — a scene's own 61 nodes are `scene_query`'s answer, not the graph's):

```json
{"subject":"assets/games/proving_ground.loom","hops":2,"truncated":false,
 "files":[{"path":"assets/scripts/fps.rhai","kind":"script","depth":1,
           "via":"references_file","from":"Player.Script.path"},
          {"path":"assets/scripts/enemy.rhai","kind":"script","depth":1,
           "via":"references_file","from":"Enemy1.Script.path, Enemy2…, Enemy3…"},
          {"path":"assets/textures/tiles_albedo.png","kind":"texture","depth":1,
           "via":"declares_asset","from":"tiles_albedo"}],
 "types":["Transform","MeshRenderer","BoxCollider","Material","Light","CharacterController",
          "Script","GameRules","Hud","Camera","Environment"],
 "missing":[],
 "index":{"schema":1,"files":288,"errors":0,"refreshed":true,"elapsed_ms":6}}
```

**Size, from the repo's own numbers.** `proving_ground.loom` is the largest game scene; it declares a
handful of assets and three distinct scripts. Across all 52 scenes there are 161 `[[asset]]`
declarations and 416 nodes, so the mean scene declares ~3 assets. **Estimated 10–25 rows, 1.5–4 KB of
JSON.** The hard stop: **`--pack` refuses above 200 rows** with
`{"error":"pack_too_large","hint":"narrow with --hops 1 or --kind"}` rather than truncating, because
a truncated context pack is a context pack that lies about the neighbourhood.

**JSON only — no prose rendering, no `--text` flag.** A fifteen-object array with four keys each is
something a model reads correctly, and a second representation of one payload is a second thing to
keep in agreement. *Skipped: a rendered text form; add it when a model demonstrably mis-reads the
JSON, which is one flag and twenty `writeln!`s.*

### 6.5 Every answer carries its own freshness

**`loom graph` refreshes before it answers. There is no mode that skips it**, which is why there is
no way to get a confident answer from an old index. `--no-refresh` exists only for `--verify`'s A/B
and prints `"refreshed": false` in the block above.

The `index` block is on **every** response, including errors:

```json
"index":{"schema":1,"files":288,"errors":2,"refreshed":true,"elapsed_ms":6,
         "incomplete":["assets/test/broken.loom"]}
```

`"errors" > 0` means some file's edges are missing because it does not parse, and `"incomplete"`
names them. **A neighbourhood computed while a file in it is unparseable is reported as incomplete,
never as complete-and-empty.**

**A file that has never been indexed** has three distinct behaviours and none of them is an empty
success:

| Case | Response | Exit |
| --- | --- | --- |
| exists, but outside the project or excluded | `{"error":"not_in_project","value":…,"hint":"the index walks the project root, skipping target/, builds/, out/ and dot-directories"}` | 2 |
| does not exist, and nothing references it | `{"error":"no_such_file","value":…}` | 1 |
| does not exist, but N edges point at it | `{"error":"no_such_file","referenced_by":[…]}` | 1 |
| exists, in the project, `status='error'` | the neighbourhood, plus `"subject_status":"error"`, `"subject_errors":[…]`, `"incomplete":[subject]` | 0 |

The third row is the one worth building: *"this file does not exist and three scenes reference it"*
is a better answer than the error alone, and it is the same `SELECT` as Q3 with a `WHERE`.

### 6.6 `--verify` — proving the index matches the disk

```
loom graph . --verify
```

Re-derives **every** file into `:memory:` from scratch, then diffs the two `gnode` and `edge` row
sets:

```json
{"ok":false,
 "only_in_index":[{"table":"edge","row":["assets/test/croft.loom","node:…#Rock","file:…/stone.png","references_asset",""]}],
 "only_on_disk":[],
 "files_differing":["assets/test/croft.loom"],
 "full_rebuild_ms":312,"elapsed_ms":340}
```

Exit 0 when identical, 1 when not. **This is the proof obligation the incremental path owes**, and
it is deliberately the same shape as `incremental_painting_equals_a_full_rasterisation` (`PLAN.md`
§2.5) — this project already has a name for "the cheap path must equal the expensive one, bit for
bit", and reusing it means nobody has to be convinced the check matters.

The shipped test, `incremental_refresh_equals_a_full_rebuild`, drives a temp project through the
sequence that has historically broken every incremental index: add a node, delete a node, add a
component, rename a scene, delete a referenced texture, truncate a scene mid-TOML, restore it,
delete a prefab file, restore it — `refresh()` after each, `--verify` at the end and at three points
in the middle.

Determinism of the *output* matters here even though the graph is not simulation code: every query
carries an explicit `ORDER BY` on a total key and **no output path iterates a `HashMap`**, so two
verify runs on one tree print byte-identical JSON. That is a self-imposed rule for the diff's sake,
not a determinism requirement, and §7 is careful about the difference.

---

## 7. Where it must not reach

The brief asks for proof that the index cannot affect a simulation result or a rendered pixel. Four
statements, in increasing strength, and the last is the one that actually holds.

1. **No simulation or rendering crate can name it.** `loom_render*`, `loom_ecs`, `loom_physics`,
   `loom_script`, `loom_field`, `loom_voxel`, `loom_grass`, `loom_scatter`, `loom_water`,
   `loom_rain`, `loom_particles`, `loom_terrain` and `loom_audio` do not depend on `loom_graph`, and
   §3.3's `check-deps.sh` rule makes adding the edge a green-check-1 failure. A function nothing can
   call cannot change a pixel.
2. **The data flows one way, and it is enforceable by grep.** `loom_graph`'s entire public surface is
   `Index::open`, `Index::refresh`, and query functions returning `serde_json::Value`. **No function
   in `loom_graph` takes `&mut` anything from `loom_scene`, and the crate constructs no `SceneOp`,
   `Transaction` or `Session`.** Its only filesystem write is its own database, outside the project.
   The review rule is one line: `loom_graph` may not import `loom_scene::{ops, edit}`.
3. **The gates never construct one.** `cargo xtask image` drives `loom render`; `cargo xtask
   validate` drives `loom validate`, `loom render` and `loom run --edit --frames`. **The `Index` is
   opened in exactly one place — `loom_cli::graph_cmd::run` — reached only by `Some("graph")`.** So
   the honest statement is *linked but never constructed*, not *absent*, and a test asserts
   `main.rs` mentions `loom_graph` only inside the `graph` arm, in the shape of the existing
   `every_tool_wraps_a_real_subcommand` string test.
4. **The strongest one: nothing derived from the graph is ever an input to anything.** No component
   reads it, no shader is fed from it, no `SceneOp` is generated by it, no scene file is written by
   it. It is a **read-only projection of files that were already the source of truth**, and the
   information direction is files → index, never index → files. **If a future feature wants the
   index to *cause* an edit — "delete the orphaned textures" as a button — that edit is an ordinary
   `Declare` removal through the ordinary op path with the ordinary undo, authored by the human who
   clicked. The graph proposes; it does not write.** Recorded as a cut in §9 with its trigger, so
   nobody rediscovers it as a good idea.

**On `HashMap` and the wall clock.** `loom_graph` uses both freely — `indexed_at`, `elapsed_ms`, hash
maps in the extractor — and that is *correct*, because clippy's ban is scoped to simulation code and
this is not it. The distinction is worth stating because it is exactly the kind of rule that gets
cargo-culted into the wrong crate: the ban exists so a sim hash is reproducible, and the graph is
downstream of everything that hash covers. Its output determinism (§6.6) is a separate, weaker,
self-imposed property for `--verify`'s benefit.

---

## 8. The panel (12.3)

`Tab::Graph`, the twelfth variant, docked as a tab of the **right column beside `Inspector`** — not
the bottom node, where the Agent panel already took the full-width slot for a reason S17 argues at
length (a diff needs width). The graph's content is a *list of file rows*, which is what the right
column is shaped like.

Body, top to bottom: the subject (the current selection's owning file, or the open scene); an
**Impact** list — Q1 at hops 2, each row clickable to open the file; **Referenced by** and
**References**, the immediate in and out neighbourhoods with the edge kind as the reason; and a
**Problems** section carrying Q2/Q3/Q4 for the whole project, which is where an orphaned texture or
a split uuid actually gets seen. Clicking a `node:` row selects it in the hierarchy and frames it,
through the ordinary selection path — the panel issues no ops.

**The force-directed canvas is deferred, with a trigger.** The design doc itself says *"build it
second; it's a view, not infrastructure"*, and at ~1,000 nodes a force-directed layout is a hairball:
the question a person actually asks is "what touches this", which is a list. **Trigger:** someone
asks twice for the shape of the whole project rather than the neighbourhood of one file. It would be
hand-drawn `egui::Painter` geometry under ADR 0030's rules — no new asset class, no layout crate —
and the layout is a hundred lines of Fruchterman–Reingold. It is cheap; it is just not first.

Refresh: the panel calls `Index::refresh()` on the editor's existing file-poll tick and shows the
`index` block's numbers in a status line, so a stale or erroring index is visible rather than
inferred.

---

## 9. What is cut, and what would bring it back

| Cut | Why | Trigger |
| --- | --- | --- |
| **A file watcher (`notify`)** | §5.1. The querying consumer is a subprocess and the editor already polls; a watcher would give the two halves different freshness mechanisms. | `refresh()` measures slow enough to be felt at 4 Hz *and* moving it to 1 Hz is not enough. |
| **`.rhai` and `.rs` body parsing** — the design doc's `reads_component`/`writes_component`/`emits`/`listens` edges and its `system` node kind | Needs a real parser for each, and this engine has no `system` as a data object. No script uses `import` (checked). | A script gains an `import`, or "which scripts write `detonate`" is asked twice. The host-variable whitelist lives in `loom_script`. |
| **A force-directed canvas** | §8. The useful question is a list; 1,000 nodes is a hairball. | Someone asks twice for the whole project's shape. |
| **Any write path from the graph** — "delete the orphaned textures" as a button | §7.4. It would make the index cause an edit, which is the one direction the cache model forbids. | Never as a graph-side write. As a `Declare` removal in an ordinary transaction, any time. |
| **Storing prefabs and assets as their own node kind, keyed by uuid** | §2.1. Recovered by Q4's `GROUP BY` at no cost, and it would put an identity in the index that no file agrees is primary. | ADR 0024's "`id` becomes primary" migration happens. Then the id *is* the identity and this reverses. |
| **Schema migrations** | It is a cache. A `user_version` mismatch deletes the file. A migration path is a promise that the database is not disposable. | Never. Deleting a 400 KB cache costs 300 ms. |
| **The database itself** | Not cut — but see ADR 0042's trigger. | **If cold start measures under ~50 ms, delete SQLite, `rusqlite`, §4, §5.2 and half of §6, and derive into a `Vec<Edge>` per invocation.** This is the single most likely thing in this document to be over-built, and the measurement that settles it is Stage 12's first commit. |
| **Indexing `target/`, `builds/`, dot-directories, `assets/shaders/generated/`** | `.claude/worktrees/` holds whole checkouts of this repository (`PLAN.md` §2.11), so a naive walk would index the project three times. ADR 0023's exclusion list already handles it; the generated Slang is a build artifact the human must not edit. | Never. |

---

## 10. Files touched, and the tests that ship with them

**New:**

- `crates/loom_graph/Cargo.toml` — `loom_scene`, `rusqlite` (`features = ["bundled"]`), `serde`,
  `serde_json`, `blake3`. **The version comes from `cargo add`, not from this document** (§11).
- `crates/loom_graph/src/lib.rs` — `Index`, `open`, `refresh`, `GraphError`.
- `crates/loom_graph/src/schema.rs` — the DDL in §2.2 as one `const`, plus `user_version` handling.
- `crates/loom_graph/src/extract.rs` — `derive`, `derive_scene`, `derive_doc`, the `AssetRef` walk,
  the path-shaped-string walk.
- `crates/loom_graph/src/refresh.rs` — the walk, the stat/hash ladder, the transaction, the sweep.
- `crates/loom_graph/src/query.rs` — Q1–Q5 and the pack, as named parameterised statements.
- `crates/loom_graph/src/verify.rs` — the full re-derive and the row-set diff.
- `crates/loom_cli/src/graph_cmd.rs` — arg parsing and JSON output, in the shape of
  `prefab_cmd::run` (`main.rs:273`).
- `docs/decisions/0042-*.md`, `docs/decisions/0043-*.md`.

**Edited:**

- `Cargo.toml` — `members` += `crates/loom_graph`.
- `crates/loom_cli/Cargo.toml` — `loom_graph` optional; `editor = ["dep:loom_editor",
  "dep:loom_graph"]`.
- `crates/loom_cli/src/main.rs` — `mod graph_cmd` (cfg'd), `Some("graph") => graph_cmd::run(args)`,
  one `USAGE` line.
- `crates/loom_scene/src/project.rs` (Stage 5) — `scenes()` generalised to `walk()`; `scenes()`
  becomes `walk().filter(|p| p.extension() == "loom")`. **One function, two callers, no new
  walker.**
- `crates/loom_agent/src/lib.rs` — `("graph_query", "loom graph")`.
- `scripts/check-deps.sh` — two names added to ADR 0022's containment rule (§3.3).
- `crates/loom_editor/src/dock.rs` — `Tab::Graph` (**Stage 3**).
- `crates/loom_editor/src/panels/graph.rs` (12.3).
- `docs/design/editor/PLAN.md` — §2.9's tab list 11 → 12; §2.12's table gains the `graph/` row; §3's
  ADR table gains 0042/0043 (twenty ADRs → twenty-two); §4 gains Stage 12; §5's
  *"knowledge-graph view"* cut row flips, its trigger having fired.
- `docs/decisions/0003-knowledge-graph-deferred.md` — status `proposed` → **accepted**, option 2 in
  substance, with the measurement that met its own revisit condition recorded: it argued *"the
  project will not have 200 files at M9"*, and `git ls-files assets docs crates | wc -l` is **288**.

**Tests:**

| Test | What it stops |
| --- | --- |
| `incremental_refresh_equals_a_full_rebuild` | the drift every incremental index eventually acquires (§6.6) |
| `an_unparseable_file_reports_error_not_emptiness` | a half-written file reading as "no references" |
| `a_rename_leaves_a_broken_reference_rather_than_healing_it` | rename detection being reinvented |
| `impact_terminates_on_a_prefab_cycle` | `UNION ALL` creeping in |
| `impact_reports_truncation_at_the_hop_limit` | a silently short answer |
| `asset_ref_walker_agrees_with_the_schema` | the structural `{asset:…}` walk drifting from `AssetRef` — walk `components::registry()`'s 26 schemas and assert the positions match |
| `the_index_is_not_opened_by_render_or_sim` | §7.3 |
| `deleting_the_database_loses_nothing` | rebuild, `--verify`, exit 0 |
| `an_excluded_directory_is_never_indexed` | `.claude/worktrees/` tripling the project |

**Green checks: all four, unchanged.** `SCENES` stays at whatever Stage 11 left it, `GOLDEN` likewise
— **the graph adds no rendering path, no component and no scene**, which is the same position Stage 6
is in. It adds one `cargo test` module and one `check-deps.sh` clause.

---

## 11. What I could not verify

- **No `cargo` command was run** (instructed). Every timing in §5.5 and §6.4 is an estimate with its
  basis stated. The parse term is the one that could be wrong by an order of magnitude and it is the
  one gated by a measurement in the stage's first commit.
- **The `rusqlite` version.** `CLAUDE.md` forbids floating a version and requires `cargo add`, so
  the pin is set by `cargo add rusqlite --features bundled` in the commit that lands it and recorded
  in ADR 0042 then. I did not check the crates.io state and I am not guessing a number. **`bundled`
  also introduces a `cc`/C-compiler build requirement for the editor build** (not for `loom-play`),
  which I have not confirmed is acceptable on a cold CI checkout.
- **SQLite's JSON1 functions** (`json_extract`, used in Q4) are compiled in by default in modern
  SQLite and in `rusqlite`'s bundled amalgamation. Near-certain, unverified. Fallback if absent:
  store the declared id in its own column instead of in `meta`.
- **Whether adding a `Tab` variant actually invalidates saved layouts.** `PLAN.md` §2.9 asserts it;
  I did not read `egui_dock`'s `DockState` serialization. If the assertion is wrong — a new variant
  merely fails to appear in an old layout, which `layout.rs`'s ignore-and-warn already tolerates —
  then `Tab::Graph` can land in Stage 12 instead and Stage 3 costs nothing. **I default to landing it
  in Stage 3 because that costs one line and settles the question either way.**
- **Q5's join shape.** I did not read how a prefab-internal node path composes into a resolved node
  path, so the override-target check (`"Shade::Material.albedo"` → which `node:` id) is a sketch. It
  ships only if the join is three lines; `loom validate` reports orphaned overrides today.
- **Whether `Scene::parse` validates every component against its schema on every parse.** It calls
  into `loom_reflect` and the module doc says "Reading is validated", but I did not trace the cost,
  which is exactly the term §5.5's estimate hinges on.
- **Basename uniqueness for `mentions`.** 458 distinct backticked path tokens across `docs/`, many of
  them bare basenames (`renderer.rs`, `viewer.rs`). I did not check how many are ambiguous. The
  extractor emits nothing for an ambiguous one and counts it, so the loss is measurable at runtime —
  but I do not know today whether that number is 5 or 200.
- **How many `[[asset]]` declarations carry an `id`.** `PrefabDecl` has an `id` field and
  `materials.loom` shows uuids on assets, but I did not audit all 161 declarations. Q4 simply skips
  rows where `json_extract(meta,'$.id')` is NULL, so a missing id costs coverage, not correctness.
- **`SceneError`'s exact serialization into `file.detail`.** It derives `Serialize`
  (`scene.rs:83`) so a `Vec<SceneError>` is one `to_string`; I did not check for a non-obvious
  field.

**One finding this design surfaced that belongs in Stage 0's list rather than here:**
`Script.path`'s doc comment (`components.rs:1675`) says *"Project-relative path to a `.rhai` file"*
and the implementation joins it onto the scene's directory (`play.rs:1090`, and
`proving_ground.loom:89` writes `../scripts/fps.rhai`). It is scene-relative. Same class as ADR
0024's amendment to `docs/format/README.md` §3, and a one-line comment fix.

---

## 12. The two ADRs

**ADR 0042 — The knowledge graph is a derived cache of files, read unresolved, stored outside the
project.**

*Decision.* A `loom_graph` crate depending on `loom_scene` alone, consumed by `loom_cli` (under the
existing `editor` feature) and `loom_editor` and by nothing else — one extension to ADR 0022's
containment rule rather than a new one. Three node kinds (`file`, `node`, `type`) with **derived,
path-based ids**, and eleven edge kinds, of which eight are a loop over data `Scene`/`Node` already
expose. **Every edge is owned by exactly one file**, which makes incremental update two statements
and makes the impact query traverse to the file that would break. **Scenes are indexed
*unresolved*** — resolution erases the `instantiates` edge the exit criterion needs, breaks per-file
ownership, and is recovered at two hops anyway. **No file watcher**: freshness is a stat sweep on the
poll the editor already runs, because the querying consumer is a subprocess that keeps no state. The
database lives in `$XDG_STATE_HOME/loom/graph/<key>.db` under §2.12's one path-keying helper, so it
is not gitignored — it is never in the project at all. `PRAGMA user_version` mismatch deletes and
rebuilds; there are no migrations, because a migration path is a promise the cache is not disposable.

*Rejected.* The design doc's uuid primary key (a fact the files do not carry, which makes the DB a
source of truth and `--verify` meaningless). Six node kinds (prefab-ness is a property of the
referrer, and modelling it as a kind makes a per-file derivation depend on another file). Indexing
resolved scenes (§3.2's three reasons). `notify` (§5.1: fires mid-write, drops on overflow, and would
give the editor and the CLI two different freshness mechanisms). A `.loom-cache/` inside the project
(ADR 0023 forbids engine-written files there, and gitignoring is a request where absence is a
guarantee). Hardcoded (component, field) tables for script and recipe paths (goes stale on the next
component; the extension rule is general and its failure mode is a spurious edge). Parsing `.rhai`
and `.rs` bodies (needs two real parsers for edges the exit criterion does not use). **And the one
that is closest: no database at all — a `Vec<Edge>` rebuilt per process. Rejected because `loom
graph` is a subprocess per MCP call and the estimated cold start is 150–450 ms paid per query, and
because two processes refreshing concurrently is a real case (ADR 0037 spawns the agent while the
editor runs) that a whole-file rewrite races and WAL does not. Reversal trigger, recorded so it is
falsifiable: if the measured cold start is under ~50 ms, delete SQLite and everything it drags in.**

**ADR 0043 — `loom graph` refreshes before it answers, and every answer carries its freshness.**

*Decision.* One subcommand with the existing exit contract (0 ok / 1 invalid / 2 bad invocation) and
the existing convention (subject first), wrapped as `graph_query` — the ninth always-loaded MCP tool.
**There is no mode that answers without refreshing**, so there is no way to get a confident answer
from a stale index. Every response, including every error, carries an `index` block with the file
count, the error count, `refreshed`, `elapsed_ms` and an `incomplete` list naming the files whose
edges are missing because they do not parse. A file that has never been indexed gets one of four
distinct, named answers and never an empty success; a file that does not exist but is referenced
reports its referrers. Truncation at the hop limit is reported, never silent; a pack over 200 rows is
refused with the flag that narrows it. `loom graph . --verify` re-derives the whole project into
`:memory:` and diffs the row sets, exiting 1 on any difference — the proof obligation the incremental
path owes, deliberately shaped like
`incremental_painting_equals_a_full_rasterisation`. Under contention the answer is served from the
index as it stands with `"refreshed": false` and the reason, rather than blocking the agent.

*Rejected.* Answering from a cached index without saying so (the failure this whole document is
against: an index that is confidently wrong is worse than no index, because a correct-looking
`impact` list is acted on). A background daemon holding a warm index (a second process whose
lifetime nobody owns, and the agent's behaviour would then depend on whether it is running — ADR
0037's objection). Silent truncation of the pack (a truncated neighbourhood misrepresents the
neighbourhood, which is the one thing the pack is for). Returning an empty result for an unindexed
file (indistinguishable from "nothing references it", which is the answer that gets a texture
deleted). A prose rendering of the pack alongside the JSON (two representations of one payload, and
a model reads fifteen four-key objects fine).
