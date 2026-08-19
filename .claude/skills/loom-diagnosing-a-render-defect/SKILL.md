---
name: loom-diagnosing-a-render-defect
description: Use when something in Loom looks wrong, runs slow, or a plausible fix produced no measurable change — before writing the fix. Covers building a probe you can trust, separating symptom from cause, and why comments and briefs are hypotheses.
---

# Diagnosing before fixing

Generic debugging discipline you already have. What follows is the part this
engine charges for.

## 1. Reproduce headless, at the exact gate arguments

```bash
./target/release/loom render <scene> --out /tmp/a.png --size 320x200 --sim <n>
```

Note the tick, the size, and whether the scene has a reference. **A scene whose
subject is a particle or a fluid is empty at tick 0.** Reproducing at different
arguments than the gate means you are debugging a different picture.

## 2. Separate the symptom from the cause before proposing a mechanism

`f174afe`: the frame rate was blamed. The bug was one `/ wsum` in
`fluidProlongMain` breaking the multigrid V-cycle's contraction. Fixing it took
`plough_cinematic` from 33.03 s to 3.58 s per run and peak density from 415× to
3×. The commit's own line: **"the frame rate was the symptom, the picture was
the bug."**

`573cbca` is the same shape one level up: a report blamed the CPU marching
cubes; two thirds of the number was host-visible memory on the wrong side of
PCIe.

## 3. Build a probe, then fault-inject the probe

The solver reached 415× rest density and 33 ms/tick **while all five green
checks passed** (`65a219b`). No instrument could see it. Its predecessor probe
— 232 lines copying solver buffers by hardcoded index — *"is exactly the class
of instrument that has already produced two wrong diagnoses in this effort."*

So:

- **Route the probe through the one function every path already calls.**
  `FluidSolver::density` is the worked example; a hand-indexed buffer dumper is
  not.
- **Print the quantities the suspected failure needs, not the ones that are
  easy.** Peak density alone cannot see a uniform ratchet, which is why `wet`,
  `mass` and the wet centre of mass were added.
- **Fault-inject it.** Re-introduce the bug and confirm the number moves.
  Restoring the missing `/wsum` takes the probe from `peak=3.0×` to `192×`.
  A probe that has never been seen to move is a decoration.
- **Ask what the probe actually answers.** `80463ba`: the human's note was "the
  foam is created before the cube even hits the water"; `fluidProbeMain`
  answered *"is there water nearby"* while both readers meant *"is this pontoon
  in the water"* — a presentation bug that was quietly also a force-path bug.

## 4. State what would falsify the mechanism, and run it

**A mechanism that is equally present before and after the fix is not the
cause.** ADR 0057's Addendum 5 retracts Addendum 4's entire mechanism for
failure 3: `psolid` was 0 before the fix as well as after, so the proposed cause
was never active (`d4949f4`). The retraction is written as an addendum that
says what it retracts, not as a silent edit — see `writing-a-loom-adr`.

Ablation is the attribution tool; check the ablation is legal first (see
`loom-taking-a-measurement`).

## 5. Comments, briefs and ADR bodies are hypotheses

Read the code that runs.

- `604f87e`: the comment above `BoxCollider.half_extents` said world scale; the
  code returned raw. A 0.7 m crate collided as a 2 m box and 23×'d the
  compression under investigation.
- `65a219b` deletes a false doc comment about `bcc`. `f38d06d` is titled *"two
  live bugs, three lying comments"*.
- A companion design doc's recipe is a hypothesis about *this* engine, not an
  instruction (ADR 0002). Precedence: CLAUDE.md > `docs/decisions/` (newest
  applicable) > the brief > companion docs.

When the brief is wrong on contact, **record the correction where the next
reader will look**, and report it as a finding rather than working around it.

## 6. Before believing a null result, find the other copy

"No change at all" from a plausible fix almost always means the code exists
twice. `renderer.rs` ↔ `viewer.rs` is the standing pair and has shipped this
class four times — see the `render-in-both-paths` skill, which owns it. The CPU
twin ↔ Slang half (`loom_field`) is the other one.

## 7. Move the camera before calling it done

Every one of the three shipped instances of the hard-analytic-boundary defect
class was found by orbiting off the gate framing, never by a gate
(`WATER-REBUILD-REVIEW.md` defect 7). `--yaw/--pitch/--dolly`, and the
`watch-loom` skill for motion. `loom-judge-a-slice` has the full sweep.
