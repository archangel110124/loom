---
name: loom-brief-a-builder
description: Use when splitting Loom work across subagents — dispatching a design pass, a builder, or parallel agents; writing the brief; deciding what an agent is allowed to run. Covers the orchestration failures that recurred here, each with its cost.
---

# Briefing a builder on Loom

This project runs multi-agent constantly and the same orchestration bugs
recurred across rounds, each with a named price. All of these are logged in
`docs/design/OVERNIGHT-DECISIONS.md`.

## 1. One agent, one kind. Say which in the first line.

- **Design / research agent** — read-only. Give it an **explicit read-only
  instruction, or its own worktree. Never a shared one.**
- **Builder** — commits.
- **Judge** — see `loom-judge-a-slice`.

**D19**: design agents were briefed to verify claims and render their own work.
An agent that can render will also edit. Two ran in one worktree, the judges
reported *"two reports of the same shared worktree"*, the actual build agent
then died on a 529 with `built: null`, and **everything that survived was
committed by the designers**.

Never brief one agent to do two of these.

## 2. The brief is a hypothesis — say so in the brief

Require the agent to verify the premise against the source and `git log` before
building, and to **report a false premise as a finding rather than working
around it.**

**D17**: a work item was briefed on a false premise — ADR 0020 already *was* the
flame rewrite being commissioned. It was saved only because the design phase
read the source instead of the brief.

Point the agent at `docs/design/README.md` and ADR 0002. A research document
that predates the work it recommends reads as current forever unless someone
marks it. Precedence: CLAUDE.md > `docs/decisions/` (newest applicable) > the
brief > companion docs.

## 3. What a builder may run — this is a lock and a cost, not a preference

**May:** `cargo clippy`, `cargo test`, `cargo check`, `scripts/check-deps.sh`,
and the un-locked hand-equivalents in `tools/` (`goldcheck.sh`,
`bytecheck.sh`, `repeatcheck.sh`, `watch.sh`).

**May not:** any `cargo xtask` gate (`validate`, `image`, `flythrough`,
`shimmer`, `repeat`), and never `--bless`.

**D4**: builders were queued to run `cargo xtask validate` and burned ~30
minutes of wall clock; the gates hold a cross-worktree singleton lock.
`fb9ea5d` is the other half — six gates ran at once and reported a scene at
44.8 ms against a 25.2 ms truth. **The verifier owns the gates and the
blessing.** State it in every brief.

## 4. Measurement hygiene, in the brief

- **One agent measures at a time.** `slosh` measured 13.5 ms/tick standalone
  and 222.6 ms/tick with clippy loading the CPU; the overnight log also records
  timings taken while a sibling held the GPU.
- Name the contention condition in every timing.
- Re-measure serially on an idle GPU before any number goes into a doc or an
  ADR.
- Per-tick and per-frame are different quantities and must be labelled.

Hand the agent the `loom-taking-a-measurement` skill rather than repeating it.

## 5. Make the commit the deliverable, not the report

**D12**: a builder crashed on `StructuredOutput retry cap (5) exceeded` — seven
required report fields, three of them long prose — **after** committing. The
work was recoverable only because the commit message carried what the report
would have.

So: commit messages carry the measurements, the rejected hypothesis and the
scene list. The structured report is a summary, kept small enough to serialise.
**A brief that demands a large structured report is a brief that can lose the
work.**

## 6. Parallel agents: pre-allocate every shared namespace

ADR numbers, struct fields, enum variants, file paths — or serialise. Parallel
agents cannot see each other's output and will collide on exactly these. Round
2 of the editor design had four documents each claiming ADR 0033; **round 3,
whose whole job was to not repeat that, did it again.**

## 7. Before going idle, schedule a wake-up

Waiting on a background agent cost one run five hours and twenty-three minutes,
costed at zero. Serialise or set a timer.

## A brief that works, in shape

> **Role:** builder, commits, own worktree `<name>`.
> **Premise (verify before building):** …, from `<file>` / `<commit>`. If it is
> false, stop and report that as the finding.
> **Do:** … **Do not:** run any `cargo xtask` gate or `--bless`; the verifier
> owns those.
> **Namespaces reserved for you:** ADR 00NN, `<field names>`, `<paths>`.
> **Verify with:** clippy, `cargo test`, `bash tools/goldcheck.sh <rows>`,
> `bash tools/repeatcheck.sh <scene> <args>`, and open the PNGs.
> **Report:** short. Put the measurements, the rejected hypothesis and the
> scene list in the commit message.
