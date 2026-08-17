# The VFX overhaul — where it stands

*17 Aug 2026, after one overnight session. Branch `overnight/2026-08-11`.*

**Read `OVERNIGHT-DECISIONS.md` alongside this.** This file says what exists; that one says
which calls were made without you and which are worth overturning.

---

## The work order, and what is done

The report's nine milestones were re-ordered into W0–W8 (`OVERNIGHT-DECISIONS.md` D8). The
report assumes a greenfield; this engine had already shipped several of its milestones.

| | | |
|---|---|---|
| **W0** | `cargo xtask repeat` — a fifth permanent gate | ✅ verified |
| **W1** | screen-space refraction, Snell on both legs | ✅ verified · **acceptance test missed, see D11** |
| **W2** | whitecaps from `fold`, with a downwind trail | ✅ verified |
| **W3** | water reflects the scene, not only the sky | ✅ verified |
| **W4** | smoke as a marched soot volume | ✅ verified |
| **W5** | splash/spray from fold + submersion events | not started |
| **W6** | interactive ripples, CPU-authoritative | not started · **needs its own ADR** |
| **W7** | the GPU particle pool | ✅ verified |
| **W8** | windowed FFT detail tier | deferred behind evidence (D2) |

Six of eight. W5 and W6 are the remainder; W8 is deliberately gated on a measurement nobody has
taken yet.

## What "verified" means here

Every item above was checked by a human-equivalent pass over the gates, not by taking a
subagent's word. The final state:

| gate | result |
|---|---|
| `cargo clippy --workspace -- -D warnings` | clean |
| `cargo test --workspace` | 46 binaries, zero failures |
| `cargo xtask image` | 35 references, every diff read before blessing |
| `cargo xtask validate` | **60 scene runs, zero validation messages** |
| `cargo xtask repeat` | 35 scenes byte-identical across 3 processes |
| `scripts/check-deps.sh` | ok |
| determinism | `b478ea4ac2622d32` / `1c33f211d7ea9916`, unmoved all session |

Scenes went 44 → 49; golden references 30 → 35.

## The ADRs this produced

- **0045 — the VFX determinism line.** Four clauses. The sim hash stays physics-only; anything
  producing a force or readable by an assertion is CPU-deterministic; GPU floats never cross that
  line; GPU-stateful effects are admissible only under ADR 0017's conditions plus the new repeat
  gate. **This is the one to read if you read one.**
- **0047 — the GPU particle pool.** Slot ownership is arithmetic, so there is no free list and no
  atomic on any seed path.
- **0048, 0049 — the W1/W2 findings**, including the acceptance test that cannot be met.
- **0050 — W3 and W4**, recorded together because they share one fact: the TLAS holds meshes
  only, so a reflected flame or plume does not appear.

## Three defects found that nobody was looking for

1. **The caustic web was applied twice** — `scene.slang:2285` carried a comment describing a
   guard that had never been written, so every bed seen through the surface got the web doubled.
2. **An un-fog clamp sat at `1e-3` where the plan specified `0.15`** — an up-to-1000× firefly
   amplifier that the new refracted lookup would have begun feeding.
3. **A scene with water and no mesh drew with descriptor set 0 unbound** —
   VUID-vkCmdDraw-None-08600, once the water shader statically referenced `sceneTLAS`. No scene
   in the repo covered it; `bare_sea.loom` was written for it, and reverting the fix fires the
   VUID on that scene and no other.

The third is crash-class and would have shipped.

## What to look at first, when you have a machine

1. **`plume`** — the smoke. This is the headline of W4 and the biggest visual change.
2. **`mirrorpool`** — water reflecting the scene rather than the sky.
3. **`beach`** — and decide whether you agree with D10. It is *less pretty* than before, because
   two bugs that flattered it were fixed. The lever to get the look back is per-`WaterBody`
   optical parameters, not restoring the bugs.
4. **`shore`** — the hue is right (R/B 0.649 against a 0.644 target) and the brightness is not
   (G−R 25.3 against an acceptance test of 38, which is unreachable — D11).

## What is not measured

- **Smoke's cost under coverage.** 0.35 ms at `plume`'s camera, ~2.8 ms extrapolated to full
  screen. Nobody has walked a camera *into* a plume and looked at the frame time. This is the
  first effect priced by screen coverage rather than by population, and the timing numbers in
  CLAUDE.md will read as misleadingly cheap beside it (D15).
- **Motion.** `cargo xtask flythrough` has not been run on the new water or the plume. A plume
  that pops or swims, or foam that snaps rather than drifting, is invisible in every still above.
- **Reproducibility on other hardware.** `cargo xtask repeat` proves byte-identity on this
  RTX 4090. The concern it was built for — that additive-blend order-independence is not
  *guaranteed* across GPUs — cannot be settled by any gate on one machine.
