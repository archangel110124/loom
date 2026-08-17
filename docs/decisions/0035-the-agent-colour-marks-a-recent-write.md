# ADR 0035 — The agent colour marks a recent write, never an owner

- **Date:** 2026-08-16
- **Status:** **accepted**
- **Decision touched:** the editor's visual language
  (`docs/design/editor/11-visual-identity.md`, which asks for an
  "agent-authored versus human-authored" state and gets told it cannot have
  one). No locked decision in CLAUDE.md moves. The `.loom` format does not
  change, so `FORMAT_VERSION` (`crates/loom_scene/src/scene.rs:18`) stays at 1.
- **Plan row:** `docs/design/editor/PLAN.md` §3's row **0035** — *"The editor colours recency, not authorship"*.

## Context — the editor was asked for a state it cannot know

The visual-identity work enumerated four row states the chrome must express:
selected, hovered, dirty, and *agent-authored versus human-authored*. The first
three are facts the editor holds. The fourth is not a fact anyone holds.

`loom_scene::ops::Transaction` (`crates/loom_scene/src/ops.rs:166-178`) carries
`label`, `ops`, `dry_run` and `expect_version`. There is no author field, and
there never was. The `.loom` file records no authorship either — `[scene]`
carries a `format` key and a version token and nothing about who typed a line.
The session's transaction log is `Vec<String>` of labels
(`crates/loom_scene/src/edit.rs:239`, pushed at `edit.rs:307`), surfaced to the
History panel as `history: &'a [String]`
(`crates/loom_editor/src/panels.rs:90`). A label, not a name.

What the editor *does* have is one inference, and it is worth stating precisely
because every tempting extension of it is a lie. `poll_file`
(`crates/loom_cli/src/run.rs:676-719`) re-reads the scene every 250 ms
(`run.rs:50`) and compares its version token against `disk_seen` — *the version
we last read or wrote*, deliberately, because comparing against our own
in-memory text would flag every unsaved keystroke as somebody else's write
(`run.rs:685-690`). When the token differs, `show_external` (`run.rs:497-520`)
diffs the node list before and after via `SceneView::changes_from`
(`crates/loom_cli/src/scene_view.rs:210`) and stamps each change with
`Instant::now()` (`run.rs:518`).

So the signal is: **the file on disk changed to something this process did not
write, and these are the nodes that differ.** It is not "the agent did this".
It is equally true of a text editor, a `git checkout`, a `cp`, or a second
`loom` process. It is session-local, it is decayed, and it is gone on restart.

## What was decided

**The `agent` colour means "written recently, by something that was not this
editor". It never means ownership.**

- `agent_marks` (`run.rs:443-487`) drops any change older than
  `CHANGE_FADE = 6.0` seconds (`run.rs:440`, `run.rs:449`) and hands the
  survivors a `freshness` of `1.0 - age / CHANGE_FADE` (`run.rs:483`).
  `agent_overlay` (`crates/loom_editor/src/panels.rs:674-706`) turns that into
  alpha — `freshness * 220` (`panels.rs:690`) — so the mark is a box that
  fades, not a badge that stays.
- The token's doc comment carries the rule at the point of definition
  (`crates/loom_editor/src/theme.rs:103-109`), because a colour constant is
  where a future reader looks before deciding what it is allowed to tint.
- The hue is unchanged from the `(120, 200, 255)` the viewport has drawn since
  M12 — `theme.rs:144` is `0x78C8FF`, and `panels.rs:691` is the same colour
  written as a decimal literal. Restyling did not get to redefine a meaning a
  user has already learned.
- **Human authorship gets no colour at all.** Rows the human wrote render in
  the default `text`. This is not an omission for later: you do not need to be
  told which parts of your own file are yours, and a "mine" tint would put a
  second permanent hue on every row in the hierarchy to convey nothing.
- `Transaction` gains no author field, and the `.loom` format gains no
  provenance key.

The last point is the one this ADR exists for. A scene file describes a scene.
A provenance field in it would be a second source of truth that is wrong after
the first `git merge`, the first `cp` of a prefab, and the first hand edit —
and unlike a stale comment it would be *rendered*, so the editor would confidently
tint a node the human rewrote line by line as the agent's.

## What was rejected

**A persistent "agent-owned" tint.** This is the shape the visual-identity task
literally asked for, and it is why this document exists: it looks like a
one-field change and is not. It needs authorship in the file, which is a
`format` bump from 1, a migration for every scene in `assets/`, and a field
that every hand edit silently invalidates. It also makes the palette carry a
claim the engine cannot verify — never-do #15's instinct, that the human's
authored work is the thing you must not misrepresent, applies to describing it
as much as to overwriting it.

**Authorship in the transaction log only.** Cheaper — no format change — and
worse than either honest answer, because it survives a session and not a
restart. The same node would be tinted or not depending on when the editor was
opened, which teaches the user that the colour means nothing.

**Inferring authorship from git blame.** An authored scene is often not
committed at all, a squashed commit erases the boundary, and every agent
transaction in this project is committed by the human anyway. It would attribute
the whole file to whoever ran `git commit`.

**Widening the existing inference into a claim.** `changes_from` already knows
*what* changed; it would be one line to keep the list forever instead of six
seconds. That is the trap: the data structure supports it and the meaning does
not.

## What this costs

- **A node the agent created last week looks exactly like one you created last
  week.** That is the truth, and it is also a real loss of a feature someone
  wanted. Reviewing an agent's work after a restart means reading the diff, not
  scanning for blue.
- **The mark is attributed wrongly on purpose when it is wrong at all.** Saving
  the scene from a text editor draws "agent" boxes. The colour's name is
  therefore slightly ahead of its meaning, which the token comment says out
  loud rather than renaming the token and breaking every reference to it.
- **A removed node gets no mark.** It has no bounds left to project
  (`run.rs:453-457`), so the console line is the only signal for a deletion —
  the least visible change is the one with the weakest surface.
- **`freshness` is wall-clock decay in the presentation loop.** It reads
  `Instant::now()` under a scoped `#[allow(clippy::disallowed_methods)]`
  (`run.rs:516-517`, with the standing justification at `run.rs:405-411`),
  which is correct — never-do #8 governs simulation only — but
  it does mean the overlay is not reproducible frame by frame and no golden
  image can cover it.
- **The History panel is the only durable provenance surface, and it is weak.**
  It lists this session's own transaction labels; an external write never
  reaches it. Anything better is a scene journal, which is a different
  decision with its own storage.

## What it forecloses

Nothing structural. Every richer answer stays available, because refusing to
put provenance in the scene file is what keeps the file honest rather than what
blocks a journal. A future scene journal — an append-only sidecar of who
applied which transaction — can supply real authorship without touching
`format`, and would be the thing that earns a persistent tint. This ADR asserts
only that **the palette must not pretend to have that data before it exists**.

The narrow rule to keep: the `agent` token may colour anything the editor can
prove happened recently and did not cause itself. It may not colour anything
described as ownership.

## How it would be reversed

By landing the storage first, in this order: a provenance source with a defined
lifetime (a journal sidecar, or an author field with a `format` bump and a
migration), then a rule for what a hand edit does to it, then the tint. The
palette change is last and is three lines. If a future reader finds themselves
writing the tint first and the storage never, that is exactly the failure this
document was written to catch.

## A note on the design doc's citations

`docs/design/editor/11-visual-identity.md:229` cites `ops.rs:102-114` for
`Transaction` and `:231` cites `run.rs:426-465` for `agent_marks`. Both have drifted; the
shipped locations are `ops.rs:166-178` and `run.rs:443-487`. The doc's claim is
correct — the struct still carries no author — but the line numbers are not,
and this ADR's are re-read from the source.

## Human approval

Not required: no locked decision in CLAUDE.md moves, no format version changes,
and no shipped colour changes value. Approval *would* be required for the
rejected option, since adding a provenance key to `.loom` is a format change.
