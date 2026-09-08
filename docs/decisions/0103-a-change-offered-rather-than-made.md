# 0103 — A change offered rather than made

Status: accepted · 2026-09-08

## Context

ADR 0100 gave the editor a box to talk to the agent from, and the agent's edits
appear in the viewport within 250 ms because the scene file is polled. That is
the right latency for a human sitting in front of it and the wrong one for a
human who is not.

The loop is asynchronous by design — you ask, and go back to work. So the
normal case is that an edit lands while the person who asked for it is looking
at something else, and they meet it by noticing the scene changed. Undo covers
the mistake; it does not cover the surprise.

`docs/design/WORKING-TOGETHER.md` named this as the obvious next thing, and the
editor critic named it too.

## Decision

An agent may answer with a **change instead of a sentence**.

`loom agent propose <scene> --id <n> --tx <tx.json>` records the transaction in
the same outbox line as the reply, with the diff it produced. The editor's Agent
panel shows the diff with **Apply** and **Discard**. The decision is written
back as a human line in the inbox, because accepting a change is something the
person did.

Four properties, each load-bearing:

- **Dry-run at propose time.** A transaction that cannot apply is refused where
  the agent is, and nothing reaches the conversation. Otherwise the human reads
  a diff, presses Apply, and the agent's mistake arrives in their hands as
  though they had caused it.
- **The diff shown is the diff stored is the transaction run.** No summary
  anywhere in the path. A summary of a diff is a second thing to get wrong, and
  the human is being asked to approve the change, not a description of it.
- **Apply goes through `transact`.** The proposal file holds a transaction until
  somebody says yes; it is not a second way into the scene. So Apply is one
  History entry, one Ctrl+Z, and the same validation as a gizmo drag — and it is
  gated by `structure_editable` like every other structural edit.
- **A stale proposal still applies.** `expect_version` is deliberately dropped.
  A `SceneOp` writes an absolute value and never a delta, so an offer applied
  ten minutes late says exactly what it said when written. An op that no longer
  makes sense is refused by the ordinary validation.

## Consequences

Proposing is opt-in per answer, not a mode. An agent working alongside someone
who is present should keep applying directly — that is the tighter loop, and it
is why the editor polls at all. Proposing is for the case where nobody is
watching, which is most of them.

The Apply and Discard buttons sit **above** the diff. With them below, a short
panel pushed Apply off the bottom edge: the one control the block exists for,
invisible, in the first screenshot taken of it. The diff is what gets clipped
now, and it scrolls.

Still missing: nothing answers on its own. The editor writes a request and
something has to be watching — a session, not a daemon.
