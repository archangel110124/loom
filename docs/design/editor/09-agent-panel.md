# The agent panel — the docked surface where a person and the agent author one scene

*Round-2 design, consistent with `PLAN.md` and additive to it. `PLAN.md` supersedes `01`–`07`;
this document does not contest anything it decided. Where it must **amend** `PLAN.md` — three
places — §8 names them.*

*Design phase. No `cargo` command was run. Every `file:line` below was read in this worktree at
`62f9ebe` with `rg`/`sed`; §10 lists what I could not check without building.*

---

## 1. The claim

**The agent panel is not a chat window bolted onto an editor. It is the one place where the two
authors of a scene can see each other, and almost all of it already exists below the UI.** The
transaction path, the version token, the atomic write, the sidecar lock, the change-mark overlay
and the labelled history are all built and all tested. What is missing is a conversation, an
approval gate, and — the one genuine defect — **the fact that today an agent write silently
destroys the human's undo stack, so the flagship promise that "a twelve-op agent transaction
undoes in one Ctrl+Z" is false for every transaction that arrives through the file watcher.**

That defect is the centre of this document. Everything else is either already there or is a
subprocess and a directory of JSON files.

Three sentences fix the model:

1. **An external write that arrives while the editor is clean becomes an ordinary undo entry**,
   labelled with the transaction's own label, instead of clearing the undo stack (§3).
2. **The panel is never in the write path.** The agent writes through `loom scene --tx` exactly
   as it does with no editor open, so the editor's presence cannot change what the agent can do
   or how it is tested (§4).
3. **The one exception is Approve**, because there the actor is the human, and the transaction it
   applies is the human's — which is precisely what makes an approved twelve-op deletion one
   Ctrl+Z (§6).

---

## 2. What already works, verified, and must not be rebuilt

| Behaviour | Where | Consequence for this design |
| --- | --- | --- |
| Agent writes land in the viewport within 250 ms | `run.rs:635` `poll_file`, `WATCH_INTERVAL` | "watch its SceneOps land live" needs **no new mechanism**. |
| Changed nodes bloom as fading outlines | `run.rs:422` `agent_marks`, `CHANGE_FADE = 6.0` at `:419`, drawn at `panels.rs:663` | The viewport half of the feature is done. The panel reuses the same hue, `#78C8FF` (`panels.rs:679`). |
| A transaction is atomic — it cannot half-apply | `ops.rs:178-232`: ops walk an in-memory `DocumentMut`, `Scene::parse` gates the result, `write_atomically` runs only on success | "an op fails halfway" is **already** all-or-nothing. §7c is about the message, not the mechanism. |
| Stale writes are rejected, never merged | `ops.rs:196-215`, `edit.rs:339-364`, lock held across re-read and write at `edit.rs:346` | The proposal queue inherits it for free (§6). |
| One transaction is one undo step | `edit.rs:174-179` — whole-scene snapshots, not structural inverses | Adopting an external write as an undo entry is a `Vec::push`, not a new undo model. |
| A gesture is one undo step, and an ordinary apply ends the run | `edit.rs:282-297` | The adopt must be an ordinary apply, and must not fire mid-drag (§3.3). |
| `loom_agent` is depended on by nothing, enforced | `scripts/check-deps.sh:33-44`; it is a `[[bin]]`, not a library | The editor cannot link the agent. This decides §4 before any argument. |
| The agent's read surface | `loom_agent/src/lib.rs:23-40` — `scene_query`, `scene_edit`, `scene_place`, `scene_measure`, `water_query`, `describe_type`, `render_preview`, `run_scene` | Seeing the scene is solved. One entry is missing and it is the editor's own state (§5). |

**Two things are verified broken and both belong to this document because the agent is their
likeliest cause.**

**`Session::reload` clears undo, redo and gesture** (`edit.rs:395-406`), for a stated and correct
reason: *"the file moving under us invalidates the undo chain, and offering it anyway would let a
user undo their way onto someone else's work."* The reasoning is sound for the case it was
written for — a foreign write of unknown provenance — and wrong for the case that actually
dominates, which is a labelled transaction whose ops we can name.

**An undeclared asset alias produces a box and no diagnostic whatsoever.** `main.rs:1150` builds
each wanted mesh with `scene_asset_path(scene, &name)?` inside a closure; the `?` returns `None`
with no log line, the alias never enters `by_name`, and `index_for` returns `0` — *"Draw index for
an asset alias; 0 (the box) when unknown"* (`main.rs:1230`). A failed *import* warns
(`main.rs:1181`); an alias that was never declared does not. This is the S4 prefab bug class
again, and it is the shape a hallucinated asset path takes (§7b).

---

## 3. ADR 0033 — the scene journal, and an external write becomes an undo entry

> **Decision.** `loom_scene::edit::apply_to_file` appends one JSON line per applied transaction to
> a journal at `$XDG_STATE_HOME/loom/journal/<blake3 of the canonical scene path>.jsonl`, holding
> the label, the resulting version token, the ops and an actor string. `Session` gains
> `adopt_external(text, label)`, which takes a new disk version **as an undo entry** rather than
> clearing the stack. The editor adopts when it is clean and the journal explains the change;
> it falls back to today's `reload` when it cannot. **The journal is a disposable cache and never
> a source of truth** — delete it and you lose labels, nothing else.

### 3.1 Why the journal, and why it is not a second source of truth

Undo works without the journal. What the journal buys is the *label*, and a label is not
decoration in this project: `CLAUDE.md` spends a paragraph on it, `Transaction.label`'s own doc
comment says *"'Block out office: 14 nodes' beats 'update scene'"* (`ops.rs:103-105`), and the
existing Transactions panel exists to show it (`panels.rs:640-650`). Losing the label at exactly
the moment the human most needs it — when somebody else wrote — is the worst place to lose it.
It also carries the ops, which is what makes partial revert possible (§3.4).

The danger with any second file is that it becomes authority. Three properties keep it from
becoming one, and they are the ADR's real content:

- **It is keyed by path under XDG state, not stored in the project.** ADR 0023 prohibits a project
  directory acquiring engine-written files; `renderer.rs:2604-2606` already shows the
  `XDG_CACHE_HOME`-or-`HOME/.cache` pattern to copy. A `git mv` orphans the journal, and that is
  correct: it is session context, not authored state.
- **Every entry is validated against the file before it is used.** An entry is only trusted when
  its `version` equals the token of the text actually on disk. A stale, truncated, hand-edited or
  absent journal degrades to "the scene changed on disk" and the old `reload` path. There is no
  code path where the journal contradicts the file and the journal wins.
- **It is capped and rotated** at 200 entries per scene, truncating oldest-first. An unbounded
  append-only file in a state directory is a thing someone finds at 4 GB.

`blake3` and `serde_json` are already `loom_scene` dependencies (`crates/loom_scene/Cargo.toml`),
so this adds none.

```rust
// crates/loom_scene/src/journal.rs   (new, ~90 lines)
#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,                 // unix seconds, for display only — never for ordering
    pub actor: String,           // "agent" | "editor" | "cli" — from $LOOM_ACTOR, default "cli"
    pub label: String,
    pub version: String,         // the token AFTER applying; the key that validates this entry
    pub ops: Vec<SceneOp>,
}
pub fn append(scene: &Path, applied: &Applied, actor: &str);
pub fn since(scene: &Path, version: &VersionToken) -> Vec<Entry>;  // entries after that token
```

`append` is called from `apply_to_file` **inside the existing lock** (`edit.rs:94`), after
`write_atomically` succeeds. Inside the lock because two concurrent CLI writers must not interleave
lines, and after the write because a journal entry for a write that failed is worse than no entry.
A journal write that itself fails is swallowed with a warning — **the journal must never be able
to fail a transaction**, which is the whole reason it is allowed to exist.

`Session::save` (`edit.rs:339`) appends too, with actor `editor`, so a human's Ctrl+S is a journal
entry like anything else and a second editor adopts it with a real label.

### 3.2 `adopt_external`, and why it is not a merge

```rust
// crates/loom_scene/src/edit.rs
/// Take the file's new contents as an ordinary undo entry.
///
/// The caller must have established that this session has no unsaved edits.
/// Nothing is merged: the previous text goes on the undo stack whole, the new
/// text becomes current whole, and undoing writes the previous one back as an
/// ordinary edit carrying a current token.
pub fn adopt_external(&mut self, text: String, label: String) {
    self.gesture = None;
    self.undo.push(std::mem::replace(&mut self.text, text));
    self.redo.clear();
    self.history.push(label);
    self.version = VersionToken::of(&self.text);
    self.disk = self.version.clone();
}
```

Nine lines, and it is `commit` (`edit.rs:299-311`) with the op application removed. It is
**not** a violation of never-do #15, and the distinction is exact: #15 forbids force-writing
against a stale token and silently reconciling two divergent states. Here there is one state —
the file's — and the session takes it whole. Undoing it later is an ordinary transaction carrying
the current token, which the agent's next write will correctly reject as stale if it raced.

The precedent is already in the tree: `accept_disk_version` (`edit.rs:366-385`) carries a
paragraph making the same argument, and its reasoning is the one to follow.

### 3.3 When the editor adopts, and when it refuses

`poll_file` (`run.rs:635`) gains one branch. The decision table is small enough to be a test, and
should be one:

| Editor state | Journal | Behaviour |
| --- | --- | --- |
| clean, no gesture | one entry ending at the disk token | **adopt**, labelled `"agent · Block out office: 14 nodes"` |
| clean, no gesture | N entries ending at the disk token | **adopt as one**, labelled `"agent · 3 transactions — …"`, with the newest label quoted |
| clean, no gesture | absent, stale, or ending elsewhere | today's `reload` — undo cleared, console line, History draws its rule |
| clean, **gesture live** | any | **defer** until the gesture key clears. `apply_coalescing`'s contract (`edit.rs:276-278`) says an ordinary apply ends the run, so adopting mid-drag would silently cut the human's drag in half; `LOOM-IMPLEMENTATION-ORDER.md:445` already asks for exactly this deferral. |
| **dirty** | any | today's divergence banner. Never adopt, never merge (§7a). |

Adopting a burst of three transactions as one undo entry is a deliberate coarsening and it is
stated in the label. The alternative — N entries — needs the intermediate texts, which the journal
does not store and should not; storing them would make it a second copy of the scene's history and
therefore a second source of truth.

**This is a behavioural change to `PLAN.md` Stage 4** and §8 records it: History's *"rule drawn
where the agent wrote saying steps above it cannot be undone"* becomes the **fallback** rendering,
drawn only on the rows where the adopt was refused. The clean case now shows an ordinary,
undoable, agent-tinted row. Doc 07 §8 was right that silence is the bug; it was one stage short of
noticing the clearing itself is usually avoidable.

### 3.4 Reverting part of a transaction

Undo is whole-scene snapshots, so partial undo does not exist and should not be invented. What
exists instead, and what the journal makes possible: a History row for a multi-op transaction gets
**"Undo and re-apply a subset…"**, which opens a list of the transaction's ops with checkboxes,
and Apply issues `undo()` followed by a **new** transaction labelled
`"Re-apply 9 of 12: Block out office"`.

**That is two undo steps and the button says so**, in the sentence ADR 0008 established for
`apply-overrides`: *"This is two steps: the undo, then the re-application."* The project already
has a case where one user action is two undo entries and already decided how to talk about it.

**A subset that does not stand on its own is rejected atomically and the panel says which op
failed.** Dropping a `SpawnNode` while keeping a `SetField` on the node it created produces
`node_not_found` on op 7, nothing is written, and the human sees exactly that. This is strictly
better than a mechanism that could produce a half-scene, and it is why the atomicity verified in
§2 matters here.

---

## 4. ADR 0034 — the agent is a subprocess, and the panel is not the write path

> **Decision.** `loom_editor::agent::Process` spawns a user-configured command as a child process
> with the project root as its working directory, piped stdin/stdout/stderr, one JSON object per
> line in each direction, read by one `std::thread` into an `mpsc` channel drained in the egui
> frame. **No LLM client, no HTTP, no async runtime, no `loom_agent` dependency, no terminal
> emulator.** The agent mutates the scene only through the `loom` CLI and `loom-mcp`, exactly as
> it does with no editor running; the panel renders the conversation and the consequences. No
> default command is shipped — an unconfigured project shows how to configure one.

### 4.1 Why a subprocess, stated against the alternatives

`scripts/check-deps.sh:33-44` forbids depending on `loom_agent`, and `loom_agent` is a `[[bin]]`
with no library target for the engine anyway (`crates/loom_agent/Cargo.toml`) — *"making it an
executable is how that stays true by construction rather than by discipline."* So the question is
not whether to link it; it is what the editor talks to.

- **An in-process LLM client is rejected.** It adds `reqwest`/`tokio`/SSE parsing, API-key storage,
  a tool-call loop and a model-name that goes stale, all inside the editor's frame loop — and it
  makes Loom an agent harness, which is a product this project is not building. It would also put
  a network dependency inside the crate `cargo xtask validate` drives.
- **An MCP client in the editor is the same thing plus a protocol.** The editor would then be
  driving the model *and* speaking MCP to itself, since `loom-mcp` already wraps the CLI.
- **A pty terminal emulator is rejected.** `portable-pty` + a `vte` parser, and the result renders
  as a terminal — at which point there is nowhere to put an approval card, an op row, or an inline
  render, and the panel's whole reason to exist evaporates.
- **The panel-as-write-path is rejected, and this is the sharpest of the four.** If the agent
  proposed and the editor applied, the agent would behave differently with the editor open than
  without it — a second code path, tested by nobody, in the write path. That is ADR 0018's defect
  class and this project has paid for it three times. **CLI first is not a slogan here; it is what
  keeps the editor's presence from being a variable in the agent's behaviour.**

### 4.2 The wire

One JSON object per line. The panel understands five keys and **renders every unrecognised line as
plain text rather than dropping it**, because an agent CLI that changes its schema must degrade to
a worse-looking panel, never to a silent one — the same rule `loom-mcp`'s `tool_args` already
learned the hard way (`loom_agent/src/main.rs:100-112`: *"Nothing is dropped silently"*).

```
→ {"type":"user","text":"make the crates smaller"}
← {"type":"assistant","text":"I'll scale …"}          streaming prose, appended
← {"type":"tool","name":"scene_edit","summary":"loom scene quay.loom --tx /tmp/tx.json"}
← {"type":"result","status":"ok"}                      turn over
← {"type":"error","text":"…"}
```

Everything else is `{"type": <unknown>}` and renders as a dim monospace line under a
"raw" disclosure. Config in `loom.toml`:

```toml
[agent]
command = ["claude", "-p", "--output-format", "stream-json",
                     "--input-format", "stream-json", "--verbose"]
approve  = "destructive"      # "none" | "destructive" | "bulk" | "all"
approve_above_nodes = 25
preamble = "…"                # optional; a default ships
```

**No command is shipped as a default.** Naming a vendor's CLI in the engine's manifest is a
coupling this project should not take, and the binary may not be installed. The unconfigured panel
shows one paragraph and a copyable snippet — which is also the honest answer for the "strangers
will use it" audience: the panel says what it needs and how to give it, rather than failing to
launch something the user never asked for.

**The panel never spawns anything under `--frames`.** `cargo xtask validate` drives
`loom run --edit --frames` (`xtask/src/main.rs:1023`), and a green check that launches an agent
process is not a green check.

### 4.3 The runtime, in full

```rust
// crates/loom_editor/src/agent/process.rs   (~110 lines)
pub struct Process { child: Child, tx: ChildStdin, rx: Receiver<Line>, state: State }
pub enum Line { Event(Event), Raw(String), Stderr(String), Exited(Option<i32>) }
pub enum State { Idle, Thinking, Tool(String), WaitingForYou, Dead }
```

Two `std::thread`s (stdout, stderr) doing `BufRead::lines` into one `mpsc::Sender`. The frame
drains with `try_iter`. **Stderr is captured always and shown on exit**, because a command that is
not installed or is given wrong flags writes there and only there, and a panel that discards it is
unusable to someone who did not write it.

Files: `crates/loom_editor/src/agent/{mod.rs, process.rs, panel.rs, proposal.rs, context.rs}`.
No new dependency in any manifest.

---

## 5. The panel itself

### 5.1 Shape

`Tab::Agent`, docked in the right column **as a tab beside `Inspector`** — so the right column
reads as "the thing I am talking to, about the thing I selected" — and it takes focus when the
agent produces output, with a dot on the tab otherwise. Tabbing beside the Inspector rather than
claiming a third column costs zero horizontal budget, which matters given `PLAN.md` Stage 3's
own arithmetic about how little viewport a Unity-shaped layout leaves.

Top to bottom:

**A header strip** — the configured agent's name, a state dot driven by `State` (never guessed
from timing), elapsed time on the current turn, and Stop. Stop sends SIGTERM and then SIGKILL after
two seconds; a stopped agent leaves the scene exactly as its last completed CLI call left it,
because the panel is not in the write path.

**The conversation**, newest at the bottom, three block kinds. Your turns and the agent's prose are
greyscale, per doc 07's governing rule that the chrome is greyscale and every colour is data.
**Event rows are the only coloured things in the panel**, and they are the feature: a landed
transaction in `#78C8FF` — the same hue the viewport outlines it in, which is what ties the row to
the box that just bloomed around the crate — a refused one in red, a validation problem in amber, a
rendered PNG as a 96 px thumbnail that opens large on click.

**A landed-transaction row is a link into History, not a copy of it.** History remains the
authoritative list of everything that happened to the scene, in order, including the human's own
edits; the Agent panel shows the subset that happened during this conversation, in conversational
context. Clicking the row selects the History entry and frames the affected nodes in the viewport,
reusing Stage 4's focus command. Two panels, one truth, and the relationship stated so nobody
builds a second history.

**The proposal card**, pinned above the composer whenever one is pending (§6).

**The composer** — multiline, Ctrl+Enter sends, with two chips: **Selection (3 nodes)** and
**Attach view**. A chip shows the exact text it adds to the turn. The panel never silently
decorates what the user typed.

### 5.2 Streaming, and the reason it is compacted

A single turn can run thirty tool calls over two minutes. Rendered raw, the panel is a firehose,
the human stops reading it, and then approves whatever it asks for — **which is precisely the
blind-approve regression `LOOM-IMPLEMENTATION-ORDER.md:451-453` locked a decision to avoid.** So
during a turn the panel shows the last three event rows plus one running line
(`running · loom sim --assert "wind@…" · 0:14`), and the full list is one click away. Prose streams
in full; tool noise collapses.

### 5.3 Selection context — how the agent knows what "this" means

`loom_editor::agent::context` writes, debounced to the existing 250 ms watch cadence:

```jsonc
// $XDG_STATE_HOME/loom/context/<blake3 of the project path>.json
{ "scene": "scenes/quay.loom",
  "version": "b478ea4a…",              // the token — the most useful field here
  "selection": ["quay/crate_03", "quay/crate_04"],
  "camera": { "eye": [4.0,2.0,9.0], "look": [0.0,1.0,0.0] },
  "tool": "select", "play": false }
```

A new subcommand `loom context [--project <dir>]` prints it, and `editor_context` joins
`loom_agent::TOOLS` — the ninth entry, and the first that is about the *editor* rather than the
scene. The version token is in it because that is what lets the agent set `expect_version` and
receive a correct rejection instead of clobbering a human's edit.

A shipped default `preamble` tells the agent to call `editor_context` before acting on a
demonstrative. That is config, not a hard-coded prompt in Rust, so a user can change it without
recompiling — and so it can be read.

**The panel writes this file even when no agent is configured.** It costs nothing, and it means an
agent driven from a terminal beside the editor gets the same context, which is the CLI-first
property applied to the panel's own state.

### 5.4 Letting the agent look at its own work

The agent can already render (`loom render`, `render_preview` in the catalog). One gap: it cannot
render *the view the human is looking at*, because `loom render`'s only camera override is
`--yaw`/`--pitch` orbiting the bounds (`main.rs:47-51`). **Add `--eye x,y,z --look x,y,z`** — two
flags, and they are useful independently of the editor, since the agent can compute a framing from
`loom measure` and render it. "Attach view" runs exactly that with the editor's current camera,
writes to `target/agent/view-<n>.png`, and names the file in the turn.

Nothing else is needed. The panel does not need a private render path, and building one would be a
second place the window and the offscreen path can disagree.

---

## 6. ADR 0035 — the destructive scope is enforced, and a gated transaction becomes a proposal

> **Decision.** `loom scene --tx` and `loom place --op` classify each transaction. A transaction
> containing `RemoveNode`, `RemoveComponent`, or `SpliceArray` with `remove > 0` is **destructive**;
> one touching more than `approve_above_nodes` distinct nodes is **bulk**. Under the project's
> `[agent] approve` policy, a gated transaction is not applied and not refused: it is **written to
> a proposal queue** at `$XDG_STATE_HOME/loom/proposals/<blake3 project>/<token>.json` and the
> command exits 0 reporting `{"status":"proposed","id":…}`. The editor's Agent panel shows it as one
> card with the real diff and Approve / Reject; `loom propose --list|--approve|--reject` is the
> headless equivalent. Approve applies the transaction through the editor's own `Session`, so it is
> the human's transaction and one Ctrl+Z. `--allow-destructive` remains as the explicit bypass for
> scripts and CI, and is **refused when `LOOM_AGENT=1` is set in the environment**, which the panel
> sets on the process it spawns.

### 6.1 The gap this closes

`SceneOp::RemoveNode`'s doc comment says *"Requires the `destructive` scope (§7.17)"*
(`ops.rs:72`), the brief says the scope is off by default, and **nothing in the tree checks
anything.** `grep -n "destructive" crates/loom_scene/src/ops.rs crates/loom_cli/src/main.rs`
returns the doc comment and no code. A comment that describes a check that does not exist is how
the next reader believes they are protected.

### 6.2 Why a proposal queue rather than a refusal

A plain refusal has a fatal shape: the agent gets `destructive_scope`, asks the human in
conversation, the human says yes, and the agent re-runs with the bypass flag — at which point the
flag is available to the agent at all times and the gate is advisory. Making the gate *produce
something* instead of refusing is what turns "may I?" into a reviewable artifact:

- The proposal carries the transaction, its `expect_version`, and the **diff**, which
  `apply_with(dry_run)` already produces (`ops.rs:124` — `Applied.diff`, *"for review and for
  `--dry-run`"*). The card is a view of a thing the engine already computes, not a new renderer.
- **Approve applies through `Session::apply`.** One transaction, one undo entry, the correct label,
  the viewport updated with no reload at all — the human's own edit, which is exactly what it is.
- **Approve re-checks the version and refuses if the scene moved.** The proposal's `expect_version`
  goes into the transaction; a stale approve returns `stale_version` and the card greys out with
  *"the scene changed since this was proposed — ask again."* Never merge. This also solves two
  editors racing on one proposal for free: the second approve is stale.
- Reject deletes the file and writes a one-line reason the agent can read back, so "no, keep the
  lamps" reaches the agent as data rather than as a hope that it read the transcript.

The mechanism is a directory of JSON files. **No daemon, no socket, no timed grant.** Time-based
approval grants were considered and rejected: a grant that outlives the intent that created it is
the blind-approve regression with a clock attached.

### 6.3 The honest limit

`LOOM_AGENT=1` is a sandbox marker the panel sets and the CLI honours. **An agent that unsets it
is out of policy and the policy cannot stop it**, which is exactly the posture the `rhai` sandbox
takes and it should be documented in those words rather than implied away. What the marker buys is
that the *default* configuration of the *shipped* panel cannot delete a subtree without a human
clicking a button, and that is the property worth having.

### 6.4 Bulk is a separate axis and that is the point

A wrong bulk edit is not destructive by op kind — two hundred `SetTransform`s destroy an
afternoon's blocking and every one of them is reversible in principle. Gating on *scale* as well as
on *kind* is one comparison (`ops.iter().map(node).collect::<BTreeSet<_>>().len()`) and it catches
the failure the user actually named. `approve_above_nodes = 25` is a guess and is written in
`loom.toml` so it can be tuned by the person it annoys.

---

## 7. The failure modes, concretely

**(a) A wrong bulk edit.** *Detected* by the History row saying `200 nodes` and by two hundred
outlines blooming at once — the overlay is already a scale signal. *Contained* by §6.4 proposing
it first. *Recovered* by one Ctrl+Z, because §3 made the adopted entry undoable.

The residual case is real and must not be papered over: **if the human had unsaved edits, the
adopt is refused and the divergence banner fires instead.** There is then no one-keystroke recovery,
because there are two divergent scenes and merging them is never-do #15. The banner keeps exactly
two destructive choices — Reload (theirs) and Keep mine (yours, and saving overwrites, which
`keep_mine` at `run.rs:702` already says out loud) — and gains one that loses nothing:

> **Reload, saving my version to `quay.mine.loom`.**

That writes a second file and therefore carries the sentence `PLAN.md` §2.6 requires of every
button that does: *"Undo restores the scene; the file stays."* It is not a merge, it is not
silent, and it converts the worst outcome in the whole design — a human losing unsaved work to a
robot — into a file they can diff. **A third button that reconciles the two versions must never be
built**, and this document says so in the same breath as offering the one that does not.

**(b) A hallucinated asset path.** Verified above: today it draws a box in silence. The fix does
not belong in the panel — it belongs in `Scene::parse`, which already refuses an undeclared
*prefab* alias and lists the declared ones (`scene.rs:388-404`). **Apply the identical check to
mesh and texture aliases**, and the transaction is refused before it is written, with a message
naming what *is* declared. That fixes it for the agent, for a hand edit, and for a `cp`ed scene
at once, which is why it is better than anything the panel could do.

The check must permit `loom_asset::primitives::NAMES` and the aliases voxel volumes generate, so
it must be built from the same set `MeshLibrary` derives `wanted` from (`main.rs:1146`). **If any
scene in `SCENES` legitimately uses an undeclared alias today, this becomes a Problems-panel
warning rather than a parse error** — the mechanism is the same, the severity is a measurement.
§10 lists this as unverified.

**(c) An op fails validation halfway through a transaction.** It cannot half-apply — §2 verifies
that. What is missing is *which op*: `TransactionError` (`ops.rs:131-140`) carries `error`, `label`,
`node`, `constraint`, `hint` and `current`, and no index. **Add
`op_index: Option<usize>`**, `skip_serializing_if = "Option::is_none"`, set at the two `apply_one`
call sites (`ops.rs:226`, `:229`). It is additive to a *result* payload, not to the scene format,
so no `format` bump. The panel then renders *"op 7 of 12 failed: `Light.intensity` must be ≥ 0"*
and the agent receives the same in JSON, which is the difference between "fix it" and "guess".

This is a Stage 0 or Stage 1 change, not a Stage 5A one, because every stage after it benefits.

**(d) The agent process dies mid-turn.** EOF on stdout → `Line::Exited(code)` → the panel shows
*"the agent exited (code 1)"* with the captured stderr in a disclosure and a Restart button. The
scene is untouched, because the panel was never in the write path — that is the property paying
for itself.

**(e) An agent write arrives mid-gesture.** Deferred until the gesture key clears (§3.3 row 4).

**(f) The agent asks a question and nobody is looking.** A dot on the tab and a status-bar line.
**No modal**, following `agent_overlay`'s own stated design (`panels.rs:660-662`: *"Deliberately
not a modal, not a list to acknowledge, not a notification to dismiss"*).

---

## 8. Where this belongs in `PLAN.md`, and the three amendments

**A new Stage 5A — the agent panel — immediately after Stage 5.** It depends on Stage 3 (the dock
and the `Tab` enum), Stage 4 (History, Transactions, Problems, the command table, focus-on-
selection) and Stage 5 (`loom.toml`, `loom_scene::project`, the XDG state helpers). Stages 6–8 do
not depend on it, so it can slot later if the painting run is more urgent — but it should not,
because it is the feature that distinguishes this editor and it is the smallest of the remaining
stages.

Numbered 5A rather than 6 deliberately: renumbering 6–9 across a plan four documents cite by stage
number costs more than a letter does.

**Three pieces must land in earlier stages, and cannot wait for 5A:**

- **Stage 0 or 1 — `op_index` on `TransactionError`.** One field. Every later stage's error
  messages get better and the cost never drops.
- **Stage 3 — `Tab::Agent` joins the enum.** Non-negotiable, and it is `PLAN.md`'s own rule that
  forces it: *"The `Tab` enum is fixed once, here … adding variants later invalidates every saved
  layout."* Its body until 5A is its real body when unconfigured — one paragraph and a `loom.toml`
  snippet — so this is not the empty-body tab the plan cut `Environment` and `Profiler` for.
- **Stage 4 — `loom_scene::journal` and `Session::adopt_external`.** They belong with History
  because they change what History shows. **This amends Stage 4's stated design**: the rule drawn
  where the agent wrote becomes the *fallback*, drawn only where the adopt was refused, and the
  common case becomes an ordinary undoable agent-tinted row.

**Amendments to round-1 ADRs:**

- **ADR 0023 must change.** `loom.toml` gains an `[agent]` table with four keys
  (`command`, `approve`, `approve_above_nodes`, `preamble`). This is not optional: the ADR
  specifies `deny_unknown_fields`, so a project carrying an `[agent]` table against a manifest
  struct that lacks it **fails to load**. The XDG state list gains `journal/`, `proposals/` and
  `context/`, all keyed by blake3 of a path exactly as `layouts/` already is.
- **ADR 0031's list gains rows, and its decision is unchanged.** Send, Stop, Approve, Reject,
  Attach view and Restart agent are `Command` rows like everything else, so they get palette
  entries, keybindings through `loom_input::ActionMap`, and documentation rows for free.
- **ADR 0026 gains a consequence, not a change**: `SpliceArray { remove > 0 }` is classified
  destructive by ADR 0035. Worth writing into 0026's text so the classifier is discoverable from
  the op that triggers it.
- **`PLAN.md` §2.6's union list gains four rows**, all in the existing "user state, outside the
  project" bucket: the conversation transcript, the proposal queue, the context file and the
  journal. §2.6 claims to be *the* list, so it has to be extended rather than shadowed.

**The ADR budget moves from twelve to fifteen** (0033, 0034, 0035). `PLAN.md` §3 asks the human to
see that as one number, so it is stated as one number here rather than discovered three times.

**The gates are unchanged.** No rendering path, no component, no scene: `SCENES` stays at 48 and
`GOLDEN` at 32, and S12's budget holds. The new tests are cheap and CPU-only:

1. `journal_round_trips_and_validates_against_the_file` — an entry whose `version` does not match
   the file on disk is not returned by `since`.
2. `adopt_decision_table` — the five rows of §3.3, as data.
3. `adopted_agent_transaction_undoes_in_one_step` — the twelve-op test at `edit.rs:457` re-run
   through `adopt_external`, which is this document's exit criterion as a test.
4. `destructive_classifier` — one case per op kind, plus `SpliceArray` at `remove = 0` and
   `remove = 1`.
5. `approving_a_stale_proposal_is_refused` — the never-do #15 case, as a test rather than a
   promise.
6. `an_unknown_line_from_the_agent_renders_rather_than_vanishing`.

**Exit criterion for Stage 5A**, in the shape M12's was: *ask the agent to delete six nodes; the
transaction arrives as one card with a readable diff; approving it lands one History entry; one
Ctrl+Z restores all six; the scene file after the undo is byte-identical to the scene file before
the approve.* Byte-identical, because the snapshot model makes that achievable and anything less
means the round trip is lossy.

---

## 9. What I rejected, and why

**A dedicated `loom_agent_ui` crate.** Its stated dependency is `loom_editor` and nothing else,
which is the definition of a module. `PLAN.md` rejected a `loom_paint` crate on the identical
argument against `LOOM-IMPLEMENTATION-ORDER.md:574`'s one-minute-warm build trigger.

**A socket or daemon so the agent's writes route through the editor.** It makes the agent's
behaviour depend on whether a window is open, which is a second write path tested by nobody. The
file watcher plus the journal gets the same result with no protocol, and it works for a second
terminal, a second editor and a shell script.

**Putting the journal in the project directory.** ADR 0023 prohibits it, and the reason is good:
a non-diffable engine-written file inside a git repository will be committed, then trusted, then
become a source of truth. The `.<name>.lock` sidecar (`edit.rs:120-136`) is the existing exception
and it is defensible precisely because it holds no data.

**Storing intermediate texts so a burst of agent writes becomes N undo entries.** That is a second
copy of the scene's history in a cache directory. One coarse entry with an honest label is better
than a fast path to a second source of truth.

**Timed approval grants** (`destructive_until = <ts>`). A grant that outlives its intent is the
blind-approve regression with a clock. The proposal queue gives per-transaction consent with a
diff, which is what the brief locked.

**A "merge" button on the divergence banner.** Never-do #15. §7a offers the non-lossy alternative
instead and names the forbidden one so nobody rediscovers it as a good idea.

**Rendering the conversation as a terminal.** Costs two dependencies, and leaves the approval card,
the op rows and the inline render with nowhere to live.

**Letting the panel decorate the user's message with selection silently.** The chip shows its text.
A panel that edits what you said, in a product whose thesis is diffable text, is the wrong instinct
in miniature.

---

## 10. What I could not verify

Design phase; no `cargo` command was run. These are the gaps that carry weight.

1. **Whether any scene in `SCENES` uses an undeclared mesh or texture alias today.** §7b's fix is a
   parse error if none do and a Problems warning if any do, and the difference is one grep I did
   not run against 43 scenes (`grep` for `mesh =` versus each file's `[[asset]]` keys). It changes
   the severity, not the mechanism.
2. **Whether anything in `xtask`, the tests, or `scripts/` applies `RemoveNode` through
   `loom scene --tx`.** If so, ADR 0035's default-refuse breaks a green check on the day it lands,
   and those call sites need `--allow-destructive`. Cheap to settle, and it should be settled in
   Stage 0 alongside the other probes.
3. **The exact JSONL schema of any specific agent CLI.** I designed the reader to be schema-tolerant
   for exactly this reason — five recognised keys, everything else rendered raw — but the shipped
   `command` example in §4.2 is written from memory and must be checked against the tool's own
   `--help` before it is committed to a documentation file where a stranger will copy it.
4. **Whether `blake3` of a canonicalised path is stable enough as a key across a project moved by
   `git mv`.** It is not, deliberately — the journal orphans and degrades to the old behaviour —
   but I have not checked whether `PLAN.md` S9's `layouts/` keying has the same property, and if it
   does, the two should share one helper rather than each hashing a path its own way.
5. **egui frame cost of a long conversation.** A thousand-message transcript in an
   immediate-mode panel is the obvious pressure point, and `LOOM-IMPLEMENTATION-ORDER.md:571`
   already names row virtualisation as the correct response. I have not measured where the knee is,
   and the design assumes retaining the last 200 turns is enough. It is a guess.
6. **Whether SIGTERM-then-SIGKILL on the child leaves an orphaned `loom` subprocess** that the
   agent itself spawned, mid-write, holding the scene lock. `File::lock` releases on process exit,
   so the lock is not the risk; a half-finished `write_atomically` is not either, since it renames.
   But I have not traced the process tree and a `loom scene --tx` orphaned mid-flight is worth one
   experiment before Stop is shipped.
7. **The whole of §5.1's layout claim — that the panel reads as first-class rather than as a
   sidebar — is a judgement no gate can make.** It belongs in `PLAN.md` R17's list of things
   settled by a session with the human, at the end of Stage 5A.
