---
name: loom-judge-a-slice
description: Use when reviewing or accepting Loom work you did not build — a slice, a phase, a branch, a subagent's report, a session hand-off — before it is merged or blessed, or when chairing a panel of judges.
---

# Judging a slice

The worked instance is `docs/design/WATER-REBUILD-REVIEW.md`: three independent
judges, **PARTIAL 3–0**, ten defects, on work that had passed every check its
builders ran. Read it before your first review. What it found:

- a Vulkan validation abort that **breaks green check 2 at HEAD** (`ribbon.loom`);
- a `GOLDEN` row not byte-reproducible — **reported by slices 4, 5, 6 and 8 and
  fixed by nobody**;
- an ADR acceptance claim falsified by nine renders giving nine hashes;
- the flagship feature of the whole rebuild absent from `loom run` entirely;
- the quad-boundary defect class shipping a **third** time, found the way the
  previous two were: **by moving the camera off the gate framing.**

## Rules of engagement

- **Judge from your own fresh build of the named HEAD.** Rebuild after any
  shader touch — stale SPIR-V is a live hazard here and one judge had to defeat
  it explicitly.
- **Judge your own PNGs, opened.** Never a builder's screenshot, never a
  builder's number.
- **You are barred from the `cargo xtask` gates** — the verifier owns them. So
  state explicitly which of your gate claims are hand-equivalents rather than
  gate runs.

## The standard sweep

```bash
# 1. the authored camera, plus crops
loom render <scene> --out /tmp/a.png --size 1920x1200 --sim <n>
loom compare /tmp/a.png /tmp/b.png --rect x,y,w,h     # mean RGB per crop, both images

# 2. off the gate framing — this is where the defects are
loom render <scene> --out /tmp/y.png --yaw 40 --pitch 15 --sim <n>
loom render <scene> --frames 16 --dolly 6             # parallax; --spin makes none

# 3. a tick sweep around the claimed instant
# 4. GPU-stateful anything: three fresh processes, sha256
bash tools/repeatcheck.sh <name> <scene> --sim <n>

# 5. no-regression on untouched GOLDEN rows
bash tools/goldcheck.sh <name> <scene> <args> && bash tools/bytecheck.sh <name>
```

Byte-check a sample of untouched rows against `tests/references/MANIFEST.txt` at
**exact gate args**; the panel did 27/27, 12/12 and 8/8 independently. Use the
`watch-loom` skill for motion.

**Check the promised feature is *visibly* present, not merely mechanically
present.** `pool_jet`'s row contained no jet; the cinematic tier existed and the
window drew none of it.

## Verify the recorded numbers yourself

Every determinism and cost claim in the ADRs and commit messages. ADR 0057's
body said `ribbon@180` was 52 ms/tick; the panel measured 107; HEAD measures
14.78. A `GOLDEN` row's own comment carried two wrong particle counts.
`loom-taking-a-measurement` has the traps.

## Verdicts

**PASS / PARTIAL / FAIL, per promised image or claim**, naming the defining
feature. *"The mechanism exists"* and *"the picture is there"* are different
verdicts, and a PARTIAL says which half failed.

**Rule PARTIAL and kick back with reproductions. Do not approve-with-notes.**

## Order the defects so a builder can act

1. **breaks a green check** (validation abort, non-reproducible GOLDEN row)
2. **falsifies a recorded claim** (an ADR, a commit message, a code comment)
3. **misses a defining feature** (promised and not visible)
4. **quality, then process debt**

Each with symptom, `file:line` if known, and a **reproduction command**.

**Process debt is a defect, not a nicety.** ADRs still at `proposed` while their
force path shipped; references blessed by their builder; gates not run over new
resources; costs reported from a machine state they do not reproduce on. All
four were defect 10.

*"A false determinism claim in an ADR is worse than a narrow true one"* — so the
required fix may be **shrinking the claim**, e.g. "byte-identical through tick
N, unstable past it" with a load-time refusal or a documented budget.

## The mandatory section

End the ruling with two headings — this is the part that makes the ruling
usable, and no one writes it unprompted:

**What I verified first-hand.** Own renders (name the HEAD, the build profile,
the resolutions, the cameras); own byte-checks (counts); own clippy/test runs;
code read, by path.

**What I took on trust / did not establish.** The xtask gates you were barred
from. Builder-admitted defects you did not independently reproduce. And
**absence of evidence recorded as absence of evidence** — "a flake that did not
fire in three runs" is not "the flake is fixed".

## Chair's discipline

Separate architecture from finishing work explicitly, so a kick-back does not
re-litigate a decision that already survived contact with implementation.

Correctness review of Rust and Slang is a different job — use `/code-review`.
