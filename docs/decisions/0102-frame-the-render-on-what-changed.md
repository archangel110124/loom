# 0102 — Frame the render on what changed

Status: accepted · 2026-09-08

## Context

The agent loop closed in ADR 0100 (the conversation) and 0101 (the read path).
Dogfooding it end to end on a real request — *"put a lantern on the bow of the
boat so it lights the water ahead at night"* — turned up a gap neither of those
covers, and it is the one that matters most.

The change applied cleanly. `loom scene --tx` reported ok. The render reported
ok. `loom compare` said **0.01% of pixels changed** at the darkest mood stage.

Every step said success, and the honest reading of that measurement is that the
change did nothing. It had in fact worked: the lantern lit the foredeck hard.
The camera was framing the whole scene, where a boat is a few hundred pixels of
a coastline and a lamp on its bow is four of them.

This is the exact failure mode CLAUDE.md names first — a checkable fact that
was checked, against the wrong question. Worse, it is silent in *both*
directions: an agent that trusts the number under-reports a change that worked,
and an agent that ignores it will one day over-report one that did not.

## Decision

`loom render --focus <node>` frames the camera on that node and everything
under it, instead of the whole scene.

- It filters the same `node_bounds` map the whole-scene framing already
  reduces, so there is one definition of where a thing is, not two.
- A node with **no geometry of its own** — a light, an empty, a rig — is framed
  at its own world position with two metres of room. That is precisely the case
  an agent hits right after adding a light, so it must not be the case that
  fails.
- A node that does not resolve is **exit 2, `no_such_node`**. Falling back to
  the whole scene would render a correct picture of the wrong question and
  exit 0 would call the framing honoured.

Same request, same edit, same measurement, framed on the boat: **1.06% of
pixels changed, worst channel 224**. The lantern is unmistakable, and the
honest answer to the human — that it lights the deck and not the water — is
readable off the image.

## Consequences

The measure-don't-reason rule needs a companion: *measure the right rectangle*.
A diff over a frame the subject barely occupies is not evidence about the
subject. `--focus` is how an agent gets a frame the subject occupies, and
`compare --rect` remains how it narrows further within one.

`--focus` implies orbit framing, so it overrides an authored `Camera` the same
way `--yaw` does — asking to look at a thing is asking not to use the shot the
scene was composed for.
