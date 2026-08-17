# Review 2 — the usability lens on `09`–`12`

*Adversarial review of `09-agent-panel.md`, `10-foliage-and-scatter-painting.md`,
`11-visual-identity.md` and `12-project-model-revised.md`, read against `PLAN.md` and the
round-1 set. Design phase — **no `cargo` command was run**. Every `file:line` below was read in
this worktree at `62f9ebe` with `rg`/`sed`; §5 lists what I could not check.*

*This document looks for what is wrong. §4 is two paragraphs, deliberately.*

---

## 1. The five worst decisions

### 1.1 Four documents claim the same three ADR numbers, and nobody states the new budget

`PLAN.md` §3 exists because round 1 had **four documents claiming 0022 and two claiming 0023**, and
it fixed that by allocating numbers *once, before any is written*. Round 2 reproduced the identical
failure at larger scale, inside the very set that was supposed to be consistent with the plan:

| # | `09` | `10` | `11` | `12` |
| --- | --- | --- | --- | --- |
| **0033** | the scene journal | a foliage mask multiplies the rule | the editor colours recency, not authorship | engine-owned assets resolve from the exe |
| **0034** | the agent is a subprocess | a species is a node | UI colour is encoded exactly once | — |
| **0035** | the destructive scope is enforced | grass generation is camera-centred | — | — |

Nine distinct decisions in three numbers. This is not a clerical problem, because the documents
**cross-reference each other's numbers as though they were stable**: `10` §12.4 tells an implementer
*"This is ADR 0035 and it is a separate slice with its own gate"* while `09` §8 tells the same
implementer that ADR 0035 is what classifies a transaction as destructive. Whoever builds second
writes an ADR file that already exists and says something else.

Worse is the budget. `PLAN.md` §3: *"**Twelve ADRs — that is the approval budget, and the human
should see it as one number.**"* `09` §8 honestly restates it as fifteen. It did not know about the
other three documents. The real number is **twelve plus nine = twenty-one**, an increase of 75% in
what the human has to approve, and **it appears in no document**. The one process control the plan
put in place to protect the human's attention was defeated by writing four documents in parallel
without a shared allocator.

**Fix.** Allocate here, once, and make `PLAN.md` §3 the sole allocator that any future document
claims from:

```
0033 engine-owned assets resolve from the executable          (12)
0034 the editor colours recency, not authorship               (11)
0035 UI colour is authored in display space, encoded once     (11)
0036 the scene journal; an external write is an undo entry    (09)
0037 the agent is a subprocess; the panel is not the write path (09)
0038 the destructive scope is enforced; a gate produces a proposal (09)
0039 a painted foliage mask multiplies the placement rule     (10)
0040 a species is a node; a removed instance is a point       (10)
0041 grass generation is camera-centred                       (10, deferred)
```

`0033`–`0035` go to the documents whose decisions land earliest (Stages 0 and 5), so the numbers
run roughly in build order. And **`PLAN.md` §3 gains a line stating twenty-one as one number**,
because the whole point of that sentence was that the human sees the total before agreeing to the
first one.

---

### 1.2 Doc 10's own capacity arithmetic contradicts its two flagship affordances, and a stranger's first foliage stroke produces a truncated field

`10` §7.1 is the most valuable section in the four documents and it is honest: verified
`MAX_BLADES = 262_144` (`renderer.rs:999`, `viewer.rs:436`), `GrassBlade` is 48 bytes
(`renderer.rs:582`), and **the largest grass field that fits at density 140 is 43 × 43 m.** A
256 m field is 3,500% of the buffer and truncates in generation order, which is z-major, so the
user gets a straight horizontal edge across their landscape.

Two of the document's own affordances were written without reading that table.

**(a) The auto-created field is 6× to 36× over the ceiling.** §3: *"The first stroke spawns the
field and paints… The field's `half_extent` is sized to the terrain node's bounds, clamped to 128 m
so the first accident is not a nine-million-blade bake."* Read `half_extent = [128, 128]` — that is
a 256 m field, which is §7.1's own **9,175,040-blade** row, the exact accident the clamp is
introduced to prevent. Read the clamp as a 128 m *side* and it is 2.29M blades, still 8.7× over.
**Either reading, the very first stroke a stranger makes on a bare terrain produces a truncated
field with a hard straight edge and an `"ok": true` render.** That is the worst first-run outcome in
the four documents, and it is created by the affordance designed to make the first run good.

**(b) The budget meter is per-field and the buffer is global.** §8 shows
`45,360 / 262,144 blades · 17%` *per field*. Verified: `grass_blades` accumulates a single `Vec`
across every `Grass` node (`main.rs:1882-1890`) and `warn_if_grass_truncated` is called once on the
total (`main.rs:695`). Doc 10's central storage decision is **a species is a node** — so the
intended scene has six `Grass` nodes, each meter reading 17%, and the field truncating at exactly
the moment the meter says everything is fine. A meter that is correct on the one-species scene and
wrong on the multi-species scene the decision exists to enable is worse than no meter, because it
is trusted.

**Fix, one change with three consequences.** Move §7.6's CPU pre-cull **into slice 1**. It is one
function, it needs no shader change, and its correctness argument is already made and is good: the
Slang cull uses `loom_hash`, which is `loom_field::noise::hash` — frozen ABI, compared *exactly* by
the S2 agreement test — so a CPU evaluation at a conservative distance is provably a subset of what
the GPU keeps. The three measurements §7.5 asks for gate the *moving ring*, not the pre-cull. With
it, §7.6's own arithmetic gives ~517,000 blades resident for a 256 m field instead of 9.17M, and
`MAX_BLADES` at 524,288 puts the ceiling out of reach of any plausible field. Then:

- the auto-created field's clamp becomes a real number derived from the budget, not a guess;
- the meter becomes **scene-global**, a stacked bar whose segments are the fields, with the
  per-field count as the segment label;
- `warn_if_grass_truncated` gains the one fact it does not have today — *which* field was cut. It
  is generation order, so it is always the last one, and saying so is a `nth` and a name.

---

### 1.3 `engine_assets()` is broken for every installed copy of Loom — which is the only case the "strangers" audience is in

`12` §4 defines `engine_assets()` as **`<exe dir>/assets` if it exists, otherwise `assets` relative
to the working directory.** It then verifies two cases and both happen to work: the repository
(cwd branch, `<repo>/assets`) and a shipped game (exe branch, because `loom ship` puts the
executable at the project root).

**The third case is the one user decision 7 is about, and it fails.** A stranger installs Loom;
the binary is at `~/.cargo/bin/loom` or `/usr/bin/loom`; there is no sibling `assets/`. They run
`loom edit` from their home directory; `./assets` does not exist. So:

- **`engine_assets()/templates/` does not resolve, the hub's Templates rail is empty, and `loom new`
  cannot create a project.** Doc 12 §5 makes templates the one thing that *never* falls back to a
  project copy — *"templates are engine content and belong to the binary that creates from them"* —
  so there is no second chance. The entry point of the entire project model does not work on an
  installed binary.
- The bindings and the weather bed degrade to the compiled-in copy and the synthesiser, silently,
  which is the exact defect §1 of that document was written to close.

The document's own verification does not catch it. **V6 is vacuous**: it runs `loom render` from
`/tmp` against a `cargo`-built binary, whose exe dir is `target/debug` with no sibling `assets/`,
so the cwd branch misses and the compiled-in bindings load — which is what the document says
happens *today*. The test produces the same result before and after the change and therefore
discriminates nothing. §11 item 4 half-notices this (*"a third branch… may be needed on the first
test"*) and ships the two-branch definition anyway.

**Fix, and the document already contains it.** It rejects embedding `rain.wav` because it is 3 MB
of WAV — correct. **Templates are not 3 MB of WAV.** They are a `loom.toml`, a `.loom` scene and a
`.rhai` script: kilobytes of text, in the same category as `loom_input::DEFAULT_BINDINGS`, which
this engine already compiles in and which doc 12 cites approvingly one paragraph earlier. Compile
the templates in (`include_str!` per file, or one `&[(&str, &str)]` table), and `loom new` writes
them out. `engine_assets()` then has exactly one remaining consumer pair — bindings and the weather
bed — both of which already have a compiled-in or synthesised fallback, so the two-branch definition
is no longer load-bearing and its known gap is harmless. **The Templates rail then works on an
installed binary on day one, and `loom new` has no filesystem precondition at all.**

Add a V9 that would have caught it: *copy the binary alone to an empty directory, `cd` elsewhere,
run `loom new /tmp/p --template first_person`, and require it to succeed.*

---

### 1.4 A generated scatter instance cannot be picked, so doc 10's two headline verbs have no input path

`10` §6.2 and §6.3 are the interactions that make it Unreal's Foliage mode rather than Unity's
Detail brush: *"delete the tree in the doorway"* and *"drag one two metres"* (detach-and-move,
`SpliceArray` into `remove` plus `SpawnNode`, one Ctrl+Z). The *storage* for both is well designed.
**Neither document says how the user indicates which tree**, and the engine cannot answer it today.

Verified. `pick_at_cursor` (`run.rs:2002-2027`) ray-tests `self.view.picks`, which is
`BTreeMap<String, Bounds>` keyed by **node path** (`scene_view.rs:60`) and built by
`node_bounds(&world, &library)` (`scene_view.rs:121`). Scattered instances are produced separately
by `scatter_objects` and appended to `objects` (`scene_view.rs:118-120`) — they are `Object` rows
with a transform and no node path, and **they are not in `picks` at all**. A generated tree is not
merely hard to click; it is invisible to the only selection mechanism the editor has.

So as specified, foliage mode is **paint and erase**. That is Unity's Detail brush. Unreal's
Foliage mode is paint, erase, fill, single-place, *select*, lasso-select, select-all-of-type, and
per-instance transform — and the select half is where an artist spends the second half of their
session, because painting gets you 90% and the last 10% is always "not that one, and move that one".
The task asks whether this is a thin imitation; on this axis, as written, it is.

**Fix, ~40 lines and it reuses everything.** `SceneView` gains
`instance_picks: Vec<(String /* field node */, u32 /* index */, Bounds)>`, filled by
`scatter_objects`, which already computes every instance's transform (`main.rs:1858`) and already
has the mesh's bounds. `pick_at_cursor` tests it *after* `picks` so a real node always wins a tie.
Selecting an instance shows a synthetic inspector — *"instance 412 of **Pines** · generated"* — with
exactly two buttons, which are §6.2 and §6.3 made reachable: **Remove** (one `SpliceArray` into
`Scatter.remove`) and **Detach to node** (the one-transaction pair). Marquee select over instances
is then free, and multi-remove is one `SpliceArray` with N points.

Without this, §6.2 and §6.3 are storage formats with no author, and the only way to remove one tree
is to hand-write two floats into TOML — which the agent can do and the human cannot.

---

### 1.5 The approval loop has no return path to the agent, and the panel ships with no working configuration

Two defects, one flagship. `09` §6's proposal queue is the right *shape* — a gate that produces a
reviewable artifact beats a gate that refuses — and the diff comes from machinery that already
exists (`ops.rs:124`). But trace the actual conversation:

1. Human: *"delete the six crates on the quay."*
2. Agent runs `loom scene --tx`. It is destructive. The CLI **exits 0 with
   `{"status":"proposed","id":…}`** and writes nothing.
3. The agent sees a success exit code and a status it was not told to interpret. Best case it says
   "I proposed it" and **its turn ends**.
4. The human reads the card, thinks, clicks Approve two minutes later. The transaction lands
   through the editor's `Session`. The scene's version token moves.
5. The agent is idle, its context holds the pre-approval token, and **it has no idea what
   happened.** Its next write is `stale_version`. Reject is worse: §6.2 says the reason is *"a
   one-line reason the agent can read back"* — through what? There is no notification, no poll
   instruction, and no tool in `loom_agent::TOOLS` that returns proposal outcomes.

So the flagship interaction is: ask for something, get told it was proposed, approve it, and then
the agent that asked cannot continue. Every multi-step plan that contains one destructive step
stalls at that step. **The exit criterion in §8 hides this** — it tests one transaction, one card,
one approve, one Ctrl+Z, and never asks the agent to do anything afterwards.

**Fix, and it costs one tool and one sentence of preamble.** `loom propose --wait <id>` blocks
until the proposal is approved, rejected or times out, and prints the outcome plus the new version
token as one JSON line. It joins `loom_agent::TOOLS` as `propose_wait` beside the `editor_context`
entry §5.3 already adds. The shipped preamble tells the agent to call it when a command returns
`proposed`. The agent's turn then *stays open* across the human's decision, which is what makes it
a conversation rather than a fire-and-forget. A rejection with a reason arrives as data, in-band, at
the moment it is useful — which is what §6.2 claims and does not deliver.

**Second defect: the panel ships unusable.** §4.2 — *"No command is shipped as a default"* — and
§10.3 admits the example command's flags *"written from memory and must be checked against the
tool's own `--help`"*. For an audience of strangers, the headline feature of the editor is an empty
box asking them to paste a vendor CLI invocation with four flags they have never heard of. Compare
what `PLAN.md` Stage 5 does for the rig-less-Play failure: a banner, one sentence, one button. The
agent panel deserves the same and gets a config chore.

The refusal to name a vendor in the manifest is right. The consequence is not forced. **Fix:**
`loom.toml` accepts `command` as today, and the unconfigured panel offers a **detect** button that
probes `$PATH` for a short list of known agent CLIs and, on a hit, writes the `[agent]` table
itself with a labelled one-line transaction the user can read and undo. Zero detected → today's
paragraph. The vendor list is *data in a config file the user can edit*, not a coupling in the
manifest schema, which is the same distinction §4.2 makes about the preamble.

---

## 2. The four questions, answered directly

### Is the foliage brush as good as Unreal's Foliage mode?

**On storage, it is better than Unreal's and that is a real win.** §6.4's claim survives checking:
a 256 m forest is eight lines of `Scatter`, ~40 stroke points and a handful of `remove` points, and
"move the forest uphill" is one changed number in `git diff`. Unreal writes every instance into the
level. That property is genuinely Loom's and it is the reason the whole three-tier model is right.

**On interaction it is Unity's Detail brush with a better data model**, for the reason in §1.4: no
instance selection, and therefore no per-instance transform, no select-all-of-type, no lasso. Also
missing against Unreal, in descending order of how often an artist reaches for it:

1. **Instance select/edit** — §1.4.
2. **A "reapply" pass.** Unreal lets you change a foliage type's scale/align/density and push it
   onto already-placed instances. Loom gets this *free and better* — the instances are regenerated
   from the rule, so editing `Scatter.scale` in the inspector reapplies by construction. **Nobody
   says so**, and it is the single best answer this design has to an Unreal user's expectations. It
   belongs in the guide and in the palette's tooltip.
3. **Painting multiple species in one stroke** with per-species weights. Loom's answer is
   species-as-node plus one active field, which is one stroke per species. Acceptable, but the
   Foliage palette should support multi-select of fields with a shared brush, which is the same
   `Outcome::Edit` carrying N `SpliceArray`s in one transaction.
4. **Erase-all-of-type in a region.** Falls out of (3) with `value = 0`.

Item 1 is a design hole; 2–4 are palette work. **Verdict: the model is stronger, the tool as
specified is thinner, and one 40-line change closes most of the gap.**

### Can a user paint a convincing meadow in two minutes?

**No, as written, and for three independent reasons — two of them internal contradictions.**

1. **The field they get is truncated** (§1.2a). Two minutes in, they are looking at a hard straight
   edge across the middle of their landscape.
2. **Every stroke commit re-places every scatter field in the scene and rebakes all grass.** §7.3
   is honest about it and cites the measurement in the tree: **103 ms on `forest.loom`**
   (`scene_view.rs`, and `build_cached` calls `scatter_objects` unconditionally — verified at
   `scene_view.rs:118`). The fixes (`scatter_key`, `reach_of` dirty regions, tiled grass output) are
   listed as slice-1 and slice-2 work, and the grass number — *"call it 4 ms"* — is arithmetic on
   two unmeasured terms (§13 admits it). **A brush whose mouse-up costs 100 ms feels broken and no
   preview hides it.** `PLAN.md` Stage 7 already establishes the right discipline for exactly this:
   *gated on a measurement, taken before any UI is drawn.* Foliage slice 1 needs the same gate —
   `grass_blades` per-tile wall time and `Session::apply` with a 200-point stroke, both in §7.5,
   both taken **before** the tool is built rather than beside it.
3. **The refusal path is guaranteed and its one-click fix is wrong** (§3.4 below).

**What two minutes *should* look like, and it is reachable:** select the foliage tool on a terrain,
drag, a correctly-sized field appears and fills under the cursor at interactive rates; the ragged
edge from §2.3 is visible in the preview; a second species is one click in the palette; erase cuts
a path. Slice 1 with the pre-cull, the budget-derived default and a measured commit cost delivers
that. Slice 1 as written delivers a truncated field with a 100 ms hitch.

### Is the agent panel genuinely useful, or a chat box bolted to an editor?

**Three things in it are genuinely useful and only one of them is the panel.**

- **`loom render --eye/--look`** (§5.4) and **`loom context` / `editor_context`** (§5.3) are CLI
  additions that make the agent able to see what the human sees. They work with no editor open,
  from a terminal, which is the property doc 09 correctly insists on. These are the best ideas in
  the document and they are not panel features.
- **`op_index` on `TransactionError`** (§7c) turns *"the transaction failed"* into *"op 7 of 12
  failed: `Light.intensity` must be ≥ 0"*, for the agent and the human at once. One field. It
  should not wait for Stage 5A and §8 correctly says so.
- **The approval card with a real diff** is the one thing that is genuinely panel-shaped, because a
  diff plus two buttons plus the viewport bloom is a review surface a terminal cannot be.

**What the panel does *not* buy, and the document oversells:** the conversation itself. `§1`'s
claim — *"the one place where the two authors of a scene can see each other"* — is already true of
the **viewport**: `agent_marks` blooms changed nodes at `#78C8FF` and has since M12 (`run.rs:422`,
`panels.rs:663`). The conversation adds convenience over an adjacent terminal, which is worth
having and is not a thesis.

**Would a real user trust it?** Not until three things are true, and two are missing:

1. **The undo channel is one-way.** §7a's recovery story is *"one Ctrl+Z, because §3 made the
   adopted entry undoable"* — correct, and then **the agent is never told.** It holds a plan built
   on a scene state the human just reverted, and its next act is to redo the thing that was
   rejected. In a co-authoring editor this is the failure a user will describe as "it fights me".
   **Fix:** `Session::undo` of an *adopted* entry appends a journal line with
   `actor = "editor"` and a label naming what it reverted, and the preamble tells the agent to call
   `editor_context` (which carries the version token) before acting on a plan older than one turn.
   No new mechanism — the journal and the context file both already exist in this design.
2. **There is no mid-turn interrupt.** §5.1 gives the composer Ctrl+Enter and a Stop that is
   SIGTERM-then-SIGKILL. The wire is bidirectional and `ChildStdin` is held open (§4.3), so
   *"no, not that one"* mid-turn is a `writeln!` — but nothing in the document permits it, and Stop
   is a sledgehammer that leaves the scene wherever the last CLI call left it. One sentence of
   policy: the composer is live during `State::Thinking` and `State::Tool`, and a mid-turn line is
   sent as an ordinary `{"type":"user"}`.
3. **The approval round-trip stalls the agent** — §1.5.

**Verdict: a chat box bolted to an editor, plus one genuinely novel surface (the approval card) and
three excellent CLI additions that are not the panel.** The novel surface does not close the loop.

### Is the visual identity concrete enough to build, and would it look good?

**Concrete: yes, and it is the most implementable of the four documents.** §3's token table, §5's
spacing block against verified `Spacing` field names, §9's five `WidgetVisuals` rows and §11's
`apply()` sketch are typeable. The API-shape corrections are exactly the kind that would otherwise
cost a day — `Margin` is `i8` in 0.35, `CornerRadius` is four `u8`s, `all_styles_mut` rather than
`set_style`, no global line-height, `Options` has no `reduce_motion`. The double-encode analysis is
sound (I followed the composition: `tok` → `pow(2.2)` → sRGB encode is an identity, so the
correction is right).

**Would it look good: it would look competent, and it would not look like Loom.** Three problems.

**The palette is stock.** `accent = #A78BFA` is *exactly* Tailwind's `violet-400`, and
`#0E1013 / #16191E / #1E232A` is within a couple of ΔE of VS Code Dark Modern's ground ramp. The
argument for violet (§3) is good and I do not want it overturned — the hue reasoning about the three
axis colours is real. But "bespoke visual identity designed for Loom" (user decision 6) is not
satisfied by picking a defensible accent off the same shelf everyone else picks from.

**The one bespoke idea is a name attached to a generic mark.** §1 names the 2 px edge "the warp"
and gives a lovely reading of why a loom's warp is the right metaphor for chrome. Then the mark is a
2 px accent bar on the active tab (VS Code's `tab.activeBorderTop`) and on the selected row
(Linear's row rail). The metaphor appears **nowhere else in the entire document**: not in the hub,
not in an empty state, not in a busy indicator, not in a logo, not in the window. §6's empty states
are icon + line + button, which is Bootstrap's. An identity asserted in prose and absent from pixels
is a theme with a good README.

**The identity has no answer for the states a person stares at.** §6 defines empty states well.
Nothing defines **busy** — the agent thinking, the terrain baking, `loom ship` building, the
thumbnail subprocess rendering — and those are the moments a user's eye has nothing else to do.
`PLAN.md` and doc 10 between them queue at least four operations measured in seconds.

**Fix, and it is cheap because it reuses `icons.rs`'s primitives and adds no asset class.** Spend the
metaphor in the three places it costs nothing and is seen constantly:

1. **A busy indicator that is a shuttle crossing a warp** — a fixed set of vertical hairlines with
   one horizontal segment traversing them, drawn with `Painter` line segments, ~20 lines. It is
   ownable, it is not a spinner, and it is the same geometry vocabulary as the icons. Used by the
   agent's `Thinking`, the bake, the build and the thumbnail.
2. **Threads that cross rather than stack.** §1 says a selected node the agent just touched *"shows
   both threads, stacked"*. Make them **cross** — the agent thread runs the row's left edge, the
   selection thread runs its top-left corner over it — and the motif produces a *behaviour* instead
   of a name. This is the one place where the metaphor can be a rule rather than a decoration.
3. **The hub headline** (§13 already allocates `Name("Display")` there) gets the lattice as its one
   piece of ownable art, drawn not shipped.

Then two corrections that are about looking *good* rather than looking *distinctive*:

- **`raised` against `surface` at 1.1:1 is too thin a bet.** §15.4 admits it may collapse to one
  grey on an eight-bit panel, and the mitigation is an opt-in high-contrast toggle a stranger will
  never find. `#1E232A` → `#232830` is ~1.35:1 — still flat, still hairline-carried, and it survives
  a bad monitor. The strategy does not depend on 1.1 specifically; it depends on the hairline, which
  §3 already lightened for exactly this reason.
- **`text_disabled` at 2.29:1 is the most-read grey in the application and it is illegible.**
  ADR 0031 mandates showing unavailable commands rather than hiding them, so the command palette a
  stranger opens on their first day is *mostly disabled rows*. 2.29:1 is below the 3:1 floor §3
  itself applies to the focus ring. Make it `#6B7484` (5.4:1 — the high-contrast value, as the
  default), and carry "disabled" on the *icon's* alpha and a right-aligned reason in `text_weak`,
  never on the label's luminance. §3's exemption argument (*"never the only carrier of meaning"*) is
  about correctness; this is about whether a person can read the thing they are supposed to read.

**Does the accent survive a snowfield and a night scene?** §7's cased-stroke rule
(3.0 px `chrome_casing` α200 under a 1.5 px core, through one `overlay::stroked` helper so there is
no other way to draw) is the correct answer and it is correctly justified against the three
alternatives, all of which are genuinely unreachable from an egui overlay. **It works, and the
document found a real live defect proving it matters**: `panels.rs:680` paints a bare 1.5 px
`(120,200,255)` agent stroke that vanishes on a bright render. One caveat the document misses:
`chrome_casing = #0A0C0F` at α200 over a *snowfield* is a dark halo on white — legible, and it will
read as a drop shadow, which is fine. Over a **`cave`** scene it is near-invisible and the 1.5 px
`accent` core is doing all the work at 6.47:1 against near-black — also fine. The failure case is
neither extreme but the **middle**: a mid-grey `#4A4E52` overcast sky, where the casing is 2.1:1 and
the violet core is 2.3:1 against the background. **Fix:** the core is `accent` and the casing is
`chrome_casing`, *plus* the outermost 0.5 px of the casing at `chrome_core` α60 — a three-layer
sandwich, still one helper, and it cannot be middle-grey on both sides.

---

## 3. Remaining findings, ranked

**HIGH — `09` §5.1 and `11` §10 specify contradictory homes for the agent panel, each citing user
decision 5, and `11` says `09`'s answer fails it.** `09`: *"docked in the right column **as a tab
beside `Inspector`**"*. `11`: *"a vertical split of the right column, **not a tab beside the
Inspector**, and that is the only layout that satisfies user decision 5."* Round 2 was supposed to
end this class of conflict, not create it.

**Neither is right, and the deciding fact is in `09` itself.** §6.2 makes the proposal card *"a view
of a thing the engine already computes"* — the transaction's **diff**. A unified diff of a `.loom`
file in `11` §10's 380 pt right column, split 60/40, is **~330 px tall and ~370 px wide with a
96 px inline thumbnail competing for the same width**. TOML lines wrap at that width; a twelve-op
diff does not fit; the one surface that justifies the panel's existence is unreadable in the place
both documents put it. **Fix: the Agent panel is a tab of the *bottom* node, beside Console and
History, and the bottom node's default height rises from 200 pt to 280 pt.** A bottom node is
full-width, which a diff, a tool-call log and a thumbnail all need; the conversation *is* a log and
logs live there in every editor; "watch its SceneOps land live" is satisfied by the viewport's
`agent_marks` and the Inspector both staying visible, which is *more* than either proposal offers,
since `11`'s split shrinks the Inspector to 60% of a column. `Tab::Agent` still joins the enum in
Stage 3 (both documents are right about that and `11` is right that `PLAN.md` §2.9 omitted it).

**HIGH — `10`'s "erase is absolute" is only true at authority 1, and the brush cannot reach it in
one pass.** §2.1 property 2 is load-bearing: *"A painter who erases grass gets no grass. That is the
single most important thing a brush must be able to promise."* But §4 says `flow` *"decides how fast
authority accumulates under repeated dabs, which is what makes a light touch feather"*. So one
confident swipe with the default flow leaves authority < 1, `lerp(1, 0, 0.6) = 0.4`, and 40% of the
grass survives — which reads as a broken eraser, not as feathering. **Fix:** the **Clear** preset
sets `flow = 1.0` (feathering an erase is what `radius`/`hardness` are for), the brush ring fills to
show accumulated authority under the cursor, and §2.1's property is restated honestly as *"erase is
exactly zero at full authority, and the ring shows when you have it."*

**HIGH — `10` §4's refusal banner offers a global fix for a local complaint, and it moves a golden
reference.** *"No grass placed on 62% of that stroke… **[Raise to 0.55]**"* changes `slope_cutoff`
on the whole field. Every other slope boundary in the meadow moves, including ones the user spent
ten minutes on, and if the scene is in `GOLDEN` its reference moves too. The user's intent was
local: *grass on this bank*. **Fix:** the primary button is **[Paint soil here]**, which switches to
Stage 6's splat brush with rock authority inverted — the composition path ADR 0028 already
establishes and §12.2 already records as the two-mask interaction — and *"or raise `slope_cutoff` on
the whole field"* is a secondary text link. Same thirty lines, correct scope, and it teaches the
two-brush model on the one occasion the user is guaranteed to be curious.

**HIGH — `12` V2 asserts "28 references, zero moved" but the change lands at Stage 5, by which point
`GOLDEN` is 30.** `PLAN.md` §2.8 grows `GOLDEN` 28 → 32, with `viewport_rect` added at Stage 2 and
`empty` at Stage 5. `12` §9's V2 and §11's *"the 28 golden references cannot move"* are round-1
arithmetic. Harmless as a fact, dangerous as a *verification step*: an implementer who runs V2 and
sees 30 will conclude the check is stale rather than that it passed. **Fix:** V2 asserts
`MANIFEST.txt` is byte-unchanged and does not name a count.

**MEDIUM — `10` §2.3's edge break-up needs a workspace edge nobody has priced.** Verified:
`crates/loom_asset/Cargo.toml` lists `blake3`, `gltf`, `png`, `serde`, `serde_json`, `uuid` — **no
`loom_field`**. §13 flags this as unverified; it is real. Adding `loom_asset → loom_field` for a 12%
radius jitter is a new edge in the crate the entire paint system sits in. **Fix, and it is the
document's own pattern:** the foliage baker takes `noise: &dyn Fn(f32, f32) -> f32`, exactly as
`loom_grass::tile` takes its ground closure (`lib.rs:315`), and `loom_cli` passes
`loom_field::noise::value`. Zero new edges, and the seam is already the one this design uses twice.

**MEDIUM — foliage is scheduled after voxel sculpting for a dependency that is a test, not a
build.** `10` §12.4 places it at "Stage 7½" and lists Stage 7 as a dependency *"for §9's 'sculpt
under painted grass and it follows' criterion"*. That is an acceptance test, not a compile
dependency — §9's mechanism is `grass_key` including every `VoxelVolume` (`main.rs:1641`), which
works today. **Fix: Stage 6½.** User decision 3 names foliage explicitly; Stage 7 (sculpt) does not
block it; and the earlier it lands the more sessions it gets before Stage 9's documentation freezes
what it is.

**MEDIUM — `11`'s `srgb_framebuffer: true` moves text antialiasing and the document treats the
change as strictly a correction.** With the constant `false`, egui blends glyph coverage in sRGB
space, which is what egui's defaults are tuned against; flipping it moves blending to linear, and
light-on-dark text gets **visibly thinner**. That is the whole reason the flag exists as a flag.
The Stage 0 probe as specified (§2: sixteen flat swatches and a grey ramp) cannot see it. **Fix:**
the probe renders the swatch strip **and a paragraph of `Body` text at both settings side by side**,
and its acceptance criterion is *"swatches within ±2 bytes **and** text weight unchanged."* If text
gets thin, the answer is a `FontTweak` or accepting the double encode with the palette retuned — and
either way it is better to know at Stage 0 than after the whole token table is judged.

**MEDIUM — `11` rejects a UNORM swapchain for a reason the same document disproves.** §12's
ADR draft: rejected because it *"moves the scene's own tonemap output, which the golden references
pin."* §7 of the same document: *"`cargo xtask image` … drives `loom render`, which is the offscreen
`Renderer` and never constructs a `Ui`."* The golden references never see the swapchain. The real
cost of UNORM is that the tonemap would have to encode sRGB itself and the *window* would change —
a genuine cost and a different one. **Fix:** record the true reason. A rejection filed under a
disprovable reason is one the next reader reverses.

**MEDIUM — `11` schedules `--theme-probe` in three different stages, and in a crate that does not
exist when it is needed.** §2: *"in Stage 0… about fifteen lines in `loom_editor`"*. §13 row 0: the
probe is Stage 0. §13 row 3: `--theme-probe` lands in Stage 3. And `PLAN.md` §2.1 creates
`loom_editor` in **Stage 1**. **Fix:** the probe is fifteen lines of `egui` in `loom_cli`'s existing
panel path at Stage 0 — it draws literal hexes and needs no theme module — and it moves into
`loom_editor` with `theme.rs` at Stage 3. Stating it once stops an implementer creating the crate
early to satisfy a probe.

**MEDIUM — `11` pins sixteen icons "so the set does not grow by improvisation" and the set was
already wrong when it was written.** The four parallel documents need at least: **foliage/species**,
**send**, **approve**, **reject**, **render/thumbnail** and **project**. Twenty-two, not sixteen.
The four geometry rules (16 pt box, one 1.5 pt weight taken from `WidgetVisuals`, three primitives,
2 pt sub-grid) are the right thing to pin and I would not touch them. **Fix:** pin the *rules* and a
*budget* (≤ 24, and the twenty-fifth is a conversation), not the list.

**MEDIUM — `09`'s journal labels agent writes "cli" unless something nobody named sets
`$LOOM_ACTOR`.** §3.1 takes `actor` from `$LOOM_ACTOR`, default `"cli"`. The panel sets `LOOM_AGENT=1`
(§6) on the process it spawns; nothing sets `LOOM_ACTOR`. And §4/§9 insist — correctly — that the
agent must work identically from a terminal with no editor open, which is precisely the case where
no wrapper exists to set it. So the History row that the whole journal exists to produce reads
`cli · Block out office: 14 nodes`. **Fix:** derive the actor from `LOOM_AGENT=1` (already set, and
already the sandbox marker), with `$LOOM_ACTOR` as an override for anything else. One expression.

**MEDIUM — `LOOM_AGENT=1` refusing `--allow-destructive` is inherited by everything the agent runs.**
§6's marker is set on the spawned process and therefore on its whole subtree — including a project
script, a test, or `cargo xtask` if an agent ever runs one. Verified good news: **nothing in
`xtask/`, `scripts/` or `tests/` applies `RemoveNode` or `RemoveComponent` through the CLI today**
(the only non-design hits in the repo are `crates/loom_cli/src/{panels,run}.rs`), so §10.2's worry
is settled negative and the gate is safe on the day it lands. The mechanism is still wrong in shape:
a refusal for a script the human deliberately started is a dead end with no proposal to review.
**Fix:** apply the document's own better idea consistently — under `LOOM_AGENT=1`,
`--allow-destructive` **proposes** rather than refuses. Same queue, same card, no dead end.

**LOW — `09` §7a's "Reload, saving my version to `quay.mine.loom`" writes a `.loom` file into the
project that nothing declares.** It is the right button and the sentence it carries is right
(*"Undo restores the scene; the file stays"*). But a stray `quay.mine.loom` beside `quay.loom` will
be picked up by `12`'s `project::scenes()` glob and listed in the hub as a scene, forever, with no
way to tell it from a real one. **Fix:** write it to `<scene>.mine.loom` **and** have `scenes()`
skip `*.mine.loom`, or write it under `$XDG_STATE_HOME` and put the path in the console line. The
first is better — the user wants to `diff` it — and it is one glob exclusion in a function
`12` §2 is already writing exclusions into.

**LOW — `10` §3's `texels_per_meter = 2.0` is reasoned from `CLUMP = 0.5` and the reasoning cuts the
other way at the stroke edge.** One texel per clump is right for the *interior*, where the clump
makes sub-half-metre control invisible. At the boundary, §2.3's whole point is that the edge must
read as ragged rather than mown, and a 0.5 m texel quantises the ragged edge to 0.5 m steps —
which is the same size as the break-up amplitude the section specifies (12% of a 3 m radius = 0.36 m).
The break-up and the raster resolution are within a factor of 1.4 of each other, so the noise may be
entirely swallowed by quantisation. **Fix:** state it as an open pair rather than as a derived
default — `texels_per_meter` and `FOLIAGE_EDGE_BREAKUP` are tuned together against
`cargo xtask flythrough` on `foliage`, and §13's honest "reasoning, not measurement" note should say
which measurement.

**LOW — `12` §2's `[ship] exclude` duplicates part of `PLAN.md` S14's fixed list.** S14 already
excludes `docs/`; the proposed engine-repo exclusion list names `docs` again. Harmless, and worth
one word so the two lists do not drift into disagreeing.

**LOW — `09` never says where the conversation transcript lives.** §8 adds it to `PLAN.md` §2.6's
"user state, outside the project" bucket and §10.5 guesses at retaining 200 turns, but no path, no
key, no rotation policy — while the journal, the proposals and the context file all get all three.
One row in `PLAN.md` S9's table.

---

## 4. What these four documents get right

`10` §7.1's capacity table, `11` §2's double-encode analysis and `12` §1's three-namespace audit are
the three best pieces of work in the entire editor design set, round 1 included. Each takes a thing
everybody assumed, reads the source, and produces a number that changes the design — a 43 m ceiling
nobody knew about, a 14.6:1 that is really 6.7:1, and an engine-owned asset addressed as though the
scene owned it. `10` §11's dependency-rule catch is verified correct (`scripts/check-deps.sh:26-31`
permits `loom_scene → loom_reflect` and nothing else) and would have failed green check 1 on the day
ADR 0027 landed as written.

`09`'s central insight — that `Session::reload` clearing the undo stack (`edit.rs:395-406`) makes
the flagship "a twelve-op agent transaction undoes in one Ctrl+Z" **false for every transaction that
arrives through the file watcher** — is the single most valuable finding in round 2, and
`adopt_external` is nine lines. `11` §7's cased-stroke rule and `12`'s refusal of a project-relative
fallback are both correct and both correctly argued from this project's own precedents rather than
from taste.

---

## 5. What I could not check

No `cargo` command was run. Beyond that:

1. **Whether a diff actually fails to fit in a 380 pt column** (§3, the agent panel's home). I
   measured it as characters against `Monospace` 12 and a 96 px thumbnail, not by rendering one. The
   argument is arithmetic; the conclusion would survive a factor of 1.5 either way, but it should be
   looked at once the dock exists.
2. **Whether flipping `srgb_framebuffer` visibly thins text.** This is the strongest claim in §3 I
   cannot settle without running the probe, and it is why the fix is *extend the probe*, not
   *do not flip*.
3. **The mid-grey viewport case for cased strokes.** `#4A4E52` is a plausible overcast sky, not a
   sampled one. I did not render a Loom scene and sample its sky.
4. **Whether `instance_picks` (§1.4) costs anything at 5,000 instances.** It is a linear ray-box
   loop the size of `scatter_objects`' output; `pick_at_cursor` already does one over `picks`. I
   assumed the same cost class and did not measure either.
5. **Whether `loom propose --wait` can be made to work against a specific agent CLI's turn model.**
   §1.5 assumes a blocking tool call keeps the turn open. That is true of every tool-calling loop I
   know of, and `09` §10.3 is right that the shipped example must be checked against the tool's own
   `--help` before it reaches a documentation file.
6. **Whether the templates are small enough to compile in** (§1.3). I checked that
   `assets/templates/` does not exist yet (`ls assets/`), so I sized them from doc 02's description —
   a `loom.toml`, a scene and `fps.rhai` — rather than from files. If a template ever wants a mesh
   or a texture the argument needs revisiting, and the answer is then `$XDG_DATA_HOME/loom/` plus an
   install step, not the cwd branch.
7. **The exact `GOLDEN` count at Stage 5** (§3, `12` V2). I read `PLAN.md` §2.8's 28 → 32 and the
   stage each addition lands in; I did not re-derive it from `xtask/src/main.rs`.
