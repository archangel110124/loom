# Building DEEPER together

The loop, written down because it now exists and is not obvious from the code.

## What you do

```
loom run assets/games/deeper_demo.loom --edit
```

Select something. Open the **Agent** tab. Type what you want and press Send
(or Ctrl+Enter). The panel shows what your request will be *about* — the
selection travels with it, so "make it sit lower" means something.

That is the whole of your side. Keep working; the answer arrives in the same
panel and the change arrives in the viewport.

## What I do

```
loom agent inbox assets/games/deeper_demo.loom      # what you asked
loom scene assets/games/deeper_demo.loom --get Rig  # what it is now
loom scene assets/games/deeper_demo.loom --tx x.json
loom render assets/games/deeper_demo.loom --focus Rig/Boat --dread 1.0 \
     --out after.png                                # what it looks like now
loom compare before.png after.png                   # did it actually change
loom agent reply assets/games/deeper_demo.loom --id 1 --text "..."
```

`--focus` is not optional politeness. Framed on the whole scene, the first real
request through this loop measured **0.01%** of pixels changed and read as a
change that did nothing; framed on the boat, the same edit measured **1.06%**
and the answer was plainly visible. ADR 0102.

You can drive your half from a terminal too, if you are not in the editor:

```
loom agent ask assets/games/deeper_demo.loom --text "..." --about Rig/Boat
```

The edit goes through `loom scene --tx`, which is **the same op path your
buttons use**. So anything I change is one Ctrl+Z for you, appears in History,
is validated by the same schema, and shows up in the viewport within 250 ms
because the editor polls the file four times a second.

## Why this shape

Your editor and my CLI write through one transaction path. That is unusual —
Unity, Unreal and Godot bolt an assistant on beside the editor's own edit path,
and then the two disagree. Here there is one, so:

- My change is undoable with your Ctrl+Z.
- My change appears in your History panel, labelled.
- A change that would break the scene is refused for me exactly as for you.
- Nothing has to be reconciled, because nothing diverged.

The conversation is two files beside the scene — `.loom-agent/inbox.jsonl` and
`outbox.jsonl`, gitignored, one JSON line per message. If the loop misbehaves,
read them.

## What to say

Anything that names a thing and a direction. The requests that work best carry a
selection and an outcome rather than a field name:

- *"the fog is too thin, make it heavier so the far shore disappears"*
- *"this crate should sit on the deck, not through it"*
- *"the boat rolls too much in the swell"*

I will find the field. If I get it wrong you have Ctrl+Z, and the reply says
what I changed and what I left alone.

## When you are away

Requests queue. `loom agent inbox` returns everything unanswered, so a session
that starts hours later picks up where you left off — the ids are stable and
the transcript is the whole history.

## What this does not do yet

- **No approval step.** My edits land and are undoable, rather than waiting for
  you to accept them. A propose-review-accept flow would use the dry-run diff
  `loom scene --tx --dry-run` already produces, and is the obvious next thing.
- **Nothing answers automatically.** The editor writes a request; something has
  to be watching. That is a session of mine, not a daemon.
- **A transform edit during Play will not stick** — the snapshot restores where
  things were. Tune parameters while playing; move things while stopped.
