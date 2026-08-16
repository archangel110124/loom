# ADR 0020 — The flame is a line integral, not a level set on a plane

- **Date:** 2026-08-16
- **Status:** **accepted**
- **Decision touched:** none that is locked. One additive quad, one draw, no new
  pass, no new asset, no new descriptor. What changed is what the fragment
  shader computes inside that quad.
- **Supersedes:** the "One additive quad, and the flame is a LEVEL SET of a
  noise field" comment block in `assets/shaders/scene.slang` and the identical
  passage in `assets/test/campfire.loom`.

## The problem, stated once

`lanternhead.loom` recorded it in its own comment: *"On the open deck it was
three detached orange shards… A fire in this engine needs a dark backdrop, and
that is a composition constraint rather than a bug."*

That was a correct description of the shader and an incorrect description of the
technique's ceiling. Three mechanisms produced it, and none of them was a bug —
each was the specified output of a constant tuned correctly against an
`R8G8B8A8_SRGB` target that **ADR 0018 deleted**:

1. **The flame returned literal alpha `0.0` at both return sites.** The particle
   pipeline is premultiplied, so alpha 0 preserves the destination entirely: the
   background was visible through the fire's core at full strength.
2. **The tongues were topologically disconnected.** `FIRE_T1 = 1.05` and
   `FIRE_GAP = 0.72` sit deliberately above the noise field's supremum of 1.0,
   so extinction is certain — and the gaps between the surviving components
   showed background at 100%.
3. **Every surviving component had a hard, near-full-brightness edge**, because
   `FIRE_TAU_FLOOR = 0.30` made the ramp's bottom two rungs dead code.

## Why no constant could fix it

**A level set on a plane renders a slice, but fire is a line integral.** A view
ray through real fire crosses many small hot pockets and sums them, and that sum
is smooth and connected even when every individual iso-surface it crossed is in
fragments. The old shader took one slice of one field at one depth and
thresholded it.

Every knob that reconnects the shards *inside the slice formulation* — a lower
`FIRE_HZ`, a lower `FIRE_GAP`, a wider `cover` — does it by deleting the detail
that makes the thing read as fire, and converges back on the fireball the design
was written to escape. So the silhouette machinery is not what was wrong and it
is not what changed.

## What was built

`flameColor` solves the ray/sphere chord analytically against the emitter's
inscribed sphere and marches `FIRE_STEPS = 48` samples front to back,
accumulating emission against transmittance and emitting
`fireRamp(heat · FIRE_HEAT)` premultiplied with `1 − T`.

**The silhouette machinery is hoisted verbatim into `fireField(p, …)` and called
per sample.** The domain warp, the fold amplitudes, the threshold above the
field's supremum, the fuel lobes and the post-warp vertical squash all mean
exactly what their own comments say. Only *where they are evaluated* changed.

Three constants are **deleted rather than retuned**, because the integral
produces what they were approximating:

- `cover` and its `fwidth`. The silhouette is now where the chord through the
  level set vanishes; a ray leaving a smooth iso-surface at tangency does so like
  a square root, which is steeper than the analytic edge it replaces and carries
  no screen-space derivative.
- `FIRE_TAU_FLOOR`. It existed because brightness inside the coverage edge was
  cubic in depth while opacity was full. In an integral the outermost pixels emit
  little **and** occlude little.
- The whole `FIRE_GLOW*` continuum. A ray through a gap between tongues still
  crosses material at other depths, just less of it, so the gap comes out dim on
  its own — the same term, derived rather than authored, and it parallaxes with
  the camera instead of being pinned to the billboard.

**The ramp is applied once, to the accumulated integral, and never per sample.**
A sum of ramp colours mixes hues and lands on an average that clips flat at an
amber rung; this is the whole reason the fire has a temperature gradient rather
than a tint.

**Soot is the same field under its own, lower ceiling** (`FIRE_SOOT_T1 = 0.70`),
contributing to transmittance and nothing to `heat`, gated on height. Against a
premultiplied target that is exactly a smoke that eats the background, and being
the same field it is structurally incapable of disagreeing with the flame about
where the fire is.

## Three things the integral does not fix, and they were grafted

The integral connects what is there. It cannot invent material a threshold
forbade, and two of the three original defects were authored into the threshold:

- **The fuel lobes had a hole on the flame's own axis.** `lobes` was
  `sin(w.x · 5.5 + …)` with `w` measured from the emitter, so `sin(0) = 0` put a
  fuel minimum on the one column the fire stands on. **The "three detached
  shards" are the fuel lobes, not noise and not the alpha.** `cos` puts a peak
  there.
- **Nothing bounded the field at the quad's inscribed circle**, so a 2.2 m quad
  on a 0.84 m brazier drew a flame two and a half times the width of the thing
  burning. `FIRE_EDGE_T` takes the threshold past the field's supremum before
  the circle, by the same arithmetic that sets `FIRE_T1`.
- **The detail cutoff was a distance in metres**, so at 4K a 250-pixel-wide
  flame had its eleven-pixel features thrown away by a bound that cannot see the
  resolution. It is Nyquist off `fwidth` on the quad now, combined by `min` with
  the march's own along-the-ray bound.

## The quadrature, and why there is no ray dither

The rectangle rule gives a ray grazing a nearly-tangent iso-surface a binary
verdict, and the contour of that flip is a comb of evenly spaced spikes off the
flame's flanks. Three ways of paying for it were rendered at 3840x2160 and all
three are visible: interleaved gradient noise is a **diagonal halftone** (it is
designed to be cleaned up by a temporal or spatial filter and this project has
neither); a pixel hash has no period but far more variance and salt-and-peppers
the flame; no dither leaves step quantisation.

**Compositing the segment with the average of its two ends removes the cause.**
It is the trapezoid rule, it costs one extra field evaluation in total because
each sample is the far end of one segment and the near end of the next, and it
moves 23,353 pixels of a 4K frame by more than 4 of 255 — all of them at material
boundaries. With it, no dither at any amplitude measures better than none.

## What this does not fix, named rather than hidden

- **The nested contour arcs inside a tongue.** They survive the trapezoid, they
  survive making `fireRamp` C1 at its rungs, and widening `FIRE_DENS_W` to 0.14
  only thins the flame without touching them. They are the domain warp's **fold
  seen in depth**: the warp amplitude is deliberately above the fold condition, a
  folded map has cusps, and a line integral through one crosses the folded sheet
  several times. All three variants judged in this investigation show them. The
  only cure is lowering the fold amplitude, which is what the silhouette is made
  of.
- **No smoke self-shadowing.** Each puff is shaded as if it were alone, so a deep
  column does not darken its own middle. That needs the plume's optical depth in
  an offscreen R16F target and a second draw of the whole particle set, with its
  own render-graph barriers. Costed, not built.
- **The light does not agree with the fire.** `campfire.loom`'s `Light` is a
  constant 3.0 and the ground pool it casts is byte-identical to the pre-fire
  baseline. A flame core reaching the ramp's white rung under a still lamp is
  half of what a filmic fire is judged on. It is scene- and engine-side work
  (a modulation authored on `Light`, driven off `weather.z` and never the wall
  clock) and it is not in this change.
- **A tongue never pinches off and rises.** Nothing in a level-set formulation
  sheds, because nothing in it curls.

## Alternatives measured and rejected

- **A depth-layered slab stack** (three evaluations offset along the view ray).
  Connects the body, but three taps is a coarse quadrature of the same integral
  and compositing ramp colours per slab clips flat at an amber rung — measured
  zero white-hot pixels on all three flame scenes against 1.9% for the march.
- **A fluid-sim flipbook atlas.** The best *connectedness* result produced and
  unmistakably a texture: a bilinear ramp off a 256 px cell magnified four times
  at 4K gives a 195–227 px silhouette falloff against the march's 8 px. Its
  useful idea — that emission and soot are two fields with two ceilings — the
  march already has in closed form, and a 3.5 MB asset and a per-scene singleton
  are what it costs. Rejected.
- **A minimum screen-space width clamp / alpha-to-coverage.** Not applicable:
  the flame's edge is a chord length going to zero, not a thin primitive.

## Blast radius

Only two scenes in the repository author `flame = true`. The smoke work reaches
further, because `pointLights` is now added to alpha-blended particles — but it
returns zero in a scene with no point lights, so a plume not standing beside a
light is byte-identical.

## Cost

`LOOM_GPU_TIMING=1`, forward pass, **minimum of twelve runs** — this box shares
one GPU with whatever else is resident, so the mean measures contention and the
minimum measures the shader. Both columns were taken in the same session, the
baseline by checking out `fire-foundation`'s `scene.slang` and rebuilding.

| scene | resolution | slice | integral | delta |
| --- | --- | --- | --- | --- |
| `campfire` | 1920x1080 | 0.745 ms | 1.460 ms | **+0.72** |
| `campfire` | 3840x2160 | 2.375 ms | 4.839 ms | **+2.46** |
| `lanternhead` | 1920x1080 | 0.865 ms | 0.873 ms | +0.008 |

`campfire` is the repository's worst case: its flame covers 2.3% of a 1080p
frame from four metres away. **+2.46 ms at 4K for one close fire is a real
budget line** — about 15% of a 16.7 ms frame — and a scene with several
near-camera fires would want `FIRE_STEPS` made a quality setting, which is not
built.

**`lanternhead`'s +0.008 ms is measured and is not explained, and it is recorded
that way on purpose.** The instrument resolves it: ten of twelve runs land
within 0.023 ms of the minimum. Quadrupling `FIRE_STEPS` to 192 moves `campfire`
from 1.460 to 3.585 ms and `lanternhead` from 0.873 to 0.880. And the two flames
have almost the same footprint — 47,916 against 42,486 pixels at 1080p, measured
by differencing against a build with the density stubbed to zero — so a
cost-per-flame-pixel model predicts roughly +0.6 ms for `lanternhead` and is
wrong by two orders of magnitude.

The likely explanation, **untested**: `campfire`'s forward pass has four objects
and no grass, so the march is the critical path, while `lanternhead`'s already
carries 31,958 grass blades and twelve objects, and ALU-bound fragments added to
a pass that is latency-bound elsewhere can cost close to nothing at the margin.
An earlier report in this investigation called the same row "unchanged — its
flame is small on screen"; the footprint measurement says that reason is wrong
even though the number is right.
