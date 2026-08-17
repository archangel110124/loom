# 15 — The knowledge graph in the editor

*ADR 0003 has been accepted, so the index is being built and M12's "knowledge-graph view" comes back
into scope. This document designs the **editor surface** over that index: what a person asks it,
where the answer appears, what it costs, and what the agent does with it. It adds one stage and two
ADRs to `PLAN.md` and changes nothing already decided there.*

*Design phase — no `cargo` command was run. Every count below came from `git ls-files`, `grep` and
`wc` in this worktree at `62f9ebe`; §13 lists what a compiler would have to settle.*

---

## 0. The ruling, in one page

**The graph's primary user interface is a list, not a drawing, and its most valuable single line of
UI is a banner that appears when you open a prefab for editing.** ADR 0003 named the trap by name —
*"attractive infrastructure … a satisfying recursive CTE, a force-directed view at the end of it"* —
and the trap is real: a force-directed picture of 288 files is a hairball that answers no question a
person actually has. Every question in the brief is *"what else touches this one thing"*, and the
shape of that answer is eleven rows, each one a path, each one clickable.

So the design is, in descending order of value per line:

1. **A "Used by" section in the inspector**, present whenever the selection has anything referencing
   it. Zero actions: it is there because you selected the thing.
2. **A banner across the top of the scene view when the open file is a prefab**: *"3 scenes instance
   this prefab · Show"*. This is ADR 0003's own exit criterion — *"what would break if I changed the
   desk prefab?"* — answered at the moment the question is live, which is when you open the prefab,
   not when you delete it. One line of UI, one query, and it is the single highest-value thing here.
3. **The impact sheet**, a modal over a destructive edit that lists what breaks *before* it happens,
   and the same widget embedded in the agent's proposal card (ADR 0038).
4. **Two new categories in the Problems panel** — *referenced file is missing* (which exists today,
   per-scene) and *nothing references this file* (which cannot exist without a project-wide index).
5. **A `References` panel**, because a result list you navigate *from* must survive the navigation
   it causes, and an inspector section cannot.
6. **The drawing**, last, kept, scoped hard, and **deterministic**: a focus node with its dependents
   in a left column and its dependencies in a right one, two hops, no physics simulation.

**Two measurements shaped everything above, and both were taken rather than assumed.**

The reference web is **entirely cross-file**. Across all 52 `.loom` files there are 364 `parent =`
keys and **no component field anywhere holds a node path** — no `target`, no `surface`, no `owner`.
So "what breaks if I delete this node" is answered completely by the hierarchy's own children, and
the index adds nothing to it. What the index answers is what crosses a file boundary: 161
`[[asset]]` declarations, 176 `path =` values, 193 `mesh =` aliases, 136 texture aliases, 3 prefab
instances, 2 `extends`.

And the whole index is **about 350 nodes and about 700 edges**. That is smaller than one scene file.
It is the number that decides §6.

---

## 1. The questions, in the user's words, and the one action that answers each

| They ask | They do | They see |
| --- | --- | --- |
| *"What uses this texture?"* | click the `.png` in the Project panel | the inspector's **Used by** section: `props.loom · Rock/Boulder · Material.albedo_map` × 11, grouped by file |
| *"What breaks if I change this prefab?"* | open `prefabs/lamp.loom` | a banner: *"3 scenes instance this prefab · Show"* → the References panel |
| *"What breaks if I delete this?"* | press Delete, or drop a `RemoveNode` proposal in the agent panel | the **impact sheet** before anything is written (§2) |
| *"Is anything using this at all?"* | Project panel → the **Unused** filter, or Problems | a list of files nothing references, with a Reveal action and no delete button (§2.4) |
| *"Where is this material used?"* | — | **nothing, and the honest answer is that the question has no referent here**: a `Material` is an inline per-node component, not a shared asset. The nearest real question is *"which nodes use this texture"*, which is row 1. If a shared-material asset type is ever added it becomes a file node for free. |
| *"Why is this asset in the build?"* | `loom graph --why <path>` | **also honest: everything in the project root is in the build.** ADR 0032 ships the whole tree minus a fixed exclusion list, and reachability-pruned shipping is on `PLAN.md`'s cut list. `--why` answers the useful half — *what makes this file reachable from `main_scene`* — as a path chain, which is what you want before deleting it, not before building. |
| *"What did the agent touch?"* | History panel | **the journal answers this, not the graph** (ADR 0034). The graph's contribution is the second hop: select an agent row → *Show impact* → the References panel shows what references the files that transaction touched. |

**Rows 5 and 6 are in this table because two of them were in the brief and neither maps cleanly.**
Answering them with something plausible would be worse than saying so: a "why is this in the build"
panel over a whole-tree copy is a feature that lies.

---

## 2. Impact analysis before a destructive edit

### 2.1 The first thing to get right: the editor does not delete files

`PLAN.md` §2.6's undo table already says that creating a prefab or copying an imported mesh writes a
second file and that undo does not remove it. **Deletion is the same asymmetry pointed the other
way, and it is worse: a deleted file has no version token, no transaction, and no undo at all.**
Every protection this project has — the version token, `Session::apply`, the one write path, the
divergence banner — protects *scene text*, and none of it reaches `unlink`.

So the Project panel's context menu has **Find references**, **Reveal in file manager** and **Copy
path**, and it has no Delete. Deleting a texture stays a shell operation, and the editor's
contribution is to tell you what it would cost *before* you go and do it. This is a smaller scope
than Unity's and it is chosen deliberately: adding file deletion means adding a trash mechanism, a
restore path and a second undo domain, which is never-do #16 arriving from outside the scene.

**What the editor does delete is nodes**, which is `RemoveNode` and is one Ctrl+Z. That is where a
modal belongs.

### 2.2 The impact sheet

One widget, `loom_editor::panels::references::impact_sheet(ui, &Impact) -> ImpactChoice`, used in
three places with no variants: the human's Delete, the agent's proposal card, and `loom graph
--impact`'s text output (same struct, `Display` instead of egui).

```
┌ Delete “LampLeft”, “LampCentre”, “LampRight” ───────────────────────────────┐
│  ⚠ These are instances of prefabs/lamp.loom.                                │
│    Deleting them leaves the prefab file with no instances in this scene.    │
│                                                                             │
│  3 nodes · 1 prefab reference · nothing outside this file is affected       │
│                                                                             │
│  ▸ prefab_room.loom                                                         │
│      LampLeft      instantiates  lamp                                       │
│      LampCentre    instantiates  lamp                                       │
│      LampRight     instantiates  lamp     (2 overrides will be lost)        │
│                                                                             │
│  [ Delete ]  [ Cancel ]                              Show in References ↗   │
└─────────────────────────────────────────────────────────────────────────────┘
```

Three properties, and the third is the one that keeps it from becoming theatre:

**It states a verdict, not a count.** The headline is *"nothing outside this file is affected"* or
*"11 references in 3 other scenes will break"*. A raw number with no verdict makes every deletion
feel equally dangerous, which trains the user to click through, which is the blind-approve
regression `LOOM-IMPLEMENTATION-ORDER.md:451-453` locked a decision to avoid.

**It never appears when the answer is "nothing".** A modal that says *"0 references — proceed?"* on
every delete is a modal people learn to dismiss without reading. Delete on a node nothing references
is silent and immediate, exactly as it is today. The sheet appears only when the subject is a prefab
instance, carries a `Declare`d alias no other node uses, or the transaction is bulk under ADR 0038's
`approve_above_nodes` axis. **The sheet's rarity is what makes it readable.**

**It offers the non-destructive alternative when one exists.** For a prefab whose file is about to
lose its last instance, that is **Unpack instances first** — `loom prefab unpack`, an operation S4
already shipped and ADR 0008 already documented. For an `[[asset]]` declaration losing its last
user, it is *leave the declaration* (an unused `[[asset]]` block costs nothing and validates clean).
There is no "delete and fix the 11 references for you" button: rewriting eleven `AssetRef` fields to
something else is a change of appearance disguised as a cleanup.

### 2.3 Where it hooks into ADR 0038

ADR 0038 classifies on net loss — `RemoveNode`, `RemoveComponent`, and `SpliceArray` where
`remove > insert` — and under `LOOM_AGENT=1` a classified transaction becomes a proposal card rather
than a write. **The impact sheet is that card's body, computed at the moment the card is rendered,
not at the moment the proposal was written.** That ordering is load-bearing: a proposal may sit in
the queue while the human edits, and an impact set computed at proposal time would describe a
project that has moved. It is recomputed on render and again on Approve, and if it changed between
the two the card redraws with a *"this changed while you were reading it"* line rather than applying
— which is the same posture `approving_a_stale_proposal_is_refused` already takes on the version
token, arriving from the index instead of from the file.

The proposal card therefore gains one section and no new mechanism. `Applied.diff` from
`apply_with(dry_run)` (`ops.rs:124`) stays the authoritative *what changes*; the impact set is the
*what else notices*. Two questions, two blocks, one card.

`loom propose --list` gains the same summary as one line per proposal, so the headless path is not a
second-class citizen: *`p-4f2a · destructive · 6 nodes · breaks 11 references in 3 scenes`*.

### 2.4 The one flow that has no modal, on purpose

**Opening a prefab file for editing is a destructive edit that the editor cannot gate**, because the
destruction is arbitrarily far in the future and the act itself is legitimate. So it gets the banner
in §0's list rather than a dialog. `prefab_room.loom`'s own header comment already says the thing
the banner says — *"Edit `prefabs/lamp.loom` and all three follow"* — which is evidence that this is
the fact people need and that comments are a bad place to keep it.

---

## 3. Integration: what is a panel, and what is an affordance somewhere else

**A graph that only lives in its own tab will not be used, and the reason is structural rather than
motivational: nobody navigates *to* a reference question.** The question arrives while you are
already looking at something. So five of the six surfaces are inline, and the panel exists for the
one job an inline surface provably cannot do.

| Surface | What the graph adds | Cost |
| --- | --- | --- |
| **Inspector** | a **Used by** section, collapsed if empty, below the last component. Rows are `file · node · field`, grouped by file, sorted, `text_weak` for the file and `text` for the node. Clicking opens that scene at that node. Present for a selected node that is a prefab instance, and for the Project panel's file selection. | one section |
| **Project panel** | a right-aligned `text_weak` use count per row, an **Unused** filter in the existing toolbar (doc 11 §6 gives Project both a toolbar and a footer already), and the three context-menu items in §2.1. | one column, one filter |
| **Problems panel** | two categories. `asset_file_missing` already exists per-scene (`main.rs:483`) and now aggregates across the project instead of only the open scene. `asset_unreferenced` is new and **is impossible without the index** — validation is per-scene, so it can say "this scene wants a file that is not there" and can never say "this file is wanted by nothing". Severity: warning, never error. | two rows in an existing list |
| **Hierarchy** | **nothing.** Measured: intra-scene references are `parent` and nothing else, and the hierarchy is already the picture of `parent`. Adding a badge here would be decoration. | zero |
| **Command palette** | four rows in `COMMANDS` (ADR 0031, data not code): `Find references` (Shift+F12), `Show unused files`, `Rebuild reference index`, `Show reference neighbourhood`. Each shows its unavailability reason when there is no project — a single scene outside a project has no index (§6.4). | four `Command` literals |
| **Prefabs panel** | the instance count per prefab in the same `text_weak` position as Project's, and the banner in §2.4 when a prefab file is the open document. | one column |
| **Agent panel** | the impact block in the proposal card (§2.3), and clickable `graph_query` tool rows (§7). | reuses existing rows |
| **References panel** (new) | the result of any query, surviving the navigation it causes; project-wide lists; the drawing. §4. | one `Tab` variant |

**The decisive argument for the panel**, and the reason a section in the inspector is not enough:
clicking a result *changes the selection*, which rebuilds the inspector, which destroys the result
list you were working through. A list you navigate from has to outlive the navigation. That is what
a dock tab is for, and it is why `Console`, `Problems` and `History` are all tabs of the bottom node
rather than sections of something.

### 3.1 The `Tab` enum, and an amendment to `PLAN.md` §2.9

`PLAN.md` fixes eleven `Tab` variants in Stage 3 and argues *"adding a variant later invalidates
every saved layout, which is why this list is decided once."* **That reason binds for removal and
reordering and does not bind for an addition.** A saved `DockState<Tab>` written before a variant
existed deserializes fine — it simply contains no such tab — and the Window menu already has to be
able to open a tab that is not currently placed, because that is what re-opening a closed panel is.

So: **`Tab::References` is added in the graph's own stage, as the twelfth variant, and `PLAN.md`
§2.9's rule is amended to read "closed against removal and reordering; an additive variant is
permitted when it arrives with a designed body and a Window-menu entry."** That is strictly cheaper
than shipping a dead tab from Stage 3 through Stage 11 whose empty state would have to say *"this
panel does not work yet"*, which is a worse first impression than no tab, by the plan's own argument.

The claim that additive variants deserialize cleanly is the one thing here a compiler must confirm —
§13.

---

## 4. The References panel, and the drawing

### 4.1 Why not force-directed

Four reasons, and the first two are enough:

**It answers no question in §1.** Every one of those questions is *"what touches X"*, whose answer is
a set of paths. A path is text. Rendering text as circles connected by springs and then asking the
user to read the labels is strictly more work than printing the labels.

**It is non-deterministic, and this project's whole posture is that derived things are reproducible.**
The same project laid out twice gives two pictures, so you cannot compare a screenshot to a
screenshot, cannot put it in a gate, and cannot describe what you saw to someone else. `cargo xtask
shimmer` exists in this repo because things that move when they should not are the failure mode
everyone here has already been bitten by.

**It costs a per-frame simulation on the UI thread**, in an editor whose motion budget (doc 11 §8)
was spent deliberately on one shuttle indicator, and whose viewport is the thing that should own the
frame.

**At 288 files it is a hairball and at 3,000 it is a smear.** The 176 asset paths in this repo mostly
run from a handful of scenes into `assets/textures/`, so the honest picture of this project is a few
enormous stars and a lot of isolated dots — which is true, and useless.

### 4.2 What is drawn instead

**A focus view: one subject, its dependents on the left, its dependencies on the right, two hops,
laid out by sorting.** No simulation, no animation, no randomness.

```
   depends on it                  focus                     it depends on
   ───────────────                ─────                     ─────────────
   quay.loom          ─┐
   props.loom         ─┼──▶  rock_beach_albedo.png
   forest.loom        ─┘            │
                                    └──▶  (a texture references nothing)

   ── second hop, collapsed ──
   [ +4 files that reference quay.loom ]
```

The layout rule is three lines: columns at fixed x, rows sorted by `(kind, path)` at a fixed 22 px
pitch, edges as straight polylines between the row centres. Deterministic by construction because
`BTreeMap` iteration is. Beyond 24 rows a column ends in a `+N more` chip that expands into the list
below — **the list is always the escape hatch, and the drawing is always optional.**

The panel is a splitter: results list on the left, drawing on the right, and the drawing collapses
to zero width with one toggle that persists in `prefs.toml`. **If the drawing is never opened, the
panel still does its whole job**, which is the test of whether it was worth building.

Readability at 3,000 files is not a layout problem because **the drawing never shows the project**.
It shows a neighbourhood, and a neighbourhood is bounded by the cap, not by the project size. There
is no "show me everything" mode; the request for one is answered by the Unused filter and the
Problems categories, which are the two project-wide questions anybody actually has.

### 4.3 How it uses the visual identity

Doc 11's governing rule — *"the chrome is greyscale; every colour in the interface is data"* — is
what makes this drawing cheap, because it forbids most of what a graph view usually spends colour on.

- **Node kinds carry no colour and no icon.** A one-word kind in `text_weak` after the filename. The
  icon budget is ≤ 24 with sixteen pinned and six already claimed by parallel documents; this design
  adds **zero icons** and says so, because "icons never appear without a label" (doc 11 §11) makes a
  per-kind glyph beside a per-kind word pure redundancy.
- **The focus node is `accent`** (`#A78BFA`) — the same violet that means *this is the thing you are
  acting on* everywhere else.
- **`agent` `#78C8FF` marks a file the editor did not write in the last `CHANGE_FADE` seconds** —
  recency, never authorship, exactly ADR 0035, reusing the mark the viewport already draws.
- **`error` `#F0736D` is a broken edge** (a `path` that resolves to nothing), drawn as the same
  polyline dashed. This is the only place the drawing beats the list, and it is why the drawing
  survived at all: a broken edge is visible at a glance in a picture and is one row among eleven in
  a list.
- **The edges are the warp.** Doc 11 spends the metaphor in three places; this is a fourth that costs
  nothing, because the columns *are* fixed vertical hairlines in `line` and the edges *are*
  horizontal threads crossing them. It is the same geometry vocabulary as `icons.rs` — straight
  segments only, one 1.5 pt weight from `WidgetVisuals` — and it uses `overlay::stroked` for nothing,
  because this is a panel body and not viewport chrome.

The panel takes doc 11 §6's standard composition: no title header, a toolbar (subject chip, hop
depth 1/2, kind filter, drawing toggle), a body, and a footer reading `11 references · 3 files`. Its
empty state is *"Select something and press Shift+F12 to see what references it."*

---

## 5. What runs on the UI thread

**Queries run on the UI thread; the index never does.** The split is the same one ADR 0037 already
uses for the agent: one `std::thread`, an `mpsc` channel, drained in the egui frame.

A query is a breadth-first walk over an adjacency map of ~700 edges with a depth cap of 2 and a
result cap of 24 per column. That is microseconds and it is fine to run per-frame; it is not run
per-frame anyway, because a result set is computed when the subject changes and cached in the panel's
own state.

The index build is a thread. It runs once when a project opens, sends `IndexReady(Arc<Index>)`, and
the editor swaps the `Arc`. Until it arrives, every graph surface renders its empty state and no
surface blocks — **a slow index must never be able to delay a frame**, and an `Arc` swap is how that
stays true without a lock in the paint path.

Re-indexing is per-file and driven by the poll the editor already has. `run.rs` polls the open scene
four times a second; the index thread does a 1 Hz `stat` sweep of the project's file list (~350
`statx` calls, well under a millisecond) and re-parses only what moved. **No `notify` crate, no
inotify, no watch descriptors, no new dependency** — the design doc says "rebuilt incrementally by a
file watcher" and a poll at 1 Hz over 350 files is the same behaviour with none of the platform
surface. The trigger to reverse is a project where the sweep is measurably visible, which at 350
files it is not and at 3,000 files is 3,000 `statx` calls, still under a frame.

The `loom graph` CLI is a fresh process per agent call and gets the cache, §6.3.

---

## 6. The index this panel needs

*A sibling document may own the indexer. This section states the **contract** the panel depends on —
six functions and a data shape — and then the store I recommend and why. **If the index ships on
SQLite, nothing above §6 changes**: the panel calls the same six functions.*

### 6.1 The model: nodes are files, and everything inside a file is edge metadata

§2.7 of the design doc gives `node.kind` as `scene|prefab|asset|component_type|script|system` and
edges including `child_of`, `reads_component`, `writes_component`, `emits`, `listens`. **Measured
against this repository, most of that is either free elsewhere or unbuildable, and the reduction is
large enough to change the storage decision.**

Kept, because they exist and are asked about:

```rust
// crates/loom_graph/src/lib.rs
pub struct File { pub path: String, pub kind: Kind, pub id: Option<String>, pub hash: [u8; 8] }
pub enum Kind { Scene, Prefab, Mesh, Texture, Script, Audio, Other }

pub struct Ref {
    pub from: u32,          // index into files
    pub to:   u32,
    pub kind: RefKind,
    pub node: String,       // "Room/LampLeft", or "" for a file-level reference
    pub field: String,      // "Material.albedo_map", or "" 
}
pub enum RefKind { Declares, References, Instantiates, Extends, Entry }
```

Cut, each with its reason:

- **`child_of`** — 364 of them in this repo, and the hierarchy panel *is* the picture of `parent`.
  Every cross-file query is unaffected by parentage.
- **Scene nodes as graph nodes** — a node is edge metadata instead. Justified by measurement: **no
  component field in any of the 52 scenes holds a node path**, so nothing outside a scene can point
  at a node, so a node can never be the *target* of a cross-file query. Trigger to reverse: the day a
  component gains a `NodePath`-typed field, nodes become nodes.
- **`component_type` nodes and `attaches` edges** — `describe_type` and `list_types` already answer
  everything about types, from the registry, without an index. An edge from every scene to
  `Transform` is 100% noise density.
- **`reads_component` / `writes_component` / `emits` / `listens`** — these require parsing `rhai`,
  which means either a real parser or a regex over a scripting language, and a regex over a language
  is a lie that reports confidently. A `Script.path` gets a file edge and its contents stay opaque.
  Trigger: someone wants it twice, and then the cost is `rhai`'s own AST, which `loom_script`
  already builds.

The logical schema, stated as SQL because that is the shape the design doc pinned and because it
documents intent regardless of the store:

```sql
CREATE TABLE file (id INTEGER PRIMARY KEY, path TEXT UNIQUE, kind TEXT, uuid TEXT, hash BLOB);
CREATE TABLE ref  (src INTEGER, dst INTEGER, kind TEXT, node TEXT, field TEXT);
CREATE INDEX ref_src ON ref(src, kind);
CREATE INDEX ref_dst ON ref(dst, kind);
```

### 6.2 The six queries

Each is one function on `Index`, and each has a `loom graph` flag and a `graph_query` question. The
SQL is what the function computes; the Rust is a BFS over two `BTreeMap<u32, Vec<u32>>` adjacency
maps.

| Function | SQL it computes | Question |
| --- | --- | --- |
| `used_by(path) -> Vec<Ref>` | `SELECT * FROM ref JOIN file s ON s.id=ref.src WHERE ref.dst=:id ORDER BY s.path, ref.node` | what references this |
| `uses(path) -> Vec<Ref>` | the same with `src`/`dst` swapped | what this references |
| `impact(subject) -> Impact` | `used_by`, plus the prefab-instance and last-alias-user special cases in §2.2 | what breaks |
| `neighbourhood(path, depth) -> Sub` | the recursive CTE in §2.7, capped at depth 2 and 24 per column | the drawing |
| `orphans() -> Vec<&File>` | `SELECT path FROM file WHERE kind<>'scene' AND NOT EXISTS (SELECT 1 FROM ref WHERE dst=file.id)` | what nothing uses |
| `broken() -> Vec<Ref>` | refs whose `dst` resolved to no file on disk | what is missing |

`why(path)` is `neighbourhood` walked backwards to the project's `main_scene`, returning the shortest
chain. It is a seventh function and it is twelve lines over the same maps.

**Every result is sorted, and that is a requirement rather than a nicety.** The agent's output must
be stable across runs or a diff of two `graph_query` calls is noise, and the panel's row order must
not depend on filesystem order or the drawing stops being deterministic. `BTreeMap` throughout; no
`HashMap` anywhere in the crate, which is the same discipline the simulation crates already carry.

### 6.3 The store: no database — ADR 0042

**The whole index of this project is ~350 files and ~700 references. It is smaller than
`proving_ground.loom`.** All 52 `.loom` files together are 394,509 bytes; a full cold rebuild is 52
`Scene::parse` calls over 385 KB of TOML.

So the index is `Vec<File>` + `Vec<Ref>` + two `BTreeMap` adjacency maps, built in memory, and
persisted as **one JSON file** at `$XDG_CACHE_HOME/loom/graph/<project-key>.json` through
`PLAN.md` §2.12's single path-keying helper. On load, each entry's `(mtime, len)` is checked and only
mismatches are re-parsed. Predicted cache size for this repo: ~100 KB.

`rusqlite` is rejected on four grounds, in order of weight:

1. **It buys nothing at this size.** SQLite's advantage is answering a query without loading the
   corpus. Loading this corpus is a 100 KB `serde_json::from_str`.
2. **It is a C dependency in a workspace whose Windows story is "shaders need no work and `ash`
   dlopens `vulkan-1.dll`"** (ADR 0032). `bundled` compiles ~250k lines of C on every clean build,
   against `LOOM-IMPLEMENTATION-ORDER.md:574`'s one-minute-warm stop-and-fix trigger; unbundled makes
   `libsqlite3` a platform requirement and puts it in the path of Stage 0's `cargo check --target
   x86_64-pc-windows-gnu`, which type-checks the whole workspace.
3. **A schema is a migration problem**, and a cache that needs migrating has started becoming a
   source of truth. A cache with no schema is deleted and rebuilt.
4. **Nothing in this workspace has a database**, and the first one is a category, not a dependency.

**The honest ceiling, stated rather than discovered:** at ~3,000 files the cache is roughly 1 MB and
the per-invocation deserialize is on the order of tens of milliseconds, which is under the process
spawn it rides along with. Past that — call it 10 MB of cache or a cold rebuild over 250 ms — the
answer is SQLite, and because the six functions in §6.2 are the only surface, swapping the store
touches one file and no caller. That is the reversal cost, and it is why the lazy version is safe.

### 6.4 Scope, and the case with no project

The index is **per project** and needs `loom.toml` (ADR 0023) to know what the file set is. A single
scene opened outside a project (`loom run --edit scene.loom`, and every `--frames` gate invocation,
which `PLAN.md` §2.11 forces to scene-only mode) **has no index and every graph surface shows its
unavailable state with the reason** — which ADR 0031 already requires of a disabled command. The
References panel's empty state in that mode is *"Reference search needs a project — this is a single
scene. [ Create a project… ]"*, matching Project's existing empty state word for word.

This is also the guarantee that the gates never touch the index: **all 43 windowed gate invocations
are scene-only, so none of them builds one.**

---

## 7. The agent's view, and how the human sees what it asked

**The graph exists mostly for the agent, and the agent's access to it is one CLI subcommand wrapped
by one MCP tool.** CLI first, MCP second, as `loom_agent/src/lib.rs:7` already states.

```
loom graph --used-by <path>            loom graph --impact <path|node>
loom graph --uses <path>               loom graph --orphans
loom graph --neighbourhood <path> [--depth 2]
loom graph --broken                    loom graph --why <path>
loom graph --rebuild | --stats         [--project <dir>] [--json]
```

`--json` on stdout, exit 0, one object, sorted — the same exit contract every other subcommand has,
and dispatched at `main.rs`'s `Some("graph")` beside `Some("prefab")`.

`graph_query` joins `loom_agent::TOOLS` as the eleventh entry (eight today, plus Stage 6's
`editor_context` and `propose_wait`):

```jsonc
{ "name": "graph_query",
  "arguments": {
    "question": "used_by|uses|impact|neighbourhood|orphans|broken|why",
    "subject":  "assets/textures/rock_beach_albedo.png",   // omitted for orphans/broken
    "depth":    2                                          // neighbourhood only
  } }
```

**It is a named-question tool, not a SQL passthrough**, which is what §2.7 asked for in the words
*"the SQL questions in §2.7, as named parameterized queries"*. A SQL string from a model into a cache
couples the agent to a schema that must stay free to change, and it makes the tool's failure modes
"syntax error" instead of "no such file". Rejected explicitly, because a SQL passthrough is the thing
that will be proposed next.

**The two-hop context pack is `neighbourhood --depth 2`**, and it is worth being clear about what it
is now worth: the design doc's argument was retrieval-beats-a-dump *at scale*, and at 350 nodes the
whole index would fit in a context window. What the agent actually gets from it today is not
compression but **direction** — `used_by` is a question the filesystem cannot answer without grepping
every scene, and grepping every scene is exactly the token spend the tool removes.

**How the human sees what the agent asked.** Every `graph_query` is an ordinary tool row in the agent
panel's compacted stream (ADR 0037's wire, doc 09 §5.2), rendered as
`queried · used_by rock_beach_albedo.png · 11 results` — **and the row is clickable**, opening the
same result set in the References panel. That is the whole integration: no new mechanism, one click
handler, and the human can check the agent's reasoning against the same index the agent read rather
than against a summary of it.

When the agent proposes a bulk delete, the card's impact block (§2.3) is computed by the *editor*
from its own index, not copied from anything the agent said. **The agent cannot supply the impact
set**, which is the property that makes the card a check rather than a restatement.

---

## 8. How the graph is prevented from affecting a pixel or a tick

Four mechanisms, in decreasing order of how much they would hurt to violate.

**A dependency rule, enforced by the script that already enforces three others.**
`scripts/check-deps.sh` gains a rule in the shape of the `loom_agent` one at `:33-44`: `loom_graph`
may depend on `loom_scene` and `loom_reflect` and nothing else, and **only `loom_cli` and
`loom_editor` may depend on `loom_graph`.** No render crate, no sim crate, no `loom_ecs`, no
`loom_scene` (which depends on nothing in-workspace and must stay that way — this is also why the
index is a crate rather than a module of `loom_scene`: it would drag `serde_json` caching and file
walking into the crate every simulation links).

**No `SceneOp` is ever derived from the index.** The impact sheet displays; every button on it issues
ops the user could have issued by hand, through `Session::apply`, with a version token. The index is
never read inside `apply`, never read by `loom render`, `loom sim` or `loom validate`, and no code
path exists by which a query result becomes a field value.

**The gates never build one.** `xtask` invokes no graph command; all 43 windowed invocations are
scene-only and therefore index-less (§6.4); `SCENES` stays 51 and `GOLDEN` stays 34; no scene, no
component, no shader, no `ObjectData` field, no descriptor.

**And the test that makes it observable rather than argued:** `green_run_writes_no_index` asserts
`$XDG_CACHE_HOME/loom/graph/` does not exist after `scripts/green.sh`. It is three lines, it fails
loudly the day someone wires the index into a code path the gate drives, and it is the same class of
check as `PLAN.md` §2.12's rule that `LOOM_JOURNAL=1` keeps the journal out of `cargo test`.

Determinism inside the crate: `BTreeMap` only, no `HashMap`, no `thread_rng`, no wall clock in any
query — mtimes are read in the *cache validator*, which is I/O, and never in a result. Two runs over
the same bytes produce byte-identical JSON, which is what makes `loom graph --json` diffable and the
drawing reproducible.

---

## 9. The two ADRs

**ADR 0042 — The project index is file-grained, derived, and has no database.**
Nodes are files; four reference kinds (`Declares`, `References`, `Instantiates`, `Extends`) plus
`Entry`; scene nodes, component types, `child_of` and every script-derived edge are cut, each with
its measurement or its trigger. The store is two `Vec`s and two `BTreeMap`s with a single JSON cache
under `$XDG_CACHE_HOME/loom/graph/<key>.json`, validated per file by `(mtime, len)`, deletable and
fully regenerable, and **outside every project directory** — so it is not gitignored, it is
un-ignorable, which is strictly stronger. No `rusqlite`, no `notify`, no new dependency of any kind;
re-indexing rides the editor's existing poll and a 1 Hz `stat` sweep. `loom graph` is the CLI and
`graph_query` is one named-question MCP tool, not SQL. `loom_graph` depends on `loom_scene` and
`loom_reflect`; only `loom_cli` and `loom_editor` depend on it; `check-deps.sh` enforces both. The
ceiling is stated (≈3,000 files / 10 MB of cache / 250 ms cold) and SQLite is the named reversal,
costing one file because the six query functions are the only surface.
*Rejected:* SQLite now (four reasons, §6.3); a SQL passthrough tool; a file-watcher crate; per-scene-node
graph nodes; component-type nodes; script-derived edges via regex; storing the cache in the project
directory (a non-diffable engine-written file in a git repository gets committed, then trusted, then
becomes a source of truth — ADR 0034 already rejected exactly this for the journal); making the index
a module of `loom_scene` (it would put file walking and a cache in the crate every simulation links).

**ADR 0043 — The reference index surfaces as lists and one impact sheet; the drawing is deterministic
and secondary.**
The primary surfaces are inline: a Used-by section in the inspector, a use count and an Unused filter
in Project, two Problems categories, an instance count and an edit-time banner in Prefabs. One new
`Tab::References` holds the results that must outlive the navigation they cause, and `PLAN.md` §2.9's
"the enum is fixed once" is amended to bind against removal and reordering only. The impact sheet is
one widget used by the human's Delete, the agent's proposal card (recomputed at render and again at
Approve) and `loom graph --impact`'s text output; it states a verdict, never appears when the answer
is "nothing", and offers `loom prefab unpack` as the non-destructive alternative where one exists.
**The editor does not delete files** — the Project panel has Find references, Reveal and Copy path,
and no Delete — because file deletion has no version token, no transaction and no undo, and adding a
trash mechanism is a second undo domain (never-do #16 from outside the scene). The drawing is a
two-column focus view at up to two hops with a 24-row cap, laid out by sorting, adding **zero icons**
to a budget with one slot left, colouring only the focus (`accent`), recency (`agent`, ADR 0035) and
broken edges (`error`).
*Rejected:* a force-directed layout (non-deterministic, unreadable at 288 files, a per-frame
simulation against a motion budget spent elsewhere, and the trap ADR 0003 named); a whole-project
view of any kind; a graph-only tab with no inline surfaces (nobody navigates *to* a reference
question); per-kind icons (redundant beside a per-kind word, in a budget with one slot); a
"delete and fix the references" button (a change of appearance disguised as a cleanup); a modal on
every delete (trains click-through, which is the blind-approve regression).

**The budget moves from twenty ADRs to twenty-two, 0022–0043.** Not three: the CLI and MCP shapes are
clauses of 0042, because they are the same decision's surface and a separate approval for them would
be approving a spelling.

---

## 10. Where this belongs, and what it depends on

**Propose Stage 12 — the knowledge graph — slottable any time after Stage 6, and before Stage 11 if
the guide is to cover it in the same pass.** It is not in the 7–10 painting chain and nothing in that
chain depends on it, so it slots the way Stage 5 does.

Dependencies, all real: **Stage 1** (the inspector, which the Used-by section is a section of, and
`loom_reflect::resolve`, which the reference extractor reuses to find `AssetRef`-shaped fields
generically rather than by a hardcoded field list); **Stage 3** (the dock, the theme tokens, the
palette); **Stage 4** (`COMMANDS`, the Problems panel, History); **Stage 5** (`loom_scene::project` —
`find_root` and `scenes()` are how the index knows what the project is, and without ADR 0023 there is
no project to index); **Stage 6** (the proposal card the impact block goes in, and the tool-row stream
the `graph_query` rows appear in).

Three slices, so the value lands before the panel does:

- **12a — the index, the CLI, the MCP tool, the Used-by section, the prefab banner.** This alone
  answers ADR 0003's exit criterion. Runnable: open `prefabs/lamp.loom` and see *"1 scene instances
  this prefab"*; run `loom graph --used-by assets/textures/rock_beach_albedo.png` and get 11 rows.
- **12b — the impact sheet, the References panel, Problems, Project's filter, the proposal block.**
  Runnable: ask the agent to delete six nodes and read what it breaks before approving.
- **12c — the drawing.** Runnable: press Shift+F12 on a texture and see its neighbourhood. **If 12a
  and 12b have been in use for a week and nobody has missed 12c, that is data, and the honest
  response is to not build it.**

**Green checks: all four, unchanged.** `SCENES` 51, `GOLDEN` 34, no fifth check. New tests:
`index_of_this_repo_finds_every_asset_declaration` (161 `[[asset]]` blocks reachable);
`a_moved_file_is_reindexed_and_a_stale_cache_is_ignored`; `queries_are_sorted_and_two_runs_agree`;
`green_run_writes_no_index` (§8); `impact_of_a_prefab_instance_names_its_prefab`;
**`every_path_shaped_field_in_the_registry_is_indexed`** — a walk over `TypeRegistry::describe`
failing when a component gains a file-path or `AssetRef` field the extractor does not know about.

**That last test is the load-bearing one.** The failure mode this whole subsystem has is the one
`CLAUDE.md` already records twice — `meadow` missing from `GOLDEN`, `grass_blades` passing a flat
`Ground` — where the machinery reports a clean pass over content it never looked at. An index that
silently stops seeing a reference kind reports *"nothing uses this"* about a file eleven nodes use,
and a person deletes it. Ranking that as the riskiest thing in this design is not modesty; it is the
same defect this repository has now shipped twice.

### Files this touches

New: `crates/loom_graph/{Cargo.toml, src/lib.rs, src/index.rs, src/query.rs, src/cache.rs}`;
`crates/loom_cli/src/graph_cmd.rs`; `crates/loom_editor/src/panels/references.rs`;
`crates/loom_editor/src/graph_view.rs`.

Edited: `scripts/check-deps.sh` (one rule); `crates/loom_cli/src/main.rs` (`Some("graph")`, and the
aggregate `asset_file_missing` path at `:483`); `crates/loom_agent/src/lib.rs` (`TOOLS`) and
`src/main.rs` (the schema); `crates/loom_editor/src/{dock.rs, command.rs}`;
`crates/loom_editor/src/panels/{inspector.rs, project.rs, problems.rs, prefabs.rs}`;
`crates/loom_editor/src/agent/{panel.rs, proposal.rs}`; `PLAN.md` §2.9, §2.12 (one cache row), §3
(two ADRs), §4 (Stage 12), §5 (strike the "knowledge-graph view" cut row, whose stated trigger —
*"ADR 0003 is accepted"* — has now fired); `docs/decisions/0003-*.md` (status → accepted, option 2
adapted); `docs/guide/` one section in Stage 11.

---

## 11. What I rejected, beyond the two ADRs' lists

**A `graph` mode inside the Project panel instead of its own tab.** It fails for the same reason the
inspector section does: the Project panel's selection is what you are navigating *from*, so the
result list dies on the first click.

**An egui floating window instead of a `Tab` variant.** It needs no enum change and no dock work,
which is genuinely lazier — and it is wrong in a docked editor, because the user has to place it
every time and it covers the viewport it is trying to explain. The Tab variant costs one enum row
once §3.1's amendment is accepted.

**Making the graph the home for "what did the agent touch".** The journal (ADR 0034) already records
actor, label, ops and resulting token per transaction, and History already renders it. Answering the
question from the graph would be a second, worse implementation of provenance — and the graph has no
notion of time at all, which is the whole substance of the question.

**Indexing `docs/` and `crates/`.** 288 files was the count that met ADR 0003's condition, and it
includes markdown and Rust. Neither is referenced by a `.loom` file and neither participates in any
question in §1. The index covers the project's authored content — scenes, prefabs, meshes, textures,
scripts, audio — and `rg` covers prose better than any index this project would build.

**A `used_by` count in the hierarchy.** Measured to be always zero for cross-file references, because
nothing outside a scene can name a node.

---

## 12. Where this contradicts something already written

Recorded plainly, because a contradiction discovered later reads as an oversight.

- **`ai-native-engine-design.md` §2.7 specifies SQLite.** §6.3 declines it on measured size and states
  the reversal trigger. The three properties §2.7 says matter more than the schema — cache not source
  of truth, incremental, answers what a context window cannot hold — are all kept exactly.
- **§2.7 specifies a file watcher.** §5 uses a 1 Hz `stat` sweep on the thread that already exists.
  Same behaviour, no platform surface, no dependency.
- **§2.7's force-directed visualization "sits on top of the same tables and is for you."** Kept as a
  view rather than infrastructure, exactly as it says — but not force-directed, §4.1.
- **`PLAN.md` §2.9 fixes eleven `Tab` variants "once".** §3.1 amends the rule to bind against removal
  and reordering. This is the one amendment here that touches a decision made in round 2.
- **`PLAN.md` §5's cut row for the knowledge-graph view** names its own trigger as *"ADR 0003 is
  accepted"*. It has been. The row is struck rather than argued with.
- **ADR 0003's own recommendation was option 1 (defer).** The accepted outcome is closest to option 2
  with the timing moved: the indexer and `graph_query` are built, but after the editor rework rather
  than before M9, and the condition the ADR set for revisiting — a project with enough cross-file
  references for the question to be real — was checked (288 tracked files, 176 asset paths) rather
  than assumed.

---

## 13. What I could not verify

**No `cargo` command was run** — this phase forbids it, and another workflow is compiling in parallel
worktrees. So every dependency, compile and timing claim here is unchecked by a compiler.

**The cold rebuild time is an estimate, not a measurement.** 52 `Scene::parse` calls over 394,509
bytes of TOML "should be tens of milliseconds" is arithmetic on an unmeasured constant, and it is the
number §6.3's whole store decision rests on. **This is Stage 12's first measurement, taken before any
UI is drawn**, in the discipline Stages 8 and 9 already use: time `Scene::parse` over every file in
`SCENES` and print it. If a full rebuild is over ~250 ms today, the JSON cache is not enough and the
SQLite reversal is the answer, not an optimisation to argue about later.

**That additive `Tab` variants deserialize an older saved `DockState` cleanly** is asserted from how
serde and `egui_dock` are expected to behave and was not run. §3.1's whole amendment depends on it,
and the check is one line: write a layout, add a variant, reopen. If it is false, the fallback is
`layout.rs`'s existing ignore-and-warn path, which costs the user one lost layout once.

**That `loom_reflect::resolve` can identify an `AssetRef`-typed field generically** — by `$ref` into
`$defs` — is inferred from `PLAN.md` Stage 1's description of the inspector's schema walk and from
`loom_reflect/src/lib.rs:233-258`'s handling of the `oneOf`+`const` enum spelling, which I read only
through the plan's citation. If it cannot, the extractor falls back to an explicit field table, and
`every_path_shaped_field_in_the_registry_is_indexed` becomes the only thing standing between that
table and silent staleness — which raises that test from important to load-bearing.

**`Script.path` and `Sound.clip` are plain `String`s, not `AssetRef`s**, so they need a small explicit
rule regardless. I confirmed `Script.path` (9 `.rhai` files, all `"../scripts/*.rhai"`) and saw two
`clip =` lines; I did not read every component in `components.rs`, so **the set of path-shaped fields
in this design may be incomplete** and the registry test is what closes that rather than my reading.

**Whether a sibling document is designing `loom_graph` itself.** §6 is written as a contract of six
functions precisely so that, if one is, only §6.3's store paragraph conflicts and the panel is
unaffected either way. `PLAN.md` is the arbiter where they disagree, per its own opening.

**Nobody has used this.** Whether a Used-by section is looked at, whether the prefab banner is read or
becomes chrome, and whether anyone ever opens the drawing are all §6-of-`PLAN.md` R24 material —
unautomatable, and the reason 12c is a separable slice with an explicit instruction to not build it if
nobody misses it.
