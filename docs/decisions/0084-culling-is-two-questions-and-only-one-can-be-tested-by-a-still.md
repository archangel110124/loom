# ADR 0084 — Culling is two questions, and only one can be tested by a still

- **Date:** 2026-09-06
- **Status:** **accepted** — second of the nine subsystems scoped as "what is
  missing for a full-blown game engine", at the general-engine bar.
- **Decision touched:** none of CLAUDE.md's locked table. No new dependency, no
  new `Access` variant, no new descriptor set. One new compute entry point in
  `scene.slang`, one module, and one widened stage mask.
- **Human decisions this records:** Hi-Z rather than frustum alone, "the golden
  images must not move" as the acceptance, and — after the scene sizes were
  measured — the *last-frame* variant rather than a depth prepass.

## 1. The gap

Every `cull` in the codebase was `CullModeFlags`: backface, a pipeline state.
Every object was submitted every frame. The forward pass is 0.05–0.11 ms, so
this bought nothing today; it is the ceiling on the world sizes that do not
exist yet, alongside the LOD octree Phase 8 defers and no streaming at all.

## 2. Two mechanisms, and they are not the same claim

**Frustum**, on the CPU, exact and deterministic. Planes come from the
view-projection by Gribb–Hartmann, so they cannot disagree with the matrix the
vertices are actually transformed by — deriving them from camera parameters is
how a cull comes to differ from the rasteriser at the edges. Per-mesh bounds are
taken from the source vertices at upload, **not** from `PackedBounds`, which
describes the quantisation grid rather than the geometry.

**Occlusion (Hi-Z)**, from the previous frame's depth. A 64×64 grid; each cell
keeps the farthest depth it covers, and an object is culled only when its
nearest corner is behind *every* cell it covers — which is the same as being
behind the largest. Anything that cannot be proved hidden is kept.

Both are conservative in the same direction. A box straddling a plane is kept;
a corner behind the eye abandons the test rather than dividing by a `w` that has
wrapped.

**Two things move geometry after the cull decides, and both are paid for.**
`sway` bends a trunk and `deform` runs a wave down a body, in the vertex shader,
after any decision made here. An object doing either has its box grown by a
quarter of its largest extent — far more than either can displace. A margin
costs a draw; a missing margin pops a tree at the screen edge.

## 3. What the goldens can and cannot see, and why that mattered

**Frustum culling is fully covered by the 62 references**, and they do not move.
That is the acceptance the human chose and it is the right one for a cull: the
failure mode is deleting something visible, and a reference image is exactly the
detector for that.

**But a cull that never culls also passes 62 unmoved references.** So the
frustum test is pinned separately in `frustum_tests`, including the case a wrong
near-plane sign silently keeps — a box ten metres *behind* the eye — which is
the single bug that would turn the whole feature into a no-op. Measured on a
real scene: `proving_ground` draws 5 of 8.

**Hi-Z is invisible to every golden image, by construction.** A single-frame
render has no previous frame, so the grid is null and nothing is occluded. That
was named as the cost when the variant was chosen, and it is answered with a
multi-frame A/B instead: render three frames with the reduction on and again
with `LOOM_NO_HIZ=1`, and compare the last one.

    lanternhead, frame 2, Hi-Z on vs off:  0 differing pixels

Objects culled: 12, then 11, then 10, as the grid fills. Zero pixels differ, so
what it removed was genuinely hidden.

## 4. Three bugs found by measuring rather than by reading

Recorded because each was silent and each produced a *plausible* wrong answer.

- **The grid began at zero.** An unwritten buffer reads as "everything is at the
  near plane", which culls the entire scene: seven of eight objects deleted and
  the picture down to bare ground. It is filled with 1.0 — the far plane, which
  occludes nothing — so a reduction that never ran costs a draw call rather than
  a picture.
- **Set 3 named only `FRAGMENT`.** The reduction samples binding 1 from a
  compute shader, which a fragment-only set cannot legally serve; the read comes
  back as zero rather than as an error anyone can see. `COMPUTE` added.
- **`opaqueDepth` is only resolved on the split path**, which needs water *and*
  MSAA. Every other scene leaves it untouched, so the reduction was reducing an
  image nothing had written — the same all-zero grid, the same deleted scene.
  The grid is now trusted only on frames that actually resolved.

That last one is the honest limitation: **occlusion culling is live on water
scenes and inert everywhere else.** For this project that is the useful half —
DEEPER is water — but it is a restriction rather than a design.

## 5. What is not built

- **The viewer culls on the frustum alone.** The grid lives on `Renderer` and
  the window has no instance of its own. This is backwards from where the
  benefit is: the window is the path with a previous frame every frame, and the
  offscreen path only benefits under `--frames`. It is the first follow-up.
- **No depth prepass.** It would make occlusion deterministic and visible to the
  goldens, and at 8–12 objects per scene it costs strictly more than it saves.
  The trigger to revisit is object counts in the hundreds, or a dense interior.
- **No LOD and no streaming.** Phase 8 defers the octree; nothing here changes
  that.
- **`ablate` has no row for this**, and cannot: ablation fails on *sameness*,
  and a correct cull's removal is defined by changing nothing. The goldens are
  the detector instead.
