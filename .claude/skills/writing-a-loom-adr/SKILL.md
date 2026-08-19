---
name: writing-a-loom-adr
description: Use when about to change a locked Loom decision, refuse something a design doc or brief prescribes, defer something with a trigger, record a measurement that overturns a previous claim, or correct an ADR found to be wrong.
---

# Writing an ADR here

Template: `docs/decisions/0000-template.md`. Directory: `docs/decisions/`.

## When one is required

- Touching CLAUDE.md's locked-decisions table.
- Refusing something a companion design doc or a brief prescribes.
- Putting new state on the force path, or adding a GPU-stateful rendering path.
- Deferring something — with an explicit reopening trigger.
- Accepting a known miss.
- Correcting an ADR that measurement has falsified.

Anything smaller belongs in the commit message, which in this repo carries the
measurements and the rejected hypothesis anyway.

## Numbering

**`ls docs/decisions/` first. Never write the number from memory.** ADR 0018
carries this line in its own header:

> **Its ADR number is 0019 and is wrong** — 0017 was the highest on disk.

There were 43 ADRs sitting in that directory when it was written. Note also
that **the count is not the highest number**: 0023–0042 are a reserved range
held by the on-hold editor rework (ADR 0045 has a `Numbering` bullet about it),
so the newest ADR is 0059 while the file count is 43.

Parallel agents collide on this constantly — the editor design round 2 had four
documents each claiming ADR 0033, and round 3, whose whole job was not to repeat
that, did it again. If you are dispatching parallel work, pre-allocate the
numbers in the brief (see `loom-brief-a-builder`).

One ADR, one number. Corrections go inside it as addenda.

## Status and approval

- New ADRs land at **`proposed`**.
- **A builder never promotes its own ADR to `accepted`**, and never blesses its
  own references. `WATER-REBUILD-REVIEW.md` defect 10 filed exactly this as
  process debt: ADRs 0054–0057 all at `proposed` and unapproved while the force
  path shipped. `faf34ad` cleared the half a builder owns and left the rest,
  by name: *"Left to the verifier, all of it out of a builder's reach: human
  approval on ADRs 0054-0057, blessing the moved and new references…"*
- When the human approves, **record their words verbatim**. ADR 0053 does, and
  that is why its scope is not relitigable.

## What separates an ADR from a paragraph

1. **Rejected alternatives, with the numbers that rejected them.** ADR 0018
   killed ACES and PBR Neutral at 0.213 against 0.400 on the amber rung; ADR
   0052 killed grid wind forcing on a 3.08 m energy-weighted wavelength against
   1–10 cm ripples.
2. **A "what this does not settle" section.**
3. **For a deferral, an explicit reopening trigger.** ADR 0014 was a deferral
   with a trigger; ADR 0017 exists because the triggers fired and could be
   pointed at.
4. **A load-time refusal where one is possible.** ADR 0059 §2–3 and ADR 0047's
   four refusals are the pattern: the constraint ships as an error message with
   the required number in it, not as advice. An ADR that ends in "authors
   should be careful" has not finished.
5. **If it amends another ADR, say so in the header, by number and clause.**
   ADR 0053 is the only ADR in this project's history to amend a locked
   decision rather than state its edge, and it names ADR 0045 clauses 1 and 2
   and ADR 0046 explicitly.

## Amendment discipline — the part most likely to be skipped

- **An ADR falsified by measurement is corrected in the same commit that
  falsifies it.** The review panel required this of `d4b30b8`.
- **Strike the false claim through in place with a pointer forward. Never
  delete it.** ADR 0057's addendum table reads:

      | `slosh` | 600 | ~~byte-identical~~ **FALSE WHEN WRITTEN — see Addendum 2** |

- **A retraction says what it retracts.** ADR 0057's Addendum 5 opens
  *"Addendum 4's mechanism for failure 3 is retracted"* and then says why
  (`psolid` was 0 before the fix as well as after, so the mechanism was never
  active).
- **Shrink a claim rather than falsify it.** From the review's required
  changes: *"A false determinism claim in an ADR is worse than a narrow true
  one."* "Byte-identical through tick N, unstable past it" with a documented
  budget beats an unqualified claim that nine renders can break.

## Do not

Write a list or summary of existing ADRs anywhere. `ls` and `grep` do that, and
such a list rots in a week — ADR 0057 took six addenda in three days.
