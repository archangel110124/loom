# ADR 0100 — A box you can talk to the agent from

- **Date:** 2026-09-08
- **Status:** **accepted** and built.
- **Adds:** `crates/loom_cli/src/agent_link.rs`, `loom agent inbox|reply`, an
  Agent panel that is a conversation rather than a log, and `--tab` / F2 rename.
- **Human decision this records:** *"agent integration is the biggest key since
  we will need a section where I can send information to the AI and it will
  update the scene in real time."*

## 1. Half of it already existed, in the wrong direction

An agent editing the scene already reaches the viewport in 250 ms: `poll_file`
re-reads the scene four times a second, reloads, refuses to merge over unsaved
edits (§7.17), draws fading marks over whatever changed, and logs it in the
Agent panel.

So "the agent changes the scene while you watch" was done. What did not exist
was any way to **ask**. The Agent panel was a read-only log of somebody else's
work.

## 2. Decision

Two append-only files beside the scene:

```text
<scene-dir>/.loom-agent/inbox.jsonl    what the human asked for
<scene-dir>/.loom-agent/outbox.jsonl   what the agent said back
```

The Agent panel gains a prompt, a Send button, Ctrl+Enter, and the transcript.
`loom agent inbox <scene>` prints what is unanswered; `loom agent reply <scene>
--id <n> --text <...>` answers one. The agent does the actual work with
`loom scene --tx`, and the watch already running shows it.

**The selection travels with the request.** "Make it sit lower" is not a
sentence about anything until it carries what was selected when it was typed,
and an agent reading the inbox has no other way to know. The panel says which
nodes it will send, above the box.

## 3. Files, not a socket and not an API key

Three reasons, in order of how much they mattered.

**It needs no dependency.** This project has spent a lot of care not acquiring
any — `loom_net` has none, the pack format is hand-rolled rather than a zip
crate, there is no file dialog. An HTTP client and a JSON-RPC transport to talk
to a model would be the largest dependency in the editor.

**It works with the agent the human actually has.** That is a Claude Code
session in this repository, which can already read a file and run a CLI. A
socket would need something written on the other end before anything worked;
this works today, with the tool that is already open.

**It is inspectable.** When the loop misbehaves, the conversation is two text
files you can read. That is not true of a socket, and it is the difference
between debugging this and guessing at it.

One line of JSON per message, appended and never rewritten, so a reader and a
writer cannot corrupt each other without a lock. A torn line is one lost
message rather than a lost conversation, and there is a test for exactly that.

## 4. What this buys that the big three cannot copy cheaply

Unity, Unreal and Godot have bolted assistants on: a chat box that emits code
you paste. Here the agent drives **the same transactions the UI does** — one
edit path, so an agent's change is undoable with Ctrl+Z, appears in History,
coalesces like a gesture, and is validated by the same schema. The human's
half was the only missing piece, and it was a panel, not an architecture.

## 5. Two things fixed on the way

**`BeginRename` was a dead verb** — declared, handler `=> {}`, emitted by
nothing. It was the one facade left in a 40-verb surface, found by diffing what
the UI can emit against what the app handles. It is now in-place rename in the
hierarchy, on F2 and in the context menu, because renaming happens where the
name is and nobody looks in the inspector to rename a node they are looking at.

**`--tab` was the last mouse-only thing.** Every other part of the editor could
be set up headlessly — `--select`, `--mode`, `--view` — but not which panel was
in front, which meant the Agent panel could not be screenshotted or gate-checked
by anything but a human clicking. That is the same reasoning that has caught two
shipped facades in this editor already.

## 6. What this does not do

- **No model is called from the editor.** The editor writes a request; something
  else answers. That is the point of §3, and it means the loop is only as live
  as whatever is watching the inbox.
- **No streaming.** A reply appears when it is written, whole.
- **No approval queue.** The agent's edits land in the scene and are undoable,
  rather than waiting to be accepted. A propose-review-accept flow is the
  obvious next step and wants the dry-run diff the CLI already produces.
