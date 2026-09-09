# 0110 — A widget model for the HUD

Status: accepted · 2026-09-09

## Context

ADR 0109 closed the scripting gap and found that two of the three "structural"
absences were less absent than five reviews had said. The one that survived was
real: **rich UI**. `Hud` was a line of text and nothing else, so a game could
say *"OXYGEN 84%"* and could not draw the bar next to it, or a backdrop under it
so the words survived a bright sky.

`hud.rs` draws a title menu, a pause menu and an inventory grid, all from Rust,
none of it authorable. Every game built on this engine would get the same three
and no fourth.

## Decision

**A `kind` on `Hud`, not three components.** `text` (the default), `bar`,
`panel`.

A bar and a backdrop share everything that makes a HUD element one: the anchor,
the inward offset, the two visibility flags, the colour, and being collected by
`World::hud_elements`. Three components would have been three copies of that,
three registry entries to keep in step, and a scene wanting a labelled bar would
carry two *nodes* rather than two components. `kind` defaults to `text`, so
every scene authored before this renders byte for byte as it did.

Five new fields, all defaulted, all schema-described — so the Inspector edits
them, `loom describe` documents them and the agent can write them, with no
per-type UI code anywhere:

- `extent` — width and height in points, for a box. Text is sized by its glyphs.
- `value` — the game-state number a bar fills from. **Bare, not braced**: `text`
  interpolates `{name}` into a sentence, and a bar has no sentence.
- `range` — what that number means at empty and at full. Authored rather than
  assumed `0..1`, because a game keeps oxygen in seconds and a hold in
  kilograms, and a bar assuming 0..1 would be full from the first tick of every
  game ever written.
- `opacity` — a backdrop is the one HUD element that must not be solid: the
  point of it is to make text legible without hiding the game.

### Three things that had to be decided, not defaulted

**A missing number reads full, not empty.** A bar that vanished because the
rules script had not written `oxygen` yet would look like a broken bar rather
than an unstarted game — and in the editor, where there is no game at all, every
bar would be an invisible rectangle nobody could find to move.

**Panels, then bars, then text.** A backdrop authored after the line it backs
would otherwise paint over it, and *"why did my label disappear when I added a
panel"* is a question a scene author should never have to ask once, let alone
each time. Within a kind the scene's own order is kept, so two panels still
stack as written. Fault-injected: with the sort removed, the test that asserts
the panel is painted first fails on the text's 64-point rect.

**A box grows inward from its anchor, exactly as text does.** The anchor names
the corner the offset is measured from. Centred on that point, or grown
down-right from it regardless, a corner panel sits half off the screen — four
cases per axis, and the sort of thing that looks right in the one corner it was
written against. Tested at three of them.

## Consequences

The authoring loop is complete for a HUD: Add Component → `Hud`, pick a kind
from the dropdown the enum gives for free, drag the offset, pick the colour, and
watch it in the viewport — `run.rs` composes HUD elements in edit mode as well
as in Play. Photographed: a translucent panel with a label and two bars, blue
and amber, authored entirely through `loom scene --tx` and visible in the
stopped editor.

**Still not authorable: anything that takes input.** A button is not another
kind — it needs a press to reach the simulation on the fixed tick, the way key
input already does, or it breaks the property that a run reproduces (ADR 0045,
0091). That is an input-path decision rather than a drawing one, and pretending
otherwise by adding a `button` kind that mutates state off-tick would trade a
missing feature for a determinism hole. The title and pause menus stay in Rust
until that decision is made.

The proof here is a screenshot and four unit tests rather than a gate row.
`loom render` builds no egui context, so no golden PNG can see a HUD at all;
only `loom run --shot` can, and green.sh's overlay row is scoped to the demo's
own text band. Extending it would mean putting these widgets into the shipped
demo, which is the game's author's decision and not this ADR's.
