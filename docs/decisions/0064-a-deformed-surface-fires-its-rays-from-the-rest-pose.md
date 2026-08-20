# ADR 0064 — A deformed surface fires its rays from where the acceleration structure thinks it is

- **Date:** 2026-08-20
- **Status:** **proposed** — needs human approval before merge.
- **Constrains:** nothing new. It **amends ADR 0062's** "no acceleration-structure
  work" trigger, which named the wrong surface and the wrong symptom.
- **Applies to:** every secondary ray in `scene.slang` — soft shadows, RTAO and
  traced reflections (ADR 0019, ADR 0061) — on any object whose vertices are
  moved in the vertex shader.

## The defect this exists to name

ADR 0062 shipped `Deform` and priced the stale acceleration structure at *the
chrome shell displaces 0.23 mm*, with a trigger that read:

> a deformed mesh's reflection or shadow becomes the subject of a shot, or a
> scene displaces a **specular** surface by more than about 2 mm.

That trigger cannot fire on the scene that shipped the bug. The gleamsprat's
fringe is `roughness 0.85` — matte — and it displaces **16.1 mm**, a hundred
times the number the ADR quoted. Every blessed `gleamsprat*` reference carried
hard black gashes down the fringe as a result, and the commit that introduced
them attributed them to a missing normal correction, added the correction,
and did not re-check that the gashes were gone. They were not.

The mechanism is not about reflections at all:

- The raster path draws the vertex where `bodyWave` put it, and hands the
  fragment shader that position as `worldPos`.
- Every ray query starts at `worldPos + geometricNormal * shadowBias`, with
  `shadowBias` at most one quantisation step — for this mesh, `1e-3`.
- The TLAS is built from `rt_positions`, an unpacked copy of the mesh **as
  authored**. Nothing moves it.

So a fringe vertex thrown 16 mm off its rest position fires its shadow, AO and
reflection rays from a point 16 mm inside the rest-pose finger. `anyHit`
returns true for all of them, `sunVisibility` and `ambientVisibility` return 0,
and the surface renders at pure shadow. **The first symptom of a stale
acceleration structure is self-shadowing, not a wrong reflection**, and it
arrives at roughly the displacement the old trigger named — measured by an
amplitude sweep on `gleamsprat.loom`, clean at 0.8 mm, faint at 3.2 mm,
prominent at the authored 16.1 mm.

## The decision

**Ray queries use the pose the acceleration structure holds, not the pose the
rasteriser draws.** `VSOutput` gains one interpolated `float3 rayOrigin`: the
world position of the vertex *before* `bodyWave` displaced it. The three call
sites in `scene.slang` — `ambientVisibility`, `sunVisibility` and
`tracedEnvironment` — take it in place of `worldPos`. Everything else keeps the
drawn position: lighting, fog, the wetness gate, the view vector and the
geometric normal are all still the shape you can see.

This is not a bias hack and deliberately not one. Padding `shadowBias` by the
displacement was built and rendered too, and it works by pushing the origin far
enough out that it misses everything — which also deletes the legitimate
contact shadow between the fringe's own fingers, and needs an answer for the
model-space/world-space scale gap `shadowBias` already documents. Firing from
the rest position instead makes the query *consistent with the thing it
queries*: a deformed body's shadows, AO and reflections are its rest pose's,
which is exactly the bargain ADR 0062 already struck for its reflection. There
was never a second answer available while the TLAS holds one pose.

Measured on `gleamsprat.loom` at tick 104, 1600x1000: the fix moves
**fraction 0.00495, worst channel 230**. The gashes are gone and the
inter-finger shading is kept. Six scenes with no `Deform` — `meadow`, `forest`,
`materials`, `rain_gantry`, `emberfall`, `proving_ground` — render to
**identical sha256** across the change, because on an object that does not
deform `rayOrigin` is the same expression applied to the same vector as
`worldPos` and produces the same bits.

## What this deliberately does not fix, and why

**`windBend` has the same defect one amplitude smaller and keeps it.** A tree
has been drawn bent and traced straight since ADR 0009, and every forest
reference in `tests/references/` is blessed that way. Backing the sway out of
`rayOrigin` as well would silently re-light scenes this change has nothing to
do with, and it is not what put gashes on a fish. `rayOrigin` is therefore the
position *after* `windBend` and *before* `bodyWave` — stated here rather than
left to be read off two lines of shader.

*Trigger:* a wind-bent tree self-shadows visibly, or someone raises `sway` far
enough to matter. The fix is one line — move `restPos` above the `windBend`
call — plus re-blessing every scene with a `Wind`.

## ADR 0062's trigger, restated

Replace *"a scene displaces a specular surface by more than about 2 mm"* with:

> **a scene displaces any surface by more than about 2 mm**, whatever its
> roughness. Past that the rest-pose geometry is far enough from the drawn
> geometry to be visible — first as self-shadowing, which `rayOrigin` now
> answers, and beyond it as a shadow cast in the wrong place and a reflection
> of the wrong shape, which it does not.

The 2 mm figure survives; only the qualifier was wrong. What `rayOrigin` buys
is that crossing it is no longer an artifact — it is an approximation, and the
approximation is stated: **the rays see the rest pose.** A body whose *cast*
shadow or whose reflected silhouette is the subject of a shot still needs the
acceleration structure to move, and that is still refused, still on the same
grounds: `ALLOW_UPDATE`, one BLAS per instance, and a device stall per build.
