# 0108 — The editor can author weather and water

Status: accepted · 2026-09-09

## Context

Asked plainly: *can a human put shapes in a scene, add and remove and tune rain,
wind and water, and change the FFT sea's numbers, from inside the editor?*

Most of the machinery said yes. `Rain`, `Wind` and `WaterBody` are registered
types, so Add Component offers them off the registry with no second list to keep
in step; the ✖ removes one; the Inspector edits fields generically with schema
ranges and enum dropdowns; Create offers five primitives. On paper, done.

Measured, three things were not.

## What was broken

**1. Add Component could not add rain or water at all.**

The menu writes every field's schema default. Four types of thirty have a field
whose default is `null` — `Rain.duration`, `WaterBody.{extent, fetch, flow,
swell}`, `Environment.cloud_type`, `Joint.limits` — and TOML has no null, so the
op layer refuses it as `unrepresentable_value`. A transaction is all-or-nothing,
so the refusal took the other fifteen `WaterBody` fields with it and the human
got a console line and no component.

The two types a person reaches for first were the two that failed.

A null default *means* the field is absent, and absence in TOML is a key that is
not written. So the defaults walk skips it. Pinned by a test over every
registered type — not "every type applies", because a `Joint` on a bare node is
legitimately refused and that is the validator working, but "no type emits a
value TOML cannot hold". Fault-injected: putting the null back fails it on
`Environment`.

**2. The FFT sea's numbers could not be edited.**

`WaterBody.swell` is `{direction, fetch, u10}` — the three numbers that *are*
the spectrum. It reached the Inspector as the literal text
`{"direction":[0.25881904,0.9659258],"fetch":420000.0,"u10":18.0}` in read-only
grey, and so did `optics`, `flow` and `waves`. The one thing this editor exists
to do is change a value, and for a whole class of fields it could only describe
one.

Nested objects are now edited as their own small grid, with the sub-schema
resolved through `$defs` so a nested number carries the same range and the same
tooltip it would at the top level. `null` says *not set* rather than printing
the word `null` in the grey a real value uses.

**3. Writing a nested object shredded the author's file.**

The inspector writes the whole object — one number changed means
`WaterBody.swell` is set to all three. `SetField` replaced the item, which
turned `[node.components.WaterBody.swell]` and the eight lines of comment above
it explaining why those numbers are those numbers into a one-line inline table.

That is exactly the reformatting this layer edits a format-preserving DOM to
prevent, and it would have happened on the first swell anyone touched. A header
sub-table is now walked and its keys updated in place, decor carried the same
way the scalar path already carried it; an inline table is still replaced as one
value, because that is one line whose own decor survives. Keys absent from the
object are removed, so "set" still means set rather than quietly meaning merge.

The `swell` diff went from twelve lines to one.

## Also

`loom describe` with no argument said, in its own usage text, that it *"lists
the known types"*. It printed the usage and exited 2. Which is a small lie about
a small command, except that "what can I add to this scene?" is the first
question an agent asks and the answer was nowhere. It lists them now, off the
same registry the menu reads.

The Create menu's five primitives were a copy of `loom_asset::primitives::NAMES`
with nothing holding them together — `loom_editor` cannot depend on
`loom_asset`, and acquiring an edge to share five strings costs more than it
saves, so a test in `loom_cli` (which sees both) asserts they agree and that
every offered name actually builds.

The component-copy button rendered as a tofu box: `⧉` is not in the bundled
font. It says `copy`.

## Consequences

An optional nested object that is absent still cannot be *created* from the
editor — `Swell`'s fields are deliberately "refused at load rather than
defaulted into silence", so there is no default a button could write that would
load. Copying the component from a node that has one brings it across, and the
CLI or the agent can author one. Inventing plausible physics numbers in a button
is the thing this project does not do.
