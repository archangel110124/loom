# ADR 0013 — The water mesh reaches the horizon, and the rings were never the problem

- **Date:** 2026-08-13
- **Status:** accepted
- **Decision touched:** none of CLAUDE.md's locked rows. W4's "rings rather than a
  quadtree" is re-examined here against the alternatives and kept. ADR 0012's fade
  survives with both constants unchanged; what changed underneath it is re-derived
  below rather than assumed.

## Context

The human sent an annotated screenshot of `river.loom` with two marks and one
sentence: *"I think the LOD is way too aggressive. rework the lod. research if
there is a better method than what you are using now."*

Both marks are real and both were measured before anything was changed.

**The horizon line.** The mesh was a 512 m disc, so there was a radius at which
the sea stopped. In `ocean` at its authored camera, 1600×900, the strongest
horizontal edge in the frame sits at y = 347 with a step of about 8/255 per
channel against the sky — small, because fog at 512 m is 85%, but a straight line
across the whole image. From higher up it is not small at all: from 120 m the
water ends in a hard edge two-fifths of the way down the frame with pale sky
above it, where the sea should be. A disc edge tracks the camera's height; a
horizon does not, and that is the tell.

**The river terminating.** Less obvious and more damning. The water's shoreline
is a fragment discard on `depth = surface − ground`, and `ground` is sampled
**once per vertex**. `river.loom`'s channel is 5.8 m wide at the waterline. The
cell was `r / 16`, so at 41 m out it was 2.6 m and at 128 m it was 8 m: a lattice
that coarse steps clean over the channel, every vertex of the quad reads dry, and
the whole quad is discarded. The river visibly ends about 41 m from the camera
and the channel closes over. The Nyquist fade is not what did this — the mesh
under-samples the *terrain* before it under-samples the waves.

**And the number behind "too aggressive".** A cell at Chebyshev radius `r` is
`r / (WATER_RES/2)` metres, which subtends `2 / WATER_RES` radians **at every
distance**. At `WATER_RES = 32` that is 3.58°, or **58 pixels of quad at 900p
through a 55° lens.** That is the whole complaint in one figure.

## The survey

### Projected grid (Johanson 2004)

A uniform grid in the *post-perspective space of a projector*, intersected with
the water plane. Constant screen-space vertex density by construction, no rings,
no LOD levels, and it reaches the true horizon because the top row of the grid
maps arbitrarily far away.

What it costs is the projector. The camera cannot be used directly: aimed away
from the plane the ray-plane intersection lands *behind* the eye and the grid
"backfires", spanning to infinity on both sides (§2.3.2). So there is a separate
projector, aimed by interpolating between two heuristics — at the camera's own
view-plane intersection, mirrored against the plane when the camera looks up, and
at a point a fixed distance ahead — because "method 1 is better when looking down
at the plane, but method 2 is more suitable when looking towards the horizon"
(§2.4.1). Johanson is candid that this "has been developed mostly by trial and
error". Then, because waves displace vertices off the base plane and can pull the
grid's edge inward on screen, the span must be computed from a *displaceable
volume* bounded by `Supper`/`Slower`, intersected with the frustum, projected,
and turned into a range matrix (§2.4.2). And the projector must be kept out of
that volume and its near plane kept clear of it, which Johanson calls "a serious
limitation as the camera must be able to move freely" (§2.3.3).

Two of its named weaknesses land squarely on this codebase:

- **Swimming.** "When the camera is moving, the locations of the points of the
  grid (and consequently the sampled height-data) will change. Swimming has a
  look which reminds of that of sheet of fabric is lying on top of the real
  height field" (§4.1). This engine's water mesh is snapped to a fixed world
  lattice *specifically* so that no vertex moves when the eye does. Adopting a
  projected grid means giving that invariant up, and it is the invariant the
  whole shimmer programme rests on.
- **World-space extent.** "The projected grid is slightly more complicated to
  restrict in world-space since it cannot exactly be limited to certain
  world-space span… In a landscape rendering engine it can therefore be hard to
  use the projected grid for water rendering" (§4.1). Here the fragment discard
  already handles that — but it handles it by *throwing vertices away*, and in
  `river.loom` roughly 85% of the frame is dry plateau.

Johanson also states the mitigation for swimming, and it is the thing this
project already built: "It is recommended that the resulting height-data is
band-limited (not containing higher frequencies than the resulting grid can
represent) to avoid aliasing" (§3.2). That is ADR 0012.

### Geometry clipmaps (Losasso & Hoppe 2004) — and what is here now

Nested regular grids centred on the viewer, shifted incrementally as it moves.
The current mesh is a clipmap in everything but the name: same nested rings, same
doubling, same fixed lattice. CDLOD's critique of it is worth quoting because it
does *not* apply here: "the level of detail is essentially based on the
two-dimensional (x, y) components of the observer position, while ignoring the
height. This results in unequal distribution of mesh complexity and aliasing
problems as, for example, when the observer is high above the mesh, the detail
level below remains much greater than required." For a *water plane* — flat,
single altitude — that objection is empty. Terrain clipmaps have a third
dimension to get wrong. This one does not.

### CDLOD (Strugar 2009) and quadtree morphing

A quadtree of grid meshes with per-vertex morphing: `vertex.xy - fracPart *
quadScale * morphK`, gradually collapsing odd-index vertices onto their even
neighbours so a block of eight triangles becomes two with no seam and no pop.
This is what **UE5's Water plugin** does — Epic's own documentation describes
"traversing a quadtree each frame to generate an optimized set of tiles that are
visible on screen", tiles arranged as "a concentric circle around the camera view
based on distance, where each lower level of detail is… half the number of
vertices as the level that precedes it", and "four quads morph into a single quad
when switching to a lower level of detail". Note what that is: **concentric rings
with morphing and per-tile culling.** Not a different family from what is here.

The morph is the interesting part, and it is the same trade as the projected
grid: `morphK` is a function of camera distance, so every vertex in a morph band
moves continuously as the eye moves. Same invariant given up, for a smoother LOD
transition than this mesh's stitch already needs — this one has no pop to hide,
because both sides of a ring boundary evaluate the same function at the same
`xz`.

The other half of a quadtree is per-tile frustum culling, and W4 already gave the
reason it buys nothing here: no indirect draw, no visible set, and a per-frame
CPU traversal to decide what a vertex shader decides for free.

### Hardware tessellation

Patch-based, tessellation factors from view distance. Distance-based factors
crack at patch boundaries when neighbours disagree, and the literature on
watertight tessellation exists precisely because "patch-based terrain rendering
using hardware tessellation introduces cracks and swimming artifacts during
navigation". It also adds two shader stages and a hull/domain pipeline to a
renderer whose water is currently one non-indexed draw with no vertex buffer.
Screen-space tessellation factors depend on the camera, so this is the morphing
trade a third time.

### What the survey actually establishes

Every alternative buys screen-space uniformity by making vertex positions a
function of the camera. **The ring mesh already has screen-space uniformity** —
`2 / WATER_RES` radians per quad, independent of distance — *without* that
dependence. What the alternatives really offer over it is **vertex efficiency**:
a ring covers a full disc while a camera sees a wedge of it, so most of the mesh
is behind or beside the eye.

So the question is not "which method", it is "can this project afford the
constant factor". The forward pass on `ocean` at 1920×1080 was 0.061 ms.

## Decision

**Keep the rings. Change three things: the density constant, the outer edge, and
an invariant nobody had written down.**

### 1. `WATER_RES` 32 → 128

The quad goes from 3.58° to 0.90° — 58 pixels to about 14 at 900p. `WATER_LEVELS`
drops 7 → 6, because reach is `(WATER_RES/2)·WATER_CELL·2^(L-1)` and six levels
of 128 already reach 1024 m, past both the 1000 m far plane and the ~830 m where
the fade has retired the last wave `ocean` authors. 43,008 vertices become
589,824.

`WATER_CELL` is untouched at 0.5 m, so the near field's floor is what it was.

### 2. A horizon skirt, and a far-plane clamp to let it exist

The outermost level's outer vertex ring is scaled out to `WATER_HORIZON` =
50 km. Every vertex on that ring has Chebyshev radius exactly
`(WATER_RES/2)·cellSize`, so a single scale factor lands the whole ring on the
horizon and keeps it square — the two triangles of each boundary quad still share
both outer vertices, so it is watertight for the same reason the rest of the mesh
is. The quads it stretches are drawn dead flat and not by luck: at that radius
`waterSamplingCell` reads hundreds of metres and every fade weight is *exactly*
zero, so `loom_sample_water` returns the plane.

50 km rather than infinity because infinity does not survive the projection: a
point at infinite distance lands at `z/w = far/(far−near)`, marginally outside
the far plane, and is clipped. So instead `waterVertexMain` holds every water
vertex inside the far plane:

```slang
if (out.clip.w > 0.0) {
    out.clip.z = min(out.clip.z, out.clip.w * 0.999999);
}
```

Guarded on `w > 0` and only ever clamping down. A vertex behind the eye has
`w < 0`, where `w * 0.999999` is the *larger* number and an unconditional `min`
would drag `z` toward it and corrupt the near-plane clip — the one clip this must
not touch. Strictly inside rather than equal to `w` because the depth test is
`LESS` against a buffer cleared to 1.0, and a horizon at exactly 1.0 draws
nothing.

This is what UE5 calls its Far Distance mesh: "used to fill the space between the
farthest tile used by the Extent in Tiles property and the horizon."

### 3. The snap quantum must fit inside level 0 — and it did not

Found while measuring, not while designing. The mesh is snapped to the coarsest
cell, `WATER_CELL·2^(LEVELS−1)`, so the eye stands up to half of that from the
snapped centre. Level 0 only reaches `(WATER_RES/2)·WATER_CELL`. At **32 and
seven levels that was 16 m of possible offset against 8 m of reach**: for most of
its travel the camera stood *outside the finest ring*, and the water directly
under it was drawn by level 1 or level 2. `WATER_CELL` cancels, so the condition
is `2^(WATER_LEVELS−1) ≤ WATER_RES`; the shipped configuration failed it by 2×,
and a trial configuration of 32 and eight levels failed it by 4× and turned
`river`'s surface into a mirror, which is how it was caught.

`the_water_draw_matches_the_shader_s_grid` now asserts it. It reads the constants
out of `scene.slang` — the same test that already checks `WATER_VERTS` — so it
costs no new file and no second copy of the numbers. Pointed at the old
configuration it fails with:

```
the 64-cell snap quantum is wider than level 0's 16-cell reach
(WATER_RES = 32, WATER_LEVELS = 7): the camera would stand outside the finest ring
```

At 128 and six it is 32 against 128.

## Why not the projected grid, stated plainly

1. **It trades the invariant this project is built on.** Vertices stop sitting on
   a fixed world lattice and start moving with the eye. Johanson names the
   resulting artifact himself. The 32 m snap — now 16 m — exists to prevent
   exactly it.
2. **`underwater.loom` violates its precondition.** The camera is inside the
   displaceable volume and looking up through the surface. That is the case
   §2.3.3 says to keep the projector out of.
3. **Its advantage here is a constant factor, and the constant factor is
   affordable.** 13.7× the vertices cost 0.015 ms. Paying that is a smaller,
   more reversible change than a projector, a range matrix, a displaceable
   volume, a backfire heuristic and an underwater special case — in both
   renderers.
4. **Its headline property is less clean than it looks at grazing angles.** The
   quad footprint is uniform in *screen* space but violently anisotropic in
   world space: at 100 m with the eye 3 m up, a projected grid's rows are ~26 m
   apart along the view direction against the rings' 6.25 m. A single Nyquist
   constant would have to be keyed to the *larger* footprint axis, which grows as
   `d²/h` — faster than the rings' linear growth. The "one constant instead of a
   function of distance" argument is true of the horizontal axis and not of the
   other one.

**What would change this answer**: a frame budget where 0.015 ms matters, or a
scene where the wasted 84% of the disc becomes real cost — a much larger
`WATER_RES`, or water drawn several times for reflections. At that point the
first thing to try is not a projected grid but an early-out in the vertex shader
that collapses off-screen cells before the wave sum, which is the same
`covered`-degenerate trick already in the file.

## ADR 0012's fade survived, unchanged, and that is the test of it

Both constants are exactly as they were: `WATER_FADE_WHOLE = 4`,
`WATER_FADE_GONE = 2`. They are stated in **samples per wavelength**, which is a
fact about reconstruction and not about this mesh. The mesh enters only through
`waterSamplingCell`, which was written as `max(offset) / (WATER_RES · 0.5)`
rather than as a hand-inlined 16 — so quadrupling the density moved the entire
curve outward with no number touched. That was luck in the sense that nobody
planned this change, and not luck in the sense that the expression was written to
be a function of the mesh rather than of a mesh.

What moved is the consequence table. For `ocean` (five waves, `Σ A` = 1.26 m):

| | before (RES 32) | after (RES 128) |
| --- | --- | --- |
| cell at radius `r` | `r / 16` | `r / 64` |
| exact out to | 8 m | **32 m** |
| fade first touches a wave | 9.4 m | **37 m** |
| sea drawn dead flat past | ~140 m | **~830 m** |

The near field is bit-identical to the unfaded surface for any wave longer than
4 m out to 32 m instead of 8 m. `sample_water` is untouched, `slang()` is
untouched, `water.slang` is untouched, and `slang_agreement` passes unchanged —
the fade is still in one vertex shader and nowhere else, and buoyancy still floats
on the whole sea.

## A floating object must sit on the drawn surface

Added as an acceptance criterion mid-change, from a screenshot of
`water_crate.loom` in which the crate appears to hover. The requirement: at any
distance where an object is big enough to see, the drawn surface under it must
be within a few centimetres of `sample_water`, not the ~0.26 m ADR 0012
permitted at 32 m.

**Met, by the density change alone, with the fade untouched.** The divergence is
`Σ Aᵢ·(1 − wᵢ)` with `wᵢ = smoothstep(2, 4, λᵢ/cell)` and `cell = max(0.5,
r/64)`, so it is computable exactly rather than estimated:

| scene | exact (0.000 m) out to | before |
| --- | --- | --- |
| `water_crate` (λ 21/13/8) | **128 m** | 32 m |
| `ocean` (λ 26/17/11/7.3/4.7) | **64 m** | 16 m |

and past that, on `ocean`: 0.089 m at 128 m, 0.339 m at 256 m — where the same
1.2 m crate subtends 8.8 px and 4.4 px at 900p, against 0.65 px and 2.5 px of
divergence. The old scheme reached 0.339 m at **64 m**, where the crate is 17 px
and the error is 5 px. That is the regime the criterion was written about, and it
has moved four times further out.

**Measured, not just derived.** Rendering `water_crate` with the fade forced off
(`WHOLE = −1, GONE = −2`, so every weight is 1) and diffing against the shipped
build: the two frames differ **only in rows 313–332** — a 19-pixel band at the
horizon, 0.37% of the frame, nothing within 200 px of the crate or the post.
ADR 0012 is now inert everywhere anything floats.

**And the crate in the screenshot is not actually hovering.** Measured
end-to-end, which is worth doing because the appearance is convincing:

- `loom water --at 0,-6 --sim 120` gives the surface at the crate's xz as
  **0.042 m**; the crate's centre is at 0.004 m with a 0.6 m half-height, so it
  is 53% submerged at that instant.
- Inverting the projection for the authored camera, `sample_water`'s waterline
  falls on screen row **538**, and the crate's bottom would fall on row **625**
  if no water occluded it. The crate's visible bottom edge measures **536–550**
  across its width. It is being cut by the water within a few pixels of where
  the CPU function says, and nowhere near its own bottom.
- The same on the `Post`, which is static and therefore an exact ruler: its
  visible bottom edge measures rows 431–436, which inverts to a drawn surface of
  **0.415–0.465 m** against `sample_water`'s **0.532 m** at that xz.

What reads as a gap is two things, neither of them LOD. The crate is 300 kg/m³
by authorship, so it is *supposed* to ride with most of its bulk clear, and its
waterline sits on a local crest while the sea just beyond is in a trough and
projects lower. And there is **no contact cue at all** — no foam, no wetness
darkening, no waterline shading — so the silhouette where water meets crate is
drawn with exactly the crispness of a geometric edge and the eye reads it as the
crate's own bottom face. `target/water-lod/crate_annotated.png` overlays
`sample_water`'s waterline on the frame; it lands on the edge in question.

**The 0.07–0.12 m residual on the post is real and is not the fade.** It is the
Gerstner horizontal displacement: buoyancy reads `sample_water(xz).height` as the
height *at* `xz`, while the renderer draws that height at `xz +
displacement.xz` — measured at −0.116 m in x here. So the surface drawn directly
above a body is a slightly different sample from the one the body floats on.
That is the only mechanism left that can put an object off the drawn surface at
close range, it is orthogonal to everything in this ADR, and it is not fixed
here. Fixing it means either solving for the lattice point that displaces onto a
given xz (an iteration, in the buoyancy solver) or accepting the offset; it wants
its own ADR.

**Keying the fade to the nearest floating body: rejected.** It was raised as an
option and it is the wrong shape for three separate reasons, the first of which
is fatal on its own. ADR 0012's property 2 requires the weight to be a function
of position only: a vertex on a ring boundary is emitted by both levels and the
two must agree, so a weight that consulted scene contents would reopen the seam
unless every body were consulted identically from both sides. Second, it would
make the drawn surface depend on physics state, so a golden image would change
when a crate is deleted and the image gate would stop being a test of the
renderer. Third and most simply, it is unnecessary: the fade-off diff above shows
the fade already contributes nothing anywhere an object floats.

## Consequences

**Measured.** `cargo xtask shimmer`: 12 frames, 640×400, camera static, sim
advancing.

| scene | before | after |
| --- | --- | --- |
| ocean | 1.803 | **1.945** (+7.9%) |
| shore | 1.968 | **2.016** (+2.4%) |
| underwater | 2.530 | **2.597** (+2.6%) |
| river | 0.350 | **0.566** (+62%) |

**The flicker went up and that is the change working.** ADR 0012 bought −14% on
`ocean` by drawing the far field flat; this gives some of it back by drawing the
far field. `river` moved most because `river` was a mirror — a flat surface
scores near zero on a metric that counts pixels changing, and the 62% is the
ripples the human asked to see. No non-water scene moved by a thousandth.

The important question is whether the extra flicker is detail or aliasing, and a
density sweep at constant reach (`WATER_LEVELS` chosen so all three reach 1024 m,
skirt on, nothing else varied) answers it:

| `WATER_RES` | quad | at 900p | vertices | ocean | shore | underwater | river |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 32 | 3.58° | 58 px | 36,864 | 1.803 | 1.899 | 2.541 | 0.350 |
| 64 | 1.79° | 29 px | 147,456 | 1.904 | 2.087 | 2.597 | 0.505 |
| **128** | **0.90°** | **14 px** | **589,824** | **1.945** | **2.016** | **2.597** | **0.566** |

The curve **flattens rather than diverging**: `ocean` gains 5.6% then 2.2%,
`underwater` saturates, and `shore` actually comes *back down* from 64 to 128.
That is the signature of the fade doing its job — past a point, extra density
resolves waves the fade was already admitting rather than admitting new ones. If
the mesh were being pushed past what it can sample, this column would climb.

Under a **moving** camera (16 orbiting flythrough frames, seven triples, `loom
flicker`) the same shape and smaller in relative terms, because parallax
dominates: ocean 5.499 → 5.689, shore 4.722 → 5.245, river 4.531 → 5.306,
underwater 3.810 → 3.932. No spike anywhere in the sequence, which is what a
popping LOD boundary would look like.

**Note the metric's blind spot, since the survey turned on it.** `shimmer` holds
the camera still. It therefore cannot see swimming *at all* — the projected grid's
signature artifact is invisible to the number this project uses to judge water,
and the flythrough triples above are the only measurement here that would catch
it. That asymmetry is a reason to be careful about ever adopting a
camera-dependent vertex position on the strength of a `shimmer` number.

**Cost.** `LOOM_GPU_TIMING=1` on `ocean` at 1920×1080, medians of eight frames:
forward pass **0.061 ms → 0.076 ms**. 13.7× the vertices for 1.25× the pass, and
the pass is still under a tenth of a millisecond. This is the number the whole
decision rests on.

**Four references move, and only the four water scenes.** `ocean`, `shore`,
`underwater`, `river` — re-blessed after looking at each against its predecessor.
The other nine golden images are byte-identical, which is the check that this
touched the water path and nothing else. The determinism hash is unchanged at
`b478ea4ac2622d32`; the change is entirely in a vertex shader and a draw count,
and the simulation cannot see either.

**The snap now jumps 16 m instead of 32 m.** Still no vertex moves: 16 m is a
whole multiple of every cell size in the mesh, up to and including the coarsest,
so the lattice maps onto itself and only the window moves. Halving it also
halves the worst-case camera offset from the centre, which is what makes the new
invariant hold with 4× margin instead of failing by 2×.

**What is still not solved.** The far field's *shading* is still geometry-only.
Waves shorter than the fade admits arrive as nothing rather than as a normal map,
which is the gap `loom_water::spectrum` documents from the spectral end and ADR
0012 documents from this one. Drawing the sea out to the horizon makes that gap
larger in screen area, not smaller: there is now a great deal of visible water
whose only detail is the swell. A normal map is where the rest belongs, and it is
still not built.

## Human approval

Not required by the letter of CLAUDE.md — no locked row changes, `sample_water`
is untouched, and W4's rings are kept rather than replaced. Recorded as an ADR
because the human asked for the research explicitly and the answer is a
negative one about the alternatives: the survey is the deliverable as much as the
constant is. Reverting is three constants and two blocks in `waterVertexMain`,
plus re-blessing four references.

## Sources

- Claes Johanson, *Real-time Water Rendering: Introducing the Projected Grid
  Concept*, MSc thesis, Lund University, 2004 —
  <https://fileadmin.cs.lth.se/graphics/theses/projects/projgrid/projgrid-hq.pdf>.
  Sections cited above: 2.3.2 (backfiring), 2.3.3 (displaceable volume, camera
  restriction), 2.4.1 (projector aiming, the two methods), 2.4.2 (range matrix),
  2.5 (projector elevation, distance dynamic range), 3.2 (band-limiting the
  height data), 4.1 (vertex efficiency, swimming, world-space extent).
- F. Losasso and H. Hoppe, *Geometry Clipmaps: Terrain Rendering Using Nested
  Regular Grids*, ACM TOG 23(3), SIGGRAPH 2004 —
  <https://hhoppe.com/proj/geomclipmap/>.
- Filip Strugar, *Continuous Distance-Dependent Level of Detail for Rendering
  Heightmaps*, Journal of Graphics, GPU, and Game Tools 14(4), 2009 —
  <https://aggrobird.com/files/cdlod_latest.pdf>. Morph code and the clipmap
  critique are quoted from §"LOD function" and §"Morph implementation".
- Epic Games, *Water Meshing System and Surface Rendering in Unreal Engine* —
  <https://dev.epicgames.com/documentation/en-us/unreal-engine/water-meshing-system-and-surface-rendering-in-unreal-engine>.
  Quadtree traversal, concentric LOD circles, quad morphing, LODScale, Far
  Distance.
- On hardware-tessellation cracks and swimming: I. Bonaventura, *Terrain and
  water rendering with hardware tessellation* —
  <https://ima.udg.edu/~xavierb/BonaventuraFDP.pdf>; and the watertight-
  tessellation literature surveyed from it. The specific claim that
  distance-based factors crack at disagreeing patch boundaries is from that
  material, not from measurement here.

**Not from a source:** the pixels-per-quad arithmetic, the `2 / WATER_RES`
identity, the `d²/h` grazing-angle footprint comparison, the diagnosis of
`river`'s termination as terrain under-sampling, the snap-quantum invariant, and
every number in the tables above are this project's own reasoning and
measurement.
