# 0107 — Copy a component, not a field

Status: accepted · 2026-09-08

## Context

Material editing in the Inspector was already real: an sRGB-correct colour
swatch over a linear `albedo`, metallic and roughness on schema-ranged sliders,
`albedo_map` and `normal_map` as dropdowns of what the scene declared,
`uv_scale`, `triplanar`. All live, all through the ordinary op path.

What was missing was not a field editor. It was **carrying values from one node
to another**. The demo boat has fifteen material nodes; matching one to another
meant reading five numbers off the first and typing them into the second, which
is how two things that are supposed to be the same drift apart.

## Decision

Every component header gets **copy** and **paste**. Copy remembers the type name
and the whole table; paste appears only on a node while a component of that type
is held, and writes every field in **one transaction**.

- **One transaction over every field, not a component swap.** There is no op
  that replaces a whole component, and adding one to carry a clipboard would be
  a second way to write a component — the thing this editor's design has spent
  its whole life avoiding. `SetField` per key is the ordinary path, and the
  fields arrive as one undo step because a transaction is one.
- **The whole table, never part of it.** A partial paste would leave the target
  reading as neither one thing nor the other.
- **Generic, not Material-only.** The same code carries a `Buoyancy` setup or a
  `Light`, and a special case for materials would have been the same size.
- Separate from the existing node clipboard: pasting a node and pasting a
  material are different gestures onto different targets.

## Consequences

Paste creates the component when the target does not have one — `SetField`
writes the table — so it doubles as "make this node like that one".

Verified headlessly by building the transaction the button builds and running it
through `loom scene --tx --dry-run`: five fields onto a node with no `Material`
at all produced the component and its five keys.
