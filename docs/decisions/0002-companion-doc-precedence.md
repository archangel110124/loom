# ADR 0002 — Companion doc precedence, and the corrections that follow

- **Date:** 2026-07-30
- **Status:** accepted
- **Decision touched:** new (documentation policy); implements LOOM-BUILD-BRIEF.md §7.13

## Context

The five companion docs landed in `docs/design/` on 2026-07-30, having existed only as downloads
until then. Every cross-reference in the build brief now resolves — but the five docs were written
in sequence, and later ones override earlier ones without the earlier ones being edited. A session
reading them in directory order will implement superseded designs with full confidence.

The concrete conflicts, all verified by reading the files:

1. **wgpu is dead, but three docs still discuss it at length.**
   `loom-vulkan-backend.md` §0 is explicit that it "supersedes" the earlier docs. But
   `loom-graphics-physics-frontier.md` §A.2 still contains ~30 lines of wgpu binding-array limit
   discussion, §B.4 quotes wgpu's experimental mesh-shader status, `loom-voxel-system.md` §3.1
   says "In Rust with `rapier` and `wgpu`", §7 benchmarks a wgpu proof-of-concept, and
   `loom-terrain-generation.md` §9 lists "`wgpu` later if erosion becomes a bottleneck". The
   Vulkan doc's §12 ripple table catches some of these and misses the voxel §3.1 and terrain §9
   mentions.

2. **The WASM/browser target is foreclosed, but three docs still plan for it.**
   `loom-vulkan-backend.md` §0 states the browser target is lost "permanently" and §13 repeats it.
   Yet `ai-native-engine-design.md` Part 3 Phase 6 lists "the WASM build target" as a deliverable,
   §A.2 of the graphics doc advises keeping "the material system abstracted over both" bindless and
   bindful paths for WASM, and §C.3 cites Box3D's small binary as "relevant to your WASM ambitions".
   `CLAUDE.md` and brief §1 both say "No web target." The engine docs are stale here.

3. **Two incompatible schedule numbering schemes.**
   `ai-native-engine-design.md` Part 3 uses Phase 0–6 with week ranges; the brief §6 uses M0–M12.
   They describe the same project. "Phase 3 is the gate" (design doc) and "M9 is the gate" (brief)
   are the same gate. A session quoting phase numbers at a human tracking milestones will not be
   understood.

4. **Crate lists disagree.** The design doc §2.13 lists `loom_graph/`; the brief §3 layout omits
   it entirely (see ADR 0003). The voxel doc §9 lists `loom_voxel_physics/` as a third crate and
   the terrain doc §9 lists `loom_terrain_erode/` and `loom_terrain_analyze/`; the brief §3
   consolidates each group into one crate.

5. **Illustrative code floats dependency versions.** `loom-vulkan-backend.md` §3 shows
   `ash = "*"`, `gpu-allocator = "*"`, etc. Brief §8 rule 6 and `CLAUDE.md` both forbid this
   absolutely. The snippet is illustrative, but it is the kind of thing a session copies verbatim.

## Decision

**Precedence, highest first.** When two documents conflict, the higher one wins without discussion:

1. `CLAUDE.md` — always-loaded rules and the locked table
2. `docs/decisions/` — ADRs, newest applicable
3. `docs/design/LOOM-BUILD-BRIEF.md` — §2 is the authority on what is locked (brief §7.13)
4. `docs/design/loom-vulkan-backend.md` — supersedes all wgpu-era claims in 5 and 6
5. `docs/design/loom-voxel-system.md`, `loom-terrain-generation.md` — subsystem plans
6. `docs/design/ai-native-engine-design.md`, `loom-graphics-physics-frontier.md` — the original
   reasoning. Architecturally still correct; every backend-specific claim in them is stale.

**The companion docs are not edited to fix the above.** They are the reasoning record and they are
more useful intact — knowing *why* wgpu was chosen and then abandoned is worth more than a doc that
pretends the decision never happened. The corrections live here instead, and `docs/design/README.md`
is the map a session reads first.

**Corrections, binding:**

- Every wgpu reference in docs 4–6 reads as Vulkan-via-`ash`. There is no wgpu in this project and
  no RHI abstraction over it (`CLAUDE.md`, brief §2). The terrain doc's "wgpu compute for erosion"
  means an async-compute Vulkan pass; the voxel doc's `silk-clouds` reference is an algorithm
  reference, not a dependency.
- **WASM is out of scope, permanently.** Design doc Phase 6's WASM deliverable is void. Do not keep
  a bindful fallback path, do not weigh crate choices by WASM binary size, do not abstract the
  material system over two binding models. Revisiting this means a second renderer and a new ADR.
- **Phase numbers map to milestones** as: Phase 0 → M1 · Phase 1 → M2–M5 · Phase 2 → M1 (prefabs,
  folded into the scene format) · Phase 3 → M9 · Phase 4 → the graph work in ADR 0003 · Phase 5 →
  M5.5 + M12 · Phase 6 → post-M12, minus WASM. **Use milestone numbers in all commits and
  conversation**; the human tracks M-numbers.
- **The brief §3 crate layout is authoritative** over the per-doc crate lists. Split a crate out
  later if compile times or the dependency rules demand it — that is a smaller change than merging
  two crates that should have been one.
- **Every `= "*"` in a companion doc is illustrative only.** Pin exactly, add with `cargo add`.

## Consequences

- Reading all six design docs cover-to-cover is ~2,600 lines and no longer necessary. The README
  routes to the two or three that matter for a given milestone.
- The stale sections stay on disk and will be re-read by future sessions. This ADR is the only
  thing standing between that and a wgpu-shaped mistake, so the README must point at it prominently.
- Dropping the WASM hedge removes a real constraint from several future decisions (material system
  design, physics engine choice at M10, meshlet path). Those get simpler.

## Human approval

Not required — this records precedence among documents rather than changing a locked decision.
Points 2 (WASM) and 4 (crate consolidation) resolve toward what `CLAUDE.md` and the brief already
say, so they are clarifications rather than new decisions. Flagged to the human on 2026-07-30.
